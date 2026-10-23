#!/usr/bin/python3 -I
"""Typed collaboration handlers; the broker owns credentials and write admission."""

import re
from functools import partial
from urllib.parse import quote


ISSUE_METADATA = {'title', 'body', 'type', 'labels', 'milestone', 'assignees'}
ISSUE_DELTAS = {'add_labels', 'remove_labels', 'add_assignees', 'remove_assignees'}
ISSUE_ACTIONS = {'issue.create', 'issue.edit', 'issue.close', 'issue.reopen',
                 'issue.child.add', 'issue.blocker.add'}
DISCUSSION_ACTIONS = {'comment.create', 'comment.edit'}
PULL_REQUEST_ACTIONS = {'pull_request.create', 'pull_request.edit', 'pull_request.close', 'pull_request.reopen',
                        'pull_request.metadata.edit',
                        'pull_request.update_branch',
                        'review.create', 'review.submit', 'review.edit', 'review.reply'}
COLLABORATION_ACTIONS = ISSUE_ACTIONS | DISCUSSION_ACTIONS | PULL_REQUEST_ACTIONS


class ActionDenied(Exception):
    """Stable error with no upstream response or credential material."""


def positive_number(value):
    return type(value) is int and 0 < value < 2 ** 63


def bounded_text(value, maximum):
    if not isinstance(value, str) or '\0' in value:
        return False
    try:
        return len(value.encode('utf-8')) <= maximum
    except UnicodeError:
        return False


def issue_fields(action, fields):
    if action not in ISSUE_ACTIONS:
        raise ActionDenied('action_unavailable')
    allowed = {
        'issue.create': ISSUE_METADATA,
        'issue.edit': ISSUE_METADATA | ISSUE_DELTAS | {'number'},
        'issue.close': {'number', 'reason'},
        'issue.reopen': {'number'},
        'issue.child.add': {'number', 'child_number'},
        'issue.blocker.add': {'number', 'blocker_number'},
    }[action]
    required = {'title'} if action == 'issue.create' else {'number'}
    if action.endswith('.add'):
        required |= allowed
    if (not isinstance(fields, dict) or fields.keys() - allowed or not required <= fields.keys()
            or (action == 'issue.edit' and not fields.keys() & (ISSUE_METADATA | ISSUE_DELTAS))):
        raise ActionDenied('invalid_action_fields')
    for name, value in fields.items():
        if name in {'number', 'child_number', 'blocker_number', 'milestone'}:
            if not (name == 'milestone' and value is None) and not positive_number(value):
                raise ActionDenied('invalid_action_fields')
        elif name in {'labels', 'assignees'} | ISSUE_DELTAS:
            if (not isinstance(value, list) or len(value) > 100
                    or any(not bounded_text(entry, 400) or not 0 < len(entry) <= 100
                           or any(ord(character) < 32 for character in entry) for entry in value)
                    or len(set(value)) != len(value)):
                raise ActionDenied('invalid_action_fields')
            if name.endswith('assignees') and any(not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]{0,38}', entry) for entry in value):
                raise ActionDenied('invalid_action_fields')
        elif name == 'reason':
            if value not in ('completed', 'not_planned'):
                raise ActionDenied('invalid_action_fields')
        elif not (name == 'type' and value is None):
            maximum = {'title': 256, 'body': 65536, 'type': 256}[name]
            if not bounded_text(value, maximum):
                raise ActionDenied('invalid_action_fields')
            if name != 'body' and not value.strip():
                raise ActionDenied('invalid_action_fields')
    for name in ['labels', 'assignees']:
        if (name in fields and {'add_' + name, 'remove_' + name} & fields.keys()
                or set(fields.get('add_' + name, [])) & set(fields.get('remove_' + name, []))):
            raise ActionDenied('invalid_action_fields')
    return fields


def issue_target(repository, number, request):
    status, issue, _ = request('GET', repository + '/issues/' + str(number))
    if (status != 200 or not isinstance(issue, dict) or 'pull_request' in issue
            or issue.get('number') != number or not positive_number(issue.get('id'))
            or issue.get('repository_url') != 'https://api.github.com' + repository):
        raise ActionDenied('issue_target_invalid')
    return issue


