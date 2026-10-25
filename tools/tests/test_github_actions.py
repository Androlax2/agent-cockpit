import importlib.util
from pathlib import Path
import unittest


MODULE = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents/github-actions.py'


class ActionFixture(unittest.TestCase):
    def setUp(self):
        self.assertTrue(MODULE.is_file(), 'typed GitHub actions are missing')
        spec = importlib.util.spec_from_file_location('test_github_actions', MODULE)
        self.actions = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.actions)
        self.record = {'repository_id': 42, 'canonical_name': 'acme/project'}
        self.responses = {}
        self.writes = []

    def request(self, method, path, body=None):
        if method != 'GET':
            self.writes.append((method, path, body))
        result = self.responses.get((method, path))
        self.assertIsNotNone(result, 'unexpected upstream route: ' + method + ' ' + path)
        return result

    def issue(self, number=7, **fields):
        return {'id': number + 100, 'number': number, 'title': 'Investigate', 'body': 'Details',
                'state': 'open', 'html_url': f'https://github.com/acme/project/issues/{number}',
                'repository_url': 'https://api.github.com/repos/acme/project', **fields}

    def perform(self, action, fields):
        return self.actions.issue_action(self.record, action, fields, self.request)


class IssueActionTests(ActionFixture):
    def test_invalid_unicode_cannot_escape_typed_validation(self):
        for action, fields in [('issue.create', {'title': '\ud800'}),
                               ('comment.create', {'number': 7, 'body': '\ud800'})]:
            with self.subTest(action=action), self.assertRaisesRegex(self.actions.ActionDenied, '^invalid_action_fields$'):
                self.actions.action_fields(action, fields)

    def test_edit_label_deltas_preserve_unrelated_existing_labels(self):
        self.responses = {('GET', '/repos/acme/project/issues/7'):
            (200, self.issue(labels=[{'name': 'triaged'}, {'name': 'obsolete'}]), {}),
            ('GET', '/repos/acme/project/labels?per_page=100&page=1'):
            (200, [{'name': 'triaged'}, {'name': 'bug'}, {'name': 'obsolete'}], {}),
            ('PATCH', '/repos/acme/project/issues/7'):
            (200, self.issue(labels=[{'name': 'triaged'}, {'name': 'bug'}]), {})}
        self.perform('issue.edit', {'number': 7, 'add_labels': ['bug'], 'remove_labels': ['obsolete']})
        self.assertEqual(self.writes, [('PATCH', '/repos/acme/project/issues/7', {'labels': ['triaged', 'bug']})])

    def test_conflicting_metadata_replacement_and_delta_never_dispatch(self):
        for fields in [{'number': 7, 'labels': [], 'add_labels': ['bug']},
                       {'number': 7, 'add_labels': ['bug'], 'remove_labels': ['bug']}]:
            with self.subTest(fields=fields), self.assertRaises(self.actions.ActionDenied):
                self.perform('issue.edit', fields)
        self.assertEqual(self.writes, [])

    def test_create_preserves_type_existing_labels_and_milestone(self):
        fields = {'title': 'Investigate', 'body': 'Details', 'type': 'Bug', 'labels': ['bug'], 'milestone': 2}
        self.responses = {
            ('GET', '/repos/acme/project/labels?per_page=100&page=1'): (200, [{'name': 'bug'}], {}),
            ('GET', '/repos/acme/project/issue-types?per_page=100&page=1'): (200, [{'name': 'Bug'}], {}),
            ('GET', '/repos/acme/project/milestones/2'): (200, {'number': 2}, {}),
            ('POST', '/repos/acme/project/issues'): (201, self.issue(type={'name': 'Bug'},
                labels=[{'name': 'bug'}], milestone={'number': 2}, temp_clone_token='SECRET'), {})}
        result = self.perform('issue.create', fields)
        self.assertEqual(self.writes, [('POST', '/repos/acme/project/issues', fields)])
        self.assertEqual(result['number'], 7)
        self.assertNotIn('temp_clone_token', result)
        self.assertEqual(result['applied_fields'], fields)

    def test_issue_close_cannot_close_a_pull_request(self):
        self.responses[('GET', '/repos/acme/project/issues/7')] = (200, self.issue(pull_request={}), {})
        with self.assertRaisesRegex(self.actions.ActionDenied, '^issue_target_invalid$'):
            self.perform('issue.close', {'number': 7})
        self.assertEqual(self.writes, [])

    def test_unknown_fields_and_cross_repository_selectors_never_dispatch(self):
        for fields in [{'title': 'x', 'url': 'https://evil.invalid'}, {'number': True},
                       {'number': 7, 'repository': 'other/repo'}, {'number': 7, 'state': 'closed'}]:
            with self.subTest(fields=fields), self.assertRaises(self.actions.ActionDenied):
                self.perform('issue.edit', fields)
        self.assertEqual(self.writes, [])

    def test_relationship_resolves_both_issue_ids_from_enrolled_repository(self):
        self.responses = {('GET', '/repos/acme/project/issues/7'): (200, self.issue(), {}),
                          ('GET', '/repos/acme/project/issues/8'): (200, self.issue(8), {}),
                          ('POST', '/repos/acme/project/issues/7/sub_issues'): (201, self.issue(8), {})}
        self.perform('issue.child.add', {'number': 7, 'child_number': 8})
        self.assertEqual(self.writes, [('POST', '/repos/acme/project/issues/7/sub_issues',
                                       {'sub_issue_id': 108, 'replace_parent': False})])

    def test_wrong_repo_relationship_target_never_dispatches(self):
        self.responses = {('GET', '/repos/acme/project/issues/7'): (200, self.issue(), {}),
                          ('GET', '/repos/acme/project/issues/8'): (200, self.issue(8,
                              repository_url='https://api.github.com/repos/other/repo'), {})}
        with self.assertRaises(self.actions.ActionDenied):
            self.perform('issue.blocker.add', {'number': 7, 'blocker_number': 8})
        self.assertEqual(self.writes, [])

    def test_unknown_label_is_not_silently_created(self):
        self.responses[('GET', '/repos/acme/project/labels?per_page=100&page=1')] = (200, [], {})
        with self.assertRaisesRegex(self.actions.ActionDenied, '^issue_metadata_unavailable$'):
            self.perform('issue.create', {'title': 'x', 'labels': ['missing']})
        self.assertEqual(self.writes, [])

    def test_silently_dropped_metadata_reports_unconfirmed_outcome(self):
        self.responses = {
            ('GET', '/repos/acme/project/issue-types?per_page=100&page=1'): (200, [{'name': 'Bug'}], {}),
            ('POST', '/repos/acme/project/issues'): (201, self.issue(), {})}
        with self.assertRaisesRegex(self.actions.ActionDenied, '^action_outcome_unknown$'):
            self.perform('issue.create', {'title': 'Investigate', 'type': 'Bug'})
        self.assertEqual(len(self.writes), 1)

    def test_partial_catalog_cannot_be_treated_as_complete(self):
        self.responses = {('GET', '/repos/acme/project/labels?per_page=100&page=1'):
                          (200, [{'name': 'bug'}], {'link': '<https://evil.invalid/page>; rel="next"'}),
                          ('GET', '/repos/acme/project/labels?per_page=100&page=2'): (403, {}, {})}
        with self.assertRaises(self.actions.ActionDenied):
            self.perform('issue.create', {'title': 'x', 'labels': ['bug']})
        self.assertEqual(self.writes, [])

    def test_missing_mutation_state_is_unknown_not_an_internal_error(self):
        result = self.issue()
        del result['state']
        self.responses[('POST', '/repos/acme/project/issues')] = (201, result, {})
        with self.assertRaisesRegex(self.actions.ActionDenied, '^action_outcome_unknown$'):
            self.perform('issue.create', {'title': 'Investigate'})

    def test_close_and_reopen_preserve_explicit_state_reason(self):
        for action, fields, state, reason in [
                ('issue.close', {'number': 7, 'reason': 'not_planned'}, 'closed', 'not_planned'),
                ('issue.reopen', {'number': 7}, 'open', 'reopened')]:
            with self.subTest(action=action):
                self.responses = {('GET', '/repos/acme/project/issues/7'): (200, self.issue(), {}),
                    ('PATCH', '/repos/acme/project/issues/7'): (200, self.issue(state=state, state_reason=reason), {})}
                result = self.perform(action, fields)
                self.assertEqual(result['applied_fields'], {'state': state, 'state_reason': reason})

    def test_edit_checks_assignees_were_not_silently_dropped(self):
        self.responses = {('GET', '/repos/acme/project/issues/7'): (200, self.issue(), {}),
            ('PATCH', '/repos/acme/project/issues/7'): (200, self.issue(assignees=[]), {})}
        with self.assertRaisesRegex(self.actions.ActionDenied, '^action_outcome_unknown$'):
            self.perform('issue.edit', {'number': 7, 'assignees': ['alice']})


