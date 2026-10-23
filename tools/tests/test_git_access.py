import json
import contextlib
import copy
from datetime import datetime, timedelta, timezone
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


REPO = Path(__file__).resolve().parents[2]
HELPER = REPO / 'config/bin/.local/bin/greywall-agents/git-access.py'


class GitAccessFixture(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix='git-access-', dir='/tmp/opencode')
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.home = self.root / 'home'
        self.home.mkdir()
        self.project = self.root / 'project'
        self.git('init', '-q', str(self.project))
        self.git('-C', str(self.project), 'remote', 'add', 'origin', 'https://github.com/acme/project.git')
        self.config = self.home / '.config/greywall-agents'
        self.config.mkdir(parents=True, mode=0o700)
        self.registry = self.config / 'git-repositories.json'
        self.environment = os.environ | {
            'HOME': str(self.home), 'XDG_CONFIG_HOME': str(self.home / '.config'),
            'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null',
            'GIT_AUTHOR_NAME': 'Fixture', 'GIT_AUTHOR_EMAIL': 'fixture@example.invalid',
            'GIT_COMMITTER_NAME': 'Fixture', 'GIT_COMMITTER_EMAIL': 'fixture@example.invalid',
        }

    def git(self, *arguments):
        result = subprocess.run(['git', *arguments], capture_output=True, text=True,
                                env=os.environ | {'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null'})
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def binding(self, checkout):
        return {
            'path': str(checkout),
            'git_dir': self.git('-C', str(checkout), 'rev-parse', '--absolute-git-dir'),
            'common_dir': str((checkout / self.git('-C', str(checkout), 'rev-parse', '--git-common-dir')).resolve()),
        }

    def enroll_fixture(self, checkouts=None, edges=None):
        record = {
            'project': 'project', 'repository_id': 42, 'canonical_name': 'acme/project',
            'checkouts': checkouts or [self.binding(self.project)], 'mirror_edges': edges or [],
        }
        self.registry.write_text(json.dumps({'version': 1, 'repositories': [record]}))
        self.registry.chmod(0o600)
        return record

    def inspect(self, checkout=None, extra=None):
        result = subprocess.run([sys.executable, str(HELPER), 'inspect', '--checkout', str(checkout or self.project)],
                                env=self.environment | (extra or {}), capture_output=True, text=True)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(result.stderr, '')
        report = json.loads(result.stdout)
        self.assertEqual(report['mode'], 'read_only')
        return report


