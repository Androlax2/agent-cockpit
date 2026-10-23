#!/usr/bin/python3 -I
"""Sandbox-only gh compatibility. Unsupported writes never fall through."""

import argparse
from contextlib import nullcontext
import json
import os
from pathlib import Path
import re
import socket
import struct
import subprocess
import sys
import time


MAX_HEADER = 16 * 1024
MAX_BODY = 1024 * 1024
PR_METADATA_FIELDS = {'labels', 'milestone', 'assignees', 'add_labels', 'remove_labels',
                      'add_assignees', 'remove_assignees'}


class ClientDenied(Exception):
    """Secret-free compatibility refusal."""


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise ClientDenied('command_requires_migration')


def repository_name(value):
    if not isinstance(value, str):
        return None
    value = value.removeprefix('https://github.com/').removeprefix('github.com/')
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', value):
        return None
    return value.lower()


def target_number(value, canonical, kind):
    if re.fullmatch(r'[1-9][0-9]{0,18}', value):
        return int(value)
    match = re.fullmatch(r'https://github\.com/([^/]+/[^/]+)/(issues|pull)/([1-9][0-9]{0,18})/?', value)
    if match is None:
        raise ClientDenied('command_requires_migration')
    if repository_name(match[1]) != repository_name(canonical):
        raise ClientDenied('repository_not_enrolled')
    if match[2] != ('issues' if kind == 'issue' else 'pull'):
        raise ClientDenied('command_requires_migration')
    return int(match[3])


def unique_fields(pairs):
    fields = {}
    for name, value in pairs:
        if name in fields:
            raise ClientDenied('invalid_broker_response')
        fields[name] = value
    return fields


def read_exact(connection, size, deadline):
    chunks = []
    remaining = size
    while remaining:
        timeout = deadline - time.monotonic()
        if timeout <= 0:
            raise ClientDenied('broker_outcome_unknown')
        connection.settimeout(min(30, timeout))
        chunk = connection.recv(min(remaining, 65536))
        if not chunk:
            raise ClientDenied('broker_outcome_unknown')
        chunks.append(chunk)
        remaining -= len(chunk)
    return b''.join(chunks)


def broker_action(action, fields):
    capability_path = os.environ.get('COCKPIT_GIT_CAPABILITY_FILE')
    socket_path = os.environ.get('COCKPIT_GIT_SOCKET')
    if capability_path is None or socket_path is None:
        raise ClientDenied('broker_unavailable')
    with Path(capability_path).open('r') as source:
        capability = source.read(65)
    if not re.fullmatch(r'[0-9a-f]{64}', capability):
        raise ClientDenied('broker_unavailable')
    body = json.dumps(fields).encode()
    if len(body) > MAX_BODY:
        raise ClientDenied('action_body_too_large')
    header = json.dumps({'version': 1, 'capability': capability,
                         'action': action, 'body_length': len(body)}).encode()
    deadline = time.monotonic() + 120
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(30)
        connection.connect(socket_path)
        connection.sendall(struct.pack('!I', len(header)) + header + body)
        connection.shutdown(socket.SHUT_WR)
        size = struct.unpack('!I', read_exact(connection, 4, deadline))[0]
        if not 0 < size <= MAX_HEADER:
            raise ClientDenied('invalid_broker_response')
        response = json.loads(read_exact(connection, size, deadline), object_pairs_hook=unique_fields)
        if (not isinstance(response, dict) or not {'status', 'content_type', 'body_length'} <= response.keys()
                or response.keys() - {'status', 'content_type', 'body_length', 'code'}
                or type(response['status']) is not int or not 200 <= response['status'] <= 599
                or response['content_type'] != 'application/json'
                or type(response['body_length']) is not int or not 0 <= response['body_length'] <= MAX_BODY):
            raise ClientDenied('invalid_broker_response')
        if response['status'] != 200:
            code = response.get('code')
            if not isinstance(code, str) or not re.fullmatch(r'[a-z_]{1,80}', code):
                raise ClientDenied('invalid_broker_response')
            raise ClientDenied(code)
        result = json.loads(read_exact(connection, response['body_length'], deadline), object_pairs_hook=unique_fields)
        outcomes = ('succeeded', 'queued') if action == 'pull_request.update_branch' else ('succeeded',)
        if (not isinstance(result, dict) or result.get('outcome') not in outcomes
                or not isinstance(result.get('request_id'), str)
                or not re.fullmatch(r'[0-9a-f]{32}', result['request_id'])):
            raise ClientDenied('broker_outcome_unknown')
        if result['outcome'] == 'queued' and result.get('head_oid') != fields['expected_head_sha']:
            raise ClientDenied('broker_outcome_unknown')
        return result


