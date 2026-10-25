import base64
import importlib.util
import contextlib
import hashlib
import io
from pathlib import Path
import json
import os
import select
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch


BROKER = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents/git-broker.py'


class BrokerFixture(unittest.TestCase):
    def setUp(self):
        self.assertTrue(BROKER.exists(), 'the host broker framing entry point does not exist')
        spec = importlib.util.spec_from_file_location('cockpit_test_broker', BROKER)
        self.broker = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.broker)

    def header(self, **changes):
        return dict(version=1, capability='a' * 64, action='git.fetch', body_length=0) | changes

    def frame(self, header, body=b''):
        encoded = header if isinstance(header, bytes) else json.dumps(header).encode()
        return struct.pack('!I', len(encoded)) + encoded + body


class BrokerFramingTests(BrokerFixture):
    def receive(self, framed, *, close_write=True, deadline=None):
        client, host = socket.socketpair()
        self.addCleanup(client.close)
        self.addCleanup(host.close)
        client.sendall(framed)
        if close_write:
            client.shutdown(socket.SHUT_WR)
        with tempfile.TemporaryFile() as body:
            header = self.broker.read_request(host, body, deadline=deadline or time.monotonic() + 2)
            body.seek(0)
            return header, body.read()

    def test_complete_request_preserves_opaque_git_bytes_in_the_private_spool(self):
        payload = b'0000PACK\x00\xff\x01'
        header = self.header(body_length=len(payload))
        self.assertEqual(self.receive(self.frame(header, payload)), (header, payload))

    def test_duplicate_unknown_and_invalid_header_fields_are_rejected(self):
        invalid_headers = [
            b'{"version":1,"version":1,"capability":"' + b'a' * 64 + b'","action":"git.fetch","body_length":0}',
            self.header(repository_id=42), self.header(url='https://github.com/another/repo'),
            self.header(headers={'Authorization': 'SYNTHETIC_SECRET'}),
            self.header(version=True), self.header(version=2), self.header(body_length=True),
            self.header(body_length=-1), self.header(capability='SYNTHETIC_SECRET'),
            self.header(action='credential.read'), self.header(action='api.request'),
            self.header(protocol=3), self.header(protocol=True), self.header(service='receive-pack'),
            self.header(action='git.advertise'),
            self.header(action='git.advertise', service='upload-pack', body_length=1),
            b'[]', b'\xff', b'{',
        ]
        for header in invalid_headers:
            with self.subTest(header=header):
                with self.assertRaises(self.broker.RequestDenied) as denied:
                    self.receive(self.frame(header))
                self.assertNotIn('SYNTHETIC_SECRET', str(denied.exception))

    def test_oversized_header_is_rejected_without_waiting_for_its_bytes(self):
        with self.assertRaisesRegex(self.broker.RequestDenied, '^invalid_frame$'):
            self.receive(struct.pack('!I', 16 * 1024 + 1), close_write=False)

    def test_oversized_git_body_is_rejected_before_spooling(self):
        with self.assertRaisesRegex(self.broker.RequestDenied, '^body_too_large$'):
            self.receive(self.frame(self.header(body_length=1024 ** 3 + 1)), close_write=False)

    def test_truncated_prefix_header_and_body_are_not_valid_requests(self):
        for framed in [b'', b'\x00\x00', struct.pack('!I', 100) + b'{}',
                       self.frame(self.header(body_length=2), b'x')]:
            with self.subTest(framed=framed), self.assertRaisesRegex(self.broker.RequestDenied, '^truncated_frame$'):
                self.receive(framed)

    def test_trailing_bytes_and_second_requests_are_rejected(self):
        for trailing in [b'\x00', self.frame(self.header())]:
            with self.subTest(trailing=trailing), self.assertRaisesRegex(self.broker.RequestDenied, '^trailing_data$'):
                self.receive(self.frame(self.header()) + trailing)

    def test_peer_must_half_close_before_the_total_deadline(self):
        with self.assertRaisesRegex(self.broker.RequestDenied, '^request_timeout$'):
            self.receive(self.frame(self.header()), close_write=False, deadline=time.monotonic() + 0.03)

    def test_long_total_budget_still_sets_the_socket_idle_deadline(self):
        client, host = socket.socketpair()
        with client, host, tempfile.TemporaryFile() as body:
            client.sendall(self.frame(self.header()))
            client.shutdown(socket.SHUT_WR)
            self.broker.read_request(host, body, deadline=time.monotonic() + 120)
            self.assertEqual(host.gettimeout(), 30)

    def test_expired_deadline_never_consumes_or_spools_a_request(self):
        with self.assertRaisesRegex(self.broker.RequestDenied, '^request_timeout$'):
            self.receive(self.frame(self.header()), deadline=time.monotonic() - 1)

    def test_a_late_half_close_cannot_publish_a_successful_request(self):
        clock = [100.0]
        receive_bytes = self.broker.receive_bytes

        def delayed_close(*arguments):
            chunk = receive_bytes(*arguments)
            if not chunk:
                clock[0] = 103.0
            return chunk

        with patch.object(self.broker.time, 'monotonic', side_effect=lambda: clock[0]), \
                patch.object(self.broker, 'receive_bytes', side_effect=delayed_close), \
                self.assertRaisesRegex(self.broker.RequestDenied, '^request_timeout$'):
            self.receive(self.frame(self.header()), deadline=102.0)

    def test_advertisement_selectors_are_typed_but_never_an_upstream_url(self):
        for service in ['upload-pack', 'receive-pack']:
            header = self.header(action='git.advertise', service=service)
            self.assertEqual(self.receive(self.frame(header)), (header, b''))

    def test_unavailable_broker_cli_never_claims_a_listener_is_running(self):
        with tempfile.TemporaryDirectory(dir='/tmp/opencode') as scratch:
            socket_path = Path(scratch) / 'channel.sock'
            result = subprocess.run([
                sys.executable, '-I', str(BROKER), 'serve', '--decision', str(Path(scratch) / 'decision.json'),
                '--socket', str(socket_path),
            ], capture_output=True, text=True, env=os.environ | {'GH_TOKEN': 'SYNTHETIC_WRITE_TOKEN'})
            self.assertEqual(result.returncode, 77, result.stdout + result.stderr)
            self.assertEqual(result.stdout, '')
            self.assertEqual(result.stderr, 'git-broker: broker_channel_unavailable\n')
            self.assertFalse(socket_path.exists())


