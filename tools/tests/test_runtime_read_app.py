import importlib.util
import json
from pathlib import Path
import subprocess
import time
import unittest
from unittest.mock import patch


ACCESS = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents/git-access.py'


class RuntimeReadAppTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location('read_app_access', ACCESS)
        self.access = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.access)
        self.permissions = {name: 'read' for name in ['metadata', 'contents', 'issues', 'pull_requests', 'checks', 'statuses', 'actions']}
        now = int(time.time())
        self.read = {'id': 'read-app', 'kind': 'github_app', 'role': 'read', 'repository_id': 42,
                     'app_id': 456, 'client_id': 'IvReadApp', 'installation_id': 78, 'owner_id': 99,
                     'setup_approved': True, 'key_epoch': 'first', 'reviewed_at': now - 60,
                     'valid_until': now + 3600, 'expires_at': now + 3600, 'fingerprint': 'a' * 64,
                     'permissions': self.permissions}
        self.forward = self.read | {'id': 'forward', 'kind': 'fine_grained_pat', 'role': 'forward',
                                   'actor_id': 88, 'fingerprint': 'b' * 64,
                                   'permissions': {'metadata': 'read', 'contents': 'write'}}
        self.record = {'repository_id': 42, 'canonical_name': 'acme/project',
                       'credential_roles': {'read': 'read-app', 'forward': 'forward'}}
        self.calls = []

    def validate(self):
        return self.access.runtime_credential_profiles(self.record, {'runtime_credentials': [self.read, self.forward]})

    def test_separate_read_app_is_accepted_without_invented_pat_actor(self):
        result = self.validate()
        self.assertEqual(result['read']['installation_id'], 78)
        self.assertEqual(result['forward']['actor_id'], 88)

    def test_app_cannot_supply_write_or_inspector_permissions(self):
        for permission in ['contents', 'checks', 'administration']:
            with self.subTest(permission=permission):
                self.read['permissions'] = self.permissions | {permission: 'write'}
                with self.assertRaises(self.access.AccessDenied):
                    self.validate()

    def test_missing_app_identity_never_becomes_a_pat_fallback(self):
        for field in ['app_id', 'installation_id', 'owner_id', 'client_id']:
            original = self.read.pop(field)
            with self.subTest(field=field), self.assertRaises(self.access.AccessDenied):
                self.validate()
            self.read[field] = original

    def request(self, method, path, token, deadline, body=None, **options):
        self.calls.append((method, path, token, body))
        if path == '/app':
            result = {'id': 456, 'client_id': 'IvReadApp', 'permissions': self.permissions}
        elif path == '/app/installations/78':
            result = {'app_id': 456, 'account': {'id': 99}, 'permissions': self.permissions,
                      'repository_selection': 'selected', 'suspended_at': None}
        elif path == '/app/installations/78/access_tokens':
            self.assertEqual(body, {'repository_ids': [42], 'permissions': self.permissions})
            result = {'token': 'SYNTHETIC_READ_APP', 'permissions': self.permissions,
                      'repositories': [{'id': 42}],
                      'expires_at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(time.time() + 3500))}
            return 201, result, {}
        elif path == '/repos/acme/project':
            result = {'id': 42, 'full_name': 'acme/project', 'owner': {'id': 99}}
        else:
            self.fail('unexpected read-App route: ' + path)
        return 200, result, {}

    def test_read_app_mints_only_enrolled_repository_and_verifies_owner(self):
        with patch.object(self.access, 'sign_app_jwt', return_value='SYNTHETIC_READ_JWT') as signer, \
                patch.object(self.access, 'api_request', side_effect=self.request):
            token = self.access.read_runtime_credential(self.record, self.read, time.monotonic() + 30)
        self.assertEqual(token, 'SYNTHETIC_READ_APP')
        self.assertEqual(self.calls[-1][1], '/repos/acme/project')
        self.assertNotIn('/user', [entry[1] for entry in self.calls])
        self.assertEqual(signer.call_args.kwargs['service'], 'cockpit-github-runtime-app')

    def test_wrong_minted_repository_never_releases_read_token(self):
        def request(*arguments, **options):
            status, value, headers = self.request(*arguments, **options)
            if arguments[1].endswith('/access_tokens'):
                value['repositories'] = [{'id': 43}]
            return status, value, headers

        with patch.object(self.access, 'sign_app_jwt', return_value='SYNTHETIC_READ_JWT'), \
                patch.object(self.access, 'api_request', side_effect=request), self.assertRaises(self.access.AccessDenied):
            self.access.read_runtime_credential(self.record, self.read, time.monotonic() + 30)

    def test_unmetered_authentication_requires_explicit_fresh_endpoint_evidence(self):
        self.assertTrue(hasattr(self.access, 'authentication_accounting'), 'explicit auth accounting is missing')
        with self.assertRaises(self.access.AccessDenied):
            self.access.authentication_accounting(self.read, '/app', {})

        self.read['authentication_accounting'] = {'mode': 'no_primary_headers',
            'reviewed_at': int(time.time()) - 60, 'valid_until': int(time.time()) + 3600,
            'evidence': 'a' * 64, 'paths': ['/app', '/app/installations/78', '/app/installations/78/access_tokens']}
        result = self.access.authentication_accounting(self.read, '/app', {})
        self.assertTrue(result['unmetered_authentication'])
        with self.assertRaises(self.access.AccessDenied):
            self.access.authentication_accounting(self.read, '/repos/acme/project', {})
        self.read['authentication_accounting']['valid_until'] = 1
        with self.assertRaises(self.access.AccessDenied):
            self.access.authentication_accounting(self.read, '/app', {})

    def test_dispatch_cannot_mint_an_unbudgeted_read_app_token(self):
        with patch.object(self.access, 'sign_app_jwt') as signer, self.assertRaises(self.access.AccessDenied):
            self.access.read_runtime_credential(self.record, self.read, time.monotonic() + 30, verify_remote=False)
        signer.assert_not_called()


if __name__ == '__main__':
    unittest.main()
