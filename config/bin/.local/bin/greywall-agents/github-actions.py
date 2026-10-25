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
                        'pull_request.ready', 'pull_request.draft',
                        'review.create', 'review.submit', 'review.edit', 'review.reply'}
PROJECT_ACTIONS = {'project.item.add', 'project.item.field_set', 'project.item.move',
                   'project.fields.list', 'project.items.list'}
PROJECT_MUTATIONS = {'project.item.add', 'project.item.field_set', 'project.item.move'}
COLLABORATION_ACTIONS = ISSUE_ACTIONS | DISCUSSION_ACTIONS | PULL_REQUEST_ACTIONS | PROJECT_ACTIONS
ITEM_QUERY = ('query($item:ID!){node(id:$item){... on ProjectV2Item{id project{id} content{__typename '
              '... on Issue{number repository{databaseId}} ... on PullRequest{number repository{databaseId}}}}}}')
FIELDS_QUERY = ('query($project:ID!){node(id:$project){... on ProjectV2{fields(first:100){nodes{'
                '... on ProjectV2FieldCommon{id name dataType} ... on ProjectV2SingleSelectField{options{id name}}} '
                'pageInfo{hasNextPage}}}}}')
ITEMS_QUERY = ('query($project:ID!,$after:String){node(id:$project){... on ProjectV2{items(first:100,after:$after){'
               'nodes{id content{__typename ... on Issue{number title body url repository{databaseId nameWithOwner}} '
               '... on PullRequest{number title body url repository{databaseId nameWithOwner}}} fieldValues(first:50){nodes{'
               '... on ProjectV2ItemFieldSingleSelectValue{optionId field{... on ProjectV2FieldCommon{id}}} '
               '... on ProjectV2ItemFieldNumberValue{number field{... on ProjectV2FieldCommon{id}}} '
               '... on ProjectV2ItemFieldTextValue{text field{... on ProjectV2FieldCommon{id}}}} '
               'pageInfo{hasNextPage}}} pageInfo{hasNextPage endCursor}}}}}')
ADD_MUTATION = ('mutation($project:ID!,$content:ID!){addProjectV2ItemById(input:{projectId:$project,'
                'contentId:$content}){item{id}}}')
FIELD_MUTATIONS = {
    'SINGLE_SELECT': ('option', 'String!', 'singleSelectOptionId'),
    'NUMBER': ('number', 'Float!', 'number'),
    'TEXT': ('text', 'String!', 'text'),
}
MOVE_MUTATION = ('mutation($project:ID!,$item:ID!,$after:ID){updateProjectV2ItemPosition(input:{projectId:$project,'
                 'itemId:$item,afterId:$after}){items(first:1){totalCount}}}')


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
    if action in PROJECT_ACTIONS:
        return project_fields(action, fields)
    raise ActionDenied('action_unavailable')


def perform_action(record, action, fields, request, *, actor_id):
    if action in ISSUE_ACTIONS:
        return issue_action(record, action, fields, request)
    if action in DISCUSSION_ACTIONS:
        return discussion_action(record, action, fields, request, actor_id=actor_id)
    if action in PULL_REQUEST_ACTIONS:
        return pull_request_action(record, action, fields, request, actor_id=actor_id)
    if action in PROJECT_ACTIONS:
        return project_action(record, action, fields, request)
    raise ActionDenied('action_unavailable')


def opaque_id(value):
    return bounded_text(value, 1024) and bool(value) and not any(ord(character) < 33 for character in value)


def project_board(record):
    """Enrolled board policy; manual_options deliberately excludes automation-owned Status values."""
    board = record.get('project_board')
    if not isinstance(board, dict) or not opaque_id(board.get('node_id')) or not isinstance(board.get('fields'), dict):
        raise ActionDenied('project_not_enrolled')
    for field_id, field in board['fields'].items():
        if (not opaque_id(field_id) or not isinstance(field, dict) or not bounded_text(field.get('name'), 256)
                or field.get('data_type') not in FIELD_MUTATIONS):
            raise ActionDenied('project_not_enrolled')
        if field['data_type'] == 'SINGLE_SELECT' and (
                not isinstance(field.get('options'), dict) or not field['options']
                or any(not opaque_id(key) or not bounded_text(name, 256) for key, name in field['options'].items())
                or not isinstance(field.get('manual_options'), list)
                or any(option not in field['options'] for option in field['manual_options'])):
            raise ActionDenied('project_not_enrolled')
    return board