class DiscussionActionTests(ActionFixture):
    def perform(self, action, fields):
        return self.actions.perform_action(self.record, action, fields, self.request, actor_id=88)

    def request(self, method, path, body=None, *, permission='issues'):
        self.permission = permission
        return super().request(method, path, body)

    def comment(self, **fields):
        return {'id': 501, 'body': 'Review notes', 'user': {'id': 88},
                'issue_url': 'https://api.github.com/repos/acme/project/issues/7',
                'html_url': 'https://github.com/acme/project/issues/7#issuecomment-501', **fields}

    def test_pr_discussion_uses_pull_request_permission_not_issue_escalation(self):
        self.responses = {('GET', '/repos/acme/project/issues/7'): (200, self.issue(pull_request={}), {}),
                          ('POST', '/repos/acme/project/issues/7/comments'): (201, self.comment(), {})}
        result = self.perform('comment.create', {'number': 7, 'body': 'Review notes'})
        self.assertEqual(self.permission, 'pull_requests')
        self.assertEqual(result['id'], 501)
        self.assertEqual(result['outcome'], 'succeeded')

    def test_comment_edit_requires_same_parent_and_actual_actor(self):
        for changed in [{'user': {'id': 99}}, {'issue_url': 'https://api.github.com/repos/other/repo/issues/7'},
                        {'issue_url': 'https://api.github.com/repos/acme/project/issues/8'}]:
            with self.subTest(changed=changed):
                self.responses = {('GET', '/repos/acme/project/issues/7'): (200, self.issue(), {}),
                    ('GET', '/repos/acme/project/issues/comments/501'): (200, self.comment(**changed), {})}
                with self.assertRaises(self.actions.ActionDenied):
                    self.perform('comment.edit', {'number': 7, 'comment_id': 501, 'body': 'New notes'})
        self.assertEqual(self.writes, [])

    def test_own_comment_edit_has_no_delete_or_author_override(self):
        self.responses = {('GET', '/repos/acme/project/issues/7'): (200, self.issue(), {}),
            ('GET', '/repos/acme/project/issues/comments/501'): (200, self.comment(), {}),
            ('PATCH', '/repos/acme/project/issues/comments/501'): (200, self.comment(body='New notes'), {})}
        self.perform('comment.edit', {'number': 7, 'comment_id': 501, 'body': 'New notes'})
        self.assertEqual(self.writes, [('PATCH', '/repos/acme/project/issues/comments/501', {'body': 'New notes'})])