class GitAccessTests(GitAccessFixture):
    def test_unregistered_project_is_read_only(self):
        self.assertEqual(self.inspect()['reason'], 'unregistered_project')
        self.assertFalse(self.registry.exists())

    def test_same_basename_never_selects_a_credential(self):
        self.enroll_fixture()
        other = self.root / 'other/project'
        self.git('init', '-q', str(other))
        self.assertEqual(self.inspect(other)['reason'], 'unregistered_project')

    def test_local_identity_is_not_write_admission(self):
        self.enroll_fixture()
        report = self.inspect()
        self.assertEqual(report['reason'], 'inspector_unavailable')
        self.assertTrue(report['identity_verified'])
        self.assertEqual(report['repository_id'], 42)

    def test_subdirectory_resolves_to_the_enrolled_checkout(self):
        self.enroll_fixture()
        directory = self.project / 'src'
        directory.mkdir()
        self.assertEqual(self.inspect(directory)['checkout'], str(self.project))

    def test_checkout_paths_with_spaces_are_not_shell_commands(self):
        checkout = self.root / 'project with spaces'
        self.git('init', '-q', str(checkout))
        self.git('-C', str(checkout), 'remote', 'add', 'origin', 'https://github.com/acme/project.git')
        self.enroll_fixture([self.binding(checkout)])
        self.assertTrue(self.inspect(checkout)['identity_verified'])

    def test_separate_git_directory_must_match_the_explicit_binding(self):
        checkout, metadata = self.root / 'separate', self.root / 'separate-metadata'
        self.git('init', '-q', '--separate-git-dir', str(metadata), str(checkout))
        self.git('-C', str(checkout), 'remote', 'add', 'origin', 'https://github.com/acme/project.git')
        self.enroll_fixture([self.binding(checkout)])
        report = self.inspect(checkout)
        self.assertTrue(report['identity_verified'])
        self.assertEqual(report['common_dir'], str(metadata))

    def test_submodule_gitfile_is_inspected_without_granting_its_parent(self):
        source = self.root / 'submodule-source'
        self.git('init', '-q', str(source))
        self.git('-C', str(source), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                 'commit', '-qm', 'Fixture', '--allow-empty')
        self.git('-C', str(self.project), '-c', 'protocol.file.allow=always', 'submodule', 'add', '-q',
                 str(source), 'component')
        checkout = self.project / 'component'
        self.git('-C', str(checkout), 'remote', 'set-url', 'origin', 'https://github.com/acme/project.git')
        self.enroll_fixture([self.binding(checkout)])
        report = self.inspect(checkout)
        self.assertTrue(report['identity_verified'])
        self.assertEqual(report['checkout'], str(checkout))
        self.assertEqual(self.inspect(self.project)['reason'], 'unregistered_project')

    def test_admin_script_exposes_inspection_without_origin_based_token_lookup(self):
        result = subprocess.run(['bash', str(REPO / 'tools/agent-github.sh'), 'inspect', '--checkout', str(self.project)],
                                env=self.environment, cwd='/', capture_output=True, text=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(json.loads(result.stdout)['reason'], 'unregistered_project')

    def test_host_inspection_does_not_execute_project_python_startup(self):
        startup = self.root / 'project-python-startup'
        startup.mkdir()
        marker = self.root / 'startup-executed'
        (startup / 'sitecustomize.py').write_text(f'from pathlib import Path\nPath({str(marker)!r}).touch()\n')
        result = subprocess.run(['bash', str(REPO / 'tools/agent-github.sh'), 'inspect', '--checkout', str(self.project)],
                                env=self.environment | {'PYTHONPATH': str(startup)},
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(json.loads(result.stdout)['reason'], 'unregistered_project')
        self.assertFalse(marker.exists(), 'host inspection executed a checkout-controlled Python startup file')

    def test_host_inspection_does_not_find_its_tools_on_project_path(self):
        tools = self.root / 'project-bin'
        tools.mkdir()
        marker = self.root / 'tool-executed'
        for name in ['python3', 'realpath', 'dirname']:
            program = tools / name
            program.write_text(f'#!/bin/sh\ntouch "{marker}"\nexec /usr/bin/{name} "$@"\n')
            program.chmod(0o700)
        result = subprocess.run(['bash', str(REPO / 'tools/agent-github.sh'), 'inspect', '--checkout', str(self.project)],
                                env=self.environment | {'PATH': str(tools) + ':' + self.environment['PATH']},
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(json.loads(result.stdout)['reason'], 'unregistered_project')
        self.assertFalse(marker.exists(), 'host inspection executed a checkout-controlled tool')

    def test_make_inspection_target_preserves_the_json_denial(self):
        result = subprocess.run(['make', '--no-print-directory', 'agent-github-inspect', f'CHECKOUT={self.project}'],
                                cwd=REPO, env=self.environment, capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertTrue(result.stdout.strip(), result.stderr)
        self.assertEqual(json.loads(result.stdout)['reason'], 'unregistered_project')

    def test_make_enrollment_target_does_not_infer_an_app_or_identity(self):
        self.enroll_fixture()
        original = self.registry.read_bytes()
        result = subprocess.run([
            'make', '--no-print-directory', 'agent-github-enroll', f'CHECKOUT={self.project}',
            'PROJECT=project', 'REPOSITORY=acme/project', 'REPOSITORY_ID=42', 'INSPECTOR_APP_ID=123',
            'HISTORY_RULESET=100', 'DEFAULT_PR_RULESET=101',
        ], cwd=REPO, env=self.environment, input='yes\n', capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertTrue(result.stdout.strip(), result.stderr)
        self.assertEqual(json.loads(result.stdout)['reason'], 'inspector_unavailable')
        self.assertEqual(self.registry.read_bytes(), original)

    def test_nested_unregistered_repository_does_not_borrow_parent_identity(self):
        self.enroll_fixture()
        nested = self.project / 'nested'
        self.git('init', '-q', str(nested))
        self.assertEqual(self.inspect(nested)['reason'], 'unregistered_project')

    def test_inherited_git_configuration_is_not_identity_authority(self):
        self.enroll_fixture()
        report = self.inspect(extra={
            'GIT_DIR': str(self.root / 'fake'), 'GIT_WORK_TREE': str(self.root / 'fake'),
            'GIT_CONFIG_COUNT': '1', 'GIT_CONFIG_KEY_0': 'remote.origin.url',
            'GIT_CONFIG_VALUE_0': 'https://github.com/other/stolen', 'GH_TOKEN': 'ghp_SYNTHETIC_NOT_A_REAL_TOKEN',
        })
        self.assertTrue(report['identity_verified'])
        self.assertNotIn('ghp_', json.dumps(report))

    def test_remote_change_requires_reapproval(self):
        self.enroll_fixture()
        self.git('-C', str(self.project), 'remote', 'set-url', 'origin', 'https://github.com/acme/replacement')
        self.assertEqual(self.inspect()['reason'], 'remote_identity_mismatch')

    def test_supported_remote_spellings_have_the_same_local_identity(self):
        self.enroll_fixture()
        for remote in ['https://github.com/acme/project', 'git@github.com:acme/project.git',
                       'ssh://git@github.com/acme/project.git']:
            with self.subTest(remote=remote):
                self.git('-C', str(self.project), 'remote', 'set-url', 'origin', remote)
                self.assertTrue(self.inspect()['identity_verified'])

    def test_unsupported_or_credential_bearing_urls_are_not_echoed(self):
        self.enroll_fixture()
        for remote in ['https://user:password@github.com/acme/project',
                       'https://github.com.evil.invalid/acme/project',
                       'https://github.com/acme/project?secret=value',
                       'ext::sh -c touch-should-never-run']:
            with self.subTest(remote=remote):
                self.git('-C', str(self.project), 'config', 'remote.origin.url', remote)
                report = self.inspect()
                self.assertEqual(report['reason'], 'unsupported_remote')
                self.assertNotIn('password', json.dumps(report))

    def test_includes_and_url_rewrites_fail_before_external_programs_run(self):
        self.enroll_fixture()
        for key, value in [('include.path', '/unreadable/secret'),
                           ('includeIf.gitdir:/.path', '/unreadable/secret'),
                           ('url.https://evil.invalid/.insteadOf', 'https://github.com/'),
                           ('remote.origin.pushurl', 'https://evil.invalid/project')]:
            with self.subTest(key=key):
                self.git('-C', str(self.project), 'config', key, value)
                self.assertEqual(self.inspect()['reason'], 'unsupported_git_configuration')
                self.git('-C', str(self.project), 'config', '--unset-all', key)

    def test_hostile_git_config_does_not_execute(self):
        self.enroll_fixture()
        marker = self.root / 'executed'
        program = self.root / 'hostile'
        program.write_text(f'#!/bin/sh\ntouch "{marker}"\n')
        program.chmod(0o700)
        for key in ['core.fsmonitor', 'core.sshCommand', 'credential.helper', 'core.pager']:
            self.git('-C', str(self.project), 'config', key, str(program))
        self.assertTrue(self.inspect()['identity_verified'])
        self.assertFalse(marker.exists())

    def test_local_mirror_needs_an_enrolled_edge(self):
        mirror = self.root / 'mirror'
        self.git('init', '-q', str(mirror))
        self.git('-C', str(mirror), 'remote', 'add', 'origin', 'https://github.com/acme/project.git')
        self.git('-C', str(self.project), 'remote', 'set-url', 'origin', str(mirror))
        self.enroll_fixture([self.binding(self.project), self.binding(mirror)])
        self.assertEqual(self.inspect()['reason'], 'unapproved_local_mirror')
        self.enroll_fixture([self.binding(self.project), self.binding(mirror)],
                            [{'checkout': str(self.project), 'origin': str(mirror)}])
        self.assertTrue(self.inspect()['identity_verified'])

    def test_mirror_cycle_fails_closed(self):
        self.git('-C', str(self.project), 'remote', 'set-url', 'origin', str(self.project))
        self.enroll_fixture(edges=[{'checkout': str(self.project), 'origin': str(self.project)}])
        self.assertEqual(self.inspect()['reason'], 'mirror_cycle')

    def test_worktrees_share_identity_without_granting_siblings(self):
        self.git('-C', str(self.project), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                 'commit', '-qm', 'Fixture', '--allow-empty')
        worktree = self.root / 'linked'
        self.git('-C', str(self.project), 'worktree', 'add', '-qb', 'fixture-linked', str(worktree))
        self.enroll_fixture([self.binding(self.project), self.binding(worktree)])
        report = self.inspect(worktree)
        self.assertTrue(report['identity_verified'])
        self.assertEqual(report['common_dir'], str(self.project / '.git'))
        self.assertNotIn('allow_paths', report)
        self.assertNotIn(str(self.project / 'src'), json.dumps(report))

    def test_metadata_pointer_change_requires_reenrollment(self):
        self.enroll_fixture()
        other = self.root / 'other'
        self.git('init', '-q', str(other))
        (self.project / '.git').rename(self.project / '.git-original')
        (self.project / '.git').write_text(f'gitdir: {other / ".git"}\n')
        self.assertEqual(self.inspect()['reason'], 'git_metadata_changed')

    def test_symlink_checkout_and_config_fail_closed(self):
        self.enroll_fixture()
        alias = self.root / 'alias'
        alias.symlink_to(self.project, target_is_directory=True)
        self.assertEqual(self.inspect(alias)['reason'], 'symlink_identity_path')
        config = self.project / '.git/config'
        original = config.read_text()
        config.unlink()
        target = self.root / 'config'
        target.write_text(original)
        config.symlink_to(target)
        self.assertEqual(self.inspect()['reason'], 'symlink_identity_path')

    def test_missing_remote_and_non_git_directories_remain_read_only(self):
        self.enroll_fixture()
        self.git('-C', str(self.project), 'remote', 'remove', 'origin')
        self.assertEqual(self.inspect()['reason'], 'missing_origin')
        folder = self.root / 'plain'
        folder.mkdir()
        self.assertEqual(self.inspect(folder)['reason'], 'not_a_repository')

    def test_registry_schema_and_permissions_fail_closed(self):
        record = self.enroll_fixture()
        for value in [[], {'version': 2, 'repositories': []},
                      {'version': 1, 'repositories': [record, record]},
                      {'version': 1, 'repositories': [record | {'repository_id': True}]}]:
            with self.subTest(value=value):
                self.registry.write_text(json.dumps(value))
                self.assertEqual(self.inspect()['reason'], 'invalid_registry')
        self.enroll_fixture()
        self.registry.chmod(0o666)
        self.assertEqual(self.inspect()['reason'], 'untrusted_registry_permissions')

    def test_enrollment_never_persists_without_app_verification(self):
        before = self.enroll_fixture()
        original = self.registry.read_bytes()
        result = subprocess.run([sys.executable, str(HELPER), 'enroll', '--checkout', str(self.project),
                                 '--repository', 'acme/project', '--project', 'project', '--repository-id', '42',
                                 '--inspector-app-id', '123', '--history-ruleset', '100', '--default-pr-ruleset', '101'],
                                input='yes\n', env=self.environment,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(json.loads(result.stdout)['reason'], 'inspector_unavailable')
        self.assertEqual(self.registry.read_bytes(), original)
        self.assertEqual(json.loads(original)['repositories'][0], before)

    def test_registered_mirror_only_is_not_a_new_checkout_grant(self):
        self.enroll_fixture()
        alias = self.root / 'new-clone'
        self.git('init', '-q', str(alias))
        self.git('-C', str(alias), 'remote', 'add', 'origin', str(self.project))
        self.assertEqual(self.inspect(alias)['reason'], 'unregistered_project')

    def test_malformed_mirror_edges_are_json_denials_not_tracebacks(self):
        record = self.enroll_fixture()
        for edge in [{'checkout': [], 'origin': str(self.project)},
                     {'checkout': str(self.project), 'origin': {}}, 'not an edge']:
            with self.subTest(edge=edge):
                record['mirror_edges'] = [edge]
                self.registry.write_text(json.dumps({'version': 1, 'repositories': [record]}))
                self.assertEqual(self.inspect()['reason'], 'invalid_registry')

    def test_duplicate_registry_json_keys_are_rejected(self):
        self.enroll_fixture()
        self.registry.write_text('{"version":1,"version":1,"repositories":[]}')
        self.assertEqual(self.inspect()['reason'], 'invalid_registry')


class ProtectionSnapshotFixture(GitAccessFixture):
    def setUp(self):
        super().setUp()
        self.record = self.enroll_fixture()
        self.record['baseline_rulesets'] = {'history': 100, 'default_pr': 101}
        self.app = {
            'app_id': 123, 'client_id': 'IvFixtureApp', 'installation_id': 77, 'owner_id': 9,
            'owner_login': 'acme', 'approved_repository_ids': [42],
            'key_epoch': 'fixture-key-1', 'setup_approved': True,
            'permissions': {'metadata': 'read', 'administration': 'read'},
        }
        self.record.update(inspector_app_id=123, installation_id=77)
        self.registry.write_text(json.dumps({'version': 1, 'inspector_apps': [self.app], 'repositories': [self.record]}))
        module_spec = importlib.util.spec_from_file_location('cockpit_git_access', HELPER)
        self.access = importlib.util.module_from_spec(module_spec)
        module_spec.loader.exec_module(self.access)
        self.snapshot = {
            'complete': True,
            'repository': {'id': 42, 'full_name': 'acme/project', 'default_branch': 'main'},
            'rulesets': [
                {'id': 100, 'name': 'agents: preserve all branch history', 'source_type': 'Repository',
                 'source': 'acme/project', 'target': 'branch', 'enforcement': 'active', 'bypass_actors': [],
                 'conditions': {'ref_name': {'include': ['~ALL'], 'exclude': []}},
                 'rules': [{'type': 'deletion'}, {'type': 'non_fast_forward'}]},
                {'id': 101, 'name': 'agents: require default branch PR', 'source_type': 'Repository',
                 'source': 'acme/project', 'target': 'branch', 'enforcement': 'active', 'bypass_actors': [],
                 'conditions': {'ref_name': {'include': ['~DEFAULT_BRANCH'], 'exclude': []}},
                 'rules': [{'type': 'pull_request', 'parameters': {
                     'required_approving_review_count': 0, 'dismiss_stale_reviews_on_push': False,
                     'require_code_owner_review': False, 'require_last_push_approval': False,
                     'required_review_thread_resolution': False}}]},
            ],
            'effective_rules': {
                'main': [{'type': 'deletion', 'ruleset_id': 100},
                         {'type': 'non_fast_forward', 'ruleset_id': 100},
                         {'type': 'pull_request', 'ruleset_id': 101}],
                'agents/protection-probe/future': [{'type': 'deletion', 'ruleset_id': 100},
                                                  {'type': 'non_fast_forward', 'ruleset_id': 100}],
            },
            'classic_protection_status': 404,
        }

    def inspect_snapshot(self, snapshot=None, before_return=None):
        selected = self.snapshot if snapshot is None else snapshot
        output = io.StringIO()

        def read_snapshot(record, deadline):
            if before_return:
                before_return()
            return selected

        with patch.dict(os.environ, self.environment, clear=True), \
                patch.object(sys, 'argv', [str(HELPER), 'inspect', '--checkout', str(self.project)]), \
                patch.object(self.access, 'read_protection_snapshot', side_effect=read_snapshot), \
                contextlib.redirect_stdout(output):
            status = self.access.main()
        return status, json.loads(output.getvalue())


class ProtectionInspectionTests(ProtectionSnapshotFixture):
    def test_registry_change_during_inspection_invalidates_the_result(self):
        def revoke_while_reading():
            document = json.loads(self.registry.read_text())
            document['inspector_apps'][0]['setup_approved'] = False
            self.registry.write_text(json.dumps(document))

        status, report = self.inspect_snapshot(before_return=revoke_while_reading)
        self.assertEqual(status, 1, report)
        self.assertEqual(report['reason'], 'registry_changed')

    def test_remote_change_during_inspection_invalidates_the_result(self):
        def move_while_reading():
            self.git('-C', str(self.project), 'remote', 'set-url', 'origin', 'https://github.com/acme/other.git')

        status, report = self.inspect_snapshot(before_return=move_while_reading)
        self.assertEqual(status, 1, report)
        self.assertEqual(report['reason'], 'remote_identity_mismatch')

    def test_all_branch_floor_includes_future_nested_names(self):
        status, report = self.inspect_snapshot()
        self.assertEqual(status, 0, report)
        self.assertEqual(report['mode'], 'verified')
        self.assertEqual(report['repository_id'], 42)

    def test_repository_recreation_changes_id(self):
        self.snapshot['repository']['id'] = 43
        status, report = self.inspect_snapshot()
        self.assertEqual(status, 1)
        self.assertEqual(report['reason'], 'remote_identity_mismatch')

    def test_transfer_requires_reapproval(self):
        self.snapshot['repository']['full_name'] = 'different-owner/project'
        status, report = self.inspect_snapshot()
        self.assertEqual(status, 1)
        self.assertEqual(report['reason'], 'remote_identity_mismatch')

    def test_hidden_bypass_is_unknown(self):
        del self.snapshot['rulesets'][0]['bypass_actors']
        status, report = self.inspect_snapshot()
        self.assertEqual(status, 1)
        self.assertEqual(report['reason'], 'protection_uninspectable')

    def test_any_floor_bypass_denies(self):
        for mode in ['always', 'pull_request', 'exempt']:
            with self.subTest(mode=mode):
                self.snapshot['rulesets'][0]['bypass_actors'] = [
                    {'actor_type': 'RepositoryRole', 'actor_id': 5, 'bypass_mode': mode}]
                self.assertEqual(self.inspect_snapshot()[1]['reason'], 'protection_bypass')

    def test_incomplete_pagination_denies(self):
        self.snapshot['complete'] = False
        self.assertEqual(self.inspect_snapshot()[1]['reason'], 'protection_uninspectable')

    def test_only_active_exact_universal_rules_prove_future_branch_coverage(self):
        changes = [
            {'enforcement': 'disabled'}, {'enforcement': 'evaluate'}, {'target': 'tag'},
            {'conditions': {'ref_name': {'include': ['*'], 'exclude': []}}},
            {'conditions': {'ref_name': {'include': ['~ALL'], 'exclude': ['refs/heads/feature/**']}}},
            {'rules': [{'type': 'deletion'}]}, {'source': 'acme/other'},
        ]
        for change in changes:
            with self.subTest(change=change):
                snapshot = copy.deepcopy(self.snapshot)
                snapshot['rulesets'][0].update(change)
                status, report = self.inspect_snapshot(snapshot)
                self.assertEqual(status, 1, report)
                self.assertEqual(report['mode'], 'read_only')

    def test_effective_rules_must_identify_the_enrolled_baselines(self):
        self.snapshot['effective_rules']['main'][2]['ruleset_id'] = 999
        self.assertEqual(self.inspect_snapshot()[1]['reason'], 'protection_not_effective')

    def test_default_branch_change_requires_checks_for_the_new_default(self):
        self.snapshot['repository']['default_branch'] = 'develop'
        self.assertEqual(self.inspect_snapshot()[1]['reason'], 'protection_not_effective')

    def test_stronger_existing_requirements_remain_acceptable_and_untouched(self):
        parameters = self.snapshot['rulesets'][1]['rules'][0]['parameters']
        parameters['required_approving_review_count'] = 2
        parameters['require_code_owner_review'] = True
        self.snapshot['rulesets'][0]['rules'].append({'type': 'required_signatures'})
        original = copy.deepcopy(self.snapshot)
        self.assertEqual(self.inspect_snapshot()[0], 0)
        self.assertEqual(self.snapshot, original)

    def test_classic_protection_errors_are_not_absence(self):
        for status in [401, 403, 429, 500, None]:
            with self.subTest(status=status):
                self.snapshot['classic_protection_status'] = status
                self.assertEqual(self.inspect_snapshot()[1]['reason'], 'protection_uninspectable')

    def test_successful_classic_protection_response_must_have_known_shape(self):
        self.snapshot['classic_protection_status'] = 200
        for body in [None, [], {}, {'message': 'not protection data'}]:
            with self.subTest(body=body):
                self.snapshot['classic_protection'] = body
                status, report = self.inspect_snapshot()
                self.assertEqual(status, 1, report)
                self.assertEqual(report['reason'], 'protection_uninspectable')
        self.snapshot['classic_protection'] = {
            'url': 'https://api.github.com/repos/acme/project/branches/main/protection',
            'enforce_admins': {'enabled': True},
            'required_status_checks': {'contexts': ['existing-required-ci'], 'strict': True},
        }
        self.assertEqual(self.inspect_snapshot()[0], 0)

    def test_malformed_snapshots_are_secret_free_denials(self):
        for snapshot in [None, [], {}, {'message': 'ghp_SYNTHETIC_NOT_A_REAL_TOKEN'}]:
            with self.subTest(snapshot=snapshot):
                status, report = self.inspect_snapshot(snapshot) if snapshot is not None else self.inspect_snapshot([])
                self.assertEqual(status, 1)
                self.assertEqual(report['reason'], 'protection_uninspectable')
                self.assertNotIn('ghp_', json.dumps(report))


class AppInspectionFixture(ProtectionSnapshotFixture):
    def setUp(self):
        super().setUp()
        self.responses = {}
        repository = self.snapshot['repository'] | {'owner': {'id': 9}}
        self.responses['/repos/acme/project'] = (200, repository, {})
        self.responses['/app/installations/77/access_tokens'] = (201, {
            'token': 'ghs_SYNTHETIC_VARIABLE_LENGTH_INSTALLATION_TOKEN',
            'expires_at': (datetime.now(timezone.utc) + timedelta(hours=1)).isoformat(),
            'permissions': self.app['permissions'].copy(), 'repositories': [{'id': 42}],
        }, {})
        self.responses['/repos/acme/project/rulesets?includes_parents=true&per_page=100&page=1'] = (
            200, [{'id': 100}, {'id': 101}], {})
        for ruleset in self.snapshot['rulesets']:
            self.responses[f'/repos/acme/project/rulesets/{ruleset["id"]}?includes_parents=true'] = (200, ruleset, {})
        for branch, rules in self.snapshot['effective_rules'].items():
            encoded = branch.replace('/', '%2F')
            self.responses[f'/repos/acme/project/rules/branches/{encoded}?per_page=100&page=1'] = (200, rules, {})
        self.responses['/repos/acme/project/branches/main/protection'] = (404, None, {})
        self.requests = []

    def request(self, method, path, token, deadline, body=None):
        self.requests.append((method, path, token, body))
        return self.responses[path]

    def inspect_app(self):
        output = io.StringIO()
        with patch.dict(os.environ, self.environment, clear=True), \
                patch.object(sys, 'argv', [str(HELPER), 'inspect', '--checkout', str(self.project)]), \
                patch.object(self.access, 'api_request', side_effect=self.request, create=True), \
                patch.object(self.access, 'sign_app_jwt', return_value='SYNTHETIC_APP_JWT', create=True), \
                contextlib.redirect_stdout(output):
            status = self.access.main()
        return status, json.loads(output.getvalue())


class AppInspectionTests(AppInspectionFixture):
    def test_app_inspector_mints_only_for_enrolled_repository_and_grants(self):
        status, report = self.inspect_app()
        self.assertEqual(status, 0, report)
        self.assertEqual(self.requests[0], (
            'POST', '/app/installations/77/access_tokens', 'SYNTHETIC_APP_JWT',
            {'repository_ids': [42], 'permissions': {'metadata': 'read', 'administration': 'read'}},
        ))
        self.assertNotIn('SYNTHETIC', json.dumps(report))
        self.assertEqual(report['request_counts'], {'app_auth': 1, 'installation': 7})

    def test_valid_authentication_token_is_reused_but_each_inspection_is_fresh(self):
        self.assertEqual(self.inspect_app()[0], 0)
        self.assertEqual(self.inspect_app()[0], 0)
        minted = [request for request in self.requests if request[0] == 'POST']
        metadata = [request for request in self.requests if request[1] == '/repos/acme/project']
        self.assertEqual(len(minted), 1)
        self.assertEqual(len(metadata), 2)

    def test_unapproved_setup_never_reads_a_signing_key_or_uses_personal_auth(self):
        self.app['setup_approved'] = False
        self.registry.write_text(json.dumps({'version': 1, 'inspector_apps': [self.app], 'repositories': [self.record]}))
        status, report = self.inspect_app()
        self.assertEqual(status, 1)
        self.assertEqual(report['reason'], 'inspector_setup_unapproved')
        self.assertEqual(self.requests, [])

    def test_administration_write_is_not_inferred_from_setup_approval(self):
        self.app['permissions']['administration'] = 'write'
        self.registry.write_text(json.dumps({'version': 1, 'inspector_apps': [self.app], 'repositories': [self.record]}))
        status, report = self.inspect_app()
        self.assertEqual(status, 1)
        self.assertEqual(report['reason'], 'inspector_grants_unapproved')
        self.assertEqual(self.requests, [])

    def test_mint_response_cannot_widen_repositories_or_permissions(self):
        original = copy.deepcopy(self.responses['/app/installations/77/access_tokens'][1])
        for field, value in [('repositories', [{'id': 42}, {'id': 43}]),
                             ('permissions', {'metadata': 'read', 'administration': 'write'}),
                             ('expires_at', 'not a timestamp')]:
            with self.subTest(field=field):
                minted = copy.deepcopy(original)
                minted[field] = value
                self.responses['/app/installations/77/access_tokens'] = (201, minted, {})
                status, report = self.inspect_app()
                self.assertEqual(status, 1)
                self.assertEqual(report['reason'], 'inspector_token_invalid')

    def test_revoked_or_suspended_installation_fails_without_fallback(self):
        for status in [401, 403, 404, 429, 500]:
            with self.subTest(status=status):
                self.responses['/app/installations/77/access_tokens'] = (status, None, {})
                outcome, report = self.inspect_app()
                self.assertEqual(outcome, 1)
                self.assertEqual(report['reason'], f'api_http_{status}')

    def test_pagination_is_complete_before_floor_evaluation(self):
        path = '/repos/acme/project/rulesets?includes_parents=true&per_page=100&page=1'
        self.responses[path] = (200, [{'id': 100}], {
            'link': '<https://api.github.com/repos/acme/project/rulesets?includes_parents=true&per_page=100&page=2>; rel="next"'})
        second = '/repos/acme/project/rulesets?includes_parents=true&per_page=100&page=2'
        self.responses[second] = (200, [{'id': 101}], {})
        self.assertEqual(self.inspect_app()[0], 0)
        self.assertTrue(any(request[1] == second for request in self.requests))

    def test_pagination_never_follows_another_host(self):
        path = '/repos/acme/project/rulesets?includes_parents=true&per_page=100&page=1'
        self.responses[path] = (200, [{'id': 100}], {
            'link': '<https://evil.invalid/steal>; rel="next"'})
        status, report = self.inspect_app()
        self.assertEqual(status, 1)
        self.assertEqual(report['reason'], 'api_pagination_invalid')

    def test_read_grants_cannot_turn_hidden_bypass_details_into_empty(self):
        del self.snapshot['rulesets'][0]['bypass_actors']
        status, report = self.inspect_app()
        self.assertEqual(status, 1)
        self.assertEqual(report['reason'], 'protection_uninspectable')
        self.assertTrue(all(request[0] == 'GET' or request[1].endswith('/access_tokens')
                            for request in self.requests))

    def test_app_repository_owner_must_match_enrolled_installation_owner(self):
        self.responses['/repos/acme/project'][1]['owner']['id'] = 10
        status, report = self.inspect_app()
        self.assertEqual(status, 1)
        self.assertEqual(report['reason'], 'remote_identity_mismatch')


class AppTransportTests(AppInspectionFixture):
    def test_inspector_transport_cannot_write_protections_or_mint_arbitrary_tokens(self):
        for method, path in [('PUT', '/repos/acme/project/rulesets/100'),
                             ('POST', '/repos/acme/project/rulesets'),
                             ('POST', '/app/installations/77/access_tokens/extra')]:
            with self.subTest(method=method, path=path), \
                    patch.object(self.access.subprocess, 'run', side_effect=AssertionError('must refuse before transport')), \
                    self.assertRaisesRegex(self.access.AccessDenied, '^inspector_request_forbidden$'):
                self.access.api_request(method, path, 'SYNTHETIC', self.access.time.monotonic() + 30)

    def setUp(self):
        super().setUp()
        self.real_run = subprocess.run
        self.curl_calls = []
        self.curl_error = 0
        self.raw_body = None
        self.private_key = None
        self.clock = None

    def external_process(self, arguments, **options):
        program = Path(arguments[0]).name
        if program == 'secret-tool':
            return subprocess.CompletedProcess(arguments, 0, self.private_key, b'')
        if program != 'curl':
            return self.real_run(arguments, **options)
        self.curl_calls.append((arguments, options))
        if self.clock is not None:
            self.clock[0] += 31
        path = arguments[arguments.index('--url') + 1].removeprefix('https://api.github.com')
        status, body, headers = self.responses[path]
        header_path = arguments[arguments.index('--dump-header') + 1]
        with open(header_path, 'wb') as header_file:
            header_file.write(f'HTTP/2 {status}\r\n'.encode())
            for key, value in headers.items():
                header_file.write(f'{key}: {value}\r\n'.encode())
            header_file.write(b'\r\n')
        content = json.dumps(body).encode() if self.raw_body is None else self.raw_body
        return subprocess.CompletedProcess(arguments, self.curl_error, content + f'\n{status}'.encode(), b'')

    def inspect_transport(self, *, real_signer=False):
        output = io.StringIO()
        signer = (contextlib.nullcontext() if real_signer else
                  patch.object(self.access, 'sign_app_jwt', return_value='SYNTHETIC_APP_JWT'))
        with patch.dict(os.environ, self.environment, clear=True), \
                patch.object(sys, 'argv', [str(HELPER), 'inspect', '--checkout', str(self.project)]), \
                patch.object(self.access.subprocess, 'run', side_effect=self.external_process), \
                patch.object(self.access, 'trusted_program', side_effect=lambda name: '/usr/bin/' + name), \
                signer, contextlib.redirect_stdout(output):
            status = self.access.main()
        return status, json.loads(output.getvalue())

    def test_transport_keeps_authentication_out_of_arguments_and_disables_curl_config(self):
        status, report = self.inspect_transport()
        self.assertEqual(status, 0, report)
        for arguments, options in self.curl_calls:
            self.assertEqual(arguments[1], '--disable')
            self.assertIn('--max-time', arguments)
            self.assertLessEqual(options['timeout'], 30)
            self.assertNotIn('--location', arguments)
            self.assertIn('Cache-Control: no-cache', arguments)
            self.assertNotIn('SYNTHETIC', ' '.join(arguments))
            self.assertTrue(options['input'].startswith(b'Authorization: Bearer '))
        self.assertNotIn('SYNTHETIC', json.dumps(report))

    def test_inspector_does_not_dispatch_when_all_upstream_slots_are_busy(self):
        self.assertTrue(hasattr(self.access, 'load_rate'), 'shared transport limiter is missing')
        with patch.dict(os.environ, self.environment, clear=True), contextlib.ExitStack() as slots:
            rate = self.access.load_rate()
            for _ in range(8):
                slots.enter_context(rate.request_slot())
            status, report = self.inspect_transport()
        self.assertEqual(status, 1)
        self.assertEqual(report['reason'], 'upstream_busy')
        self.assertEqual(self.curl_calls, [])

    def test_redirects_are_denied_instead_of_followed(self):
        self.responses['/app/installations/77/access_tokens'] = (302, None, {'location': 'https://evil.invalid/steal'})
        status, report = self.inspect_transport()
        self.assertEqual(status, 1)
        self.assertEqual(report['reason'], 'api_http_302')
        self.assertEqual(len(self.curl_calls), 1)

    def test_malformed_and_duplicate_api_json_are_denied(self):
        for body in [b'not json', b'{"token":"first","token":"second"}']:
            with self.subTest(body=body):
                self.raw_body = body
                status, report = self.inspect_transport()
                self.assertEqual(status, 1)
                self.assertEqual(report['reason'], 'api_invalid_response')

    def test_network_and_total_timeout_errors_are_secret_free(self):
        for code, expected in [(6, 'api_network_error'), (28, 'inspection_timeout')]:
            with self.subTest(code=code):
                self.curl_error = code
                status, report = self.inspect_transport()
                self.assertEqual(status, 1)
                self.assertEqual(report['reason'], expected)
                self.assertNotIn('SYNTHETIC', json.dumps(report))

    def test_complete_deadline_includes_token_minting_time(self):
        self.clock = [1000.0]
        with patch.object(self.access.time, 'monotonic', side_effect=lambda: self.clock[0]):
            status, report = self.inspect_transport()
        self.assertEqual(status, 1)
        self.assertEqual(report['reason'], 'inspection_timeout')
        self.assertEqual(len(self.curl_calls), 1)

    def test_real_rs256_signer_uses_keyring_key_without_exposing_the_key(self):
        key = self.real_run(['openssl', 'genpkey', '-algorithm', 'RSA', '-pkeyopt', 'rsa_keygen_bits:2048'],
                            capture_output=True)
        self.assertEqual(key.returncode, 0)
        self.private_key = key.stdout
        status, report = self.inspect_transport(real_signer=True)
        self.assertEqual(status, 0, report)
        header_bytes = self.curl_calls[0][1]['input']
        token = header_bytes.decode().removeprefix('Authorization: Bearer ').strip()
        header, payload, signature = token.split('.')
        claims = json.loads(base64.urlsafe_b64decode(payload + '=' * (-len(payload) % 4)))
        self.assertEqual(claims['iss'], 'IvFixtureApp')
        self.assertEqual(claims['exp'] - claims['iat'], 660)
        public_key = self.real_run(['openssl', 'pkey', '-pubout'], input=self.private_key, capture_output=True)
        self.assertEqual(public_key.returncode, 0)
        public_path, signature_path = self.root / 'public.pem', self.root / 'signature'
        public_path.write_bytes(public_key.stdout)
        signature_path.write_bytes(base64.urlsafe_b64decode(signature + '=' * (-len(signature) % 4)))
        verified = self.real_run(['openssl', 'dgst', '-sha256', '-verify', str(public_path), '-signature',
                                  str(signature_path)], input=f'{header}.{payload}'.encode(), capture_output=True)
        self.assertEqual(verified.returncode, 0, verified.stderr)
        self.assertNotIn('PRIVATE KEY', json.dumps(report))


class EnrollmentTests(AppInspectionFixture):
    def setUp(self):
        super().setUp()
        self.candidate = self.root / 'candidate'
        self.git('init', '-q', str(self.candidate))
        self.git('-C', str(self.candidate), 'remote', 'add', 'origin', 'https://github.com/acme/project.git')
        self.app.update(owner_login='acme', approved_repository_ids=[42])
        self.document = {'version': 1, 'inspector_apps': [self.app], 'repositories': [self.record]}
        self.registry.write_text(json.dumps(self.document))

    def enroll(self, *, confirmation=None, before_confirm=None, overrides=()):
        output, error = io.StringIO(), io.StringIO()
        arguments = [str(HELPER), 'enroll', '--checkout', str(self.candidate), '--repository', 'acme/project',
                     '--project', 'project', '--repository-id', '42', '--inspector-app-id', '123',
                     '--history-ruleset', '100', '--default-pr-ruleset', '101', *overrides]

        def answer():
            if before_confirm:
                before_confirm()
            return confirmation if confirmation is not None else f'ENROLL acme/project 42 {self.candidate}'

        with patch.dict(os.environ, self.environment, clear=True), patch.object(sys, 'argv', arguments), \
                patch.object(self.access, 'api_request', side_effect=self.request), \
                patch.object(self.access, 'sign_app_jwt', return_value='SYNTHETIC_APP_JWT'), \
                patch('builtins.input', side_effect=answer), \
                contextlib.redirect_stdout(output), contextlib.redirect_stderr(error):
            try:
                status = self.access.main()
            except SystemExit as stopped:
                status = stopped.code
        return status, json.loads(output.getvalue()) if output.getvalue() else {}, error.getvalue()

    def test_enrollment_requires_fresh_app_proof_and_exact_confirmation(self):
        status, report, error = self.enroll()
        self.assertEqual(status, 0, report or error)
        record = json.loads(self.registry.read_text())['repositories'][0]
        self.assertEqual(record['repository_id'], 42)
        self.assertEqual({entry['path'] for entry in record['checkouts']}, {str(self.project), str(self.candidate)})
        self.assertEqual(self.registry.stat().st_mode & 0o777, 0o600)
        self.assertNotIn('SYNTHETIC', self.registry.read_text())
        self.assertNotIn('SYNTHETIC', json.dumps(report))

    def test_enrollment_cancellation_never_changes_registry(self):
        original = self.registry.read_bytes()
        status, report, error = self.enroll(confirmation='yes')
        self.assertEqual(status, 1, report or error)
        self.assertEqual(report.get('reason'), 'enrollment_not_confirmed')
        self.assertEqual(self.registry.read_bytes(), original)

    def test_hidden_bypass_blocks_enrollment_even_with_confirmation(self):
        original = self.registry.read_bytes()
        del self.snapshot['rulesets'][0]['bypass_actors']
        status, report, error = self.enroll()
        self.assertEqual(status, 1, report or error)
        self.assertEqual(report.get('reason'), 'protection_uninspectable')
        self.assertEqual(self.registry.read_bytes(), original)

    def test_unapproved_repository_cannot_borrow_an_owner_app(self):
        self.app['approved_repository_ids'] = [99]
        self.registry.write_text(json.dumps(self.document))
        original = self.registry.read_bytes()
        status, report, error = self.enroll()
        self.assertEqual(status, 1, report or error)
        self.assertEqual(report.get('reason'), 'inspector_repository_unapproved')
        self.assertEqual(self.requests, [])
        self.assertEqual(self.registry.read_bytes(), original)

    def test_control_characters_in_checkout_paths_are_rejected_before_app_access(self):
        changed = self.root / 'candidate\twith-control'
        self.candidate.rename(changed)
        self.candidate = changed
        original = self.registry.read_bytes()
        status, report, error = self.enroll()
        self.assertEqual(status, 1, report or error)
        self.assertEqual(report.get('reason'), 'unsupported_identity_path')
        self.assertEqual(self.requests, [])
        self.assertEqual(self.registry.read_bytes(), original)

    def test_concurrent_registry_edit_is_not_overwritten_after_confirmation(self):
        def concurrent_edit():
            self.document['operator_note'] = 'concurrent host update'
            self.registry.write_text(json.dumps(self.document))

        status, report, error = self.enroll(before_confirm=concurrent_edit)
        self.assertEqual(status, 1, report or error)
        self.assertEqual(report.get('reason'), 'registry_changed')
        saved = json.loads(self.registry.read_text())
        self.assertEqual(saved['operator_note'], 'concurrent host update')
        self.assertEqual(len(saved['repositories'][0]['checkouts']), 1)

    def test_registry_snapshot_and_parsed_policy_cannot_come_from_different_versions(self):
        original_loader = self.access.load_registry
        has_changed = False

        def load_then_change():
            nonlocal has_changed
            loaded = original_loader()
            if not has_changed:
                has_changed = True
                self.document['operator_note'] = 'changed after parsing'
                self.registry.write_text(json.dumps(self.document))
            return loaded

        with patch.object(self.access, 'load_registry', side_effect=load_then_change):
            status, report, error = self.enroll()
        self.assertEqual(status, 1, report or error)
        self.assertEqual(report.get('reason'), 'registry_changed')
        self.assertEqual(json.loads(self.registry.read_text())['operator_note'], 'changed after parsing')

    def test_existing_project_mapping_is_not_reassigned(self):
        self.record['repository_id'] = 99
        self.registry.write_text(json.dumps(self.document))
        original = self.registry.read_bytes()
        status, report, error = self.enroll()
        self.assertEqual(status, 1, report or error)
        self.assertEqual(report.get('reason'), 'project_already_enrolled')
        self.assertEqual(self.registry.read_bytes(), original)

    def test_new_project_is_added_without_overwriting_other_records(self):
        self.document['repositories'] = []
        self.registry.write_text(json.dumps(self.document))
        status, report, error = self.enroll()
        self.assertEqual(status, 0, report or error)
        saved = json.loads(self.registry.read_text())
        self.assertEqual(saved['repositories'][0]['project'], 'project')
        self.assertEqual(saved['repositories'][0]['canonical_name'], 'acme/project')
        self.assertEqual(saved['inspector_apps'], self.document['inspector_apps'])

    def test_enrolled_checkout_cannot_keep_a_stale_copy_of_app_approval(self):
        status, report, error = self.enroll()
        self.assertEqual(status, 0, report or error)
        saved = json.loads(self.registry.read_text())
        saved['inspector_apps'][0]['setup_approved'] = False
        self.registry.write_text(json.dumps(saved))
        self.requests.clear()
        status, report = self.inspect_app()
        self.assertEqual(status, 1, report)
        self.assertEqual(report['reason'], 'inspector_setup_unapproved')
        self.assertEqual(self.requests, [])

    def test_removing_repository_approval_blocks_an_existing_enrollment(self):
        self.assertEqual(self.enroll()[0], 0)
        saved = json.loads(self.registry.read_text())
        saved['inspector_apps'][0]['approved_repository_ids'] = []
        self.registry.write_text(json.dumps(saved))
        self.requests.clear()
        status, report = self.inspect_app()
        self.assertEqual(status, 1, report)
        self.assertEqual(report['reason'], 'inspector_repository_unapproved')
        self.assertEqual(self.requests, [])


class RuntimeCredentialFixture(AppInspectionFixture):
    def setUp(self):
        super().setUp()
        now = int(self.access.time.time())
        self.tokens = {'project-read': 'SYNTHETIC_READ_TOKEN', 'project-forward': 'SYNTHETIC_FORWARD_TOKEN'}
        self.profiles = [
            {'id': 'project-read', 'role': 'read', 'kind': 'fine_grained_pat', 'repository_id': 42, 'actor_id': 88,
             'permissions': {name: 'read' for name in ['metadata', 'contents', 'issues', 'pull_requests', 'checks', 'statuses', 'actions']}},
            {'id': 'project-forward', 'role': 'forward', 'kind': 'fine_grained_pat', 'repository_id': 42, 'actor_id': 88,
             'permissions': {'metadata': 'read', 'contents': 'write', 'issues': 'write', 'pull_requests': 'write'}},
        ]
        for profile in self.profiles:
            profile.update(setup_approved=True, key_epoch='fixture-epoch', reviewed_at=now - 60,
                           valid_until=now + 3600, expires_at=now + 7200,
                           fingerprint=self.access.hashlib.sha256(self.tokens[profile['id']].encode()).hexdigest())
        self.record['credential_roles'] = {'read': 'project-read', 'forward': 'project-forward'}
        self.keyring_reads = []
        self.runtime_requests = []
        self.runtime_scopes = ''
        self.runtime_actor = 88
        self.runtime_repository = 42
        self.missing_credentials = set()
        self.app.update(rate_setup_approved=True, rate_buckets={
            'installation': 'installation:77:core', 'authentication': 'app:123:core'})
        self.save_profiles()
        spec = importlib.util.spec_from_file_location('cockpit_test_rate', HELPER.with_name('github-rate.py'))
        self.rate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.rate)
        self.rate_headers = {'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '5000',
                             'x-ratelimit-remaining': '5000', 'x-ratelimit-reset': str(now + 3600)}
        with patch.dict(os.environ, self.environment, clear=True):
            for principal in ['installation:77', 'app:123', 'user:88']:
                self.rate.observe(principal, self.rate_headers)

    def save_profiles(self):
        self.document = {'version': 1, 'repositories': [self.record], 'inspector_apps': [self.app],
                         'runtime_credentials': self.profiles}
        self.registry.write_text(json.dumps(self.document))

    def inspect_runtime(self, purpose='admission', before_runtime=None):
        actual_run = subprocess.run

        def command(arguments, **kwargs):
            if Path(arguments[0]).name != 'secret-tool':
                return actual_run(arguments, **kwargs)
            self.assertEqual(arguments[1:4], ['lookup', 'service', 'cockpit-github-runtime'])
            credential_id = arguments[arguments.index('credential') + 1]
            self.assertEqual(arguments[arguments.index('repository') + 1], '42')
            self.assertEqual(arguments[arguments.index('epoch') + 1], 'fixture-epoch')
            self.keyring_reads.append(credential_id)
            if credential_id in self.missing_credentials:
                return subprocess.CompletedProcess(arguments, 1, b'', b'SYNTHETIC_KEYRING_ERROR')
            return subprocess.CompletedProcess(arguments, 0, (self.tokens[credential_id] + '\n').encode(), b'')

        def request(method, path, token, deadline, body=None):
            if token not in self.tokens.values():
                status, result, headers = self.request(method, path, token, deadline, body)
                return status, result, self.rate_headers | headers
            if before_runtime:
                before_runtime()
            self.runtime_requests.append((method, path))
            if path == '/user':
                return 200, {'id': self.runtime_actor}, self.rate_headers | {'x-oauth-scopes': self.runtime_scopes}
            if path == '/repos/acme/project':
                return 200, {'id': self.runtime_repository, 'full_name': 'acme/project', 'owner': {'id': 9}}, self.rate_headers
            self.fail(f'unexpected runtime request {method} {path}')

        with patch.dict(os.environ, self.environment, clear=True), \
                patch.object(self.access.subprocess, 'run', side_effect=command), \
                patch.object(self.access, 'github_request', side_effect=request), \
                patch.object(self.access, 'sign_app_jwt', return_value='SYNTHETIC_APP_JWT'):
            if purpose == 'write':
                return self.access.inspect_access(self.project, purpose=purpose, operation_costs={'user:88:core': 1})
            return self.access.inspect_access(self.project, purpose=purpose)


class RuntimeCredentialTests(RuntimeCredentialFixture):
    def test_read_and_write_credentials_have_distinct_verified_roles(self):
        report = self.inspect_runtime()
        self.assertEqual(report['mode'], 'verified', report)
        self.assertEqual(self.keyring_reads, ['project-read', 'project-forward'])
        self.assertEqual(report['credential_roles_verified'], ['read', 'forward'])
        self.assertEqual(report['request_counts']['runtime'], 4)
        self.assertNotIn('SYNTHETIC', json.dumps(report))

    def test_untrusted_legacy_pat_is_not_admitted(self):
        del self.record['credential_roles']
        self.save_profiles()
        report = self.inspect_runtime()
        self.assertEqual(report['mode'], 'read_only')
        self.assertEqual(report['reason'], 'runtime_credential_unavailable')
        self.assertEqual(self.keyring_reads, [])

    def test_read_role_never_accepts_a_write_token_or_privileged_grants(self):
        for permission, level in [('contents', 'write'), ('checks', 'write'), ('statuses', 'write'),
                                  ('actions', 'write'), ('administration', 'read'), ('workflows', 'write')]:
            with self.subTest(permission=permission):
                self.profiles[0]['permissions'][permission] = level
                self.save_profiles()
                report = self.inspect_runtime()
                self.assertEqual(report['reason'], 'runtime_grants_unapproved', report)
                self.assertEqual(self.keyring_reads, [])
                self.profiles[0]['permissions'].pop(permission)
                if permission not in {'administration', 'workflows'}:
                    self.profiles[0]['permissions'][permission] = 'read'

    def test_forward_role_rejects_workflows_administration_and_status_writes(self):
        for permission in ['workflows', 'administration', 'checks', 'statuses', 'actions']:
            with self.subTest(permission=permission):
                self.profiles[1]['permissions'][permission] = 'write'
                self.save_profiles()
                report = self.inspect_runtime()
                self.assertEqual(report['reason'], 'runtime_grants_unapproved', report)
                self.assertEqual(self.keyring_reads, [])
                del self.profiles[1]['permissions'][permission]

    def test_expired_unapproved_or_wrong_repository_enrollment_denies_before_keyring(self):
        for key, value in [('setup_approved', False), ('valid_until', 1), ('expires_at', 1),
                           ('repository_id', 99), ('kind', 'classic_pat')]:
            with self.subTest(key=key):
                original = self.profiles[0][key]
                self.profiles[0][key] = value
                self.save_profiles()
                report = self.inspect_runtime()
                self.assertEqual(report['mode'], 'read_only', report)
                self.assertEqual(self.keyring_reads, [])
                self.profiles[0][key] = original

    def test_keyring_replacement_cannot_keep_an_old_approved_fingerprint(self):
        self.tokens['project-read'] = 'SYNTHETIC_REPLACEMENT_TOKEN'
        report = self.inspect_runtime()
        self.assertEqual(report['reason'], 'runtime_credential_mismatch', report)
        self.assertEqual(self.runtime_requests, [])

    def test_server_actor_repository_or_classic_scope_mismatch_denies(self):
        for attribute, value in [('runtime_actor', 99), ('runtime_repository', 99), ('runtime_scopes', 'repo')]:
            with self.subTest(attribute=attribute):
                original = getattr(self, attribute)
                setattr(self, attribute, value)
                report = self.inspect_runtime()
                self.assertEqual(report['mode'], 'read_only', report)
                self.assertNotIn('SYNTHETIC', json.dumps(report))
                setattr(self, attribute, original)

    def test_one_secret_cannot_back_both_read_and_forward_roles(self):
        self.profiles[1]['fingerprint'] = self.profiles[0]['fingerprint']
        self.save_profiles()
        report = self.inspect_runtime()
        self.assertEqual(report['reason'], 'runtime_roles_not_separate', report)
        self.assertEqual(self.keyring_reads, [])

    def test_runtime_revocation_during_inspection_invalidates_the_result(self):
        def revoke():
            self.document['runtime_credentials'][0]['setup_approved'] = False
            self.registry.write_text(json.dumps(self.document))

        report = self.inspect_runtime(before_runtime=revoke)
        self.assertEqual(report['mode'], 'read_only', report)
        self.assertEqual(report['reason'], 'registry_changed')
        self.assertNotIn('credential_roles_verified', report)

    def test_runtime_enrollment_expiring_during_the_check_never_reports_verified(self):
        now = self.access.time.time()
        with patch.object(self.access.time, 'time', return_value=now) as clock:
            report = self.inspect_runtime(before_runtime=lambda: setattr(clock, 'return_value', now + 3601))
        self.assertEqual(report['mode'], 'read_only', report)
        self.assertIn(report['reason'], ['runtime_enrollment_expired', 'rate_headers_uninspectable', 'rate_quota_unknown'])

    def test_each_write_checks_roles_again_even_when_app_authentication_is_cached(self):
        self.assertEqual(self.inspect_runtime(purpose='write')['mode'], 'verified')
        self.assertEqual(self.inspect_runtime(purpose='write')['mode'], 'verified')
        self.profiles[0]['setup_approved'] = False
        self.save_profiles()
        report = self.inspect_runtime(purpose='write')
        self.assertEqual(report['mode'], 'read_only', report)
        self.assertEqual(self.keyring_reads, ['project-read', 'project-forward'] * 2)
        self.assertEqual(sum(method == 'GET' and path == '/repos/acme/project'
                             for method, path, token, body in self.requests), 2)

    def test_missing_forward_secret_never_falls_back_to_the_read_token(self):
        self.missing_credentials.add('project-forward')
        report = self.inspect_runtime()
        self.assertEqual(report['reason'], 'runtime_credential_unavailable', report)
        self.assertNotIn('SYNTHETIC', json.dumps(report))
        self.assertEqual(self.keyring_reads, ['project-read', 'project-forward'])

    def test_protection_only_inspection_never_reads_runtime_credentials(self):
        report = self.inspect_runtime(purpose='inspection')
        self.assertEqual(report['mode'], 'verified')
        self.assertNotIn('credential_roles_verified', report)
        self.assertEqual(self.keyring_reads, [])


class RateInspectionTests(RuntimeCredentialFixture):
    def observe(self, principal, remaining):
        with patch.dict(os.environ, self.environment, clear=True):
            self.rate.observe(principal, self.rate_headers | {'x-ratelimit-remaining': str(remaining)})

    def test_unapproved_rate_principal_mapping_never_admits_or_reads_runtime_secrets(self):
        self.app['rate_setup_approved'] = False
        self.save_profiles()
        report = self.inspect_runtime()
        self.assertEqual(report['mode'], 'read_only', report)
        self.assertEqual(report['reason'], 'rate_setup_unapproved')
        self.assertEqual(self.requests, [])
        self.assertEqual(self.keyring_reads, [])

    def test_insufficient_headroom_denies_the_whole_inspection_before_upstream_requests(self):
        self.observe('installation:77', 1006)
        report = self.inspect_runtime()
        self.assertEqual(report['mode'], 'read_only', report)
        self.assertEqual(report['reason'], 'rate_headroom_reserved')
        self.assertEqual(self.requests, [])
        self.assertEqual(self.keyring_reads, [])

    def test_each_write_gets_a_new_funded_check_and_operation_reservation(self):
        first = self.inspect_runtime(purpose='write')
        second = self.inspect_runtime(purpose='write')
        self.assertTrue(first['rate_verified'], first)
        self.assertTrue(second['rate_verified'], second)
        self.assertNotEqual(first['rate_reservation'], second['rate_reservation'])
        self.assertEqual(first['rate_operation_costs'], {'user:88:core': 1})
        self.assertEqual(len(self.runtime_requests), 8)
        self.assertEqual(sum(method == 'GET' for method, path, token, body in self.requests), 14)

    def test_scheduled_remote_evidence_coalesces_but_local_identity_is_still_checked(self):
        first = self.inspect_runtime(purpose='scheduled')
        self.assertEqual(first['mode'], 'verified', first)
        counts = len(self.requests), len(self.runtime_requests)
        second = self.inspect_runtime(purpose='scheduled')
        self.assertEqual(second['mode'], 'verified', second)
        self.assertEqual((len(self.requests), len(self.runtime_requests)), counts)
        self.assertEqual(first['rate_evidence_checked_at'], second['rate_evidence_checked_at'])
        self.git('-C', str(self.project), 'remote', 'set-url', 'origin', 'https://github.com/acme/another.git')
        denied = self.inspect_runtime(purpose='scheduled')
        self.assertEqual(denied['reason'], 'remote_identity_mismatch')

    def test_extra_protection_pages_are_funded_or_the_result_is_read_only(self):
        self.observe('installation:77', 1008)
        self.responses['/repos/acme/project/rulesets?includes_parents=true&per_page=100&page=1'] = (200, [{'id': 100}, {'id': 101}] * 50, {})
        self.responses['/repos/acme/project/rulesets?includes_parents=true&per_page=100&page=2'] = (200, [{'id': 100}] * 100, {})
        self.responses['/repos/acme/project/rulesets?includes_parents=true&per_page=100&page=3'] = (200, [], {})
        report = self.inspect_runtime()
        self.assertEqual(report['mode'], 'read_only', report)
        self.assertEqual(report['reason'], 'rate_headroom_reserved')
        self.assertNotIn('credential_roles_verified', report)

    def test_server_resource_mismatch_is_not_silently_charged_to_another_pool(self):
        self.rate_headers['x-ratelimit-resource'] = 'search'
        report = self.inspect_runtime()
        self.assertEqual(report['mode'], 'read_only', report)
        self.assertEqual(report['reason'], 'rate_resource_mismatch')

    def test_latest_headers_must_still_fund_the_operation_after_the_complete_check(self):
        def external_usage():
            if len(self.runtime_requests) == 3:
                self.rate_headers['x-ratelimit-remaining'] = '1000'

        report = self.inspect_runtime(purpose='write', before_runtime=external_usage)
        self.assertEqual(report['mode'], 'read_only', report)
        self.assertEqual(report['reason'], 'rate_headroom_reserved')
        self.assertNotIn('rate_reservation', report)

    def test_a_late_finished_check_never_publishes_reusable_success(self):
        finish = self.access.finish_inspection_budget
        clocks = []

        def delayed_finish(budget, *, evidence=None):
            result = finish(budget, evidence=evidence)
            if evidence is not None:
                clock = patch.object(self.access.time, 'monotonic', return_value=self.access.time.monotonic() + 31)
                clock.start()
                clocks.append(clock)
            return result

        try:
            with patch.object(self.access, 'finish_inspection_budget', side_effect=delayed_finish), \
                    contextlib.redirect_stderr(io.StringIO()):
                report = self.inspect_runtime(purpose='scheduled')
            self.assertEqual(report['mode'], 'read_only', report)
            self.assertEqual(report['reason'], 'inspection_timeout')
            with patch.dict(os.environ, self.environment, clear=True):
                state = json.loads(self.rate.ledger_path().read_text())
            self.assertEqual(state['scheduled'], {})
        finally:
            for clock in clocks:
                clock.stop()


if __name__ == '__main__':
    unittest.main()
import base64
