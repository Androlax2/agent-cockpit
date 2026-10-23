import importlib.util
import io
import json
import os
import select
from pathlib import Path
import socketserver
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
import urllib.error


TRANSPORT = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents/git-transport.py'


class GitTransportTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(TRANSPORT.exists(), 'bounded Git transport worker is missing')
        spec = importlib.util.spec_from_file_location('test_git_transport', TRANSPORT)
        self.transport = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.transport)
        self.control = {'header': {'version': 1, 'capability': 'a' * 64, 'action': 'git.fetch',
                                   'body_length': 4, 'protocol': 2},
                        'canonical_name': 'acme/project', 'token': 'SYNTHETIC_FORWARD_SECRET'}

    def test_fixed_https_request_has_only_host_constructed_authentication(self):
        response = io.BytesIO(b'0000')
        response.status = 200
        response.headers = {'Content-Type': 'application/x-git-upload-pack-result'}
        sent = []

        class Opener:
            def open(self, request, timeout):
                sent.append(request)
                return response

        output = io.BytesIO()
        with patch.object(self.transport.urllib.request, 'build_opener', return_value=Opener()):
            result = self.transport.transfer(self.control, io.BytesIO(b'0000'), output)
        self.assertEqual(sent[0].full_url, 'https://github.com/acme/project.git/git-upload-pack')
        self.assertEqual(sent[0].get_header('Git-protocol'), 'version=2')
        self.assertTrue(sent[0].get_header('Authorization').startswith('Basic '))
        self.assertEqual(output.getvalue(), b'0000')
        self.assertNotIn('SYNTHETIC', json.dumps(result))

    def test_redirect_handler_refuses_to_build_an_authenticated_followup(self):
        handler = self.transport.NoRedirect()
        request = self.transport.urllib.request.Request('https://github.com/acme/project.git/git-upload-pack')
        self.assertIsNone(handler.redirect_request(request, None, 302, 'redirect', {}, 'https://evil.invalid'))

    def test_upstream_errors_and_wrong_content_types_never_expose_the_body(self):
        for status, content_type in [(302, 'text/html'), (403, 'application/json'), (200, 'text/html')]:
            response = io.BytesIO(b'SYNTHETIC_FORWARD_SECRET')
            response.status = status
            response.headers = {'Content-Type': content_type}

            class Opener:
                def open(self, request, timeout):
                    return response

            output = io.BytesIO()
            with self.subTest(status=status), \
                    patch.object(self.transport.urllib.request, 'build_opener', return_value=Opener()), \
                    self.assertRaises(self.transport.TransportDenied) as failure:
                self.transport.transfer(self.control, io.BytesIO(b'0000'), output)
            self.assertEqual(output.getvalue(), b'')
            self.assertNotIn('SYNTHETIC', str(failure.exception))

    def test_unknown_control_fields_never_open_a_connection(self):
        with patch.object(self.transport.urllib.request, 'build_opener') as opener:
            with self.assertRaises(self.transport.TransportDenied):
                self.transport.transfer(self.control | {'url': 'https://evil.invalid'}, io.BytesIO(), io.BytesIO())
            opener.assert_not_called()

    def test_truncated_success_body_is_not_reported_as_a_complete_response(self):
        response = io.BytesIO(b'partial')
        response.status = 200
        response.headers = {'Content-Type': 'application/x-git-upload-pack-result', 'Content-Length': '100'}

        class Opener:
            def open(self, request, timeout):
                return response

        with patch.object(self.transport.urllib.request, 'build_opener', return_value=Opener()), \
                self.assertRaisesRegex(self.transport.TransportDenied, '^upstream_response_denied$'):
            self.transport.transfer(self.control, io.BytesIO(b'0000'), io.BytesIO())


class GitTransportProcessTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix='git-https-', dir='/tmp/opencode')
        self.addCleanup(self.scratch.cleanup)
        root = Path(self.scratch.name)
        certificate, key = root / 'ca.pem', root / 'key.pem'
        generated = subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes',
                                    '-keyout', str(key), '-out', str(certificate), '-days', '1',
                                    '-subj', '/CN=github.com', '-addext', 'subjectAltName=DNS:github.com'],
                                   capture_output=True)
        self.assertEqual(generated.returncode, 0, generated.stderr.decode())
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certificate, key)
        self.requests = []
        self.response_status = 200
        self.received = threading.Event()
        self.release = threading.Event()
        self.release.set()
        fixture = self

        class Proxy(socketserver.StreamRequestHandler):
            def handle(self):
                try:
                    self.connection.settimeout(5)
                    connect = self.rfile.readline()
                    if connect not in (b'CONNECT github.com:443 HTTP/1.0\r\n', b'CONNECT github.com:443 HTTP/1.1\r\n'):
                        return
                    while self.rfile.readline() != b'\r\n':
                        pass
                    self.wfile.write(b'HTTP/1.0 200 Connection established\r\n\r\n')
                    self.wfile.flush()
                    with context.wrap_socket(self.connection, server_side=True) as encrypted:
                        with encrypted.makefile('rb') as source:
                            request = source.readline().decode().strip()
                            headers = {}
                            while line := source.readline():
                                if line == b'\r\n':
                                    break
                                name, value = line.decode().split(':', 1)
                                headers[name.lower()] = value.strip()
                            body = source.read(int(headers.get('content-length', '0')))
                            fixture.requests.append((request, headers, body))
                            fixture.received.set()
                            fixture.release.wait(5)
                            status = fixture.response_status
                            content = b'0000' if status == 200 else b'SYNTHETIC_FORWARD_SECRET'
                            encrypted.sendall(f'HTTP/1.1 {status} Response\r\nContent-Type: application/x-git-upload-pack-result\r\nContent-Length: {len(content)}\r\nRetry-After: 60\r\nLocation: https://evil.invalid/steal\r\nConnection: close\r\n\r\n'.encode() + content)
                except (OSError, ValueError):
                    return

        server = socketserver.ThreadingTCPServer(('127.0.0.1', 0), Proxy)
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever)
        thread.start()

        def close_server():
            self.release.set()
            server.shutdown()
            server.server_close()
            thread.join(3)

        self.addCleanup(close_server)
        environment = patch.dict(os.environ, {'https_proxy': f'http://127.0.0.1:{server.server_address[1]}',
                                              'HTTPS_PROXY': f'http://127.0.0.1:{server.server_address[1]}',
                                              'no_proxy': '', 'NO_PROXY': '', 'CURL_CA_BUNDLE': str(certificate),
                                              'GH_TOKEN': 'SYNTHETIC_ENV_SECRET'})
        environment.start()
        self.addCleanup(environment.stop)
        spec = importlib.util.spec_from_file_location('process_broker', TRANSPORT.with_name('git-broker.py'))
        self.broker = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.broker)
        self.header = {'version': 1, 'capability': 'a' * 64, 'action': 'git.fetch', 'body_length': 4, 'protocol': 2}

    def run_transport(self, deadline=None, check_live=lambda: None, *, read_before_dispatch=False):
        with tempfile.TemporaryFile(dir=self.scratch.name) as body, tempfile.TemporaryFile(dir=self.scratch.name) as output:
            body.write(b'0000')
            if read_before_dispatch:
                body.seek(0)
                body.read(1)
                body.seek(0)
            response = self.broker.run_git_transport(self.header, {'canonical_name': 'acme/project'},
                                                     'SYNTHETIC_FORWARD_SECRET', body, output,
                                                     deadline=deadline or time.monotonic() + 5, check_live=check_live)
            return response, output.read()

    def test_worker_sends_complete_body_after_parent_buffered_validation(self):
        response, _ = self.run_transport(read_before_dispatch=True)
        self.assertEqual(response['status'], 200)
        self.assertEqual(self.requests[0][2], b'0000')

    def test_worker_performs_verified_tls_through_host_proxy_and_fixed_route(self):
        response, content = self.run_transport()
        self.assertEqual(response['status'], 200)
        self.assertEqual(content, b'0000')
        self.assertEqual(self.requests[0][0], 'POST /acme/project.git/git-upload-pack HTTP/1.1')
        self.assertEqual(self.requests[0][2], b'0000')
        self.assertNotIn('SYNTHETIC', json.dumps(response))

    def test_real_redirect_returns_no_authenticated_followup(self):
        self.response_status = 302
        with self.assertRaises(self.broker.RequestDenied) as failure:
            self.run_transport()
        self.assertNotIn('SYNTHETIC', str(failure.exception))
        self.assertEqual(len(self.requests), 1)

    def test_revocation_kills_a_worker_waiting_for_upstream(self):
        self.release.clear()

        def check_live():
            if self.received.is_set():
                raise self.broker.RequestDenied('admission_revoked')

        start = time.monotonic()
        with self.assertRaisesRegex(self.broker.RequestDenied, '^admission_revoked$'):
            self.run_transport(check_live=check_live)
        self.assertLess(time.monotonic() - start, 2)

    def test_total_deadline_kills_a_stalled_worker(self):
        self.release.clear()
        with self.assertRaisesRegex(self.broker.RequestDenied, '^upstream_timeout$'):
            self.run_transport(deadline=time.monotonic() + 0.5)

    def test_untrusted_tls_certificate_never_receives_authorization(self):
        with patch.dict(os.environ, {'CURL_CA_BUNDLE': '', 'SSL_CERT_FILE': ''}):
            with self.assertRaises(self.broker.RequestDenied):
                self.run_transport()
        self.assertEqual(self.requests, [])

    def test_throttle_returns_only_private_rate_metadata_and_no_upstream_body(self):
        self.response_status = 429
        response, content = self.run_transport()
        self.assertEqual(response['status'], 429)
        self.assertEqual(response['rate_headers'], {'retry-after': '60'})
        self.assertEqual(content, b'')

    def test_worker_dies_when_its_broker_parent_is_killed(self):
        self.release.clear()
        script = '''import importlib.util, os, sys, tempfile, time
spec = importlib.util.spec_from_file_location('broker', sys.argv[1])
broker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(broker)
real_popen = broker.subprocess.Popen
def announce(*args, **kwargs):
    child = real_popen(*args, **kwargs)
    print(child.pid, flush=True)
    return child
broker.subprocess.Popen = announce
with tempfile.TemporaryFile(dir=sys.argv[2]) as body, tempfile.TemporaryFile(dir=sys.argv[2]) as output:
    body.write(b'0000')
    broker.run_git_transport({'version': 1, 'capability': 'a' * 64, 'action': 'git.fetch', 'body_length': 4},
                             {'canonical_name': 'acme/project'}, 'SYNTHETIC', body, output,
                             deadline=time.monotonic() + 30, check_live=lambda: None)
'''
        parent = subprocess.Popen([sys.executable, '-I', '-c', script, str(TRANSPORT.with_name('git-broker.py')),
                                   self.scratch.name], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        try:
            self.assertTrue(select.select([parent.stdout], [], [], 3)[0])
            worker = int(parent.stdout.readline())
            self.assertTrue(self.received.wait(3))
            descriptor = os.pidfd_open(worker)
            try:
                parent.kill()
                parent.wait(timeout=3)
                self.assertTrue(select.select([descriptor], [], [], 3)[0], 'transport survived its broker')
            finally:
                os.close(descriptor)
        finally:
            if parent.poll() is None:
                parent.kill()
                parent.wait(timeout=3)
            parent.stdout.close()


if __name__ == '__main__':
    unittest.main()
