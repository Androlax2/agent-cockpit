import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch


CLIENT = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents/gh-compat.py'


class GhCompatibilityTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(CLIENT.is_file(), 'sandbox gh compatibility client is missing')
        self.scratch = tempfile.TemporaryDirectory(prefix='gh-compat-', dir='/tmp/opencode')
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.path = self.root / 'broker.sock'
        self.capability = self.root / 'capability'
        self.capability.write_text('a' * 64)
        self.capability.chmod(0o600)
        self.environment = {**os.environ, 'COCKPIT_GIT_SOCKET': str(self.path),
                            'COCKPIT_GIT_CAPABILITY_FILE': str(self.capability),
                            'COCKPIT_GITHUB_REPOSITORY': 'acme/project', 'GH_TOKEN': 'SYNTHETIC_READ_TOKEN'}
        self.requests = []
        self.result = {'request_id': 'b' * 32, 'outcome': 'succeeded', 'number': 7,
                       'html_url': 'https://github.com/acme/project/issues/7'}
        self.response_code = None
        self.results = []

    def start_broker(self, count=1):
        server = socket.socket(socket.AF_UNIX)
        server.bind(str(self.path))
        server.listen()
        server.settimeout(3)
        failures = []

        def exact(connection, size):
            chunks = b''
            while len(chunks) < size:
                chunk = connection.recv(size - len(chunks))
                if not chunk:
                    raise RuntimeError('truncated test request')
                chunks += chunk
            return chunks

        def serve():
            try:
                for _ in range(count):
                    with server.accept()[0] as connection:
                        connection.settimeout(3)
                        size = struct.unpack('!I', exact(connection, 4))[0]
                        header = json.loads(exact(connection, size))
                        body = json.loads(exact(connection, header['body_length']))
                        self.assertEqual(connection.recv(1), b'')
                        self.requests.append((header, body))
                        result = self.results.pop(0) if self.results else self.result
                        payload = json.dumps(result).encode() if self.response_code is None else b''
                        response = {'status': 200 if self.response_code is None else 403,
                                    'content_type': 'application/json', 'body_length': len(payload)}
                        if self.response_code:
                            response['code'] = self.response_code
                        encoded = json.dumps(response).encode()
                        connection.sendall(struct.pack('!I', len(encoded)) + encoded + payload)
            except Exception as error:
                failures.append(error)
            finally:
                server.close()

        thread = threading.Thread(target=serve)
        thread.start()

        def cleanup():
            thread.join(4)
            self.assertFalse(thread.is_alive())
            self.assertEqual(failures, [])

        self.addCleanup(cleanup)

    def invoke(self, arguments, stdin=None):
        return subprocess.run([sys.executable, '-I', str(CLIENT), *arguments], cwd=self.root,
                              env=self.environment, input=stdin, capture_output=True, text=True, timeout=5)

    def test_issue_create_preserves_type_label_milestone_and_body_file(self):
        (self.root / 'body.txt').write_text('Body from sandbox file')
        self.start_broker()
        result = self.invoke(['issue', 'create', '-R', 'acme/project', '--title', 'Investigate',
                              '--body-file', 'body.txt', '--type', 'Bug', '--label', 'bug,urgent', '--milestone', '2'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), 'https://github.com/acme/project/issues/7')
        header, fields = self.requests[0]
        self.assertEqual(header['action'], 'issue.create')
        self.assertEqual(fields, {'title': 'Investigate', 'body': 'Body from sandbox file',
                                 'type': 'Bug', 'labels': ['bug', 'urgent'], 'milestone': 2})
        self.assertNotIn('body.txt', json.dumps(fields))

    def test_stdin_body_is_read_inside_client_not_host(self):
        self.start_broker()
        result = self.invoke(['issue', 'create', '--title', 'Investigate', '--body-file', '-'], 'stdin body')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.requests[0][1]['body'], 'stdin body')

    def test_other_repository_is_refused_without_contacting_broker(self):
        result = self.invoke(['issue', 'close', '7', '-R', 'other/repo'])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('repository_not_enrolled', result.stderr)
        self.assertEqual(self.requests, [])

    def test_same_repo_github_url_and_host_qualified_selector_are_supported(self):
        self.start_broker()
        result = self.invoke(['pr', 'comment', 'https://github.com/acme/project/pull/7',
                              '-R', 'github.com/acme/project', '--body', 'Review notes'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.requests[0][1], {'number': 7, 'body': 'Review notes'})

    def test_cross_repository_issue_url_is_never_reinterpreted_locally(self):
        result = self.invoke(['issue', 'close', 'https://github.com/other/repo/issues/7'])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('repository_not_enrolled', result.stderr)
        self.assertEqual(self.requests, [])

    def test_close_reason_is_translated_without_generic_api_fields(self):
        self.start_broker()
        result = self.invoke(['issue', 'close', '7', '--reason', 'not planned'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.requests[0][1], {'number': 7, 'reason': 'not_planned'})

    def test_close_with_comment_dispatches_two_separately_guarded_actions(self):
        self.start_broker(count=2)
        result = self.invoke(['issue', 'close', '7', '--comment', 'Resolved by the change'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([(header['action'], fields) for header, fields in self.requests], [
            ('comment.create', {'number': 7, 'body': 'Resolved by the change'}),
            ('issue.close', {'number': 7})])

    def test_pr_comment_body_file_does_not_fall_through_to_write_token(self):
        self.start_broker()
        result = self.invoke(['pr', 'comment', '7', '--body-file', '-'], 'Review notes')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.requests[0][0]['action'], 'comment.create')
        self.assertEqual(self.requests[0][1], {'number': 7, 'body': 'Review notes'})

    def test_pr_create_keeps_explicit_head_base_draft_and_body(self):
        self.start_broker()
        result = self.invoke(['pr', 'create', '--head', 'feature/nested', '--base', 'main', '--draft',
                              '--title', 'Change', '--body-file', '-'], 'Details')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.requests[0][0]['action'], 'pull_request.create')
        self.assertEqual(self.requests[0][1], {'title': 'Change', 'body': 'Details', 'head': 'feature/nested',
                                             'base': 'main', 'draft': True})

    def test_pr_create_metadata_uses_returned_number_and_a_separate_typed_write(self):
        self.start_broker(count=2)
        result = self.invoke(['pr', 'create', '--head', 'feature', '--base', 'main', '--title', 'Change',
                              '--label', 'ready', '--milestone', '2'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([(header['action'], fields) for header, fields in self.requests], [
            ('pull_request.create', {'head': 'feature', 'base': 'main', 'title': 'Change'}),
            ('pull_request.metadata.edit', {'number': 7, 'labels': ['ready'], 'milestone': 2})])

    def test_pr_edit_metadata_only_does_not_emit_an_empty_lifecycle_edit(self):
        self.start_broker()
        result = self.invoke(['pr', 'edit', '7', '--add-label', 'ready'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.requests[0][0]['action'], 'pull_request.metadata.edit')
        self.assertEqual(self.requests[0][1], {'number': 7, 'add_labels': ['ready']})

    def test_pr_review_uses_native_current_head_read_then_pinned_typed_action(self):
        client = self.load_client()
        self.start_broker()
        native_result = subprocess.CompletedProcess([], 0, json.dumps({'headRefOid': 'a' * 40}).encode(), b'')
        with patch.dict(os.environ, self.environment), patch.object(subprocess, 'run', return_value=native_result), \
                contextlib.redirect_stdout(io.StringIO()):
            status = client.main(['pr', 'review', '7', '--approve', '--body', 'Reviewed'])
        self.assertEqual(status, 0)
        self.assertEqual(self.requests[0][0]['action'], 'review.create')
        self.assertEqual(self.requests[0][1], {'number': 7, 'expected_head_sha': 'a' * 40,
                                             'event': 'APPROVE', 'body': 'Reviewed'})

    def test_update_branch_reports_queued_not_merged_or_complete(self):
        client = self.load_client()
        self.result = {'request_id': 'b' * 32, 'outcome': 'queued', 'head_oid': 'a' * 40}
        self.start_broker()
        native_result = subprocess.CompletedProcess([], 0, json.dumps({'headRefOid': 'a' * 40}).encode(), b'')
        stdout = io.StringIO()
        with patch.dict(os.environ, self.environment), patch.object(subprocess, 'run', return_value=native_result), \
                contextlib.redirect_stdout(stdout):
            status = client.main(['pr', 'update-branch', '7'])
        self.assertEqual(status, 0)
        self.assertIn('queued', stdout.getvalue())
        self.assertEqual(self.requests[0][0]['action'], 'pull_request.update_branch')
        self.assertEqual(self.requests[0][1], {'number': 7, 'expected_head_sha': 'a' * 40})

    def test_update_branch_rebase_is_an_explicit_migration_exception(self):
        result = self.invoke(['pr', 'update-branch', '7', '--rebase'])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('command_requires_migration', result.stderr)
        self.assertEqual(self.requests, [])

    def project_page(self, item_id, status, cursor):
        return {'request_id': None, 'outcome': 'succeeded', 'next_cursor': cursor, 'items': [{
            'item_id': item_id, 'values': {'Status': status, 'Effort': 3},
            'content': {'type': 'Issue', 'title': 'Investigate', 'body': 'Details', 'number': 7,
                        'repository': 'acme/project', 'url': 'https://github.com/acme/project/issues/7'}}]}

    def test_project_item_list_json_matches_gh_shape_across_pages(self):
        self.results = [self.project_page('PVTI_a', 'Next', 'cursor-2'), self.project_page('PVTI_b', 'Done', None)]
        self.start_broker(count=2)
        result = self.invoke(['project', 'item-list', '1', '--owner', 'acme', '--format', 'json'])
        self.assertEqual(result.returncode, 0, result.stderr)
        output = json.loads(result.stdout)
        self.assertEqual(output['totalCount'], 2)
        self.assertEqual(output['items'][0], {'id': 'PVTI_a', 'status': 'Next', 'effort': 3, 'content': {
            'type': 'Issue', 'title': 'Investigate', 'body': 'Details', 'number': 7,
            'repository': 'acme/project', 'url': 'https://github.com/acme/project/issues/7'}})
        self.assertEqual([(header['action'], fields) for header, fields in self.requests], [
            ('project.items.list', {'board_number': 1, 'owner': 'acme'}),
            ('project.items.list', {'board_number': 1, 'owner': 'acme', 'cursor': 'cursor-2'})])

    def test_project_writes_and_other_project_commands_need_migration(self):
        for arguments in [['project', 'item-edit', '--id', 'PVTI_a', '--field-id', 'F', '--text', 'x'],
                          ['project', 'item-delete', '1', '--owner', 'acme', '--id', 'PVTI_a'],
                          ['project', 'item-list', '1', '--owner', 'acme', '--query', 'status:done']]:
            with self.subTest(arguments=arguments):
                result = self.invoke(arguments)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('command_requires_migration', result.stderr)
        self.assertEqual(self.requests, [])

    def test_pr_merge_pins_native_current_head_and_explicit_method(self):
        client = self.load_client()
        self.result = {'request_id': 'b' * 32, 'outcome': 'succeeded', 'merge_commit': 'f' * 40}
        self.start_broker()
        native_result = subprocess.CompletedProcess([], 0, json.dumps({'headRefOid': 'a' * 40}).encode(), b'')
        with patch.dict(os.environ, self.environment), patch.object(subprocess, 'run', return_value=native_result), \
                contextlib.redirect_stdout(io.StringIO()):
            status = client.main(['pr', 'merge', '7', '--squash'])
        self.assertEqual(status, 0)
        self.assertEqual(self.requests[0][0]['action'], 'pull_request.merge')
        self.assertEqual(self.requests[0][1], {'number': 7, 'expected_head_sha': 'a' * 40, 'merge_method': 'squash'})

    def test_match_head_commit_is_used_without_a_native_read(self):
        self.result = {'request_id': 'b' * 32, 'outcome': 'succeeded', 'merge_commit': 'f' * 40}
        self.start_broker()
        result = self.invoke(['pr', 'merge', '7', '--merge', '--match-head-commit', 'c' * 40])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.requests[0][1]['expected_head_sha'], 'c' * 40)

    def test_merge_bypass_auto_delete_and_stack_forms_never_reach_broker(self):
        for arguments, code in [(['pr', 'merge', '7', '--squash', '--admin'], 'command_requires_migration'),
                                (['pr', 'merge', '7', '--squash', '--auto'], 'command_requires_migration'),
                                (['pr', 'merge', '7', '--squash', '--delete-branch'], 'command_requires_migration'),
                                (['pr', 'merge', '7', '--squash', '-d'], 'command_requires_migration'),
                                (['pr', 'merge', '7'], 'command_requires_migration'),
                                (['stack', 'merge', '7', '--yes'], 'stack_merge_unproven')]:
            with self.subTest(arguments=arguments):
                result = self.invoke(arguments)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(code, result.stderr)
        self.assertEqual(self.requests, [])

    def test_ready_and_undo_emit_typed_actions_without_client_graphql(self):
        self.start_broker(count=2)
        for arguments in [['pr', 'ready', '7'], ['pr', 'ready', '7', '--undo']]:
            result = self.invoke(arguments)
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([(header['action'], fields) for header, fields in self.requests], [
            ('pull_request.ready', {'number': 7}), ('pull_request.draft', {'number': 7})])

    def test_issue_edit_metadata_deltas_are_not_replaced_by_empty_fields(self):
        self.start_broker()
        result = self.invoke(['issue', 'edit', '7', '--add-label', 'bug', '--remove-label', 'obsolete',
                              '--add-assignee', 'alice'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.requests[0][1], {'number': 7, 'add_labels': ['bug'],
                                             'remove_labels': ['obsolete'], 'add_assignees': ['alice']})

    def test_broker_uncertain_response_is_reported_without_retry(self):
        self.response_code = 'action_outcome_unknown'
        self.start_broker()
        result = self.invoke(['issue', 'create', '--title', 'Investigate'])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('action_outcome_unknown', result.stderr)
        self.assertEqual(len(self.requests), 1)

    def test_unsupported_mutation_never_falls_through_to_native_gh(self):
        result = self.invoke(['api', '--method', 'DELETE', 'repos/acme/project/git/refs/heads/main'])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('command_requires_migration', result.stderr)
        self.assertEqual(self.requests, [])

    def load_client(self):
        spec = importlib.util.spec_from_file_location('test_gh_client', CLIENT)
        client = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(client)
        return client

    def test_native_read_fields_and_exit_status_are_preserved(self):
        client = self.load_client()
        arguments = ['pr', 'list', '--json', 'number,title,author,isDraft,reviewDecision,statusCheckRollup,headRefOid',
                     '--jq', '.[].number', '-R', 'acme/project']
        requests = []

        def native(command, **options):
            requests.append(command)
            self.assertEqual(options['env']['GH_HOST'], 'github.com')
            self.assertEqual(options['env']['GH_TOKEN'], 'SYNTHETIC_READ_TOKEN')
            self.assertNotIn('GH_PROJECT_TOKEN', options['env'])
            print('7')
            return subprocess.CompletedProcess(command, 4)

        stdout = io.StringIO()
        with patch.dict(os.environ, self.environment), patch.object(subprocess, 'run', side_effect=native), \
                contextlib.redirect_stdout(stdout):
            status = client.main(arguments)
        self.assertEqual(status, 4)
        self.assertEqual(stdout.getvalue(), '7\n')
        self.assertEqual(requests, [['/usr/bin/gh', *arguments]])

    def test_missing_read_token_cannot_fall_back_to_personal_login(self):
        client = self.load_client()
        environment = self.environment.copy()
        environment.pop('GH_TOKEN')
        stderr = io.StringIO()
        with patch.dict(os.environ, environment, clear=True), \
                patch.object(subprocess, 'run', side_effect=AssertionError('personal-login fallback')), \
                contextlib.redirect_stderr(stderr):
            self.assertEqual(client.main(['pr', 'list']), 1)
        self.assertIn('read_credential_unavailable', stderr.getvalue())

    def test_named_milestone_is_resolved_by_native_read_without_losing_type(self):
        client = self.load_client()
        calls = []

        def native(command, **options):
            calls.append(command)
            return subprocess.CompletedProcess(command, 0, b'[[{"number":2,"title":"Release"}]]', b'')

        with patch.dict(os.environ, self.environment), patch.object(subprocess, 'run', side_effect=native):
            [(action, fields)] = client.command_actions(['issue', 'create', '--title', 'Investigate',
                                                         '--milestone', 'Release', '--type', 'Bug'])
        self.assertEqual(action, 'issue.create')
        self.assertEqual(fields, {'title': 'Investigate', 'milestone': 2, 'type': 'Bug'})
        self.assertEqual(calls, [['/usr/bin/gh', 'api',
                                 'repos/acme/project/milestones?state=all&per_page=100', '--paginate', '--slurp']])

    def test_implicit_api_post_and_graphql_cannot_enter_native_read_path(self):
        client = self.load_client()
        for arguments in [['api', 'repos/acme/project/issues', '-f', 'title=x'],
                          ['api', 'graphql', '-f', 'query=mutation {anything}'],
                          ['api', '-XPOST', 'repos/acme/project/issues'],
                          ['pr', 'merge', '7', '--admin']]:
            with self.subTest(arguments=arguments), patch.dict(os.environ, self.environment), \
                    patch.object(subprocess, 'run', side_effect=AssertionError('native mutation dispatched')), \
                    contextlib.redirect_stderr(io.StringIO()):
                self.assertNotEqual(client.main(arguments), 0)

    def test_reviewed_subissue_api_resolves_id_in_enrolled_repo_before_typed_action(self):
        client = self.load_client()
        self.start_broker()
        native_result = subprocess.CompletedProcess([], 0, b'[[{"id":108,"number":8}]]', b'')
        with patch.dict(os.environ, self.environment), patch.object(subprocess, 'run', return_value=native_result), \
                contextlib.redirect_stdout(io.StringIO()):
            status = client.main(['api', '-X', 'POST', 'repos/acme/project/issues/7/sub_issues',
                                  '-F', 'sub_issue_id=108', '-F', 'replace_parent=false'])
        self.assertEqual(status, 0)
        self.assertEqual(self.requests[0][0]['action'], 'issue.child.add')
        self.assertEqual(self.requests[0][1], {'number': 7, 'child_number': 8})

    def test_relationship_api_does_not_accept_an_id_missing_from_repo_reads(self):
        client = self.load_client()
        native_result = subprocess.CompletedProcess([], 0, b'[[{"id":108,"number":8}]]', b'')
        with patch.dict(os.environ, self.environment), patch.object(subprocess, 'run', return_value=native_result), \
                contextlib.redirect_stderr(io.StringIO()):
            status = client.main(['api', '-X', 'POST', 'repos/acme/project/issues/7/dependencies/blocked_by',
                                  '-F', 'issue_id=999'])
        self.assertNotEqual(status, 0)
        self.assertEqual(self.requests, [])


if __name__ == '__main__':
    unittest.main()
import contextlib
import importlib.util
import io
