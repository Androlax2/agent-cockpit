import copy
import contextlib
import os
import importlib.util
import io
from pathlib import Path
import json
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from test_git_access import AppInspectionFixture


REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / 'tools/agent-github.sh'
POLICY = REPO / 'config/bin/.local/bin/greywall-agents/git-protection.py'


class ProtectionPlanTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(POLICY.is_file(), 'additive protection policy is not implemented')
        spec = importlib.util.spec_from_file_location('cockpit_git_protection', POLICY)
        self.policy = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.policy)

    def owned(self, role, rules=None):
        body = self.policy.floor_body(role)
        if rules:
            body['rules'].extend(rules)
        return body | {'id': 100 if role == 'history' else 101,
                       'source_type': 'Repository', 'source': 'acme/project'}

    def test_new_baselines_cover_all_branches_without_restricting_creation(self):
        plan = self.policy.plan_baselines('acme/project', [], {})
        self.assertEqual([item['method'] for item in plan], ['POST', 'POST'])
        history, default = [item['body'] for item in plan]
        self.assertEqual(history['conditions'], {'ref_name': {'include': ['~ALL'], 'exclude': []}})
        self.assertEqual({rule['type'] for rule in history['rules']}, {'non_fast_forward', 'deletion'})
        self.assertEqual(history['bypass_actors'], [])
        self.assertEqual(default['conditions'], {'ref_name': {'include': ['~DEFAULT_BRANCH'], 'exclude': []}})
        self.assertEqual(default['rules'][0]['parameters']['required_approving_review_count'], 0)

    def test_existing_ci_and_legacy_rules_are_not_modified(self):
        legacy = {'id': 9, 'name': 'agents: protect default branch', 'target': 'branch', 'enforcement': 'active',
                  'rules': [{'type': 'required_status_checks', 'parameters': {'contexts': ['existing-ci']}}]}
        existing = [legacy]
        before = copy.deepcopy(existing)
        plan = self.policy.plan_baselines('acme/project', existing, {})
        self.assertEqual([item['method'] for item in plan], ['POST', 'POST'])
        self.assertEqual(existing, before)

    def test_same_named_unowned_ruleset_is_not_overwritten(self):
        unowned = self.owned('history')
        with self.assertRaisesRegex(ValueError, '^unowned_ruleset_name_collision$'):
            self.policy.plan_baselines('acme/project', [unowned], {})

    def test_existing_stronger_owned_rules_are_preserved(self):
        history = self.owned('history', [{'type': 'required_signatures'}])
        default = self.owned('default_pr', [{'type': 'required_status_checks', 'parameters': {
            'required_status_checks': [{'context': 'real-ci', 'integration_id': 123}],
            'strict_required_status_checks_policy': True}}])
        default['rules'][0]['parameters']['required_approving_review_count'] = 2
        default['rules'][0]['parameters']['require_code_owner_review'] = True
        default['enforcement'] = 'disabled'
        before = copy.deepcopy(default)
        plan = self.policy.plan_baselines('acme/project', [history, default], {'history': 100, 'default_pr': 101})
        self.assertEqual(len(plan), 1)
        self.assertEqual(plan[0]['method'], 'PUT')
        self.assertEqual(plan[0]['id'], 101)
        self.assertEqual(plan[0]['body']['rules'], before['rules'])
        self.assertEqual(plan[0]['body']['enforcement'], 'active')
        self.assertEqual(default, before)

    def test_valid_owned_baselines_need_no_mutations(self):
        existing = [self.owned('history'), self.owned('default_pr')]
        self.assertEqual(self.policy.plan_baselines('acme/project', existing,
                                                   {'history': 100, 'default_pr': 101}), [])

    def test_high_existing_review_requirement_is_never_lowered_or_rejected(self):
        default = self.owned('default_pr')
        default['rules'][0]['parameters']['required_approving_review_count'] = 10
        plan = self.policy.plan_baselines('acme/project', [default], {'default_pr': 101})
        self.assertEqual([item['role'] for item in plan], ['history'])

    def test_missing_owned_ids_are_not_recreated_or_adopted(self):
        with self.assertRaisesRegex(ValueError, '^owned_ruleset_missing$'):
            self.policy.plan_baselines('acme/project', [], {'history': 100})

    def test_malformed_rules_and_ambiguous_ownership_never_produce_a_plan(self):
        for changes in [{'rules': None}, {'rules': [{'type': 'deletion'}, {'type': 'deletion'}]},
                        {'rules': [{'type': 'pull_request', 'parameters': {}}]}]:
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                self.policy.plan_baselines('acme/project', [self.owned('default_pr') | changes],
                                           {'default_pr': 101})
        for owned_ids in [{'history': True}, {'history': 100, 'default_pr': 100}, {'unknown': 100}]:
            with self.subTest(owned_ids=owned_ids), self.assertRaises(ValueError):
                self.policy.plan_baselines('acme/project', [], owned_ids)

    def test_uninspectable_or_differently_scoped_owned_rules_require_review(self):
        for changes in [{'target': 'tag'}, {'source': 'another/repository'},
                        {'conditions': {'ref_name': {'include': ['refs/heads/main'], 'exclude': []}}},
                        {'bypass_actors': None}]:
            with self.subTest(changes=changes):
                history = self.owned('history') | changes
                with self.assertRaises(ValueError):
                    self.policy.plan_baselines('acme/project', [history], {'history': 100})

    def test_duplicate_names_are_not_guessed(self):
        first = self.owned('history')
        second = first | {'id': 200}
        with self.assertRaisesRegex(ValueError, '^ruleset_name_ambiguous$'):
            self.policy.plan_baselines('acme/project', [first, second], {'history': 100})

    def test_repair_adds_missing_history_rule_without_dropping_existing_ci(self):
        history = self.owned('history', [{'type': 'required_status_checks', 'parameters': {'contexts': ['ci']}}])
        history['rules'] = [rule for rule in history['rules'] if rule['type'] != 'non_fast_forward']
        history['bypass_actors'] = [{'actor_type': 'RepositoryRole', 'actor_id': 5, 'bypass_mode': 'always'}]
        plan = self.policy.plan_baselines('acme/project', [history], {'history': 100})
        repaired = plan[0]['body']
        self.assertEqual(repaired['bypass_actors'], [])
        self.assertTrue(any(rule == history['rules'][-1] for rule in repaired['rules']))
        self.assertTrue(any(rule['type'] == 'non_fast_forward' for rule in repaired['rules']))


