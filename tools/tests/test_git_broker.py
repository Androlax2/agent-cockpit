import importlib.util
from pathlib import Path
import json
import os
import socket
import struct
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


BROKER = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents/git-broker.py'


class BrokerFramingTests(unittest.TestCase):
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


if __name__ == '__main__':
    unittest.main()
