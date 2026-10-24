import importlib.util
from pathlib import Path
import unittest


MODULE = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents/github-actions.py'
ITEM_QUERY = ('query($item:ID!){node(id:$item){... on ProjectV2Item{id project{id} content{__typename '
              '... on Issue{number repository{databaseId}} ... on PullRequest{number repository{databaseId}}}}}}')


class ProjectActionTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location('test_project_actions', MODULE)
        self.actions = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.actions)
        status = {'name': 'Status', 'data_type': 'SINGLE_SELECT',
                  'options': {'opt_backlog': 'Backlog', 'opt_next': 'Next', 'opt_progress': 'In progress', 'opt_done': 'Done'},
                  'manual_options': ['opt_backlog', 'opt_next']}
        self.record = {'repository_id': 42, 'canonical_name': 'acme/project', 'project_board': {
            'node_id': 'PVT_board', 'fields': {'PVTSSF_status': status,
                                                'PVTF_effort': {'name': 'Effort', 'data_type': 'NUMBER'}}}}
        self.calls = []
        self.items = {'PVTI_ours': ('PVT_board', 42), 'PVTI_other_repo': ('PVT_board', 99),
                      'PVTI_other_board': ('PVT_elsewhere', 42)}
        self.graphql = {}

    def request(self, method, path, body=None, *, permission='issues', resource='core', role=None, mutation=None):
        self.calls.append((method, path, body, role, resource, mutation))
        if path == '/repos/acme/project/issues/7':
            return 200, {'id': 107, 'node_id': 'I_seven', 'number': 7,
                         'repository_url': 'https://api.github.com/repos/acme/project'}, {}
        if path == '/repos/acme/project/pulls/8':
            return 200, {'id': 208, 'node_id': 'PR_eight', 'number': 8, 'state': 'open',
                         'html_url': 'https://github.com/acme/project/pull/8'}, {}
        self.assertEqual((method, path, resource, role), ('POST', '/graphql', 'graphql', 'project'))
        if body['query'] == ITEM_QUERY:
            board, repository = self.items.get(body['variables']['item'], (None, None))
            if board is None:
                return 200, {'data': {'node': None}}, {}
            return 200, {'data': {'node': {'id': body['variables']['item'], 'project': {'id': board},
                'content': {'__typename': 'Issue', 'number': 7, 'repository': {'databaseId': repository}}}}}, {}
        return self.graphql[body['query'].split('{', 2)[1].split('(')[0]]

    def perform(self, action, fields):
        return self.actions.perform_action(self.record, action, fields, self.request, actor_id=88)

    def mutations(self):
        return [call for call in self.calls if call[5] is True]

    def test_add_resolves_enrolled_issue_node_and_uses_fixed_variables(self):
        self.graphql['addProjectV2ItemById'] = (200, {'data': {'addProjectV2ItemById': {'item': {'id': 'PVTI_new'}}}}, {})
        result = self.perform('project.item.add', {'content_kind': 'issue', 'number': 7})
        self.assertEqual(result['item_id'], 'PVTI_new')
        [(_, _, body, _, _, _)] = self.mutations()
        self.assertEqual(body['variables'], {'project': 'PVT_board', 'content': 'I_seven'})
        self.assertNotIn('I_seven', body['query'])

    def test_manual_status_only_allows_enrolled_triage_options(self):
        self.graphql['updateProjectV2ItemFieldValue'] = (200, {'data': {'updateProjectV2ItemFieldValue': {
            'projectV2Item': {'id': 'PVTI_ours'}}}}, {})
        self.perform('project.item.field_set', {'item_id': 'PVTI_ours', 'field_id': 'PVTSSF_status', 'value': 'opt_next'})
        self.assertEqual(self.mutations()[-1][2]['variables'],
                         {'project': 'PVT_board', 'item': 'PVTI_ours', 'field': 'PVTSSF_status', 'option': 'opt_next'})
        before = len(self.mutations())
        for value in ['opt_progress', 'opt_done', 'opt_unknown']:
            with self.subTest(value=value), self.assertRaises(self.actions.ActionDenied):
                self.perform('project.item.field_set', {'item_id': 'PVTI_ours', 'field_id': 'PVTSSF_status', 'value': value})
        self.assertEqual(len(self.mutations()), before)

    def test_number_field_uses_typed_number_template(self):
        self.graphql['updateProjectV2ItemFieldValue'] = (200, {'data': {'updateProjectV2ItemFieldValue': {
            'projectV2Item': {'id': 'PVTI_ours'}}}}, {})
        self.perform('project.item.field_set', {'item_id': 'PVTI_ours', 'field_id': 'PVTF_effort', 'value': 3})
        self.assertEqual(self.mutations()[-1][2]['variables']['number'], 3)
        for value in ['3', True, float('inf')]:
            with self.subTest(value=value), self.assertRaises(self.actions.ActionDenied):
                self.perform('project.item.field_set', {'item_id': 'PVTI_ours', 'field_id': 'PVTF_effort', 'value': value})

    def test_items_of_other_repositories_or_boards_are_never_mutated(self):
        for item in ['PVTI_other_repo', 'PVTI_other_board', 'PVTI_missing']:
            with self.subTest(item=item), self.assertRaises(self.actions.ActionDenied):
                self.perform('project.item.field_set', {'item_id': item, 'field_id': 'PVTF_effort', 'value': 1})
        self.assertEqual(self.mutations(), [])

    def test_unenrolled_field_or_client_query_text_is_rejected_before_upstream(self):
        for fields in [{'item_id': 'PVTI_ours', 'field_id': 'PVTF_new', 'value': 1},
                       {'item_id': 'PVTI_ours', 'field_id': 'PVTF_effort', 'value': 1, 'query': 'mutation {x}'}]:
            with self.subTest(fields=fields), self.assertRaises(self.actions.ActionDenied):
                self.perform('project.item.field_set', fields)
        self.assertEqual(self.calls, [])

    def test_move_requires_same_board_predecessor(self):
        self.graphql['updateProjectV2ItemPosition'] = (200, {'data': {'updateProjectV2ItemPosition': {
            'items': {'totalCount': 3}}}}, {})
        self.perform('project.item.move', {'item_id': 'PVTI_ours', 'after_id': 'PVTI_other_repo'})
        self.assertEqual(self.mutations()[-1][2]['variables'],
                         {'project': 'PVT_board', 'item': 'PVTI_ours', 'after': 'PVTI_other_repo'})
        self.perform('project.item.move', {'item_id': 'PVTI_ours', 'after_id': None})
        self.assertIsNone(self.mutations()[-1][2]['variables']['after'])
        before = len(self.mutations())
        with self.assertRaises(self.actions.ActionDenied):
            self.perform('project.item.move', {'item_id': 'PVTI_ours', 'after_id': 'PVTI_other_board'})
        self.assertEqual(len(self.mutations()), before)

    def test_graphql_errors_after_mutation_are_uncertain(self):
        self.graphql['updateProjectV2ItemFieldValue'] = (200, {'errors': [{'message': 'SECRET'}], 'data': None}, {})
        with self.assertRaisesRegex(self.actions.ActionDenied, '^action_outcome_unknown$'):
            self.perform('project.item.field_set', {'item_id': 'PVTI_ours', 'field_id': 'PVTF_effort', 'value': 2})

    def test_field_listing_returns_only_enrolled_fields_and_detects_policy_drift(self):
        nodes = [{'id': 'PVTSSF_status', 'name': 'Status', 'dataType': 'SINGLE_SELECT',
                  'options': [{'id': key, 'name': value} for key, value in
                              self.record['project_board']['fields']['PVTSSF_status']['options'].items()]},
                 {'id': 'PVTF_effort', 'name': 'Effort', 'dataType': 'NUMBER'},
                 {'id': 'PVTF_secret', 'name': 'Other team field', 'dataType': 'TEXT'}]
        self.graphql['node'] = (200, {'data': {'node': {'fields': {'nodes': nodes, 'pageInfo': {'hasNextPage': False}}}}}, {})
        result = self.perform('project.fields.list', {})
        self.assertEqual(sorted(field['id'] for field in result['fields']), ['PVTF_effort', 'PVTSSF_status'])
        self.assertEqual(self.mutations(), [])
        nodes[1]['name'] = 'Size'
        with self.assertRaisesRegex(self.actions.ActionDenied, '^project_policy_changed$'):
            self.perform('project.fields.list', {})

    def test_item_listing_filters_other_repositories_and_reports_incomplete_values(self):
        def item(item_id, repository, more=False):
            return {'id': item_id, 'content': {'__typename': 'Issue', 'number': 7, 'title': 'Investigate', 'body': 'Details',
                                               'url': 'https://github.com/acme/project/issues/7',
                                               'repository': {'databaseId': repository, 'nameWithOwner': 'acme/project'}},
                    'fieldValues': {'nodes': [{'optionId': 'opt_next', 'field': {'id': 'PVTSSF_status'}},
                                              {'text': 'x', 'field': {'id': 'PVTF_secret'}}],
                                    'pageInfo': {'hasNextPage': more}}}
        page = {'nodes': [item('PVTI_ours', 42), item('PVTI_other_repo', 99)],
                'pageInfo': {'hasNextPage': True, 'endCursor': 'cursor-2'}}
        self.graphql['node'] = (200, {'data': {'node': {'items': page}}}, {})
        result = self.perform('project.items.list', {})
        self.assertEqual([entry['item_id'] for entry in result['items']], ['PVTI_ours'])
        self.assertEqual(result['items'][0]['fields'], {'PVTSSF_status': 'opt_next'})
        self.assertEqual(result['items'][0]['values'], {'Status': 'Next'})
        self.assertEqual(result['items'][0]['content'], {'type': 'Issue', 'title': 'Investigate', 'body': 'Details',
            'number': 7, 'repository': 'acme/project', 'url': 'https://github.com/acme/project/issues/7'})
        self.assertEqual(result['next_cursor'], 'cursor-2')
        self.record['project_board'].update(number=1, owner_login='acme')
        self.perform('project.items.list', {'board_number': 1, 'owner': 'Acme'})
        for selector in [{'board_number': 2, 'owner': 'acme'}, {'board_number': 1, 'owner': 'other'}]:
            with self.subTest(selector=selector), self.assertRaisesRegex(self.actions.ActionDenied, '^project_not_enrolled$'):
                self.perform('project.items.list', selector)
        self.assertNotIn('PVTI_other_repo', str(result))
        page['nodes'] = [item('PVTI_ours', 42, more=True)]
        with self.assertRaisesRegex(self.actions.ActionDenied, '^project_read_incomplete$'):
            self.perform('project.items.list', {})

    def test_missing_or_invalid_board_enrollment_denies_every_project_action(self):
        for board in [None, {'node_id': 'PVT_board'}, {'node_id': '', 'fields': {}}]:
            self.record['project_board'] = board
            with self.subTest(board=board), self.assertRaisesRegex(self.actions.ActionDenied, '^project_not_enrolled$'):
                self.perform('project.items.list', {})
        self.assertEqual(self.calls, [])


if __name__ == '__main__':
    unittest.main()