def catalog(repository, suffix, request):
    values = []
    for page in range(1, 101):
        status, entries, headers = request('GET', repository + suffix + f'?per_page=100&page={page}')
        if status != 200 or not isinstance(entries, list) or any(not isinstance(entry, dict) for entry in entries):
            raise ActionDenied('issue_metadata_unavailable')
        values.extend(entries)
        link = headers.get('link', '')
        if not isinstance(link, str):
            raise ActionDenied('issue_metadata_unavailable')
        if 'rel="next"' not in link and 'rel=next' not in link:
            return values
    raise ActionDenied('issue_metadata_incomplete')


def verify_metadata(repository, fields, request):
    for field, suffix in [('labels', '/labels'), ('type', '/issue-types')]:
        selected = fields.get(field)
        if not selected:
            continue
        available = catalog(repository, suffix, request)
        names = [entry.get('name') for entry in available]
        expected = selected if field == 'labels' else [selected]
        if any(name not in names for name in expected):
            raise ActionDenied('issue_metadata_unavailable')
    if fields.get('milestone') is not None:
        status, milestone, _ = request('GET', repository + '/milestones/' + str(fields['milestone']))
        if status != 200 or not isinstance(milestone, dict) or milestone.get('number') != fields['milestone']:
            raise ActionDenied('issue_metadata_unavailable')


def applied_issue_fields(issue, fields):
    for name, expected in fields.items():
        actual = issue.get(name)
        if name in {'type', 'milestone'} and actual is not None:
            if not isinstance(actual, dict):
                raise ActionDenied('action_outcome_unknown')
            actual = actual.get('name' if name == 'type' else 'number')
        elif name in {'labels', 'assignees'}:
            if not isinstance(actual, list) or any(not isinstance(entry, dict) for entry in actual):
                raise ActionDenied('action_outcome_unknown')
            actual = [entry.get('name' if name == 'labels' else 'login') for entry in actual]
            if any(not isinstance(value, str) for value in actual) or sorted(actual) != sorted(expected):
                raise ActionDenied('action_outcome_unknown')
            continue
        if actual != expected:
            raise ActionDenied('action_outcome_unknown')


def issue_action(record, action, fields, request):
    """Request is the broker's fixed credential/admission boundary, never a client callback."""
    fields = issue_fields(action, fields)
    repository = '/repos/' + record['canonical_name']
    payload = {name: value for name, value in fields.items() if name in ISSUE_METADATA}
    if action == 'issue.create':
        method, path = 'POST', repository + '/issues'
        number = None
    else:
        number = fields['number']
        target = issue_target(repository, number, request)
        payload = metadata_payload(fields, target)
        method, path = 'PATCH', repository + '/issues/' + str(number)
    if action == 'issue.close':
        payload = {'state': 'closed', 'state_reason': fields.get('reason', 'completed')}
    elif action == 'issue.reopen':
        payload = {'state': 'open', 'state_reason': 'reopened'}
    elif action in {'issue.child.add', 'issue.blocker.add'}:
        related = fields['child_number' if action == 'issue.child.add' else 'blocker_number']
        if related == number:
            raise ActionDenied('issue_target_invalid')
        target = issue_target(repository, related, request)
        payload = ({'sub_issue_id': target['id'], 'replace_parent': False} if action == 'issue.child.add'
                   else {'issue_id': target['id']})
        path += '/sub_issues' if action == 'issue.child.add' else '/dependencies/blocked_by'
        status, result, _ = request('POST', path, payload)
        if status not in (200, 201) or not isinstance(result, dict) or result.get('id') != target['id']:
            raise ActionDenied('action_outcome_unknown')
        return {'id': target['id'], 'number': related, 'outcome': 'succeeded'}
    verify_metadata(repository, payload, request)
    status, result, _ = request(method, path, payload)
    if (status not in (200, 201) or not isinstance(result, dict) or 'pull_request' in result
            or not positive_number(result.get('id')) or not positive_number(result.get('number'))
            or result.get('state') not in ('open', 'closed')
            or (number is not None and result['number'] != number)
            or result.get('repository_url') != 'https://api.github.com' + repository
            or result.get('html_url') != 'https://github.com/' + record['canonical_name'] + '/issues/' + str(result['number'])):
        raise ActionDenied('action_outcome_unknown')
    applied_issue_fields(result, payload)
    return {name: result[name] for name in ['id', 'number', 'html_url', 'state']} | {
        'outcome': 'succeeded', 'applied_fields': payload}