class PullRequestActionTests(ActionFixture):
    def request(self, method, path, body=None, *, permission='pull_requests'):
        self.assertEqual(permission, 'contents' if method == 'PUT' and path.endswith('/update-branch') else 'pull_requests')
        return super().request(method, path, body)

    def perform(self, action, fields):
        return self.actions.perform_action(self.record, action, fields, self.request, actor_id=88)

    def pull_request(self, **fields):
        return {'id': 207, 'number': 7, 'state': 'open', 'title': 'Change', 'body': 'Details',
                'user': {'id': 99}, 'head': {'ref': 'feature/nested', 'sha': 'a' * 40, 'repo': {'id': 42}},
                'base': {'ref': 'main', 'sha': 'b' * 40, 'repo': {'id': 42}},
                'html_url': 'https://github.com/acme/project/pull/7', **fields}

    def review(self, **fields):
        return {'id': 301, 'commit_id': 'a' * 40, 'state': 'COMMENTED', 'body': '', 'user': {'id': 88},
                'pull_request_url': 'https://api.github.com/repos/acme/project/pulls/7', **fields}

    def test_create_checks_same_repo_branch_names_and_preserves_draft(self):
        self.responses = {
            ('GET', '/repos/acme/project/branches/feature%2Fnested'):
                (200, {'name': 'feature/nested', 'commit': {'sha': 'a' * 40}}, {}),
            ('GET', '/repos/acme/project/branches/main'):
                (200, {'name': 'main', 'commit': {'sha': 'b' * 40}}, {}),
            ('POST', '/repos/acme/project/pulls'): (201, self.pull_request(draft=True), {})}
        fields = {'title': 'Change', 'body': 'Details', 'head': 'feature/nested', 'base': 'main', 'draft': True}
        result = self.perform('pull_request.create', fields)
        self.assertEqual(self.writes, [('POST', '/repos/acme/project/pulls', fields)])
        self.assertEqual(result['head_oid'], 'a' * 40)

    def test_close_never_deletes_branch_or_uses_issue_state_bypass(self):
        self.responses = {('GET', '/repos/acme/project/pulls/7'): (200, self.pull_request(), {}),
                          ('PATCH', '/repos/acme/project/pulls/7'): (200, self.pull_request(state='closed'), {})}
        self.perform('pull_request.close', {'number': 7})
        self.assertEqual(self.writes, [('PATCH', '/repos/acme/project/pulls/7', {'state': 'closed'})])

    def test_pr_metadata_edits_classify_target_without_issue_state_bypass(self):
        self.responses = {('GET', '/repos/acme/project/pulls/7'): (200, self.pull_request(), {}),
            ('GET', '/repos/acme/project/issues/7'):
                (200, self.issue(pull_request={}, labels=[{'name': 'triaged'}]), {}),
            ('GET', '/repos/acme/project/labels?per_page=100&page=1'):
                (200, [{'name': 'triaged'}, {'name': 'ready'}], {}),
            ('PATCH', '/repos/acme/project/issues/7'):
                (200, self.issue(pull_request={}, labels=[{'name': 'triaged'}, {'name': 'ready'}],
                                 html_url='https://github.com/acme/project/pull/7'), {})}
        self.perform('pull_request.metadata.edit', {'number': 7, 'add_labels': ['ready']})
        self.assertEqual(self.writes, [('PATCH', '/repos/acme/project/issues/7', {'labels': ['triaged', 'ready']})])
        self.writes.clear()
        with self.assertRaises(self.actions.ActionDenied):
            self.perform('pull_request.metadata.edit', {'number': 7, 'state': 'closed'})
        self.assertEqual(self.writes, [])

    def test_cross_repository_heads_and_client_admin_flags_never_dispatch(self):
        self.responses = {('GET', '/repos/acme/project/pulls/7'):
            (200, self.pull_request(head={'ref': 'feature', 'sha': 'a' * 40, 'repo': {'id': 99}}), {})}
        for action, fields in [('pull_request.edit', {'number': 7, 'title': 'New title'}),
                               ('pull_request.close', {'number': 7, 'delete_branch': True}),
                               ('pull_request.create', {'head': 'other:branch', 'base': 'main', 'title': 'x'})]:
            with self.subTest(action=action), self.assertRaises(self.actions.ActionDenied):
                self.perform(action, fields)
        self.assertEqual(self.writes, [])

    def test_review_pins_expected_head_and_rejects_self_approval(self):
        self.responses = {('GET', '/repos/acme/project/pulls/7'): (200, self.pull_request(), {}),
            ('POST', '/repos/acme/project/pulls/7/reviews'):
            (200, self.review(state='APPROVED', body='Reviewed'), {})}
        fields = {'number': 7, 'expected_head_sha': 'a' * 40, 'body': 'Reviewed', 'event': 'APPROVE'}
        result = self.perform('review.create', fields)
        self.assertEqual(self.writes, [('POST', '/repos/acme/project/pulls/7/reviews',
                                       {'commit_id': 'a' * 40, 'body': 'Reviewed', 'event': 'APPROVE'})])
        self.assertEqual(result['review_state'], 'APPROVED')
        self.writes.clear()
        for changed in [self.pull_request(user={'id': 88}),
                        self.pull_request(head={'ref': 'feature/nested', 'sha': 'c' * 40, 'repo': {'id': 42}})]:
            self.responses[('GET', '/repos/acme/project/pulls/7')] = (200, changed, {})
            with self.assertRaises(self.actions.ActionDenied):
                self.perform('review.create', fields)
        self.assertEqual(self.writes, [])

    def test_review_inline_fields_cannot_select_unbounded_or_other_head_operations(self):
        for comments in [[{'path': 'file.py', 'line': 0, 'side': 'RIGHT', 'body': 'x'}],
                         [{'path': 'file.py', 'line': 1, 'side': 'RIGHT', 'body': 'x', 'commit_id': 'c' * 40}],
                         [{'path': '../file.py', 'line': 1, 'side': 'RIGHT', 'body': 'x'}]]:
            with self.subTest(comments=comments), self.assertRaises(self.actions.ActionDenied):
                self.perform('review.create', {'number': 7, 'expected_head_sha': 'a' * 40,
                    'event': 'COMMENT', 'body': 'Notes', 'comments': comments})
        self.assertEqual(self.writes, [])

    def test_inline_review_accepts_hidden_paths_spaces_and_lock_files(self):
        paths = ['.github/workflows/check.yml', 'docs/review notes.md', 'package.lock']
        self.responses = {('GET', '/repos/acme/project/pulls/7'): (200, self.pull_request(), {}),
            ('GET', '/repos/acme/project/pulls/7/files?per_page=100&page=1'):
                (200, [{'filename': path} for path in paths], {}),
            ('POST', '/repos/acme/project/pulls/7/reviews'):
                (200, self.review(body='Notes'), {})}
        comments = [{'path': path, 'line': 1, 'side': 'RIGHT', 'body': 'Please check'} for path in paths]
        self.perform('review.create', {'number': 7, 'expected_head_sha': 'a' * 40, 'event': 'COMMENT',
                                       'body': 'Notes', 'comments': comments})
        self.assertEqual(self.writes[0][2]['comments'], comments)

    def test_owned_pending_review_submit_and_review_body_edit_preserve_head(self):
        cases = [
            ('review.submit', {'review_id': 301, 'event': 'COMMENT', 'body': 'Ready'}, 'POST',
             '/repos/acme/project/pulls/7/reviews/301/events', 'PENDING', 'COMMENTED'),
            ('review.edit', {'review_id': 301, 'body': 'Ready'}, 'PUT',
             '/repos/acme/project/pulls/7/reviews/301', 'COMMENTED', 'COMMENTED')]
        for action, extra, method, path, initial, final in cases:
            with self.subTest(action=action):
                review = self.review(state=initial)
                self.responses = {('GET', '/repos/acme/project/pulls/7'): (200, self.pull_request(), {}),
                    ('GET', '/repos/acme/project/pulls/7/reviews/301'): (200, review, {}),
                    (method, path): (200, review | {'body': 'Ready', 'state': final}, {})}
                result = self.perform(action, {'number': 7, 'expected_head_sha': 'a' * 40, **extra})
                self.assertEqual(result['review_state'], final)
                self.assertEqual(self.writes[-1], (method, path, {name: extra[name] for name in ('body', 'event') if name in extra}))

    def test_review_edit_cannot_change_another_actors_review(self):
        self.responses = {('GET', '/repos/acme/project/pulls/7'): (200, self.pull_request(), {}),
            ('GET', '/repos/acme/project/pulls/7/reviews/301'):
            (200, self.review(user={'id': 99}), {})}
        with self.assertRaises(self.actions.ActionDenied):
            self.perform('review.edit', {'number': 7, 'expected_head_sha': 'a' * 40, 'review_id': 301, 'body': 'Edited'})
        self.assertEqual(self.writes, [])

    def test_review_id_with_wrong_parent_url_never_reaches_mutation(self):
        self.responses = {('GET', '/repos/acme/project/pulls/7'): (200, self.pull_request(), {}),
            ('GET', '/repos/acme/project/pulls/7/reviews/301'):
                (200, self.review(pull_request_url='https://api.github.com/repos/other/repo/pulls/7'), {}),
            ('PUT', '/repos/acme/project/pulls/7/reviews/301'): (200, self.review(body='Edited'), {})}
        with self.assertRaises(self.actions.ActionDenied):
            self.perform('review.edit', {'number': 7, 'expected_head_sha': 'a' * 40, 'review_id': 301, 'body': 'Edited'})
        self.assertEqual(self.writes, [])

    def test_inline_reply_resolves_same_pr_comment_before_fixed_reply_route(self):
        comment = {'id': 401, 'pull_request_url': 'https://api.github.com/repos/acme/project/pulls/7'}
        self.responses = {('GET', '/repos/acme/project/pulls/7'): (200, self.pull_request(), {}),
            ('GET', '/repos/acme/project/pulls/comments/401'): (200, comment, {}),
            ('POST', '/repos/acme/project/pulls/7/comments/401/replies'):
            (201, comment | {'id': 402, 'body': 'Resolved', 'user': {'id': 88}}, {})}
        result = self.perform('review.reply', {'number': 7, 'expected_head_sha': 'a' * 40,
                                              'comment_id': 401, 'body': 'Resolved'})
        self.assertEqual(result['id'], 402)
        self.assertEqual(self.writes, [('POST', '/repos/acme/project/pulls/7/comments/401/replies', {'body': 'Resolved'})])

    def test_update_branch_is_merge_forward_with_expected_head_and_queued_outcome(self):
        self.responses = {('GET', '/repos/acme/project/pulls/7'): (200, self.pull_request(), {}),
            ('PUT', '/repos/acme/project/pulls/7/update-branch'): (202, None, {})}
        result = self.perform('pull_request.update_branch', {'number': 7, 'expected_head_sha': 'a' * 40})
        self.assertEqual(result, {'outcome': 'queued', 'head_oid': 'a' * 40})
        self.assertEqual(self.writes, [('PUT', '/repos/acme/project/pulls/7/update-branch',
                                       {'expected_head_sha': 'a' * 40})])
        self.writes.clear()
        with self.assertRaises(self.actions.ActionDenied):
            self.perform('pull_request.update_branch', {'number': 7, 'expected_head_sha': 'a' * 40, 'rebase': True})
        self.assertEqual(self.writes, [])


if __name__ == '__main__':
    unittest.main()