def command_actions(arguments):
    parser = Parser(add_help=False)
    parser.add_argument('group', choices=['issue', 'pr'])
    parser.add_argument('operation', choices=['create', 'edit', 'close', 'reopen', 'comment'])
    parser.add_argument('number', nargs='?')
    parser.add_argument('-R', '--repo')
    parser.add_argument('-t', '--title')
    bodies = parser.add_mutually_exclusive_group()
    bodies.add_argument('-b', '--body')
    bodies.add_argument('-F', '--body-file')
    parser.add_argument('--type')
    parser.add_argument('-l', '--label', action='append')
    parser.add_argument('-m', '--milestone')
    parser.add_argument('-H', '--head')
    parser.add_argument('-B', '--base')
    parser.add_argument('-d', '--draft', action='store_true', default=None)
    parser.add_argument('-a', '--assignee', action='append')
    for name in ['add-label', 'remove-label', 'add-assignee', 'remove-assignee']:
        parser.add_argument('--' + name, action='append')
    parser.add_argument('-r', '--reason', choices=['completed', 'not planned'])
    parser.add_argument('-c', '--comment')
    options = parser.parse_args(arguments)
    canonical = os.environ.get('COCKPIT_GITHUB_REPOSITORY')
    if canonical is None or not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', canonical):
        raise ClientDenied('repository_not_enrolled')
    if options.repo is not None and repository_name(options.repo) != repository_name(canonical):
        raise ClientDenied('repository_not_enrolled')
    fields = {}
    if options.operation == 'create':
        if options.number is not None or options.title is None:
            raise ClientDenied('command_requires_migration')
    else:
        if options.number is None:
            raise ClientDenied('command_requires_migration')
        fields['number'] = target_number(options.number, canonical, options.group)
    for name in ['title', 'body', 'type', 'head', 'base', 'draft']:
        value = getattr(options, name)
        if value is not None:
            fields[name] = value
    if options.body_file is not None:
        with (nullcontext(sys.stdin) if options.body_file == '-' else Path(options.body_file).open()) as source:
            fields['body'] = source.read(65537)
        if len(fields['body'].encode()) > 65536:
            raise ClientDenied('action_body_too_large')
    for option, field in [('label', 'labels'), ('assignee', 'assignees')]:
        values = getattr(options, option)
        if values is not None:
            fields[field] = [value for entry in values for value in entry.split(',')]
    for option, field in [('add_label', 'add_labels'), ('remove_label', 'remove_labels'),
                          ('add_assignee', 'add_assignees'), ('remove_assignee', 'remove_assignees')]:
        values = getattr(options, option)
        if values is not None:
            if options.operation != 'edit':
                raise ClientDenied('command_requires_migration')
            fields[field] = [value for entry in values for value in entry.split(',')]
    if options.milestone is not None:
        fields['milestone'] = resolve_milestone(options.milestone, canonical)
    if options.reason is not None:
        fields['reason'] = options.reason.replace(' ', '_')
    metadata = {}
    if options.group == 'pr' and options.operation in ('create', 'edit'):
        metadata = {name: value for name, value in fields.items() if name in PR_METADATA_FIELDS}
        fields = {name: value for name, value in fields.items() if name not in PR_METADATA_FIELDS}
    if options.operation == 'comment':
        allowed = {'number', 'body'}
    elif options.operation == 'close':
        allowed = {'number', 'reason'} if options.group == 'issue' else {'number'}
    elif options.operation == 'reopen':
        allowed = {'number'}
    elif options.group == 'pr':
        allowed = {'title', 'body', 'head', 'base', 'draft'} if options.operation == 'create' else {'number', 'title', 'body', 'base'}
    else:
        allowed = {'number', 'title', 'body', 'type'} | PR_METADATA_FIELDS
    if fields.keys() - allowed or (options.group == 'pr' and options.operation == 'create'
                                   and not {'head', 'base'} <= fields.keys()):
        raise ClientDenied('command_requires_migration')
    if options.operation == 'comment':
        if 'body' not in fields or options.comment is not None:
            raise ClientDenied('command_requires_migration')
        return [('comment.create', fields)]
    actions = [(('issue.' if options.group == 'issue' else 'pull_request.') + options.operation, fields)]
    if metadata:
        if options.operation == 'edit' and fields.keys() == {'number'}:
            actions = []
        actions.append(('pull_request.metadata.edit', metadata | {'number': fields.get('number')}))
    if options.comment is not None:
        if options.operation not in {'close', 'reopen'}:
            raise ClientDenied('command_requires_migration')
        actions.insert(0, ('comment.create', {'number': fields['number'], 'body': options.comment}))
    return actions