class BrokerPushPolicyTests(BrokerFixture):
    def command(self, ref=b'refs/heads/feature/nested', old=b'0' * 40, new=b'1' * 40,
                capabilities=b'report-status side-band-64k'):
        payload = old + b' ' + new + b' ' + ref
        if capabilities is not None:
            payload += b'\0' + capabilities
        return f'{len(payload) + 4:04x}'.encode() + payload

    def validate(self, encoded):
        self.assertTrue(hasattr(self.broker, 'validate_push'), 'receive-pack policy is missing')
        body = io.BytesIO(encoded)
        result = self.broker.validate_push(body, default_branch='main')
        self.assertEqual(body.tell(), 0, 'forwarding must retain the exact original bytes')
        self.assertEqual(body.read(), encoded)
        return result

    def test_new_and_nested_feature_updates_preserve_opaque_pack(self):
        encoded = self.command() + self.command(b'refs/heads/other', old=b'2' * 40,
                                                capabilities=None) + b'0000PACK\x00opaque'
        self.assertEqual(self.validate(encoded), ['refs/heads/feature/nested', 'refs/heads/other'])

    def test_delete_default_tag_and_duplicate_refs_deny(self):
        for encoded in [self.command(new=b'0' * 40), self.command(b'refs/heads/main'),
                        self.command(b'refs/tags/v1'), self.command() + self.command(capabilities=None)]:
            with self.subTest(encoded=encoded), self.assertRaises(self.broker.RequestDenied):
                self.validate(encoded + b'0000PACK')

    def test_malformed_or_unsupported_receive_commands_deny(self):
        for encoded in [b'', b'0000PACK', b'0001', b'0002', b'0003', b'ffffshort', b'zzzz',
                        self.command()[:-1], self.command(old=b'1' * 64) + b'0000',
                        self.command(new=b'g' * 40) + b'0000',
                        self.command(capabilities=b'push-options') + b'0000',
                        self.command(capabilities=b'object-format=sha256') + b'0000',
                        b'000epush-cert\n0000',
                        self.command() + self.command(b'refs/heads/next') + b'0000']:
            with self.subTest(encoded=encoded), self.assertRaises(self.broker.RequestDenied):
                self.validate(encoded)

    def test_exact_empty_rpc_probe_is_not_a_ref_update(self):
        self.assertEqual(self.validate(b'0000'), [])

    def test_shallow_push_preserves_boundaries_and_validates_updates(self):
        for ending in [b'', b'\n']:
            payload = b'shallow ' + b'a' * 40 + ending
            shallow = f'{len(payload) + 4:04x}'.encode() + payload
            with self.subTest(ending=ending):
                self.assertEqual(self.validate(shallow + self.command() + b'0000PACK'),
                                 ['refs/heads/feature/nested'])
                for encoded in [shallow + b'0000', self.command() + shallow + b'0000',
                                shallow + self.command(new=b'0' * 40) + b'0000',
                                shallow + self.command(b'refs/heads/main') + b'0000']:
                    with self.assertRaises(self.broker.RequestDenied):
                        self.validate(encoded)

    def test_native_git_capability_separator_is_supported(self):
        self.assertEqual(self.validate(self.command(capabilities=b' report-status-v2 side-band-64k quiet object-format=sha1 agent=git/2.56.0-Linux')
                                       + b'0000PACK'), ['refs/heads/feature/nested'])

    def test_invalid_ref_names_cannot_reach_forwarding(self):
        for suffix in [b'', b'.hidden', b'foo..bar', b'foo@{bar', b'foo.lock', b'foo/.bar',
                       b'foo//bar', b'foo/', b'foo.', b'foo bar', b'foo\\bar', b'foo\x7f',
                       b'foo:bar', b'foo?bar', b'foo*bar', b'foo[bar']:
            with self.subTest(suffix=suffix), self.assertRaises(self.broker.RequestDenied):
                self.validate(self.command(b'refs/heads/' + suffix) + b'0000')

    def test_ref_and_prefix_limits_apply_before_pack(self):
        commands = [self.command(f'refs/heads/branch-{number}'.encode(), capabilities=None)
                    for number in range(1025)]
        with self.assertRaises(self.broker.RequestDenied):
            self.validate(b''.join(commands) + b'0000')
        commands = [self.command(b'refs/heads/' + str(number).encode() + b'x' * 1100,
                                 capabilities=None) for number in range(1000)]
        with self.assertRaises(self.broker.RequestDenied):
            self.validate(b''.join(commands) + b'0000')


class BrokerDispatchTests(BrokerFixture):
    def test_no_primary_cost_requires_fresh_action_specific_evidence(self):
        access = self.broker.load_access()
        record = {'repository_id': 42, 'canonical_name': 'acme/project'}
        serialized = b'private registry fixture'
        binding = record | {'registry_digest': hashlib.sha256(serialized).hexdigest()}
        profile = {'kind': 'fine_grained_pat', 'actor_id': 99, 'permissions': {'contents': 'write'},
                   'git_rate_setup_approved': True, 'git_rate_costs': {'git.push': {}}}
        with patch.object(access, 'load_registry', return_value=({'repositories': [record]}, serialized)), \
                patch.object(access, 'runtime_credential_profiles', return_value={'forward': profile}):
            with self.assertRaises(self.broker.RequestDenied):
                self.broker.git_authority(access, binding, self.header(action='git.push'))
            profile['git_unmetered_evidence'] = {'git.push': {'reviewed_at': int(time.time()) - 60,
                'valid_until': int(time.time()) + 3600, 'evidence': 'a' * 64, 'mode': 'no_primary_headers'}}
            self.assertEqual(self.broker.git_authority(access, binding, self.header(action='git.push'))[2], {})
            profile['git_unmetered_evidence']['git.push']['valid_until'] = 1
            with self.assertRaises(self.broker.RequestDenied):
                self.broker.git_authority(access, binding, self.header(action='git.push'))

    def test_git_authority_requires_reviewed_role_specific_costs(self):
        access = self.broker.load_access()
        record = {'repository_id': 42, 'canonical_name': 'acme/project'}
        serialized = b'private registry fixture'
        binding = record | {'registry_digest': hashlib.sha256(serialized).hexdigest()}
        profiles = {'read': {'kind': 'fine_grained_pat', 'actor_id': 88, 'permissions': {'contents': 'read'}},
                    'forward': {'kind': 'fine_grained_pat', 'actor_id': 99, 'permissions': {'contents': 'write'}}}
        with patch.object(access, 'load_registry', return_value=({'repositories': [record]}, serialized)), \
                patch.object(access, 'runtime_credential_profiles', return_value=profiles):
            with self.assertRaisesRegex(self.broker.RequestDenied, '^git_rate_setup_required$'):
                self.broker.git_authority(access, binding, self.header(action='git.push'))
            profiles['forward'].update(git_rate_setup_approved=True,
                                       git_rate_costs={'git.push': {'user:99:git': 1}})
            _, profile, costs = self.broker.git_authority(access, binding, self.header(action='git.push'))
            self.assertEqual(profile['actor_id'], 99)
            self.assertEqual(costs, {'user:99:git': 1})
            profiles['read'].update(git_rate_setup_approved=True, git_rate_costs={'git.fetch': {'user:88:git': 1}})
            _, profile, _ = self.broker.git_authority(access, binding, self.header())
            self.assertEqual(profile['actor_id'], 88)
            profiles['forward']['git_rate_costs']['git.push'] = {'user:88:git': 1}
            with self.assertRaisesRegex(self.broker.RequestDenied, '^git_rate_setup_required$'):
                self.broker.git_authority(access, binding, self.header(action='git.push'))

    def test_fixed_routes_never_accept_a_client_destination(self):
        self.assertTrue(hasattr(self.broker, 'git_route'), 'fixed Git routes are missing')
        cases = [(self.header(action='git.fetch'), 'POST', '/acme/project.git/git-upload-pack'),
                 (self.header(action='git.push'), 'POST', '/acme/project.git/git-receive-pack'),
                 (self.header(action='git.advertise', service='receive-pack'), 'GET',
                  '/acme/project.git/info/refs?service=git-receive-pack')]
        for header, method, path in cases:
            with self.subTest(header=header):
                route = self.broker.git_route(header, 'acme/project')
                self.assertEqual(route['method'], method)
                self.assertEqual(route['url'], 'https://github.com' + path)
        for name in ['//evil/repo', 'acme/../evil', 'acme/project?token=x', 'acme/project\r\nX: x']:
            with self.subTest(name=name), self.assertRaises(self.broker.RequestDenied):
                self.broker.git_route(self.header(), name)

    def test_receive_pack_rejects_upload_protocol_versions(self):
        self.assertTrue(hasattr(self.broker, 'git_route'), 'fixed Git routes are missing')
        for action in ['git.push', 'git.advertise']:
            header = self.header(action=action, protocol=2)
            if action == 'git.advertise':
                header['service'] = 'receive-pack'
            with self.assertRaises(self.broker.RequestDenied):
                self.broker.git_route(header, 'acme/project')


