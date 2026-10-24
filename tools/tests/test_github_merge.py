import base64
import importlib.util
from pathlib import Path
import unittest


MODULE = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents/github-actions.py'
AUTHORIZED = ('# Project\n\n## Git authorization\n\nClaude may create feature branches.\n'
              'Claude may merge pull requests once required checks pass.\n\n## Tests\n\nmake test\n')


class MergeGuardTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location('test_merge_actions', MODULE)
        self.actions = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.actions)
        self.record = {'repository_id': 42, 'canonical_name': 'acme/project'}
        self.policy = AUTHORIZED
        self.contexts = [{'__typename': 'CheckRun', 'name': 'test', 'status': 'COMPLETED', 'conclusion': 'SUCCESS', 'isRequired': True},
                         {'__typename': 'StatusContext', 'context': 'lint', 'state': 'SUCCESS', 'isRequired': True},
                         {'__typename': 'CheckRun', 'name': 'optional', 'status': 'COMPLETED', 'conclusion': 'FAILURE', 'isRequired': False}]
        self.pull = {'id': 'PR_seven', 'state': 'OPEN', 'isDraft': False, 'headRefOid': 'a' * 40, 'baseRefName': 'main',
                     'isCrossRepository': False, 'mergeStateStatus': 'UNSTABLE', 'reviewDecision': 'APPROVED',
                     'isMergeQueueEnabled': False}
        self.repository = {'mergeCommitAllowed': True, 'squashMergeAllowed': True, 'rebaseMergeAllowed': False}
        self.has_more_contexts = False
        self.writes = []
        self.merge_response = (200, {'merged': True, 'sha': 'd' * 40, 'message': 'Pull Request successfully merged'}, {})

    def request(self, method, path, body=None, *, permission='issues', resource='core', role=None, mutation=None):
        if method == 'PUT':
            self.assertEqual(permission, 'contents')
            self.writes.append((path, body))
            return self.merge_response
        if path == '/repos/acme/project':
            return 200, {'id': 42, 'full_name': 'acme/project', 'default_branch': 'main'}, {}
        if path == '/repos/acme/project/branches/main':
            return 200, {'name': 'main', 'commit': {'sha': 'c' * 40}}, {}
        if path == '/repos/acme/project/contents/CLAUDE.md?ref=' + 'c' * 40:
            if self.policy is None:
                return 404, None, {}
            return 200, {'type': 'file', 'path': 'CLAUDE.md', 'sha': 'e' * 40, 'encoding': 'base64',
                         'content': base64.b64encode(self.policy.encode()).decode()}, {}
        self.assertEqual((method, path, resource, mutation), ('POST', '/graphql', 'graphql', False))
        self.assertEqual(body['variables'], {'owner': 'acme', 'name': 'project', 'number': 7})
        rollup = {'contexts': {'pageInfo': {'hasNextPage': self.has_more_contexts}, 'nodes': self.contexts}}
        return 200, {'data': {'repository': self.repository | {'pullRequest': self.pull | {
            'commits': {'nodes': [{'commit': {'oid': self.pull['headRefOid'], 'statusCheckRollup': rollup}}]}}}}}, {}

    def merge(self, **fields):
        return self.actions.perform_action(self.record, 'pull_request.merge',
                                           {'number': 7, 'expected_head_sha': 'a' * 40, 'merge_method': 'squash'} | fields,
                                           self.request, actor_id=88)

    def assert_denied(self, code=None):
        with self.assertRaises(self.actions.ActionDenied) as raised:
            self.merge()
        if code is not None:
            self.assertEqual(str(raised.exception), code)
        self.assertEqual(self.writes, [])

    def test_authorized_merge_pins_head_and_records_policy_provenance(self):
        result = self.merge()
        self.assertEqual(self.writes, [('/repos/acme/project/pulls/7/merge', {'sha': 'a' * 40, 'merge_method': 'squash'})])
        self.assertEqual(result['merge_commit'], 'd' * 40)
        self.assertEqual(result['policy'], {'default_commit': 'c' * 40, 'blob': 'e' * 40})

    def test_missing_or_misplaced_authorization_denies(self):
        for policy in [None, '# Project\n\nClaude may merge pull requests.\n',
                       '## Git authorization\n\nClaude may commit.\n\n## Notes\n\nClaude may merge pull requests.\n',
                       '## Git authorization\n\nClaude may never merge pull requests.\n']:
            with self.subTest(policy=policy):
                self.policy = policy
                self.assert_denied('merge_not_authorized')

    def test_failing_pending_or_missing_required_checks_deny(self):
        for index, change in [(0, {'conclusion': 'FAILURE'}), (0, {'status': 'IN_PROGRESS', 'conclusion': None}),
                              (0, {'conclusion': 'SKIPPED'}), (1, {'state': 'PENDING'})]:
            with self.subTest(change=change):
                original = dict(self.contexts[index])
                self.contexts[index].update(change)
                self.assert_denied('required_checks_not_passing')
                self.contexts[index] = original
        self.pull['mergeStateStatus'] = 'BLOCKED'
        self.assert_denied('pull_request_not_mergeable')

    def test_stale_head_reviews_queue_drafts_and_forks_deny(self):
        for change in [{'headRefOid': 'b' * 40}, {'reviewDecision': 'REVIEW_REQUIRED'},
                       {'reviewDecision': 'CHANGES_REQUESTED'}, {'isMergeQueueEnabled': True},
                       {'isDraft': True}, {'isCrossRepository': True}, {'state': 'CLOSED'},
                       {'mergeStateStatus': 'UNKNOWN'}, {'mergeStateStatus': 'BEHIND'}]:
            with self.subTest(change=change):
                original = dict(self.pull)
                self.pull.update(change)
                self.assert_denied()
                self.pull = original

    def test_disallowed_method_or_bypass_fields_never_dispatch(self):
        with self.assertRaises(self.actions.ActionDenied):
            self.merge(merge_method='rebase')
        for extra in [{'admin': True}, {'delete_branch': True}, {'auto': True}, {'bypass_rules': True}]:
            with self.subTest(extra=extra), self.assertRaises(self.actions.ActionDenied):
                self.merge(**extra)
        self.assertEqual(self.writes, [])

    def test_incomplete_check_pages_deny(self):
        self.has_more_contexts = True
        self.assert_denied('required_checks_incomplete')

    def test_unconfirmed_merge_response_is_uncertain(self):
        self.merge_response = (200, {'merged': False, 'message': 'not merged'}, {})
        with self.assertRaisesRegex(self.actions.ActionDenied, '^action_outcome_unknown$'):
            self.merge()


if __name__ == '__main__':
    unittest.main()