def metadata_payload(fields, target):
    payload = {name: value for name, value in fields.items() if name in ISSUE_METADATA}
    for name in ['labels', 'assignees']:
        if not {'add_' + name, 'remove_' + name} & fields.keys():
            continue
        current = target.get(name)
        key = 'name' if name == 'labels' else 'login'
        if not isinstance(current, list):
            raise ActionDenied('issue_metadata_unavailable')
        names = [entry if isinstance(entry, str) else entry.get(key) if isinstance(entry, dict) else None
                 for entry in current]
        if any(not isinstance(entry, str) for entry in names):
            raise ActionDenied('issue_metadata_unavailable')
        retained = [entry for entry in names if entry not in fields.get('remove_' + name, [])]
        payload[name] = retained + [entry for entry in fields.get('add_' + name, []) if entry not in retained]
    return payload


def discussion_fields(action, fields):
    required = {'number', 'body'} | ({'comment_id'} if action == 'comment.edit' else set())
    if (action not in DISCUSSION_ACTIONS or not isinstance(fields, dict) or fields.keys() != required
            or not positive_number(fields['number'])
            or ('comment_id' in fields and not positive_number(fields['comment_id']))
            or not bounded_text(fields['body'], 65536) or not fields['body'].strip()):
        raise ActionDenied('invalid_action_fields')
    return fields


def discussion_action(record, action, fields, request, *, actor_id):
    fields = discussion_fields(action, fields)
    repository = '/repos/' + record['canonical_name']
    parent = repository + '/issues/' + str(fields['number'])
    status, issue, _ = request('GET', parent)
    if (status != 200 or not isinstance(issue, dict) or issue.get('number') != fields['number']
            or not positive_number(issue.get('id'))
            or issue.get('repository_url') != 'https://api.github.com' + repository):
        raise ActionDenied('comment_target_invalid')
    permission = 'pull_requests' if 'pull_request' in issue else 'issues'
    method, path = 'POST', parent + '/comments'
    if action == 'comment.edit':
        method, path = 'PATCH', repository + '/issues/comments/' + str(fields['comment_id'])
        status, comment, _ = request('GET', path, permission=permission)
        if (status != 200 or not isinstance(comment, dict) or comment.get('id') != fields['comment_id']
                or comment.get('issue_url') != 'https://api.github.com' + parent
                or not isinstance(comment.get('user'), dict) or comment['user'].get('id') != actor_id):
            raise ActionDenied('comment_target_invalid')
    status, result, _ = request(method, path, {'body': fields['body']}, permission=permission)
    if (status not in (200, 201) or not isinstance(result, dict) or not positive_number(result.get('id'))
            or result.get('body') != fields['body'] or result.get('issue_url') != 'https://api.github.com' + parent
            or not isinstance(result.get('user'), dict) or result['user'].get('id') != actor_id
            or (action == 'comment.edit' and result['id'] != fields['comment_id'])
            or result.get('html_url') not in {
                'https://github.com/' + record['canonical_name'] + '/issues/' + str(fields['number']) + '#issuecomment-' + str(result['id']),
                'https://github.com/' + record['canonical_name'] + '/pull/' + str(fields['number']) + '#issuecomment-' + str(result['id'])}):
        raise ActionDenied('action_outcome_unknown')
    return {name: result[name] for name in ['id', 'html_url']} | {'outcome': 'succeeded'}


def action_fields(action, fields):
    if action in ISSUE_ACTIONS:
        return issue_fields(action, fields)
    if action in DISCUSSION_ACTIONS:
        return discussion_fields(action, fields)
    if action in PULL_REQUEST_ACTIONS:
        return pull_request_fields(action, fields)
    raise ActionDenied('action_unavailable')


def perform_action(record, action, fields, request, *, actor_id):
    if action in ISSUE_ACTIONS:
        return issue_action(record, action, fields, request)
    if action in DISCUSSION_ACTIONS:
        return discussion_action(record, action, fields, request, actor_id=actor_id)
    if action in PULL_REQUEST_ACTIONS:
        return pull_request_action(record, action, fields, request, actor_id=actor_id)
    raise ActionDenied('action_unavailable')


