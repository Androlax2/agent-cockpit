#!/usr/bin/python3 -I
"""Sandbox-local smart HTTP adapter. The host broker remains the authority."""

import argparse
import http.server
import importlib.util
import json
import os
from pathlib import Path
import re
import socket
import stat
import struct
import tempfile
import threading
import time


MAX_BODY_BYTES = 1024 ** 3
MAX_HEADER_BYTES = 16 * 1024
TRANSFER_SECONDS = 900


class AdapterDenied(Exception):
    pass


def load_broker():
    spec = importlib.util.spec_from_file_location('cockpit_adapter_protocol', Path(__file__).with_name('git-broker.py'))
    broker = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(broker)
    return broker


def git_overlay(inherited, canonical_name, adapter_url):
    broker = load_broker()
    broker.git_route({'version': 1, 'capability': '0' * 64, 'action': 'git.fetch', 'body_length': 0}, canonical_name)
    match = re.fullmatch(r'http://127\.0\.0\.1:([0-9]{1,5})/repo\.git', adapter_url)
    if match is None or not 0 < int(match[1]) < 65536:
        raise AdapterDenied()
    environment = {key: value for key, value in inherited.items() if not key.startswith('GIT_CONFIG_')}
    entries = [('credential.helper', '')]
    for prefix in ['https://github.com/', 'ssh://git@github.com/', 'git@github.com:']:
        for suffix in ['', '.git']:
            entries.append(('url.' + adapter_url + '.insteadOf', prefix + canonical_name + suffix))
    environment.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL='/dev/null', GIT_CONFIG_COUNT=str(len(entries)))
    for index, (key, value) in enumerate(entries):
        environment[f'GIT_CONFIG_KEY_{index}'] = key
        environment[f'GIT_CONFIG_VALUE_{index}'] = value
    return environment


class GitHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def setup(self):
        super().setup()
        self.connection.settimeout(30)
        self.deadline = time.monotonic() + TRANSFER_SECONDS

        def expire():
            try:
                self.connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                return

        self.expiry = threading.Timer(TRANSFER_SECONDS, expire)
        self.expiry.daemon = True
        self.expiry.start()

    def finish(self):
        self.expiry.cancel()
        super().finish()

    def log_message(self, format, *arguments):
        pass

    def read_bytes(self, count, deadline):
        if time.monotonic() >= deadline:
            raise AdapterDenied()
        self.connection.settimeout(min(30, deadline - time.monotonic()))
        result = self.rfile.read(count)
        if len(result) != count:
            raise AdapterDenied()
        return result

    def spool_request(self, body, deadline):
        if sum(len(name) + len(value) + 4 for name, value in self.headers.items()) > MAX_HEADER_BYTES:
            raise AdapterDenied()
        for name in ['Content-Length', 'Transfer-Encoding', 'Content-Type', 'Git-Protocol', 'Content-Encoding']:
            if len(self.headers.get_all(name, [])) > 1:
                raise AdapterDenied()
        if any(name in self.headers for name in ['Content-Encoding', 'Authorization', 'Proxy-Authorization']):
            raise AdapterDenied()
        length, transfer = self.headers.get('Content-Length'), self.headers.get('Transfer-Encoding')
        if length is not None and transfer is not None:
            raise AdapterDenied()
        total = 0
        if transfer is not None:
            if transfer != 'chunked' or self.command != 'POST':
                raise AdapterDenied()
            while True:
                self.connection.settimeout(min(30, max(0.001, deadline - time.monotonic())))
                line = self.rfile.readline(4097)
                if not re.fullmatch(b'[0-9a-fA-F]{1,8}\r\n', line):
                    raise AdapterDenied()
                count = int(line, 16)
                if total + count > MAX_BODY_BYTES:
                    raise AdapterDenied()
                if count == 0:
                    if self.read_bytes(2, deadline) != b'\r\n':
                        raise AdapterDenied()
                    break
                remaining = count
                while remaining:
                    chunk = self.read_bytes(min(remaining, 64 * 1024), deadline)
                    body.write(chunk)
                    remaining -= len(chunk)
                if self.read_bytes(2, deadline) != b'\r\n':
                    raise AdapterDenied()
                total += count
        else:
            if length is None:
                if self.command != 'GET':
                    raise AdapterDenied()
                length = '0'
            if not re.fullmatch(r'[0-9]{1,10}', length) or int(length) > MAX_BODY_BYTES:
                raise AdapterDenied()
            total = int(length)
            if self.command == 'GET' and total:
                raise AdapterDenied()
            remaining = total
            while remaining:
                chunk = self.read_bytes(min(remaining, 64 * 1024), deadline)
                body.write(chunk)
                remaining -= len(chunk)
        body.seek(0)
        return total

    def serve_git(self):
        self.close_connection = True
        deadline = self.deadline
        sent_response = False
        try:
            routes = {
                ('GET', '/repo.git/info/refs?service=git-upload-pack'): {'action': 'git.advertise', 'service': 'upload-pack'},
                ('GET', '/repo.git/info/refs?service=git-receive-pack'): {'action': 'git.advertise', 'service': 'receive-pack'},
                ('POST', '/repo.git/git-upload-pack'): {'action': 'git.fetch'},
                ('POST', '/repo.git/git-receive-pack'): {'action': 'git.push'},
            }
            route = routes.get((self.command, self.path))
            if route is None:
                raise AdapterDenied()
            header = {'version': 1, 'capability': self.server.capability, **route}
            protocol = self.headers.get('Git-Protocol')
            if protocol is not None:
                if not re.fullmatch(r'version=[012]', protocol):
                    raise AdapterDenied()
                header['protocol'] = int(protocol[-1])
            if self.command == 'POST':
                expected = 'application/x-git-' + ('receive-pack' if route['action'] == 'git.push' else 'upload-pack') + '-request'
                if self.headers.get('Content-Type') != expected:
                    raise AdapterDenied()
            with tempfile.TemporaryFile(dir=self.server.spool) as body, tempfile.TemporaryFile(dir=self.server.spool) as output:
                header['body_length'] = self.spool_request(body, deadline)
                self.server.broker.request_header(json.dumps(header).encode())
                with socket.socket(socket.AF_UNIX) as host:
                    host.settimeout(30)
                    host.connect(str(self.server.broker_socket))
                    encoded = json.dumps(header).encode()
                    host.sendall(struct.pack('!I', len(encoded)) + encoded)
                    while chunk := body.read(64 * 1024):
                        if time.monotonic() >= deadline:
                            raise AdapterDenied()
                        host.settimeout(min(30, deadline - time.monotonic()))
                        host.sendall(chunk)
                    host.shutdown(socket.SHUT_WR)
                    size = struct.unpack('!I', self.server.broker.read_exact(host, 4, deadline))[0]
                    if not 0 < size <= MAX_HEADER_BYTES:
                        raise AdapterDenied()
                    response = json.loads(self.server.broker.read_exact(host, size, deadline),
                                          object_pairs_hook=self.server.broker.unique_fields,
                                          parse_constant=self.server.broker.reject_constant)
                    required = {'status', 'content_type', 'body_length'}
                    if (not isinstance(response, dict) or not required <= response.keys()
                            or response.keys() - required - {'code'} or type(response['status']) is not int
                            or not 200 <= response['status'] <= 599 or type(response['body_length']) is not int
                            or not 0 <= response['body_length'] <= MAX_BODY_BYTES
                            or response['content_type'] not in {'application/json',
                                'application/x-git-upload-pack-result', 'application/x-git-upload-pack-advertisement',
                                'application/x-git-receive-pack-result', 'application/x-git-receive-pack-advertisement'}
                            or (response['status'] != 200 and response['body_length'] != 0)):
                        raise AdapterDenied()
                    remaining = response['body_length']
                    while remaining:
                        chunk = self.server.broker.read_exact(host, min(remaining, 64 * 1024), deadline)
                        output.write(chunk)
                        remaining -= len(chunk)
                    try:
                        if self.server.broker.receive_bytes(host, 1, deadline):
                            raise AdapterDenied()
                    except ConnectionResetError:
                        if response['status'] == 200:
                            raise AdapterDenied() from None
                self.send_response(response['status'])
                self.send_header('Content-Type', response['content_type'])
                self.send_header('Content-Length', str(response['body_length']))
                self.send_header('Connection', 'close')
                self.end_headers()
                sent_response = True
                output.seek(0)
                while chunk := output.read(64 * 1024):
                    if time.monotonic() >= deadline:
                        raise AdapterDenied()
                    self.connection.settimeout(min(30, deadline - time.monotonic()))
                    self.wfile.write(chunk)
        except (AdapterDenied, self.server.broker.RequestDenied, OSError, ValueError):
            if not sent_response:
                try:
                    self.send_response(502)
                    self.send_header('Content-Length', '0')
                    self.send_header('Connection', 'close')
                    self.end_headers()
                except OSError:
                    return

    do_GET = serve_git
    do_POST = serve_git


def make_server(socket_path, capability_path):
    descriptor = os.open(capability_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, 'r') as source:
        metadata = os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
            raise AdapterDenied()
        capability = source.read(65)
    if not re.fullmatch(r'[0-9a-f]{64}', capability):
        raise AdapterDenied()
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), GitHandler)
    server.broker_socket = socket_path
    server.capability = capability
    # The capability's channel folder is mounted read-only into the sandbox; spool in private /tmp.
    server.spool = Path(tempfile.mkdtemp(prefix='cockpit-git-adapter-'))
    server.broker = load_broker()
    return server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['serve'])
    parser.add_argument('--socket', type=Path, required=True)
    parser.add_argument('--capability-file', type=Path, required=True)
    arguments = parser.parse_args()
    try:
        load_broker().protect_parent()
        with make_server(arguments.socket, arguments.capability_file) as server:
            print(json.dumps({'url': f'http://127.0.0.1:{server.server_port}/repo.git'}), flush=True)
            server.serve_forever()
    except Exception:
        return 77
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
