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

    def test_revoked_generation_stops_accepting_requests(self):
        _, worker, failures = self.start()
        self.binding['revoked'] = True
        self.write_binding()
        worker.join(2)
        self.assertFalse(worker.is_alive())
        self.assertFalse(self.socket_path.exists())
        self.assertEqual(failures, [])

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