def branch_name(value):
    return (bounded_text(value, 1024) and bool(value) and not value.endswith('.')
            and '..' not in value and '@{' not in value
            and not any(ord(character) < 33 or ord(character) == 127 or character in '~^:?*[\\' for character in value)
            and all(part and not part.startswith('.') and not part.endswith('.lock') for part in value.split('/')))


def review_file_path(value):
    return (bounded_text(value, 4096) and bool(value) and not value.startswith('/')
            and not any(ord(character) < 32 for character in value)
            and all(part not in ('', '.', '..') for part in value.split('/')))


def pull_request_fields(action, fields):
    if action == 'pull_request.metadata.edit':
        if not isinstance(fields, dict) or fields.keys() - ({'number', 'labels', 'milestone', 'assignees'} | ISSUE_DELTAS):
            raise ActionDenied('invalid_action_fields')
        return issue_fields('issue.edit', fields)
    allowed = {
        'pull_request.create': {'head', 'base', 'title', 'body', 'draft'},
        'pull_request.edit': {'number', 'title', 'body', 'base'},
        'pull_request.close': {'number'}, 'pull_request.reopen': {'number'},
        'pull_request.update_branch': {'number', 'expected_head_sha'},
        'review.create': {'number', 'expected_head_sha', 'event', 'body', 'comments'},
        'review.submit': {'number', 'expected_head_sha', 'review_id', 'event', 'body'},
        'review.edit': {'number', 'expected_head_sha', 'review_id', 'body'},
        'review.reply': {'number', 'expected_head_sha', 'comment_id', 'body'},
    }.get(action)
    required = ({'head', 'base', 'title'} if action == 'pull_request.create' else
                {'number', 'expected_head_sha', 'event'} if action == 'review.create' else {'number'})
    if action in ('review.submit', 'review.edit', 'review.reply'):
        required |= {'expected_head_sha', 'comment_id' if action == 'review.reply' else 'review_id'}
        required |= {'event'} if action == 'review.submit' else {'body'}
    if action == 'pull_request.update_branch':
        required.add('expected_head_sha')
    if (allowed is None or not isinstance(fields, dict) or fields.keys() - allowed
            or not required <= fields.keys() or (action == 'pull_request.edit' and len(fields) < 2)):
        raise ActionDenied('invalid_action_fields')
    for name, value in fields.items():
        if name in ('number', 'review_id', 'comment_id') and not positive_number(value):
            raise ActionDenied('invalid_action_fields')
        if name in ('title', 'body') and (not bounded_text(value, 256 if name == 'title' else 65536)
                                         or (name == 'title' and not value.strip())):
            raise ActionDenied('invalid_action_fields')
        if name in ('head', 'base') and not branch_name(value):
            raise ActionDenied('invalid_action_fields')
        if name == 'draft' and type(value) is not bool:
            raise ActionDenied('invalid_action_fields')
        if name == 'expected_head_sha' and (not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{40}', value)):
            raise ActionDenied('invalid_action_fields')
        if name == 'event' and value not in ('COMMENT', 'APPROVE', 'REQUEST_CHANGES'):
            raise ActionDenied('invalid_action_fields')
        if name == 'comments':
            if not isinstance(value, list) or len(value) > 100:
                raise ActionDenied('invalid_action_fields')
            for comment in value:
                if (not isinstance(comment, dict) or not {'path', 'line', 'side', 'body'} <= comment.keys()
                        or comment.keys() - {'path', 'line', 'side', 'body', 'start_line', 'start_side'}
                        or not review_file_path(comment['path']) or not positive_number(comment['line'])
                        or comment['side'] not in ('LEFT', 'RIGHT') or not bounded_text(comment['body'], 65536)
                        or not comment['body'].strip()
                        or (('start_line' in comment) != ('start_side' in comment))
                        or ('start_line' in comment and (not positive_number(comment['start_line'])
                            or comment['start_line'] > comment['line'] or comment['start_side'] not in ('LEFT', 'RIGHT')))):
                    raise ActionDenied('invalid_action_fields')
    if action == 'pull_request.create' and fields['head'] == fields['base']:
        raise ActionDenied('invalid_action_fields')
    return fields


def verified_pull_request(record, number, result):
    if (not isinstance(result, dict) or result.get('number') != number or not positive_number(result.get('id'))
            or result.get('state') not in ('open', 'closed')
            or result.get('html_url') != 'https://github.com/' + record['canonical_name'] + '/pull/' + str(number)):
        raise ActionDenied('pull_request_target_invalid')
    for side in ['head', 'base']:
        branch = result.get(side)
        if (not isinstance(branch, dict) or not isinstance(branch.get('repo'), dict)
                or branch['repo'].get('id') != record['repository_id'] or not branch_name(branch.get('ref'))
                or not isinstance(branch.get('sha'), str) or not re.fullmatch(r'[0-9a-f]{40}', branch['sha'])):
            raise ActionDenied('pull_request_target_invalid')
    return result


def pull_request_action(record, action, fields, request, *, actor_id):
    fields = pull_request_fields(action, fields)
    request = partial(request, permission='pull_requests')
    repository = '/repos/' + record['canonical_name']
    path = repository + '/pulls'
    method = 'POST' if action == 'pull_request.create' else 'PATCH'
    payload = {name: value for name, value in fields.items() if name in {'title', 'body', 'head', 'base', 'draft'}}
    if action == 'pull_request.create':
        number = None
    else:
        number = fields['number']
        path += '/' + str(number)
        status, current, _ = request('GET', path)
        if status != 200:
            raise ActionDenied('pull_request_target_invalid')
        current = verified_pull_request(record, number, current)
    if action == 'pull_request.update_branch':
        if current['head']['sha'] != fields['expected_head_sha'] or current['state'] != 'open':
            raise ActionDenied('pull_request_head_changed')
        status, _, _ = request('PUT', path + '/update-branch',
                               {'expected_head_sha': fields['expected_head_sha']}, permission='contents')
        if status != 202:
            raise ActionDenied('action_outcome_unknown')
        return {'outcome': 'queued', 'head_oid': fields['expected_head_sha']}
    if action == 'pull_request.metadata.edit':
        path = repository + '/issues/' + str(number)
        status, issue, _ = request('GET', path)
        if (status != 200 or not isinstance(issue, dict) or 'pull_request' not in issue
                or issue.get('number') != number or not positive_number(issue.get('id'))
                or issue.get('repository_url') != 'https://api.github.com' + repository):
            raise ActionDenied('pull_request_target_invalid')
        payload = metadata_payload(fields, issue)
        verify_metadata(repository, payload, request)
        status, result, _ = request('PATCH', path, payload)
        if (status != 200 or not isinstance(result, dict) or 'pull_request' not in result
                or result.get('id') != issue['id'] or result.get('number') != number
                or result.get('repository_url') != 'https://api.github.com' + repository
                or result.get('state') not in ('open', 'closed')
                or result.get('html_url') != current['html_url']):
            raise ActionDenied('action_outcome_unknown')
        applied_issue_fields(result, payload)
        return {name: result[name] for name in ['id', 'number', 'html_url', 'state']} | {
            'outcome': 'succeeded', 'applied_fields': payload}
    for side in ['head', 'base']:
        if side not in payload:
            continue
        status, branch, _ = request('GET', repository + '/branches/' + quote(payload[side], safe=''))
        if (status != 200 or not isinstance(branch, dict) or branch.get('name') != payload[side]
                or not isinstance(branch.get('commit'), dict)
                or not isinstance(branch['commit'].get('sha'), str)
                or not re.fullmatch(r'[0-9a-f]{40}', branch['commit']['sha'])):
            raise ActionDenied('pull_request_target_invalid')
    if action in {'pull_request.close', 'pull_request.reopen'}:
        payload = {'state': 'closed' if action == 'pull_request.close' else 'open'}
    if action.startswith('review.'):
        return review_action(record, action, fields, current, request, actor_id=actor_id)
    status, result, _ = request(method, path, payload)
    if status not in (200, 201) or not isinstance(result, dict) or not positive_number(result.get('number')):
        raise ActionDenied('action_outcome_unknown')
    try:
        result = verified_pull_request(record, number or result['number'], result)
    except ActionDenied:
        raise ActionDenied('action_outcome_unknown') from None
    for name, expected in payload.items():
        actual = result[name]['ref'] if name in ('head', 'base') else result.get(name)
        if actual != expected:
            raise ActionDenied('action_outcome_unknown')
    return {name: result[name] for name in ['id', 'number', 'html_url', 'state']} | {
        'head_oid': result['head']['sha'], 'applied_fields': payload, 'outcome': 'succeeded'}


def review_action(record, action, fields, current, request, *, actor_id):
    if (current['head']['sha'] != fields['expected_head_sha']
            or (action in ('review.create', 'review.submit') and current['state'] != 'open')):
        raise ActionDenied('review_head_changed')
    if action in ('review.create', 'review.submit') and (
            not isinstance(current.get('user'), dict) or not positive_number(current['user'].get('id'))
            or (current['user']['id'] == actor_id and fields['event'] != 'COMMENT')):
        raise ActionDenied('review_actor_denied')
    repository = '/repos/' + record['canonical_name']
    path = repository + '/pulls/' + str(fields['number'])
    payload = {'body': fields.get('body', '')}
    if action == 'review.reply':
        status, comment, _ = request('GET', repository + '/pulls/comments/' + str(fields['comment_id']))
        if (status != 200 or not isinstance(comment, dict) or comment.get('id') != fields['comment_id']
                or comment.get('pull_request_url') != 'https://api.github.com' + path):
            raise ActionDenied('review_target_invalid')
        status, result, _ = request('POST', path + '/comments/' + str(fields['comment_id']) + '/replies', payload)
        if (status != 201 or not isinstance(result, dict) or not positive_number(result.get('id'))
                or result.get('pull_request_url') != 'https://api.github.com' + path
                or result.get('body') != payload['body'] or not isinstance(result.get('user'), dict)
                or result['user'].get('id') != actor_id):
            raise ActionDenied('action_outcome_unknown')
        return {'id': result['id'], 'head_oid': fields['expected_head_sha'], 'outcome': 'succeeded'}
    path += '/reviews'
    method = 'POST'
    if action == 'review.create':
        if fields.get('comments'):
            files = catalog(repository, '/pulls/' + str(fields['number']) + '/files', request)
            if any(comment['path'] not in [entry.get('filename') for entry in files] for comment in fields['comments']):
                raise ActionDenied('review_path_invalid')
        payload.update(commit_id=fields['expected_head_sha'], event=fields['event'])
        if 'comments' in fields:
            payload['comments'] = fields['comments']
    else:
        path += '/' + str(fields['review_id'])
        status, review, _ = request('GET', path)
        if (status != 200 or not isinstance(review, dict) or review.get('id') != fields['review_id']
                or review.get('commit_id') != fields['expected_head_sha']
                or review.get('pull_request_url') != 'https://api.github.com' + repository + '/pulls/' + str(fields['number'])
                or not isinstance(review.get('user'), dict) or review['user'].get('id') != actor_id
                or review.get('state') not in ('PENDING', 'COMMENTED', 'APPROVED', 'CHANGES_REQUESTED')
                or (action == 'review.submit' and review['state'] != 'PENDING')):
            raise ActionDenied('review_target_invalid')
        if action == 'review.submit':
            path += '/events'
            payload['event'] = fields['event']
        else:
            method = 'PUT'
    status, result, _ = request(method, path, payload)
    expected_state = ({'COMMENT': 'COMMENTED', 'APPROVE': 'APPROVED', 'REQUEST_CHANGES': 'CHANGES_REQUESTED'}[fields['event']]
                      if 'event' in fields else review['state'])
    if (status not in (200, 201) or not isinstance(result, dict) or not positive_number(result.get('id'))
            or result.get('commit_id') != fields['expected_head_sha'] or result.get('state') != expected_state
            or result.get('pull_request_url') != 'https://api.github.com' + repository + '/pulls/' + str(fields['number'])
            or not isinstance(result.get('user'), dict) or result['user'].get('id') != actor_id
            or (payload['body'] and result.get('body') != payload['body'])
            or ('review_id' in fields and result['id'] != fields['review_id'])):
        raise ActionDenied('action_outcome_unknown')
    return {'id': result['id'], 'review_state': result['state'], 'head_oid': result['commit_id'], 'outcome': 'succeeded'}