def review_actions(arguments):
    parser = Parser(add_help=False)
    parser.add_argument('group', choices=['pr'])
    parser.add_argument('operation', choices=['review'])
    parser.add_argument('number')
    parser.add_argument('-R', '--repo')
    events = parser.add_mutually_exclusive_group(required=True)
    events.add_argument('-a', '--approve', action='store_const', const='APPROVE', dest='event')
    events.add_argument('-r', '--request-changes', action='store_const', const='REQUEST_CHANGES', dest='event')
    events.add_argument('-c', '--comment', action='store_const', const='COMMENT', dest='event')
    bodies = parser.add_mutually_exclusive_group()
    bodies.add_argument('-b', '--body', default='')
    bodies.add_argument('-F', '--body-file')
    options = parser.parse_args(arguments)
    canonical = os.environ.get('COCKPIT_GITHUB_REPOSITORY')
    if canonical is None or (options.repo is not None and repository_name(options.repo) != repository_name(canonical)):
        raise ClientDenied('repository_not_enrolled')
    number = target_number(options.number, canonical, 'pr')
    head = read_pull_request_head(number, canonical)
    body = options.body
    if options.body_file is not None:
        with (nullcontext(sys.stdin) if options.body_file == '-' else Path(options.body_file).open()) as source:
            body = source.read(65537)
    if len(body.encode()) > 65536:
        raise ClientDenied('action_body_too_large')
    return [('review.create', {'number': number, 'expected_head_sha': head,
                               'event': options.event, 'body': body})]


def read_pull_request_head(number, canonical):
    result = native_read(['pr', 'view', str(number), '--repo', canonical, '--json', 'headRefOid'], capture_output=True)
    if result.returncode != 0 or len(result.stdout) > MAX_BODY:
        raise ClientDenied('review_head_unavailable')
    current = json.loads(result.stdout, object_pairs_hook=unique_fields)
    if (not isinstance(current, dict) or not isinstance(current.get('headRefOid'), str)
            or not re.fullmatch(r'[0-9a-f]{40}', current['headRefOid'])):
        raise ClientDenied('review_head_unavailable')
    return current['headRefOid']


def update_branch_actions(arguments):
    parser = Parser(add_help=False)
    parser.add_argument('group', choices=['pr'])
    parser.add_argument('operation', choices=['update-branch'])
    parser.add_argument('number')
    parser.add_argument('-R', '--repo')
    options = parser.parse_args(arguments)
    canonical = os.environ.get('COCKPIT_GITHUB_REPOSITORY')
    if canonical is None or (options.repo is not None and repository_name(options.repo) != repository_name(canonical)):
        raise ClientDenied('repository_not_enrolled')
    number = target_number(options.number, canonical, 'pr')
    return [('pull_request.update_branch', {'number': number,
             'expected_head_sha': read_pull_request_head(number, canonical)})]