class ProtectionPolicyFixture(AppInspectionFixture):
    def setUp(self):
        super().setUp()
        spec = importlib.util.spec_from_file_location('cockpit_git_protection', POLICY)
        self.policy = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.policy)
        self.snapshot['rulesets'][1]['name'] = 'agents: require default branch pull requests'

    def preview(self, project='project', request=None):
        with patch.dict('os.environ', self.environment, clear=True), \
                patch.object(self.access, 'api_request', side_effect=request or self.request), \
                patch.object(self.access, 'sign_app_jwt', return_value='SYNTHETIC_APP_JWT'):
            return self.policy.preview_protection(self.access, project)


class ProtectionPreviewTests(ProtectionPolicyFixture):
    def test_preview_is_identity_bound_and_never_mutates_or_grants_access(self):
        self.snapshot['rulesets'][0]['enforcement'] = 'disabled'
        before = self.registry.read_bytes()
        report = self.preview()
        self.assertEqual(report['mode'], 'preview')
        self.assertEqual(report['repository_id'], 42)
        self.assertEqual(report['mutations'][0]['id'], 100)
        self.assertEqual(report['mutations'][0]['body']['enforcement'], 'active')
        self.assertEqual(self.registry.read_bytes(), before)
        self.assertTrue(all(method == 'GET' or path.endswith('/access_tokens')
                            for method, path, token, body in self.requests))
        self.assertNotIn('SYNTHETIC', str(report))

    def test_unenrolled_project_cannot_preview_using_an_origin_or_personal_login(self):
        with self.assertRaisesRegex(self.access.AccessDenied, '^unregistered_project$'):
            self.preview('other')
        self.assertEqual(self.requests, [])

    def test_protect_entry_point_denies_unregistered_project_without_origin_lookup(self):
        result = subprocess.run(['bash', str(REPO / 'tools/agent-github.sh'), 'protect', 'unregistered-project'],
                                env=self.environment, capture_output=True, text=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(result.stdout.strip(), result.stderr)
        self.assertEqual(json.loads(result.stdout)['reason'], 'unregistered_project')

    def test_explicit_apply_entry_point_denies_unregistered_project_before_administration(self):
        result = subprocess.run(['bash', str(REPO / 'tools/agent-github.sh'), 'protect', 'unregistered-project', '--apply'],
                                env=self.environment, capture_output=True, text=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(json.loads(result.stdout)['reason'], 'unregistered_project')

    def test_remote_identity_mismatch_blocks_preview(self):
        self.responses['/repos/acme/project'][1]['id'] = 99
        with self.assertRaisesRegex(self.access.AccessDenied, '^remote_identity_mismatch$'):
            self.preview()

    def test_registry_edit_during_preview_invalidates_the_plan(self):
        def request(method, path, token, deadline, body=None):
            result = self.request(method, path, token, deadline, body)
            if '/rulesets/' in path:
                self.registry.write_bytes(self.registry.read_bytes() + b'\n')
            return result

        with self.assertRaisesRegex(self.access.AccessDenied, '^registry_changed$'):
            self.preview(request=request)


class ProtectionApplyTests(ProtectionPolicyFixture):
    def setUp(self):
        super().setUp()
        self.snapshot['rulesets'][0]['enforcement'] = 'disabled'
        self.admin_writes = []

    def apply(self, confirmation='APPLY acme/project 42', before_confirm=None, admin=None):
        def answer():
            if before_confirm:
                before_confirm()
            return confirmation

        def write(access, record, mutation, deadline):
            self.admin_writes.append(copy.deepcopy(mutation))
            for ruleset in self.snapshot['rulesets']:
                if ruleset['id'] == mutation.get('id'):
                    ruleset.update(copy.deepcopy(mutation['body']))
                    return copy.deepcopy(ruleset)
            self.fail('unexpected mutation')

        with patch.dict('os.environ', self.environment, clear=True), \
                patch.object(self.access, 'api_request', side_effect=self.request), \
                patch.object(self.access, 'sign_app_jwt', return_value='SYNTHETIC_APP_JWT'), \
                patch('builtins.input', side_effect=answer), \
                contextlib.redirect_stderr(io.StringIO()), \
                patch.object(self.policy, 'admin_mutation', side_effect=admin or write, create=True):
            return self.policy.apply_protection(self.access, 'project')

    def test_apply_requires_exact_confirmation_and_inspector_post_write_proof(self):
        report = self.apply()
        self.assertEqual(report['mode'], 'verified')
        self.assertFalse(report['rollout_enabled'])
        self.assertEqual([mutation['id'] for mutation in self.admin_writes], [100])
        self.assertEqual(report['reason'], 'protections_verified')
        self.assertNotIn('SYNTHETIC', str(report))

    def test_cancelled_apply_never_uses_administration(self):
        with self.assertRaisesRegex(self.access.AccessDenied, '^remediation_not_confirmed$'):
            self.apply(confirmation='yes')
        self.assertEqual(self.admin_writes, [])

    def test_changed_preview_after_confirmation_never_writes(self):
        def change():
            self.snapshot['rulesets'][0]['rules'].append({'type': 'required_signatures'})

        with self.assertRaisesRegex(self.access.AccessDenied, '^remediation_plan_changed$'):
            self.apply(before_confirm=change)
        self.assertEqual(self.admin_writes, [])

    def test_partial_install_never_reports_verified_or_rolls_back(self):
        def ineffective_write(access, record, mutation, deadline):
            self.admin_writes.append(mutation)
            return copy.deepcopy(self.snapshot['rulesets'][0])

        with self.assertRaises(self.access.AccessDenied):
            self.apply(admin=ineffective_write)
        self.assertEqual(len(self.admin_writes), 1)
        self.assertEqual(self.snapshot['rulesets'][0]['enforcement'], 'disabled')

    def test_live_inspection_must_preserve_stronger_rules_even_if_write_response_looks_correct(self):
        self.snapshot['rulesets'][0]['rules'].append({'type': 'required_signatures'})

        def inconsistent_write(access, record, mutation, deadline):
            response = copy.deepcopy(self.snapshot['rulesets'][0]) | copy.deepcopy(mutation['body'])
            self.snapshot['rulesets'][0].update(copy.deepcopy(mutation['body']))
            self.snapshot['rulesets'][0]['rules'] = [rule for rule in self.snapshot['rulesets'][0]['rules']
                                                    if rule['type'] != 'required_signatures']
            return response

        with self.assertRaisesRegex(self.access.AccessDenied, '^remediation_outcome_unverified$'):
            self.apply(admin=inconsistent_write)

    def test_post_write_api_failure_is_not_success_and_does_not_retry(self):
        def write_then_fail(access, record, mutation, deadline):
            self.admin_writes.append(mutation)
            self.responses['/repos/acme/project'] = (503, None, {})
            return copy.deepcopy(self.snapshot['rulesets'][0]) | mutation['body']

        with self.assertRaisesRegex(self.access.AccessDenied, '^api_http_503$'):
            self.apply(admin=write_then_fail)
        self.assertEqual(len(self.admin_writes), 1)

    def configure_missing_baselines(self):
        self.record['baseline_rulesets'] = {}
        self.registry.write_text(json.dumps({'version': 1, 'inspector_apps': [self.app], 'repositories': [self.record]}))
        self.snapshot['rulesets'].clear()
        self.responses['/repos/acme/project/rulesets?includes_parents=true&per_page=100&page=1'] = (200, [], {})

    def create_baseline(self, access, record, mutation, deadline):
        self.admin_writes.append(copy.deepcopy(mutation))
        ruleset = copy.deepcopy(mutation['body']) | {
            'id': 100 if mutation['role'] == 'history' else 101,
            'source_type': 'Repository', 'source': 'acme/project',
        }
        self.snapshot['rulesets'].append(ruleset)
        self.responses['/repos/acme/project/rulesets?includes_parents=true&per_page=100&page=1'][1].append({'id': ruleset['id']})
        self.responses[f'/repos/acme/project/rulesets/{ruleset["id"]}?includes_parents=true'] = (200, ruleset, {})
        return copy.deepcopy(ruleset)

    def test_apply_creates_baselines_and_records_only_the_returned_numeric_ids(self):
        self.configure_missing_baselines()
        report = self.apply(admin=self.create_baseline)
        self.assertEqual(report['mode'], 'verified')
        self.assertEqual([entry['method'] for entry in self.admin_writes], ['POST', 'POST'])
        self.assertEqual(json.loads(self.registry.read_text())['repositories'][0]['baseline_rulesets'],
                         {'history': 100, 'default_pr': 101})

    def test_failed_second_creation_leaves_a_partial_read_only_record_without_rollback(self):
        self.configure_missing_baselines()

        def fail_second(access, record, mutation, deadline):
            if mutation['role'] == 'default_pr':
                access.deny('api_http_503')
            return self.create_baseline(access, record, mutation, deadline)

        with self.assertRaisesRegex(self.access.AccessDenied, '^api_http_503$'):
            self.apply(admin=fail_second)
        self.assertEqual(json.loads(self.registry.read_text())['repositories'][0]['baseline_rulesets'], {'history': 100})
        self.assertEqual(len(self.admin_writes), 1)
        status, report = self.inspect_app()
        self.assertEqual(status, 1)
        self.assertEqual(report['mode'], 'read_only')


class AdminTransportTests(ProtectionPolicyFixture):
    def mutate(self, *, repository_id=42, command_status=0):
        self.sent = []
        self.environments = []

        def command(arguments, **kwargs):
            self.assertEqual(arguments[1:], ['auth', 'token', '--hostname', 'github.com'])
            self.environments.append(kwargs['env'])
            return subprocess.CompletedProcess(arguments, command_status, b'SYNTHETIC_HOST_ADMIN_TOKEN\n', b'secret error')

        def request(method, path, token, deadline, body=None):
            self.sent.append((method, path, body))
            self.assertEqual(token, 'SYNTHETIC_HOST_ADMIN_TOKEN')
            if method == 'GET':
                return 200, {'id': repository_id, 'full_name': 'acme/project'}, {}
            return 200, self.snapshot['rulesets'][0], {}

        mutation = {'method': 'PUT', 'id': 100, 'body': self.policy.floor_body('history')}
        with patch.dict('os.environ', self.environment | {'GH_TOKEN': 'UNTRUSTED', 'GITHUB_TOKEN': 'UNTRUSTED',
                                                        'GH_CONFIG_DIR': '/checkout/config'}, clear=True), \
                patch.object(self.policy.subprocess, 'run', side_effect=command), \
                patch.object(self.access, 'github_request', side_effect=request, create=True):
            return self.policy.admin_mutation(self.access, self.record, mutation, self.access.time.monotonic() + 30)

    def test_host_admin_rechecks_numeric_identity_and_ignores_agent_token_environment(self):
        self.mutate()
        self.assertEqual([(method, path) for method, path, body in self.sent],
                         [('GET', '/repos/acme/project'), ('PUT', '/repos/acme/project/rulesets/100')])
        self.assertNotIn('GH_TOKEN', self.environments[0])
        self.assertNotIn('GITHUB_TOKEN', self.environments[0])
        self.assertEqual(self.environments[0]['GH_CONFIG_DIR'], str(self.home / '.config/gh'))

    def test_admin_cannot_mutate_a_recreated_repository(self):
        with self.assertRaisesRegex(self.access.AccessDenied, '^remote_identity_mismatch$'):
            self.mutate(repository_id=99)
        self.assertEqual(len(self.sent), 1)

    def test_missing_host_admin_login_never_falls_back_or_prints_credentials(self):
        with self.assertRaisesRegex(self.access.AccessDenied, '^admin_authentication_unavailable$'):
            self.mutate(command_status=1)
        self.assertEqual(self.sent, [])


class TokenSetupTests(unittest.TestCase):
    def helper(self, command):
        result = subprocess.run(['bash', '-c', 'source "$1"; ' + command, 'test', str(REPO / 'tools/agent-github.sh')],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout

    def test_legacy_token_template_never_requests_workflows_or_administration(self):
        url = self.helper('fine_grained_token_url project acme').strip()
        permissions = parse_qs(urlsplit(url).query)
        self.assertNotIn('workflows', permissions)
        self.assertNotIn('administration', permissions)
        self.assertNotIn('checks', permissions)
        for name in ['actions', 'statuses', 'metadata']:
            self.assertEqual(permissions.get(name), ['read'])
        for name in ['contents', 'issues', 'pull_requests']:
            self.assertEqual(permissions.get(name), ['write'])

    def test_token_instructions_do_not_ask_for_workflows_and_keep_required_ci_reads(self):
        text = self.helper('print_token_instructions project acme/project')
        self.assertNotIn('Pull requests, Workflows', text)
        self.assertIn('Workflows          No access', text)
        self.assertIn('Checks', text)
        self.assertIn('Commit statuses', text)
        self.assertIn('separately proved', text)


class AgentGithubTokenTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.home = self.root / "home"
        (self.home / "IdeaProjects/shop").mkdir(parents=True)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.url_file = self.root / "opened-url"
        self.environment = {
            **os.environ,
            "HOME": str(self.home),
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "OPENED_URL": str(self.url_file),
        }
        self.environment.pop("ALLOW_WORKFLOWS", None)
        self.shim("git", 'echo https://github.com/example/shop.git')
        self.shim("xdg-open", 'printf "%s" "$1" > "$OPENED_URL"')
        self.shim("secret-tool", '[ "$1" != lookup ] || echo fake-test-token')
        self.shim("curl", '''
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) body=$2; shift 2 ;;
        -D) headers=$2; shift 2 ;;
        *) shift ;;
    esac
done
if [ -n "${body:-}" ]; then
    printf '%s' '{"private":false,"full_name":"example/shop"}' > "$body"
    printf 'github-authentication-token-expiration: tomorrow\r\n' > "$headers"
    printf 200
else
    printf '[]'
fi
''')

    def tearDown(self):
        self.directory.cleanup()

    def shim(self, name, body):
        path = self.bin / name
        path.write_text(f"#!/bin/sh\nset -eu\n{body}\n")
        path.chmod(0o755)

    def create_token(self, allow_workflows=None, through_make=False):
        environment = dict(self.environment)
        if allow_workflows is not None:
            environment["ALLOW_WORKFLOWS"] = allow_workflows
        arguments = [str(SCRIPT), "token", "shop"]
        if through_make:
            arguments = ["make", "--no-print-directory", "agent-github-token", "PROJECT=shop"]
            if allow_workflows is not None:
                arguments.append(f"ALLOW_WORKFLOWS={allow_workflows}")
        return subprocess.run(arguments, cwd=REPO, env=environment, capture_output=True, text=True)

    def token_parameters(self):
        return parse_qs(urlsplit(self.url_file.read_text()).query)

    def test_default_token_does_not_request_workflow_access(self):
        result = self.create_token()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("workflows", self.token_parameters())
        self.assertNotIn("Pull requests, Workflows", result.stdout)

    def test_regular_repository_permissions_are_preserved(self):
        result = self.create_token()
        self.assertEqual(result.returncode, 0, result.stderr)
        permissions = self.token_parameters()
        for permission, expected in {
            "contents": ["write"], "issues": ["write"], "pull_requests": ["write"],
            "actions": ["read"], "metadata": ["read"],
        }.items():
            with self.subTest(permission=permission):
                self.assertEqual(permissions[permission], expected)

    def test_explicit_opt_in_requests_workflow_write_access(self):
        result = self.create_token("1", through_make=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.token_parameters().get("workflows"), ["write"])
        self.assertIn("Pull requests, Workflows", result.stdout)

    def test_make_defaults_to_no_workflow_access(self):
        result = self.create_token(through_make=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("workflows", self.token_parameters())

    def test_zero_does_not_opt_in(self):
        result = self.create_token("0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("workflows", self.token_parameters())

    def test_invalid_opt_in_opens_no_token_page(self):
        result = self.create_token("yes")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ALLOW_WORKFLOWS must be 0 or 1", result.stderr)
        self.assertFalse(self.url_file.exists())


if __name__ == '__main__':
    unittest.main()