def project_fields(action, fields):
    allowed = {'project.item.add': {'content_kind', 'number'}, 'project.item.field_set': {'item_id', 'field_id', 'value'},
               'project.item.move': {'item_id', 'after_id'}, 'project.fields.list': set(),
               'project.items.list': {'cursor', 'board_number', 'owner'}}[action]
    required = allowed - {'cursor', 'board_number', 'owner'}
    if not isinstance(fields, dict) or fields.keys() - allowed or not required <= fields.keys():
        raise ActionDenied('invalid_action_fields')
    if action == 'project.item.add' and (fields['content_kind'] not in ('issue', 'pull_request')
                                         or not positive_number(fields['number'])):
        raise ActionDenied('invalid_action_fields')
    for name in ['item_id', 'field_id']:
        if name in fields and not opaque_id(fields[name]):
            raise ActionDenied('invalid_action_fields')
    if 'after_id' in fields and fields['after_id'] is not None and not opaque_id(fields['after_id']):
        raise ActionDenied('invalid_action_fields')
    if 'cursor' in fields and not opaque_id(fields['cursor']):
        raise ActionDenied('invalid_action_fields')
    if ('board_number' in fields and not positive_number(fields['board_number'])
            or 'owner' in fields and not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]{0,38}', str(fields['owner']))):
        raise ActionDenied('invalid_action_fields')
    return fields


def project_graphql(request, query, variables, *, mutation):
    status, result, _ = request('POST', '/graphql', {'query': query, 'variables': variables},
                                role='project', resource='graphql', mutation=mutation)
    if (status != 200 or not isinstance(result, dict) or ('errors' in result and result['errors'] != [])
            or not isinstance(result.get('data'), dict)):
        raise ActionDenied('action_outcome_unknown' if mutation else 'project_read_failed')
    return result['data']


def verified_item(record, board, item_id, request, *, own_repository=True):
    node = project_graphql(request, ITEM_QUERY, {'item': item_id}, mutation=False).get('node')
    content = node.get('content') if isinstance(node, dict) else None
    if (not isinstance(node, dict) or node.get('id') != item_id or not isinstance(node.get('project'), dict)
            or node['project'].get('id') != board['node_id']
            or (own_repository and (not isinstance(content, dict) or not isinstance(content.get('repository'), dict)
                                    or content['repository'].get('databaseId') != record['repository_id']))):
        raise ActionDenied('project_item_invalid')
    return node