def readiness_actions(arguments):
    parser = Parser(add_help=False)
    parser.add_argument('group', choices=['pr'])
    parser.add_argument('operation', choices=['ready'])
    parser.add_argument('number')
    parser.add_argument('-R', '--repo')
    parser.add_argument('--undo', action='store_true')
    options = parser.parse_args(arguments)
    canonical = os.environ.get('COCKPIT_GITHUB_REPOSITORY')
    if canonical is None or (options.repo is not None and repository_name(options.repo) != repository_name(canonical)):
        raise ClientDenied('repository_not_enrolled')
    number = target_number(options.number, canonical, 'pr')
    return [('pull_request.draft' if options.undo else 'pull_request.ready', {'number': number})]


def is_native_read(arguments):
    commands = {('pr', 'list'), ('pr', 'view'), ('pr', 'checks'), ('pr', 'diff'), ('pr', 'status'),
                ('issue', 'list'), ('issue', 'view'), ('issue', 'status'), ('repo', 'view'),
                ('run', 'list'), ('run', 'view'), ('run', 'download'), ('search', 'prs'), ('search', 'issues')}
    if tuple(arguments[:2]) in commands:
        return '--web' not in arguments
    if arguments[:1] != ['api']:
        return False
    endpoint = None
    options_with_values = {'-X', '--method', '-H', '--header', '--jq', '-q', '--template', '-t', '--cache'}
    flags = {'--paginate', '--slurp', '--silent', '--include', '-i', '--verbose'}
    index = 1
    while index < len(arguments):
        argument = arguments[index]
        option, separator, value = argument.partition('=')
        if option in options_with_values:
            if not separator:
                index += 1
                if index >= len(arguments):
                    return False
                value = arguments[index]
            if option in {'-X', '--method'} and value != 'GET':
                return False
        elif argument in flags:
            pass
        elif argument.startswith('-') or endpoint is not None:
            return False
        else:
            endpoint = argument.removeprefix('/')
        index += 1
    canonical = os.environ.get('COCKPIT_GITHUB_REPOSITORY', '')
    prefix = 'repos/' + canonical
    return (endpoint is not None and canonical != '' and (endpoint == prefix or endpoint.startswith(prefix + '/'))
            and not any(character in endpoint for character in '%\\?#') and '..' not in endpoint)


def native_read(arguments, *, capture_output=False):
    canonical = os.environ.get('COCKPIT_GITHUB_REPOSITORY')
    if canonical is None:
        raise ClientDenied('repository_not_enrolled')
    for index, argument in enumerate(arguments):
        selected = None
        if argument in {'-R', '--repo'}:
            if index + 1 == len(arguments):
                raise ClientDenied('repository_not_enrolled')
            selected = arguments[index + 1]
        elif argument.startswith('--repo='):
            selected = argument.removeprefix('--repo=')
        if selected is not None and repository_name(selected) != repository_name(canonical):
            raise ClientDenied('repository_not_enrolled')
    token = os.environ.get('GH_TOKEN')
    if not token:
        raise ClientDenied('read_credential_unavailable')
    environment = {key: value for key, value in os.environ.items()
                   if key not in {'GITHUB_TOKEN', 'GH_ENTERPRISE_TOKEN', 'GITHUB_ENTERPRISE_TOKEN', 'GH_PROJECT_TOKEN'}}
    environment.update(GH_HOST='github.com', GH_REPO=canonical, GH_PROMPT_DISABLED='1')
    return subprocess.run(['/usr/bin/gh', *arguments], env=environment, capture_output=capture_output)


def resolve_milestone(value, canonical):
    if re.fullmatch(r'[1-9][0-9]{0,18}', value):
        return int(value)
    entries = native_pages(['api', 'repos/' + canonical + '/milestones?state=all&per_page=100',
                            '--paginate', '--slurp'])
    matches = [entry.get('number') for entry in entries if entry.get('title') == value]
    if len(matches) != 1 or type(matches[0]) is not int or matches[0] <= 0:
        raise ClientDenied('milestone_read_failed')
    return matches[0]