class BrokerChannelTests(BrokerFixture):
    def setUp(self):
        super().setUp()
        self.assertTrue(hasattr(self.broker, 'serve'), 'host-only broker channel does not exist')
        self.scratch = tempfile.TemporaryDirectory(prefix='broker-', dir='/tmp/opencode')
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.runtime = self.root / 'run'
        self.runtime.mkdir(mode=0o700)
        self.home = self.root / 'home'
        self.home.mkdir(mode=0o700)
        environment = patch.dict(os.environ, {'HOME': str(self.home), 'XDG_RUNTIME_DIR': str(self.runtime),
                                              'GH_TOKEN': 'SYNTHETIC_FORWARD_SECRET',
                                              'GH_PROJECT_TOKEN': 'SYNTHETIC_PROJECT_SECRET'})
        environment.start()
        self.addCleanup(environment.stop)
        registry = self.home / '.config/greywall-agents/git-repositories.json'
        registry.parent.mkdir(parents=True, mode=0o700)
        registry.write_text(json.dumps({'version': 1, 'repositories': [], 'inspector_apps': []}))
        registry.chmod(0o600)
        self.control = self.runtime / 'greywall-agents/git-access'
        self.control.mkdir(parents=True, mode=0o700)
        self.binding = {
            'version': 1, 'mode': 'verified', 'session': '1' * 32, 'generation': '2' * 32,
            'capability': 'a' * 64, 'revoked': False,
            'checkout': str(self.root / 'checkout'), 'repository_id': 42,
            'canonical_name': 'acme/project', 'git_dir': str(self.root / 'checkout/.git'),
            'common_dir': str(self.root / 'checkout/.git'), 'configuration_digest': '3' * 64,
            'registry_digest': hashlib.sha256(registry.read_bytes()).hexdigest(),
            'checked_at': time.monotonic(), 'checked_wall': time.time(),
            'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
        }
        self.decision = self.control / (self.binding['session'] + '.json')
        self.write_binding()
        self.socket_path = self.runtime / 'greywall-git-channel' / self.binding['session'] / 'broker.sock'
        self.access = self.broker.load_access()
        report = {key: self.binding[key] for key in ['checkout', 'repository_id', 'canonical_name',
                                                   'git_dir', 'common_dir', 'configuration_digest']}
        self.report = report | {'mode': 'verified', 'identity_verified': True, 'rate_verified': True,
                                'default_branch': 'main',
                                'credential_roles_verified': ['read', 'forward']}
        inspector = patch.object(self.access, 'inspect_access', side_effect=lambda *args, **kwargs: self.report.copy())
        inspector.start()
        self.addCleanup(inspector.stop)
        loader = patch.object(self.broker, 'load_access', return_value=self.access)
        loader.start()
        self.addCleanup(loader.stop)

    def write_binding(self):
        self.decision.write_text(json.dumps(self.binding))
        self.decision.chmod(0o600)

    def start(self):
        ready = threading.Event()
        stop = threading.Event()
        failures = []

        def run():
            try:
                self.broker.serve(self.decision, self.socket_path, ready=ready.set, stop=stop)
            except Exception as error:
                failures.append(error)

        worker = threading.Thread(target=run)
        worker.start()

        def cleanup():
            stop.set()
            worker.join(3)
            self.assertFalse(worker.is_alive(), 'broker thread survived shutdown')

        self.addCleanup(cleanup)
        self.assertTrue(ready.wait(2), f'broker was not ready: {failures}')
        return stop, worker, failures

    def exchange(self, header, *, body=b'', close_write=True):
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(2)
            client.connect(str(self.socket_path))
            client.sendall(self.frame(header, body))
            if close_write:
                client.shutdown(socket.SHUT_WR)
            prefix = self.broker.read_exact(client, 4, time.monotonic() + 2)
            length = struct.unpack('!I', prefix)[0]
            response = json.loads(self.broker.read_exact(client, length, time.monotonic() + 2))
            payload = self.broker.read_exact(client, response['body_length'], time.monotonic() + 2)
            try:
                self.assertEqual(client.recv(1), b'')
            except ConnectionResetError:
                self.assertGreaterEqual(response['status'], 400)
            return response, payload

    def test_missing_or_wrong_capability_denies(self):
        self.start()
        missing = self.header()
        del missing['capability']
        self.assertEqual(self.exchange(missing)[0]['code'], 'invalid_header')
        self.assertEqual(self.exchange(self.header(capability='b' * 64))[0]['code'], 'capability_denied')

    def test_bad_capability_is_rejected_before_spooling_an_unfinished_body(self):
        self.start()
        response, _ = self.exchange(self.header(capability='b' * 64, body_length=100), close_write=False)
        self.assertEqual(response['code'], 'capability_denied')

    def test_client_cannot_select_repository_or_credential(self):
        self.start()
        for selector in ['repository_id', 'checkout', 'credential', 'generation', 'session', 'purpose', 'url']:
            with self.subTest(selector=selector):
                self.assertEqual(self.exchange(self.header(**{selector: 'another-target'}))[0]['code'], 'invalid_header')

    def test_authenticated_channel_has_no_forwarding_or_secret_response(self):
        self.start()
        response, body = self.exchange(self.header())
        self.assertEqual(response['code'], 'action_unavailable')
        self.assertEqual(response['status'], 503)
        self.assertEqual(body, b'')
        for secret in [self.binding['capability'], 'SYNTHETIC_FORWARD_SECRET', 'SYNTHETIC_PROJECT_SECRET']:
            self.assertNotIn(secret, json.dumps(response))

    def test_direct_push_cannot_bypass_receive_command_validation(self):
        self.start()
        payload = b'0' * 40 + b' ' + b'1' * 40 + b' refs/heads/main'
        body = f'{len(payload) + 4:04x}'.encode() + payload + b'0000'
        response, _ = self.exchange(self.header(action='git.push', body_length=len(body)), body=body)
        self.assertEqual(response['code'], 'push_ref_denied')

    def test_direct_receive_advertisement_rejects_upload_protocol(self):
        self.start()
        response, _ = self.exchange(self.header(action='git.advertise', service='receive-pack', protocol=2))
        self.assertEqual(response['code'], 'unsupported_git_protocol')

    def enable_issue_fixture(self):
        self.enable_fixture_forwarding()
        now = int(time.time())
        shared = {'kind': 'fine_grained_pat', 'repository_id': 42, 'actor_id': 88,
                  'key_epoch': 'fixture-1', 'fingerprint': 'b' * 64, 'setup_approved': True,
                  'reviewed_at': now - 60, 'valid_until': now + 3600, 'expires_at': now + 3600}
        profiles = [shared | {'id': 'read', 'role': 'read', 'permissions': {name: 'read' for name in
                    ['metadata', 'contents', 'issues', 'pull_requests', 'checks', 'statuses', 'actions']}},
                    shared | {'id': 'forward', 'role': 'forward', 'fingerprint': 'c' * 64,
                              'permissions': {'metadata': 'read', 'issues': 'write'}}]
        registry, _ = self.access.load_registry()
        registry['repositories'] = [{'project': 'project', 'repository_id': 42, 'canonical_name': 'acme/project',
                                     'credential_roles': {'read': 'read', 'forward': 'forward'},
                                     'mirror_edges': [], 'checkouts': [{'path': self.binding['checkout'],
                                         'git_dir': self.binding['git_dir'], 'common_dir': self.binding['common_dir']}]}]
        registry['runtime_credentials'] = profiles
        path = self.access.registry_path()
        path.write_text(json.dumps(registry))
        self.binding['registry_digest'] = hashlib.sha256(path.read_bytes()).hexdigest()
        self.write_binding()
        self.api_calls = []

        def request(method, path, token, deadline, body=None):
            self.api_calls.append((method, path, body))
            self.assertEqual(path, '/repos/acme/project/issues')
            self.assertEqual(method, 'POST')
            return 201, {'id': 107, 'number': 7, 'title': 'Investigate', 'state': 'open',
                         'repository_url': 'https://api.github.com/repos/acme/project',
                         'html_url': 'https://github.com/acme/project/issues/7',
                         'temp_clone_token': 'SYNTHETIC_FORWARD_SECRET'}, {
                             'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '5000',
                             'x-ratelimit-remaining': '4900', 'x-ratelimit-reset': str(now + 3600)}

        upstream = patch.object(self.access, 'github_request', side_effect=request)
        upstream.start()
        self.addCleanup(upstream.stop)

    def test_typed_issue_create_checks_before_dispatch_and_projects_result(self):
        self.enable_issue_fixture()
        self.start()
        body = b'{"title":"Investigate"}'
        response, payload = self.exchange(self.header(action='issue.create', body_length=len(body)), body=body)
        self.assertEqual(response['status'], 200, response)
        result = json.loads(payload)
        self.assertEqual(result['number'], 7)
        self.assertEqual(result['outcome'], 'succeeded')
        self.assertRegex(result['request_id'], r'^[0-9a-f]{32}$')
        self.assertNotIn('SYNTHETIC', payload.decode())
        self.assertEqual(self.api_calls, [('POST', '/repos/acme/project/issues', {'title': 'Investigate'})])
        self.assertEqual(self.purposes, ['admission', 'write'])
        journal = self.home / '.local/state/greywall-agents/github-actions'
        records = list(journal.iterdir())
        self.assertEqual([path.name for path in records], [result['request_id'] + '.completed'])
        self.assertNotIn('Investigate', records[0].read_text())
        self.assertNotIn('SYNTHETIC', records[0].read_text())
        self.assertEqual(records[0].stat().st_mode & 0o777, 0o600)

    def test_typed_issue_failed_check_never_mutates(self):
        self.enable_issue_fixture()
        self.write_report['mode'] = 'read_only'
        self.start()
        body = b'{"title":"Investigate"}'
        response, _ = self.exchange(self.header(action='issue.create', body_length=len(body)), body=body)
        self.assertEqual(response['code'], 'write_check_denied')
        self.assertEqual(self.api_calls, [])

    def test_failed_write_inspection_closes_session_channel_immediately(self):
        self.enable_issue_fixture()
        self.write_report['mode'] = 'read_only'
        stop, worker, _ = self.start()
        body = b'{"title":"Investigate"}'
        response, _ = self.exchange(self.header(action='issue.create', body_length=len(body)), body=body)
        self.assertEqual(response['code'], 'write_check_denied')
        worker.join(1)
        self.assertTrue(stop.is_set())
        self.assertFalse(worker.is_alive())
        self.assertFalse(self.socket_path.exists())

    def test_uncertain_throttled_git_write_still_revokes_session(self):
        self.enable_fixture_forwarding()
        stop, worker, _ = self.start()
        body = self.push_body()
        with patch.object(self.broker, 'run_git_transport', return_value={
                'status': 403, 'content_type': 'application/json', 'body_length': 0,
                'rate_headers': {'retry-after': '60'}}):
            response, _ = self.exchange(self.header(action='git.push', body_length=len(body)), body=body)
        self.assertEqual(response['code'], 'upstream_outcome_unknown')
        worker.join(1)
        self.assertTrue(stop.is_set())
        self.assertFalse(worker.is_alive())

    def test_typed_issue_rejects_duplicate_json_and_unknown_fields_before_upstream(self):
        self.enable_issue_fixture()
        self.start()
        for body in [b'{"title":"x","title":"y"}', b'{"title":"x","url":"https://evil.invalid"}']:
            response, _ = self.exchange(self.header(action='issue.create', body_length=len(body)), body=body)
            self.assertGreaterEqual(response['status'], 400)
        self.assertEqual(self.api_calls, [])

    def test_uncertain_issue_create_is_not_replayed(self):
        self.enable_issue_fixture()
        self.start()
        body = b'{"title":"Investigate"}'
        with patch.object(self.access, 'github_request', side_effect=OSError('private upstream error')):
            response, _ = self.exchange(self.header(action='issue.create', body_length=len(body)), body=body)
        self.assertEqual(response['code'], 'action_outcome_unknown')
        response, _ = self.exchange(self.header(action='issue.create', body_length=len(body)), body=body)
        self.assertEqual(response['code'], 'action_reconciliation_required')
        self.assertEqual(self.api_calls, [])

    def run_operator(self, arguments, stdin=''):
        output = io.StringIO()
        with patch.object(sys, 'argv', [str(BROKER), *arguments]), \
                patch.object(sys, 'stdin', io.StringIO(stdin)), contextlib.redirect_stdout(output), \
                contextlib.redirect_stderr(io.StringIO()):
            status = self.broker.main()
        return status, output.getvalue()

    def leave_uncertain_issue_create(self):
        self.enable_issue_fixture()
        self.start()
        body = b'{"title":"Investigate"}'
        with patch.object(self.access, 'github_request', side_effect=OSError('private upstream error')):
            response, _ = self.exchange(self.header(action='issue.create', body_length=len(body)), body=body)
        self.assertEqual(response['code'], 'action_outcome_unknown')
        return body

    def test_operator_lists_uncertain_actions_without_bodies_or_tokens(self):
        self.leave_uncertain_issue_create()
        status, output = self.run_operator(['pending'])
        self.assertEqual(status, 0)
        [record] = json.loads(output)
        self.assertEqual((record['action'], record['repository_id'], record['target_number']), ('issue.create', 42, None))
        self.assertRegex(record['request_id'], r'^[0-9a-f]{32}$')
        self.assertNotIn('Investigate', output)
        self.assertNotIn('SYNTHETIC', output)

    def test_operator_resolution_requires_exact_confirmation_and_then_unblocks(self):
        body = self.leave_uncertain_issue_create()
        [record] = json.loads(self.run_operator(['pending'])[1])
        request_id = record['request_id']
        for confirmation in ['', 'RESOLVE ' + request_id + ' failed', 'RESOLVE ' + 'f' * 32 + ' succeeded']:
            with self.subTest(confirmation=confirmation):
                status, _ = self.run_operator(['resolve', '--request-id', request_id, '--outcome', 'succeeded'], confirmation)
                self.assertNotEqual(status, 0)
        self.assertEqual(len(json.loads(self.run_operator(['pending'])[1])), 1)
        status, _ = self.run_operator(['resolve', '--request-id', request_id, '--outcome', 'succeeded'],
                                      'RESOLVE ' + request_id + ' succeeded\n')
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(self.run_operator(['pending'])[1]), [])
        journal = self.home / '.local/state/greywall-agents/github-actions'
        [resolved] = list(journal.glob('*.resolved'))
        self.assertEqual(json.loads(resolved.read_text())['resolution'], 'succeeded')
        time.sleep(1.1)  # the uncertain write keeps its pacing slot; resolution never refunds it
        response, _ = self.exchange(self.header(action='issue.create', body_length=len(body)), body=body)
        self.assertEqual(response['status'], 200, response)

    def test_operator_resolution_rejects_unknown_ids_and_invalid_outcomes(self):
        self.leave_uncertain_issue_create()
        for arguments in [['resolve', '--request-id', 'f' * 32, '--outcome', 'failed'],
                          ['resolve', '--request-id', '../x', '--outcome', 'failed'],
                          ['resolve', '--request-id', 'f' * 32, '--outcome', 'maybe']]:
            with self.subTest(arguments=arguments):
                status, _ = self.run_operator(arguments, 'RESOLVE ' + arguments[2] + ' ' + arguments[4] + '\n')
                self.assertNotEqual(status, 0)
        self.assertEqual(len(json.loads(self.run_operator(['pending'])[1])), 1)

    def enable_merge_fixture(self, *, head_after_scan='a' * 40):
        self.enable_review_fixture()
        now = int(time.time())
        registry, _ = self.access.load_registry()
        registry['runtime_credentials'][1]['permissions']['contents'] = 'write'
        registry['runtime_credentials'][0]['graphql_accounting'] = {'pull_request.merge': {
            'setup_approved': True, 'principal': 'installation:77', 'max_cost': 1,
            'reviewed_at': now - 60, 'valid_until': now + 1800, 'evidence': 'd' * 64}}
        path = self.access.registry_path()
        path.write_text(json.dumps(registry))
        self.binding['registry_digest'] = hashlib.sha256(path.read_bytes()).hexdigest()
        self.write_binding()
        self.rate.observe('installation:77', {'x-ratelimit-resource': 'graphql', 'x-ratelimit-limit': '5000',
            'x-ratelimit-remaining': '5000', 'x-ratelimit-reset': str(now + 3600)})
        self.merge_calls = []
        policy = base64.b64encode(b'## Git authorization\n\nClaude may merge pull requests.\n').decode()

        def upstream(method, path, token, deadline, body=None):
            self.merge_calls.append((method, path))
            headers = {'x-ratelimit-resource': 'graphql' if path == '/graphql' else 'core', 'x-ratelimit-limit': '5000',
                       'x-ratelimit-remaining': '4900', 'x-ratelimit-reset': str(now + 3600)}
            if path == '/repos/acme/project':
                return 200, {'id': 42, 'default_branch': 'main'}, headers
            if path == '/repos/acme/project/branches/main':
                return 200, {'commit': {'sha': 'c' * 40}}, headers
            if path.startswith('/repos/acme/project/contents/CLAUDE.md'):
                return 200, {'type': 'file', 'encoding': 'base64', 'sha': 'e' * 40, 'content': policy}, headers
            if path == '/graphql':
                return 200, {'data': {'repository': {'squashMergeAllowed': True, 'pullRequest': {
                    'state': 'OPEN', 'isDraft': False, 'isCrossRepository': False, 'headRefOid': 'a' * 40,
                    'isMergeQueueEnabled': False, 'reviewDecision': None, 'mergeStateStatus': 'CLEAN',
                    'commits': {'nodes': [{'commit': {'oid': 'a' * 40, 'statusCheckRollup': None}}]}}}}}, headers
            if path == '/repos/acme/project/pulls/7':
                return 200, {'id': 207, 'number': 7, 'state': 'open', 'user': {'id': 99},
                    'html_url': 'https://github.com/acme/project/pull/7',
                    'head': {'ref': 'feature', 'sha': head_after_scan, 'repo': {'id': 42}},
                    'base': {'ref': 'main', 'sha': 'b' * 40, 'repo': {'id': 42}}}, headers
            self.assertEqual((method, path), ('PUT', '/repos/acme/project/pulls/7/merge'))
            return 200, {'merged': True, 'sha': 'f' * 40}, headers

        active = patch.object(self.access, 'github_request', side_effect=upstream)
        active.start()
        self.addCleanup(active.stop)

    def merge_exchange(self):
        body = json.dumps({'number': 7, 'expected_head_sha': 'a' * 40, 'merge_method': 'squash'}).encode()
        return self.exchange(self.header(action='pull_request.merge', body_length=len(body)), body=body)

    def test_authorized_merge_rechecks_protection_and_head_before_submission(self):
        self.enable_merge_fixture()
        self.start()
        response, payload = self.merge_exchange()
        self.assertEqual(response['status'], 200, response)
        self.assertEqual(json.loads(payload)['merge_commit'], 'f' * 40)
        self.assertEqual(self.purposes, ['admission', 'write'])
        self.assertEqual(self.merge_calls[-2:], [('GET', '/repos/acme/project/pulls/7'),
                                                 ('PUT', '/repos/acme/project/pulls/7/merge')])

    def test_head_advancing_during_merge_scan_prevents_submission(self):
        self.enable_merge_fixture(head_after_scan='9' * 40)
        self.start()
        response, _ = self.merge_exchange()
        self.assertEqual(response.get('code'), 'review_head_changed')
        self.assertNotIn(('PUT', '/repos/acme/project/pulls/7/merge'), self.merge_calls)

    def enable_project_fixture(self, *, scopes='project', enroll_role=True):
        self.enable_issue_fixture()
        now = int(time.time())
        registry, _ = self.access.load_registry()
        record = registry['repositories'][0]
        record['project_board'] = {'node_id': 'PVT_board', 'fields': {
            'PVTF_effort': {'name': 'Effort', 'data_type': 'NUMBER'}}}
        if enroll_role:
            record['credential_roles']['project'] = 'board'
        registry['runtime_credentials'].append({
            'id': 'board', 'kind': 'classic_project_pat', 'role': 'project', 'repository_id': 42, 'actor_id': 77,
            'key_epoch': 'fixture-1', 'fingerprint': 'e' * 64, 'setup_approved': True, 'scopes': ['project'],
            'reviewed_at': now - 60, 'valid_until': now + 3600, 'expires_at': now + 3600,
            'graphql_accounting': {action: {'setup_approved': True, 'principal': 'user:77', 'max_cost': 1,
                                            'reviewed_at': now - 60, 'valid_until': now + 1800, 'evidence': 'f' * 64}
                                   for action in ['project.item.field_set', 'project.items.list']}})
        path = self.access.registry_path()
        path.write_text(json.dumps(registry))
        self.binding['registry_digest'] = hashlib.sha256(path.read_bytes()).hexdigest()
        self.write_binding()
        for resource in ['core', 'graphql']:
            self.rate.observe('user:77', {'x-ratelimit-resource': resource, 'x-ratelimit-limit': '5000',
                'x-ratelimit-remaining': '5000', 'x-ratelimit-reset': str(now + 3600)})
        self.project_calls = []

        def upstream(method, path, token, deadline, body=None):
            self.project_calls.append((method, path, body))
            headers = {'x-ratelimit-limit': '5000', 'x-ratelimit-remaining': '4990',
                       'x-ratelimit-reset': str(now + 3600)}
            if path == '/user':
                return 200, {'id': 77}, headers | {'x-ratelimit-resource': 'core', 'x-oauth-scopes': scopes}
            self.assertEqual(path, '/graphql')
            headers['x-ratelimit-resource'] = 'graphql'
            if body['query'].startswith('query($item'):
                return 200, {'data': {'node': {'id': 'PVTI_ours', 'project': {'id': 'PVT_board'},
                    'content': {'__typename': 'Issue', 'number': 7, 'repository': {'databaseId': 42}}}}}, headers
            return 200, {'data': {'updateProjectV2ItemFieldValue': {'projectV2Item': {'id': 'PVTI_ours'}}}}, headers

        active = patch.object(self.access, 'github_request', side_effect=upstream)
        active.start()
        self.addCleanup(active.stop)

    def project_exchange(self, action='project.item.field_set',
                         fields=b'{"item_id":"PVTI_ours","field_id":"PVTF_effort","value":3}'):
        return self.exchange(self.header(action=action, body_length=len(fields)), body=fields)

    def test_project_field_set_uses_host_only_board_role_after_fresh_write_check(self):
        self.enable_project_fixture()
        self.start()
        response, payload = self.project_exchange()
        self.assertEqual(response['status'], 200, response)
        self.assertEqual(json.loads(payload)['applied_fields'], {'PVTF_effort': 3})
        self.assertEqual(self.purposes, ['admission', 'write'])
        self.assertEqual([call[1] for call in self.project_calls], ['/user', '/graphql', '/user', '/graphql'])
        self.assertNotIn('SYNTHETIC', payload.decode())

    def test_project_token_with_code_scopes_never_mutates(self):
        self.enable_project_fixture(scopes='project, repo')
        self.start()
        response, _ = self.project_exchange()
        self.assertEqual(response.get('code'), 'project_credential_scope_invalid')
        self.assertEqual([call[1] for call in self.project_calls], ['/user'])

    def test_project_actions_require_explicit_board_role_enrollment(self):
        self.enable_project_fixture(enroll_role=False)
        self.start()
        response, _ = self.project_exchange()
        self.assertEqual(response.get('code'), 'project_role_unavailable')
        self.assertEqual(self.project_calls, [])

    def test_project_mutation_without_measured_graphql_cost_is_refused(self):
        self.enable_project_fixture()
        self.start()
        response, _ = self.project_exchange('project.item.move', b'{"item_id":"PVTI_ours","after_id":null}')
        self.assertEqual(response.get('code'), 'graphql_rate_setup_required')
        self.assertNotIn('updateProjectV2ItemPosition', json.dumps(self.project_calls))

    def test_project_reads_are_funded_without_write_inspection(self):
        self.enable_project_fixture()
        self.start()
        response, _ = self.project_exchange('project.items.list', b'{}')
        self.assertNotEqual(response.get('code'), 'write_check_denied')
        self.assertEqual(self.purposes, ['admission'])

    def test_pr_comment_cannot_borrow_issue_only_write_grant(self):
        self.enable_issue_fixture()
        self.start()
        calls = []

        def upstream(method, path, token, deadline, body=None):
            calls.append((method, path))
            self.assertEqual((method, path), ('GET', '/repos/acme/project/issues/7'))
            return 200, {'number': 7, 'id': 107, 'pull_request': {},
                         'repository_url': 'https://api.github.com/repos/acme/project'}, {
                'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '5000',
                'x-ratelimit-remaining': '4900', 'x-ratelimit-reset': str(int(time.time()) + 3600)}

        body = b'{"number":7,"body":"Review notes"}'
        with patch.object(self.access, 'github_request', side_effect=upstream):
            response, _ = self.exchange(self.header(action='comment.create', body_length=len(body)), body=body)
        self.assertEqual(response['code'], 'collaboration_role_denied')
        self.assertEqual(calls, [('GET', '/repos/acme/project/issues/7')])

    def enable_review_fixture(self):
        self.enable_issue_fixture()
        registry, _ = self.access.load_registry()
        registry['runtime_credentials'][1]['permissions']['pull_requests'] = 'write'
        registry['runtime_credentials'][0].update(kind='github_app', app_id=456, client_id='IvReadFixture',
                                                installation_id=77, owner_id=99)
        del registry['runtime_credentials'][0]['actor_id']
        self.rate.observe('installation:77', {'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '5000',
            'x-ratelimit-remaining': '5000', 'x-ratelimit-reset': str(int(time.time()) + 3600)})
        path = self.access.registry_path()
        path.write_text(json.dumps(registry))
        self.binding['registry_digest'] = hashlib.sha256(path.read_bytes()).hexdigest()
        self.write_binding()
        self.review_calls = []
        self.review_head = 'a' * 40

        def upstream(method, path, token, deadline, body=None):
            self.review_calls.append((method, path, body))
            headers = {'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '5000',
                       'x-ratelimit-remaining': '4900', 'x-ratelimit-reset': str(int(time.time()) + 3600)}
            if method != 'GET':
                return 200, {'id': 301, 'commit_id': 'a' * 40, 'state': 'APPROVED', 'user': {'id': 88},
                             'pull_request_url': 'https://api.github.com/repos/acme/project/pulls/7'}, headers
            head = 'a' * 40 if len(self.review_calls) == 1 else self.review_head
            return 200, {'id': 207, 'node_id': 'PR_fixture_207', 'number': 7, 'state': 'open', 'user': {'id': 99},
                'html_url': 'https://github.com/acme/project/pull/7',
                'head': {'ref': 'feature', 'sha': head, 'repo': {'id': 42}},
                'base': {'ref': 'main', 'sha': 'b' * 40, 'repo': {'id': 42}}}, headers
        active = patch.object(self.access, 'github_request', side_effect=upstream)
        active.start()
        self.addCleanup(active.stop)

    def test_review_head_change_during_protection_scan_prevents_submission(self):
        self.enable_review_fixture()
        self.review_head = 'c' * 40
        self.start()

        body = json.dumps({'number': 7, 'expected_head_sha': 'a' * 40, 'event': 'APPROVE'}).encode()
        response, _ = self.exchange(self.header(action='review.create', body_length=len(body)), body=body)
        self.assertEqual(response.get('code'), 'review_head_changed')
        self.assertEqual(self.review_calls, [('GET', '/repos/acme/project/pulls/7', None),
                                            ('GET', '/repos/acme/project/pulls/7', None)])

    def test_review_funds_separate_read_installation_guard_and_user_submission(self):
        self.enable_review_fixture()
        self.start()
        body = json.dumps({'number': 7, 'expected_head_sha': 'a' * 40, 'event': 'APPROVE'}).encode()
        response, payload = self.exchange(self.header(action='review.create', body_length=len(body)), body=body)
        self.assertEqual(response['status'], 200, response)
        result = json.loads(payload)
        self.assertEqual(result['review_state'], 'APPROVED')
        self.assertEqual(result['head_oid'], 'a' * 40)
        self.assertEqual(self.review_calls, [('GET', '/repos/acme/project/pulls/7', None),
            ('GET', '/repos/acme/project/pulls/7', None), ('POST', '/repos/acme/project/pulls/7/reviews',
                                                       {'commit_id': 'a' * 40, 'event': 'APPROVE', 'body': ''})])
        self.assertEqual(self.purposes, ['admission', 'write'])

    def test_queued_branch_update_remains_journaled_and_cannot_be_replayed(self):
        self.enable_review_fixture()
        registry, _ = self.access.load_registry()
        registry['runtime_credentials'][1]['permissions']['contents'] = 'write'
        path = self.access.registry_path()
        path.write_text(json.dumps(registry))
        self.binding['registry_digest'] = hashlib.sha256(path.read_bytes()).hexdigest()
        self.write_binding()
        existing_upstream = self.access.github_request.side_effect

        def upstream(method, path, token, deadline, body=None):
            if method != 'PUT':
                return existing_upstream(method, path, token, deadline, body)
            self.review_calls.append((method, path, body))
            return 202, None, {'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '5000',
                'x-ratelimit-remaining': '4900', 'x-ratelimit-reset': str(int(time.time()) + 3600)}

        self.start()
        body = json.dumps({'number': 7, 'expected_head_sha': 'a' * 40}).encode()
        with patch.object(self.access, 'github_request', side_effect=upstream):
            response, payload = self.exchange(self.header(action='pull_request.update_branch', body_length=len(body)), body=body)
            self.assertEqual(response['status'], 200, response)
            self.assertEqual(json.loads(payload)['outcome'], 'queued')
            response, _ = self.exchange(self.header(action='pull_request.update_branch', body_length=len(body)), body=body)
        self.assertEqual(response['code'], 'action_reconciliation_required')
        self.assertEqual([entry[0] for entry in self.review_calls], ['GET', 'GET', 'PUT'])
        records = list((self.home / '.local/state/greywall-agents/github-actions').iterdir())
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0].suffix, '.pending')
        journal = json.loads(records[0].read_text())
        self.assertEqual(journal['target_number'], 7)
        self.assertEqual(journal['head_oid'], 'a' * 40)

    def test_graphql_write_requires_fresh_action_bound_primary_cost_evidence(self):
        self.enable_review_fixture()
        self.start()
        body = b'{"number":7}'
        response, _ = self.exchange(self.header(action='pull_request.ready', body_length=len(body)), body=body)
        self.assertEqual(response.get('code'), 'graphql_rate_setup_required')
        self.assertEqual([entry[0] for entry in self.review_calls], ['GET'])

    def test_ready_funds_measured_graphql_bucket_not_rest_or_inspector(self):
        self.enable_review_fixture()
        registry, _ = self.access.load_registry()
        now = int(time.time())
        registry['runtime_credentials'][1]['graphql_accounting'] = {'pull_request.ready': {
            'setup_approved': True, 'principal': 'user:88', 'max_cost': 2,
            'reviewed_at': now - 60, 'valid_until': now + 1800, 'evidence': 'd' * 64}}
        path = self.access.registry_path()
        path.write_text(json.dumps(registry))
        self.binding['registry_digest'] = hashlib.sha256(path.read_bytes()).hexdigest()
        self.write_binding()
        self.rate.observe('user:88', {'x-ratelimit-resource': 'graphql', 'x-ratelimit-limit': '5000',
            'x-ratelimit-remaining': '5000', 'x-ratelimit-reset': str(now + 3600)})
        existing_upstream = self.access.github_request.side_effect

        def upstream(method, path, token, deadline, body=None):
            if path != '/graphql':
                return existing_upstream(method, path, token, deadline, body)
            self.review_calls.append((method, path, body))
            return 200, {'data': {'markPullRequestReadyForReview': {
                'pullRequest': {'id': 'PR_fixture_207', 'isDraft': False}}}}, {
                    'x-ratelimit-resource': 'graphql', 'x-ratelimit-limit': '5000',
                    'x-ratelimit-remaining': '4998', 'x-ratelimit-reset': str(now + 3600)}

        self.start()
        with patch.object(self.access, 'github_request', side_effect=upstream):
            body = b'{"number":7}'
            response, payload = self.exchange(self.header(action='pull_request.ready', body_length=len(body)), body=body)
        self.assertEqual(response['status'], 200, response)
        self.assertEqual(json.loads(payload)['applied_fields'], {'is_draft': False})
        self.assertEqual(self.purposes, ['admission', 'write'])
        self.assertEqual(self.review_calls[-1][1], '/graphql')
        with self.rate.locked_state() as state:
            self.assertEqual(state['buckets']['user:88:graphql']['server_remaining'], 4998)
            self.assertEqual(state['reservations'], {})

    def enable_fixture_forwarding(self):
        self.assertTrue(hasattr(self.broker, 'git_authority'), 'per-request Git authority lookup is missing')
        self.rate = self.access.load_rate()
        self.rate.observe('user:88', {'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '5000',
                                     'x-ratelimit-remaining': '5000',
                                     'x-ratelimit-reset': str(int(time.time()) + 3600)})
        self.purposes = []
        self.upstream = []
        self.write_report = self.report.copy()

        def inspect(checkout, *, purpose, operation_costs=None, unmetered_operation=False):
            self.purposes.append(purpose)
            report = (self.write_report if purpose == 'write' else self.report).copy()
            if purpose == 'write' and report['mode'] == 'verified':
                report['rate_reservation'] = self.rate.reserve(operation_costs or {'user:88:core': 7}, purpose='write', repository_id=42,
                                                             deadline=time.monotonic() + 30)['reservation']
                report['rate_operation_costs'] = operation_costs
            return report

        def transport(header, binding, token, body, output, *, deadline, check_live):
            check_live()
            self.upstream.append((header['action'], binding['canonical_name'], body.read()))
            output.write(b'0000')
            output.seek(0)
            return {'status': 200, 'content_type': self.broker.git_route(header, binding['canonical_name'])['response_type'],
                    'body_length': 4, 'rate_headers': {}}

        patches = [patch.object(self.broker, 'git_authority', return_value=(
                       {'repository_id': 42, 'canonical_name': 'acme/project'},
                       {'id': 'fixture', 'kind': 'fine_grained_pat', 'actor_id': 88}, {'user:88:core': 1})),
                   patch.object(self.access, 'inspect_access', side_effect=inspect),
                   patch.object(self.access, 'read_runtime_credential', return_value='SYNTHETIC_FORWARD_SECRET'),
                   patch.object(self.broker, 'run_git_transport', side_effect=transport)]
        for active in patches:
            active.start()
            self.addCleanup(active.stop)

    def push_body(self, ref=b'refs/heads/feature/nested'):
        payload = b'0' * 40 + b' ' + b'1' * 40 + b' ' + ref
        return f'{len(payload) + 4:04x}'.encode() + payload + b'0000PACK'

    def test_each_push_rechecks_even_inside_session_lease(self):
        self.enable_fixture_forwarding()
        self.start()
        body = self.push_body()
        for attempt in range(2):
            if attempt:
                time.sleep(1.01)
            response, payload = self.exchange(self.header(action='git.push', body_length=len(body)), body=body)
            self.assertEqual(response['status'], 200)
            self.assertEqual(payload, b'0000')
        self.assertEqual(self.purposes, ['admission', 'write', 'write'])
        self.assertEqual(self.upstream, [('git.push', 'acme/project', body)] * 2)

    def test_failed_check_forwards_no_write(self):
        self.enable_fixture_forwarding()
        self.start()
        self.write_report['mode'] = 'read_only'
        body = self.push_body()
        response, _ = self.exchange(self.header(action='git.push', body_length=len(body)), body=body)
        self.assertEqual(response['code'], 'write_check_denied')
        self.assertEqual(self.upstream, [])

    def test_reviewed_no_primary_cost_still_checks_and_paces_each_write(self):
        self.enable_fixture_forwarding()
        self.start()
        body = self.push_body()
        with patch.object(self.broker, 'git_authority', return_value=(
                {'repository_id': 42, 'canonical_name': 'acme/project'},
                {'kind': 'fine_grained_pat', 'actor_id': 88}, {})):
            response, _ = self.exchange(self.header(action='git.push', body_length=len(body)), body=body)
            self.assertEqual(response['status'], 200)
            response, _ = self.exchange(self.header(action='git.push', body_length=len(body)), body=body)
            self.assertEqual(response['code'], 'mutation_paced')
        self.assertEqual(self.purposes, ['admission', 'write', 'write'])
        self.assertEqual(len(self.upstream), 1)

    def test_current_default_is_rechecked_after_upload(self):
        self.enable_fixture_forwarding()
        self.start()
        self.write_report['default_branch'] = 'feature/nested'
        body = self.push_body()
        response, _ = self.exchange(self.header(action='git.push', body_length=len(body)), body=body)
        self.assertEqual(response['code'], 'push_ref_denied')
        self.assertEqual(self.upstream, [])

    def test_large_slow_spool_is_checked_after_upload(self):
        self.enable_fixture_forwarding()
        self.start()
        body = self.push_body() + b'x' * (1024 * 1024)
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(2)
            client.connect(str(self.socket_path))
            framed = self.frame(self.header(action='git.push', body_length=len(body)), body)
            client.sendall(framed[:-1])
            self.assertEqual(self.purposes, ['admission'])
            self.write_report['mode'] = 'read_only'
            client.sendall(framed[-1:])
            client.shutdown(socket.SHUT_WR)
            length = struct.unpack('!I', self.broker.read_exact(client, 4, time.monotonic() + 2))[0]
            response = json.loads(self.broker.read_exact(client, length, time.monotonic() + 2))
            self.assertEqual(response['code'], 'write_check_denied')
        self.assertEqual(self.upstream, [])

    def test_uncertain_push_is_not_retried_or_reported_as_cancelled(self):
        self.enable_fixture_forwarding()
        self.start()
        body = self.push_body()

        def uncertain(*arguments, **keywords):
            self.upstream.append('possibly-applied')
            raise self.broker.RequestDenied('upstream_timeout')

        with patch.object(self.broker, 'run_git_transport', side_effect=uncertain):
            response, _ = self.exchange(self.header(action='git.push', body_length=len(body)), body=body)
        self.assertEqual(response['code'], 'upstream_outcome_unknown')
        self.assertEqual(self.upstream, ['possibly-applied'])

    def test_fetch_uses_a_fresh_read_admission_without_mutation_pacing(self):
        self.enable_fixture_forwarding()
        self.start()
        for _ in range(2):
            response, _ = self.exchange(self.header())
            self.assertEqual(response['status'], 200)
        self.assertEqual(self.purposes, ['admission', 'admission', 'admission'])
        with self.rate.locked_state() as state:
            self.assertEqual(state.get('mutations', []), [])

    def test_empty_rpc_probe_does_not_consume_content_mutation_pacing(self):
        self.enable_fixture_forwarding()
        self.start()
        response, _ = self.exchange(self.header(action='git.push', body_length=4), body=b'0000')
        self.assertEqual(response['status'], 200)
        body = self.push_body()
        response, _ = self.exchange(self.header(action='git.push', body_length=len(body)), body=body)
        self.assertEqual(response['status'], 200)
        self.assertEqual(self.purposes, ['admission', 'write', 'write'])
        with self.rate.locked_state() as state:
            self.assertEqual(len(state['mutations']), 1)

    def test_revoked_generation_stops_accepting_requests(self):
        _, worker, failures = self.start()
        self.binding['revoked'] = True
        self.write_binding()
        worker.join(2)
        self.assertFalse(worker.is_alive())
        self.assertFalse(self.socket_path.exists())
        self.assertEqual(failures, [])

    def test_supervisor_renewal_of_check_times_keeps_the_channel_open(self):
        _, worker, failures = self.start()
        time.sleep(0.2)
        self.binding.update(checked_at=time.monotonic(), checked_wall=time.time())
        self.write_binding()
        time.sleep(0.5)
        self.assertTrue(worker.is_alive(), failures)
        self.assertEqual(self.exchange(self.header())[0]['code'], 'action_unavailable')

    def test_renewal_cannot_change_identity_capability_or_move_time_backwards(self):
        for field, value in [('capability', 'b' * 64), ('generation', '4' * 32), ('repository_id', 99),
                             ('checked_at', -1.0), ('checked_wall', -1.0)]:
            with self.subTest(field=field):
                self.binding.update(generation=os.urandom(16).hex(), capability=os.urandom(32).hex())
                self.write_binding()
                _, worker, _ = self.start()
                changed = dict(self.binding)
                changed[field] = changed[field] + value if field.startswith('checked') else value
                self.decision.write_text(json.dumps(changed))
                self.decision.chmod(0o600)
                worker.join(2)
                self.assertFalse(worker.is_alive(), field)

    def test_unrenewed_session_still_expires_at_the_hard_limit(self):
        self.binding['checked_at'] -= 599.5
        self.binding['checked_wall'] -= 599.5
        self.write_binding()
        _, worker, _ = self.start()
        worker.join(3)
        self.assertFalse(worker.is_alive())

    def test_expired_admission_never_publishes_a_socket(self):
        self.binding['checked_at'] -= 601
        self.write_binding()
        with self.assertRaisesRegex(self.broker.RequestDenied, '^admission_expired$'):
            self.broker.serve(self.decision, self.socket_path)
        self.assertFalse(self.socket_path.exists())

    def test_fresh_inspection_must_match_the_host_binding(self):
        for changes in [{'mode': 'read_only'}, {'repository_id': 99}, {'rate_verified': False},
                        {'common_dir': str(self.root / 'sibling/.git')}, {'credential_roles_verified': []}]:
            with self.subTest(changes=changes):
                original = self.report.copy()
                self.report.update(changes)
                with self.assertRaisesRegex(self.broker.RequestDenied, '^admission_denied$'):
                    self.broker.serve(self.decision, self.socket_path)
                self.report = original
                self.assertFalse(self.socket_path.exists())

    def test_sibling_socket_is_not_published(self):
        self.start()
        channel = self.socket_path.parent
        self.assertEqual(sorted(path.name for path in channel.iterdir()), ['broker.sock'])
        self.assertEqual(channel.stat().st_mode & 0o777, 0o700)
        self.assertEqual(self.socket_path.stat().st_mode & 0o777, 0o600)
        self.assertNotIn(self.control, channel.parents)
        self.assertNotIn(self.decision, channel.iterdir())
        other = self.runtime / 'greywall-git-channel' / ('9' * 32) / 'broker.sock'
        with self.assertRaisesRegex(self.broker.RequestDenied, '^channel_path_denied$'):
            self.broker.serve(self.decision, other)
        self.assertFalse(other.exists())

    def test_restart_never_reuses_an_old_generation_or_capability(self):
        stop, worker, _ = self.start()
        stop.set()
        worker.join(2)
        with self.assertRaisesRegex(self.broker.RequestDenied, '^generation_consumed$'):
            self.broker.serve(self.decision, self.socket_path)
        self.binding['generation'] = '4' * 32
        self.write_binding()
        with self.assertRaisesRegex(self.broker.RequestDenied, '^generation_consumed$'):
            self.broker.serve(self.decision, self.socket_path)
        self.binding.update(generation='5' * 32, capability='c' * 64,
                            checked_at=time.monotonic(), checked_wall=time.time())
        self.write_binding()
        self.start()
        self.assertEqual(self.exchange(self.header(capability='a' * 64))[0]['code'], 'capability_denied')
        self.assertEqual(self.exchange(self.header(capability='c' * 64))[0]['code'], 'action_unavailable')

    def test_replacing_a_socket_does_not_make_cleanup_unlink_the_replacement(self):
        stop, worker, _ = self.start()
        self.socket_path.unlink()
        with socket.socket(socket.AF_UNIX) as replacement:
            replacement.bind(str(self.socket_path))
            stop.set()
            worker.join(2)
            self.assertTrue(self.socket_path.exists())

    def test_registry_change_revokes_the_channel_without_renewing_it(self):
        _, worker, failures = self.start()
        registry = self.access.registry_path()
        registry.write_text(json.dumps({'version': 1, 'repositories': [], 'inspector_apps': [], 'epoch': 2}))
        worker.join(2)
        self.assertFalse(worker.is_alive())
        self.assertFalse(self.socket_path.exists())
        self.assertEqual(failures, [])

    def test_untrusted_control_permissions_never_publish_a_channel(self):
        self.decision.chmod(0o644)
        with self.assertRaisesRegex(self.broker.RequestDenied, '^untrusted_control_path$'):
            self.broker.serve(self.decision, self.socket_path)
        self.assertFalse(self.socket_path.exists())

    def test_cross_session_capabilities_cannot_borrow_a_sibling_channel(self):
        self.start()
        first_socket = self.socket_path
        self.binding = self.binding | {'session': '9' * 32, 'generation': '8' * 32, 'capability': 'b' * 64}
        self.decision = self.control / (self.binding['session'] + '.json')
        self.socket_path = self.runtime / 'greywall-git-channel' / self.binding['session'] / 'broker.sock'
        self.write_binding()
        self.start()
        self.assertEqual(self.exchange(self.header(capability='a' * 64))[0]['code'], 'capability_denied')
        self.assertEqual(self.exchange(self.header(capability='b' * 64))[0]['code'], 'action_unavailable')
        self.socket_path = first_socket
        self.assertEqual(self.exchange(self.header(capability='b' * 64))[0]['code'], 'capability_denied')

    def test_direct_socket_rejects_duplicate_oversized_truncated_and_trailing_frames(self):
        self.start()
        duplicate = b'{"version":1,"version":1,"capability":"' + b'a' * 64 + b'","action":"git.fetch","body_length":0}'
        for header, body, code in [(duplicate, b'', 'invalid_header'),
                                   (b' ' * (16 * 1024 + 1), b'', 'invalid_frame'),
                                   (self.header(body_length=2), b'x', 'truncated_frame'),
                                   (self.header(), b'x', 'trailing_data')]:
            with self.subTest(code=code):
                self.assertEqual(self.exchange(header, body=body)[0]['code'], code)

    def test_wall_rollback_future_evidence_and_old_boot_never_start_a_channel(self):
        original = self.binding.copy()
        for changes in [{'checked_wall': time.time() + 60}, {'checked_at': time.monotonic() + 60},
                        {'boot_id': 'a-different-boot'}]:
            with self.subTest(changes=changes):
                self.binding = original | changes
                self.write_binding()
                with self.assertRaisesRegex(self.broker.RequestDenied, '^admission_expired$'):
                    self.broker.serve(self.decision, self.socket_path)
                self.assertFalse(self.socket_path.exists())

    def test_symlinked_channel_parent_is_rejected_without_outside_writes(self):
        outside = self.root / 'outside'
        outside.mkdir(mode=0o700)
        (self.runtime / 'greywall-git-channel').symlink_to(outside)
        with self.assertRaisesRegex(self.broker.RequestDenied, '^untrusted_control_path$'):
            self.broker.serve(self.decision, self.socket_path)
        self.assertEqual(list(outside.iterdir()), [])

    def test_inspector_failure_cannot_put_a_secret_in_cli_errors(self):
        with patch.object(self.access, 'inspect_access', side_effect=RuntimeError('SYNTHETIC_INSPECTOR_SECRET')), \
                patch.object(self.broker, 'protect_parent'), \
                patch.object(sys, 'argv', [str(BROKER), 'serve', '--decision', str(self.decision),
                                           '--socket', str(self.socket_path)]), \
                contextlib.redirect_stderr(io.StringIO()) as error_output, \
                contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(self.broker.main(), 77)
        self.assertEqual(output.getvalue(), '')
        self.assertNotIn('SYNTHETIC_INSPECTOR_SECRET', error_output.getvalue())
        self.assertFalse(self.socket_path.exists())

    def test_revocation_is_not_a_successful_cli_exit(self):
        serve = self.broker.serve
        ready = threading.Event()
        results = []

        def signaled_serve(*arguments, **keywords):
            keywords['ready'] = ready.set
            return serve(*arguments, **keywords)

        with patch.object(self.broker, 'serve', side_effect=signaled_serve), \
                patch.object(self.broker, 'protect_parent'), \
                patch.object(sys, 'argv', [str(BROKER), 'serve', '--decision', str(self.decision),
                                           '--socket', str(self.socket_path)]), \
                contextlib.redirect_stderr(io.StringIO()):
            worker = threading.Thread(target=lambda: results.append(self.broker.main()))
            worker.start()
            try:
                self.assertTrue(ready.wait(2), 'CLI broker was not ready')
                self.binding['revoked'] = True
                self.write_binding()
            finally:
                worker.join(3)
            self.assertFalse(worker.is_alive())
        self.assertEqual(results, [77])

    def test_a_third_active_request_is_refused_instead_of_queued(self):
        barrier = threading.Barrier(3)
        read_request = self.broker.read_request

        def tracked_request(*arguments, **keywords):
            barrier.wait(2)
            return read_request(*arguments, **keywords)

        with patch.object(self.broker, 'read_request', side_effect=tracked_request):
            self.start()
            with socket.socket(socket.AF_UNIX) as first, socket.socket(socket.AF_UNIX) as second:
                for client in (first, second):
                    client.connect(str(self.socket_path))
                    client.sendall(self.frame(self.header(body_length=1)))
                barrier.wait(2)
                response, _ = self.exchange(self.header())
                self.assertEqual(response['code'], 'session_busy')
                self.assertEqual(response['status'], 429)

    def test_existing_socket_path_is_not_unlinked_or_overwritten(self):
        self.socket_path.parent.parent.mkdir(mode=0o700)
        self.socket_path.parent.mkdir(mode=0o700)
        self.socket_path.write_bytes(b'not this broker socket')
        with self.assertRaises(OSError):
            self.broker.serve(self.decision, self.socket_path)
        self.assertEqual(self.socket_path.read_bytes(), b'not this broker socket')

    def test_parent_death_ends_the_cli_broker_and_leaves_no_live_channel(self):
        runner = self.root / 'broker-fixture.py'
        runner.write_text(
            'import importlib.util, sys\n'
            f'spec = importlib.util.spec_from_file_location("fixture_broker", {str(BROKER)!r})\n'
            'broker = importlib.util.module_from_spec(spec)\nspec.loader.exec_module(broker)\n'
            'access = broker.load_access()\n'
            f'access.inspect_access = lambda *args, **kwargs: {self.report!r}\n'
            'broker.load_access = lambda: access\n'
            f'sys.argv = [{str(BROKER)!r}, "serve", "--decision", {str(self.decision)!r}, "--socket", {str(self.socket_path)!r}]\n'
            'raise SystemExit(broker.main())\n')
        supervisor_code = (
            'import json, subprocess, sys\n'
            'child = subprocess.Popen([sys.executable, "-I", sys.argv[1]], stdout=subprocess.PIPE, stderr=subprocess.PIPE)\n'
            'ready = child.stdout.readline()\n'
            'if not ready: raise SystemExit("broker fixture failed to start")\n'
            'print(json.dumps({"pid": child.pid, "ready": json.loads(ready)}), flush=True)\n'
            'sys.stdin.buffer.read(1)\n')
        supervisor = subprocess.Popen([sys.executable, '-I', '-c', supervisor_code, str(runner)],
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: supervisor.kill() if supervisor.poll() is None else None)
        self.assertTrue(select.select([supervisor.stdout], [], [], 3)[0], 'fixture readiness timed out')
        readiness = json.loads(supervisor.stdout.readline())
        descriptor = os.pidfd_open(readiness['pid'])
        try:
            supervisor.stdin.close()
            self.assertEqual(supervisor.wait(timeout=3), 0)
            self.assertTrue(select.select([descriptor], [], [], 3)[0], 'broker survived its supervisor')
            with socket.socket(socket.AF_UNIX) as client, self.assertRaises(OSError):
                client.connect(str(self.socket_path))
        finally:
            os.close(descriptor)
            supervisor.stdout.close()
            supervisor.stderr.close()


if __name__ == '__main__':
    unittest.main()
