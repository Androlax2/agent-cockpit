import importlib.util
import io
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import patch


ADAPTER = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents/git-adapter.py'


class GitAdapterTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(ADAPTER.exists(), 'native Git adapter is missing')
        spec = importlib.util.spec_from_file_location('test_git_adapter', ADAPTER)
        self.adapter = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.adapter)
        self.scratch = tempfile.TemporaryDirectory(prefix='git-adapter-', dir='/tmp/opencode')
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.path = self.root / 'broker.sock'
        self.capability = self.root / 'capability'
        self.capability.write_text('a' * 64)
        self.capability.chmod(0o600)
        self.received = []
        self.listener = socket.socket(socket.AF_UNIX)
        self.listener.bind(str(self.path))
        self.listener.listen()
        self.listener.settimeout(0.1)
        self.stop = threading.Event()
        self.handler = lambda header, body: (200, 'application/x-git-upload-pack-result', b'0000')
        self.failures = []

        def receive_exact(client, count):
            value = b''
            while len(value) < count:
                chunk = client.recv(count - len(value))
                if not chunk:
                    raise ValueError('truncated adapter frame')
                value += chunk
            return value

        def broker():
            while not self.stop.is_set():
                try:
                    client, _ = self.listener.accept()
                except TimeoutError:
                    continue
                except OSError:
                    return
                try:
                    with client:
                        client.settimeout(3)
                        length = struct.unpack('!I', receive_exact(client, 4))[0]
                        header = json.loads(receive_exact(client, length))
                        body = receive_exact(client, header['body_length'])
                        self.assertEqual(client.recv(1), b'')
                        self.received.append((header, body))
                        status, content_type, response = self.handler(header, body)
                        framed = json.dumps({'status': status, 'content_type': content_type, 'body_length': len(response)}).encode()
                        client.sendall(struct.pack('!I', len(framed)) + framed + response)
                except Exception as error:
                    self.failures.append(error)

        worker = threading.Thread(target=broker)
        worker.start()
        self.server = self.adapter.make_server(self.path, self.capability)
        http_worker = threading.Thread(target=self.server.serve_forever)
        http_worker.start()
        self.url = f'http://127.0.0.1:{self.server.server_port}/repo.git'

        def cleanup():
            self.stop.set()
            self.server.shutdown()
            self.server.server_close()
            self.listener.close()
            worker.join(3)
            http_worker.join(3)
            self.assertEqual(self.failures, [])

        self.addCleanup(cleanup)

    def request(self, encoded):
        with socket.create_connection(self.server.server_address, timeout=3) as client:
            client.sendall(encoded)
            client.shutdown(socket.SHUT_WR)
            response = b''
            while chunk := client.recv(65536):
                response += chunk
            return response

    def test_chunked_push_is_decoded_into_one_bounded_frame(self):
        response = self.request(b'POST /repo.git/git-receive-pack HTTP/1.1\r\nHost: localhost\r\n'
                                b'Transfer-Encoding: chunked\r\nContent-Type: application/x-git-receive-pack-request\r\n\r\n'
                                b'3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n')
        self.assertIn(b'200', response.split(b'\r\n')[0])
        self.assertEqual(self.received[0][0]['action'], 'git.push')
        self.assertEqual(self.received[0][1], b'abcde')

    def test_https_and_ssh_spelling_map_only_enrolled_repository(self):
        self.assertTrue(hasattr(self.adapter, 'git_overlay'), 'trusted Git overlay is missing')
        environment = self.adapter.git_overlay({'PATH': os.environ['PATH'], 'GIT_CONFIG_COUNT': '99',
                                                 'GIT_CONFIG_KEY_98': 'credential.helper', 'GIT_CONFIG_VALUE_98': 'unsafe'},
                                                'acme/project', self.url)
        for spelling in ['https://github.com/acme/project', 'https://github.com/acme/project.git',
                         'ssh://git@github.com/acme/project', 'ssh://git@github.com/acme/project.git',
                         'git@github.com:acme/project', 'git@github.com:acme/project.git']:
            with self.subTest(spelling=spelling):
                result = subprocess.run(['git', 'ls-remote', '--get-url', spelling], env=environment,
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), self.url)
        self.assertNotIn('GIT_CONFIG_KEY_98', environment)
        result = subprocess.run(['git', 'ls-remote', '--get-url', 'https://github.com/another/repo.git'],
                                env=environment, capture_output=True, text=True)
        self.assertEqual(result.stdout.strip(), 'https://github.com/another/repo.git')

    def test_local_mirror_is_not_rewritten_into_a_writable_upstream(self):
        self.assertTrue(hasattr(self.adapter, 'git_overlay'), 'trusted Git overlay is missing')
        environment = self.adapter.git_overlay(os.environ, 'acme/project', self.url)
        mirror = str(self.root / 'host-mirror.git')
        result = subprocess.run(['git', 'ls-remote', '--get-url', mirror], env=environment,
                                capture_output=True, text=True)
        self.assertEqual(result.stdout.strip(), mirror)

    def test_changed_overlay_cannot_select_another_host_repository(self):
        response = self.request(b'GET /another/repo.git/info/refs?service=git-upload-pack HTTP/1.1\r\n\r\n')
        self.assertIn(b'502', response.split(b'\r\n')[0])
        self.assertEqual(self.received, [])

    def test_total_deadline_interrupts_an_incomplete_http_body(self):
        with patch.object(self.adapter, 'TRANSFER_SECONDS', 0.1, create=True), \
                socket.create_connection(self.server.server_address, timeout=0.5) as client:
            client.sendall(b'POST /repo.git/git-upload-pack HTTP/1.1\r\n'
                           b'Content-Type: application/x-git-upload-pack-request\r\nContent-Length: 100\r\n\r\nx')
            try:
                response = client.recv(1024)
            except ConnectionResetError:
                response = b''
            self.assertNotIn(b' 200 ', response)
        self.assertEqual(self.received, [])

    def test_ambiguous_encoded_and_unsupported_http_never_reaches_broker(self):
        cases = [b'GET /repo.git/%69nfo/refs?service=git-upload-pack HTTP/1.1\r\n\r\n',
                 b'GET /repo.git/info/refs?service=git-upload-pack&other=1 HTTP/1.1\r\n\r\n',
                 b'POST /repo.git/git-receive-pack HTTP/1.1\r\nContent-Type: application/x-git-receive-pack-request\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\nx',
                 b'POST /repo.git/git-upload-pack HTTP/1.1\r\nContent-Type: application/x-git-upload-pack-request\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx',
                 b'POST /repo.git/git-upload-pack HTTP/1.1\r\nContent-Type: application/x-git-upload-pack-request\r\nContent-Length: 1\r\nContent-Encoding: gzip\r\n\r\nx',
                 b'GET /repo.git/info/refs?service=git-upload-pack HTTP/1.1\r\nGit-Protocol: version=99\r\n\r\n']
        for request in cases:
            with self.subTest(request=request):
                response = self.request(request)
                self.assertNotIn(b' 200 ', response.split(b'\r\n')[0])
        self.assertEqual(self.received, [])

    def git(self, *arguments, cwd=None, check=True):
        result = subprocess.run(['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                                 *arguments], cwd=cwd, capture_output=True, env=os.environ | {
                                     'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1',
                                     'GIT_TERMINAL_PROMPT': '0'})
        if check:
            self.assertEqual(result.returncode, 0, result.stderr.decode())
        return result

    def test_native_fetch_and_feature_push_need_no_token(self):
        upstream = self.root / 'upstream.git'
        checkout = self.root / 'checkout'
        self.git('init', '--bare', '--initial-branch=main', str(upstream))
        self.git('init', '--initial-branch=main', str(checkout))
        (checkout / 'file').write_text('first\n')
        self.git('add', 'file', cwd=checkout)
        self.git('commit', '-m', 'fixture', cwd=checkout)
        self.git('push', str(upstream), 'main', cwd=checkout)
        self.git('--git-dir', str(upstream), 'config', 'receive.denyNonFastForwards', 'true')
        broker = self.adapter.load_broker()

        def smart_http(header, body):
            if header['action'] == 'git.push':
                broker.validate_push(io.BytesIO(body), default_branch='main')
            service = header.get('service') or ('receive-pack' if header['action'] == 'git.push' else 'upload-pack')
            arguments = ['git', service, '--stateless-rpc']
            if header['action'] == 'git.advertise':
                arguments.append('--advertise-refs')
            arguments.append(str(upstream))
            environment = os.environ.copy()
            if 'protocol' in header:
                environment['GIT_PROTOCOL'] = 'version=' + str(header['protocol'])
            result = subprocess.run(arguments, input=body, capture_output=True, env=environment)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            response = result.stdout
            suffix = 'result'
            if header['action'] == 'git.advertise':
                prefix = ('# service=git-' + service + '\n').encode()
                response = f'{len(prefix) + 4:04x}'.encode() + prefix + b'0000' + response
                suffix = 'advertisement'
            return 200, 'application/x-git-' + service + '-' + suffix, response

        self.handler = smart_http
        clone = self.root / 'clone'
        self.git('clone', self.url, str(clone))
        (clone / 'file').write_text('second\n')
        self.git('add', 'file', cwd=clone)
        self.git('commit', '-m', 'feature', cwd=clone)
        self.git('-c', 'http.postBuffer=1', 'push', 'origin', 'HEAD:refs/heads/feature/nested', cwd=clone)
        self.assertEqual(self.git('rev-parse', 'HEAD', cwd=clone).stdout,
                         self.git('--git-dir', str(upstream), 'rev-parse', 'refs/heads/feature/nested').stdout)
        self.assertTrue(any(header['action'] == 'git.fetch' for header, _ in self.received))
        self.assertTrue(any(header['action'] == 'git.push' for header, _ in self.received))
        protected_head = self.git('--git-dir', str(upstream), 'rev-parse', 'refs/heads/feature/nested').stdout
        self.git('checkout', '-b', 'divergent', 'origin/main', cwd=clone)
        (clone / 'file').write_text('divergent\n')
        self.git('add', 'file', cwd=clone)
        self.git('commit', '-m', 'divergent', cwd=clone)
        rejected = self.git('push', '--force', 'origin', 'HEAD:refs/heads/feature/nested', cwd=clone, check=False)
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn(b'non-fast-forward', rejected.stderr)
        self.assertEqual(self.git('--git-dir', str(upstream), 'rev-parse', 'refs/heads/feature/nested').stdout, protected_head)


if __name__ == '__main__':
    unittest.main()