def native_pages(arguments):
    result = native_read(arguments, capture_output=True)
    if result.returncode != 0 or len(result.stdout) > MAX_BODY:
        raise ClientDenied('metadata_read_failed')
    pages = json.loads(result.stdout, object_pairs_hook=unique_fields)
    if (not isinstance(pages, list) or any(not isinstance(page, list) for page in pages)
            or any(not isinstance(entry, dict) for page in pages for entry in page)):
        raise ClientDenied('metadata_read_failed')
    return [entry for page in pages for entry in page]


def relationship_actions(arguments):
    parser = Parser(add_help=False)
    parser.add_argument('group', choices=['api'])
    parser.add_argument('endpoint')
    parser.add_argument('-X', '--method', default='POST', choices=['POST'])
    parser.add_argument('-F', '--field', '-f', '--raw-field', action='append', dest='fields', default=[])
    options = parser.parse_args(arguments)
    canonical = os.environ.get('COCKPIT_GITHUB_REPOSITORY')
    if canonical is None:
        raise ClientDenied('repository_not_enrolled')
    match = re.fullmatch(re.escape('repos/' + canonical) + r'/issues/([1-9][0-9]*)/(sub_issues|dependencies/blocked_by)',
                         options.endpoint.removeprefix('/'))
    if match is None:
        raise ClientDenied('command_requires_migration')
    fields = {}
    for field in options.fields:
        name, separator, value = field.partition('=')
        if not separator or name in fields:
            raise ClientDenied('command_requires_migration')
        fields[name] = value
    is_child = match[2] == 'sub_issues'
    identifier = 'sub_issue_id' if is_child else 'issue_id'
    if (identifier not in fields or fields.keys() - ({identifier, 'replace_parent'} if is_child else {identifier})
            or not re.fullmatch(r'[1-9][0-9]{0,18}', fields[identifier])
            or ('replace_parent' in fields and fields['replace_parent'] != 'false')):
        raise ClientDenied('command_requires_migration')
    entries = native_pages(['api', 'repos/' + canonical + '/issues?state=all&per_page=100', '--paginate', '--slurp'])
    numbers = [entry.get('number') for entry in entries
               if entry.get('id') == int(fields[identifier]) and 'pull_request' not in entry]
    if len(numbers) != 1 or type(numbers[0]) is not int or numbers[0] <= 0:
        raise ClientDenied('relationship_target_not_enrolled')
    return [('issue.child.add' if is_child else 'issue.blocker.add',
             {'number': int(match[1]), 'child_number' if is_child else 'blocker_number': numbers[0]})]


def main(arguments=None):
    completed = 0
    previous_number = None
    try:
        arguments = sys.argv[1:] if arguments is None else arguments
        if is_native_read(arguments):
            return native_read(arguments).returncode
        if arguments[:1] not in (['issue'], ['pr'], ['api']):
            raise ClientDenied('command_requires_migration')
        if arguments[:1] == ['api']:
            actions = relationship_actions(arguments)
        elif arguments[:2] == ['pr', 'review']:
            actions = review_actions(arguments)
        elif arguments[:2] == ['pr', 'update-branch']:
            actions = update_branch_actions(arguments)
        elif arguments[:2] == ['pr', 'ready']:
            actions = readiness_actions(arguments)
        else:
            actions = command_actions(arguments)
        for action, fields in actions:
            if action == 'pull_request.metadata.edit' and fields.get('number') is None:
                if type(previous_number) is not int or previous_number <= 0:
                    raise ClientDenied('broker_outcome_unknown')
                fields = fields | {'number': previous_number}
            if completed:
                time.sleep(1)
            result = broker_action(action, fields)
            if type(result.get('number')) is int and result['number'] > 0:
                previous_number = result['number']
            completed += 1
            if result['outcome'] == 'queued':
                print('gh: branch update queued (' + result['request_id'] + ')')
            url = result.get('html_url')
            if isinstance(url, str):
                print(url)
        return 0
    except ClientDenied as error:
        print('gh: ' + ('partial_outcome: ' if completed else '') + str(error), file=sys.stderr)
        return 1
    except (OSError, ValueError, UnicodeError, RecursionError):
        print('gh: ' + ('partial_outcome: ' if completed else '') + 'broker_outcome_unknown', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