def project_action(record, action, fields, request):
    board = project_board(record)
    fields = project_fields(action, fields)
    if ('board_number' in fields and fields['board_number'] != board.get('number')
            or 'owner' in fields and str(board.get('owner_login', '')).lower() != fields['owner'].lower()):
        raise ActionDenied('project_not_enrolled')
    if action == 'project.fields.list':
        return list_project_fields(board, request)
    if action == 'project.items.list':
        return list_project_items(record, board, fields.get('cursor'), request)
    if action == 'project.item.add':
        repository = '/repos/' + record['canonical_name']
        if fields['content_kind'] == 'issue':
            status, content, _ = request('GET', repository + '/issues/' + str(fields['number']))
            if (status != 200 or not isinstance(content, dict) or 'pull_request' in content
                    or content.get('number') != fields['number']
                    or content.get('repository_url') != 'https://api.github.com' + repository):
                raise ActionDenied('project_content_invalid')
        else:
            status, content, _ = request('GET', repository + '/pulls/' + str(fields['number']), permission='pull_requests')
            if (status != 200 or not isinstance(content, dict) or content.get('number') != fields['number']
                    or content.get('html_url') != 'https://github.com/' + record['canonical_name'] + '/pull/' + str(fields['number'])):
                raise ActionDenied('project_content_invalid')
        if not opaque_id(content.get('node_id')):
            raise ActionDenied('project_content_invalid')
        data = project_graphql(request, ADD_MUTATION, {'project': board['node_id'], 'content': content['node_id']}, mutation=True)
        item = data.get('addProjectV2ItemById', {}).get('item') if isinstance(data.get('addProjectV2ItemById'), dict) else None
        if not isinstance(item, dict) or not opaque_id(item.get('id')):
            raise ActionDenied('action_outcome_unknown')
        return {'item_id': item['id'], 'outcome': 'succeeded'}
    if action == 'project.item.field_set' and fields['field_id'] not in board['fields']:
        raise ActionDenied('project_field_not_enrolled')
    verified_item(record, board, fields['item_id'], request)
    if action == 'project.item.move':
        if fields['after_id'] is not None:
            verified_item(record, board, fields['after_id'], request, own_repository=False)
        data = project_graphql(request, MOVE_MUTATION, {'project': board['node_id'], 'item': fields['item_id'],
                                                        'after': fields['after_id']}, mutation=True)
        if not isinstance(data.get('updateProjectV2ItemPosition'), dict):
            raise ActionDenied('action_outcome_unknown')
        return {'item_id': fields['item_id'], 'outcome': 'succeeded'}
    field = board['fields'].get(fields['field_id'])
    if field is None:
        raise ActionDenied('project_field_not_enrolled')
    value = fields['value']
    if field['data_type'] == 'SINGLE_SELECT' and value not in field['manual_options']:
        raise ActionDenied('project_value_not_permitted')
    if field['data_type'] == 'NUMBER' and (type(value) not in (int, float) or value != value
                                           or value in (float('inf'), float('-inf'))):
        raise ActionDenied('invalid_action_fields')
    if field['data_type'] == 'TEXT' and not bounded_text(value, 1024):
        raise ActionDenied('invalid_action_fields')
    variable, graphql_type, input_name = FIELD_MUTATIONS[field['data_type']]
    query = ('mutation($project:ID!,$item:ID!,$field:ID!,$' + variable + ':' + graphql_type + '){'
             'updateProjectV2ItemFieldValue(input:{projectId:$project,itemId:$item,fieldId:$field,'
             'value:{' + input_name + ':$' + variable + '}}){projectV2Item{id}}}')
    data = project_graphql(request, query, {'project': board['node_id'], 'item': fields['item_id'],
                                            'field': fields['field_id'], variable: value}, mutation=True)
    updated = data.get('updateProjectV2ItemFieldValue')
    if not isinstance(updated, dict) or not isinstance(updated.get('projectV2Item'), dict) \
            or updated['projectV2Item'].get('id') != fields['item_id']:
        raise ActionDenied('action_outcome_unknown')
    return {'item_id': fields['item_id'], 'outcome': 'succeeded', 'applied_fields': {fields['field_id']: value}}


def list_project_fields(board, request):
    node = project_graphql(request, FIELDS_QUERY, {'project': board['node_id']}, mutation=False).get('node')
    connection = node.get('fields') if isinstance(node, dict) else None
    if (not isinstance(connection, dict) or not isinstance(connection.get('nodes'), list)
            or connection.get('pageInfo', {}).get('hasNextPage') is not False):
        raise ActionDenied('project_read_incomplete')
    live = {entry.get('id'): entry for entry in connection['nodes'] if isinstance(entry, dict)}
    fields = []
    for field_id, field in board['fields'].items():
        current = live.get(field_id)
        if (current is None or current.get('name') != field['name'] or current.get('dataType') != field['data_type']
                or (field['data_type'] == 'SINGLE_SELECT'
                    and {option.get('id'): option.get('name') for option in current.get('options') or []
                         if isinstance(option, dict)} != field['options'])):
            raise ActionDenied('project_policy_changed')
        fields.append({'id': field_id, 'name': field['name'], 'data_type': field['data_type']}
                      | ({'options': field['options'], 'manual_options': field['manual_options']}
                         if field['data_type'] == 'SINGLE_SELECT' else {}))
    return {'fields': fields, 'outcome': 'succeeded'}


def list_project_items(record, board, cursor, request):
    node = project_graphql(request, ITEMS_QUERY, {'project': board['node_id'], 'after': cursor}, mutation=False).get('node')
    connection = node.get('items') if isinstance(node, dict) else None
    if (not isinstance(connection, dict) or not isinstance(connection.get('nodes'), list)
            or not isinstance(connection.get('pageInfo'), dict)
            or type(connection['pageInfo'].get('hasNextPage')) is not bool):
        raise ActionDenied('project_read_incomplete')
    items = []
    for entry in connection['nodes']:
        content = entry.get('content') if isinstance(entry, dict) else None
        if (not isinstance(content, dict) or not isinstance(content.get('repository'), dict)
                or content['repository'].get('databaseId') != record['repository_id']):
            continue
        values = entry.get('fieldValues')
        if (not isinstance(values, dict) or not isinstance(values.get('nodes'), list)
                or values.get('pageInfo', {}).get('hasNextPage') is not False or not opaque_id(entry.get('id'))):
            raise ActionDenied('project_read_incomplete')
        selected, display = {}, {}
        for value in values['nodes']:
            field_id = value.get('field', {}).get('id') if isinstance(value, dict) else None
            if field_id in board['fields']:
                field = board['fields'][field_id]
                selected[field_id] = next(value[key] for key in ('optionId', 'number', 'text') if key in value)
                display[field['name']] = (field['options'].get(selected[field_id])
                                          if field['data_type'] == 'SINGLE_SELECT' else selected[field_id])
                if display[field['name']] is None:
                    raise ActionDenied('project_policy_changed')
        kind = 'PullRequest' if content.get('__typename') == 'PullRequest' else 'Issue'
        items.append({'item_id': entry['id'], 'content_kind': 'pull_request' if kind == 'PullRequest' else 'issue',
                      'number': content.get('number'), 'fields': selected, 'values': display,
                      'content': {'type': kind, 'title': content.get('title'), 'body': content.get('body'),
                                  'number': content.get('number'), 'repository': content['repository'].get('nameWithOwner'),
                                  'url': content.get('url')}})
    next_cursor = connection['pageInfo'].get('endCursor') if connection['pageInfo']['hasNextPage'] else None
    if next_cursor is not None and not opaque_id(next_cursor):
        raise ActionDenied('project_read_incomplete')
    return {'items': items, 'next_cursor': next_cursor, 'outcome': 'succeeded'}


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
        'pull_request.ready': {'number'}, 'pull_request.draft': {'number'},
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
    if action in ('pull_request.ready', 'pull_request.draft'):
        node_id = current.get('node_id')
        if (current['state'] != 'open' or not bounded_text(node_id, 1024) or not node_id
                or any(ord(character) < 32 for character in node_id)):
            raise ActionDenied('pull_request_target_invalid')
        mutation = 'markPullRequestReadyForReview' if action == 'pull_request.ready' else 'convertPullRequestToDraft'
        desired = action == 'pull_request.draft'
        status, result, _ = request('POST', '/graphql', {
            'query': 'mutation($id:ID!){' + mutation + '(input:{pullRequestId:$id}){pullRequest{id isDraft}}}',
            'variables': {'id': node_id}}, resource='graphql')
        if (status != 200 or not isinstance(result, dict) or ('errors' in result and result['errors'] != [])
                or not isinstance(result.get('data'), dict) or not isinstance(result['data'].get(mutation), dict)
                or not isinstance(result['data'][mutation].get('pullRequest'), dict)):
            raise ActionDenied('action_outcome_unknown')
        changed = result['data'][mutation]['pullRequest']
        if changed.get('id') != node_id or changed.get('isDraft') is not desired:
            raise ActionDenied('action_outcome_unknown')
        return {name: current[name] for name in ['id', 'number', 'html_url', 'state']} | {
            'outcome': 'succeeded', 'applied_fields': {'is_draft': desired}}
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
