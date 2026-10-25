#!/usr/bin/python3 -I
"""Host-only, session-bound Git channel. Upstream forwarding is not enabled."""

import argparse
import ctypes
import hashlib
import hmac
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import signal
import socket
import stat
import struct
import sys
import tempfile
import threading
import time


MAX_HEADER_BYTES = 16 * 1024
MAX_GIT_BODY_BYTES = 1024 ** 3
TRANSFER_SECONDS = 15 * 60
IDLE_SECONDS = 30
CAPABILITY = re.compile(r'[0-9a-f]{64}\Z')
HANDLE = re.compile(r'[0-9a-f]{32}\Z')
BINDING_FIELDS = {'version', 'mode', 'session', 'generation', 'capability', 'revoked', 'checkout',
                  'repository_id', 'canonical_name', 'git_dir', 'common_dir', 'configuration_digest',
                  'registry_digest', 'checked_at', 'checked_wall', 'boot_id'}
BINDING_IDENTITY = ('checkout', 'repository_id', 'canonical_name', 'git_dir', 'common_dir', 'configuration_digest')


class RequestDenied(Exception):
    """Stable public error code; never includes untrusted request data."""


def unique_fields(pairs):
    fields = {}
    for name, value in pairs:
        if name in fields:
            raise RequestDenied('invalid_header')
        fields[name] = value
    return fields


def reject_constant(value):
    raise RequestDenied('invalid_header')


def request_header(encoded):
    try:
        header = json.loads(encoded.decode('utf-8'), object_pairs_hook=unique_fields,
                            parse_constant=reject_constant)
    except (ValueError, UnicodeError, RecursionError):
        raise RequestDenied('invalid_header') from None
    required = {'version', 'capability', 'action', 'body_length'}
    if not isinstance(header, dict) or not required <= header.keys():
        raise RequestDenied('invalid_header')
    if (type(header['version']) is not int or header['version'] != 1
            or not isinstance(header['capability'], str) or not CAPABILITY.fullmatch(header['capability'])
            or not isinstance(header['action'], str)
            or type(header['body_length']) is not int or header['body_length'] < 0):
        raise RequestDenied('invalid_header')
    allowed = required | {'protocol'}
    if header['action'] == 'git.advertise':
        allowed.add('service')
        if header.get('service') not in ('upload-pack', 'receive-pack') or header['body_length'] != 0:
            raise RequestDenied('invalid_header')
    elif header['action'] not in ('git.fetch', 'git.push'):
        raise RequestDenied('action_unavailable')
    if header.keys() - allowed:
        raise RequestDenied('invalid_header')
    if 'protocol' in header and (type(header['protocol']) is not int or header['protocol'] not in (0, 1, 2)):
        raise RequestDenied('invalid_header')
    if header['body_length'] > MAX_GIT_BODY_BYTES:
        raise RequestDenied('body_too_large')
    return header


def receive_bytes(connection, count, deadline):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise RequestDenied('request_timeout')
    connection.settimeout(min(IDLE_SECONDS, remaining))
    return connection.recv(count)


def read_exact(connection, count, deadline):
    chunks = []
    while count:
        chunk = receive_bytes(connection, count, deadline)
        if not chunk:
            raise RequestDenied('truncated_frame')
        chunks.append(chunk)
        count -= len(chunk)
    return b''.join(chunks)


def read_request(connection, body, *, deadline, authorize=None):
    """Spool one half-closed request into a host-created private binary file.

    Parsed data remains untrusted: callers must independently bind the
    capability, admission and action policy before any credential access.
    """
    deadline = min(deadline, time.monotonic() + TRANSFER_SECONDS)
    try:
        header_length = struct.unpack('!I', read_exact(connection, 4, deadline))[0]
        if not 0 < header_length <= MAX_HEADER_BYTES:
            raise RequestDenied('invalid_frame')
        header = request_header(read_exact(connection, header_length, deadline))
        if authorize is not None:
            authorize(header)
        remaining = header['body_length']
        while remaining:
            chunk = receive_bytes(connection, min(remaining, 64 * 1024), deadline)
            if not chunk:
                raise RequestDenied('truncated_frame')
            if body.write(chunk) != len(chunk):
                raise RequestDenied('request_io_failed')
            remaining -= len(chunk)
        if receive_bytes(connection, 1, deadline):
            raise RequestDenied('trailing_data')
        if time.monotonic() >= deadline:
            raise RequestDenied('request_timeout')
        if authorize is not None:
            authorize(header)
        return header
    except TimeoutError:
        raise RequestDenied('request_timeout') from None
    except OSError:
        raise RequestDenied('request_io_failed') from None


def load_access():
    path = Path(__file__).with_name('git-access.py')
    spec = importlib.util.spec_from_file_location('cockpit_broker_access', path)
    access = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(access)
    return access


def private_directory(path, *, create=False):
    if path.absolute() != path.resolve(strict=False):
        raise RequestDenied('untrusted_control_path')
    if create and not path.exists():
        private_directory(path.parent, create=True)
        try:
            path.mkdir(mode=0o700)
        except FileExistsError:
            pass
    if path.absolute() != path.resolve(strict=True):
        raise RequestDenied('untrusted_control_path')
    metadata = path.stat()
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
        raise RequestDenied('untrusted_control_path')


def read_binding(path):
    private_directory(path.parent)
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, 'rb') as source:
        before = os.fstat(source.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid() or before.st_mode & 0o077:
            raise RequestDenied('untrusted_control_path')
        encoded = source.read(MAX_HEADER_BYTES + 1)
        after = os.fstat(source.fileno())
    if (len(encoded) > MAX_HEADER_BYTES or (before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
            != (after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns)):
        raise RequestDenied('invalid_binding')
    try:
        binding = json.loads(encoded.decode('utf-8'), object_pairs_hook=unique_fields, parse_constant=reject_constant)
    except (ValueError, UnicodeError, RecursionError):
        raise RequestDenied('invalid_binding') from None
    if not isinstance(binding, dict) or set(binding) != BINDING_FIELDS:
        raise RequestDenied('invalid_binding')
    if (type(binding['version']) is not int or binding['version'] != 1
            or type(binding['repository_id']) is not int or binding['repository_id'] <= 0
            or not isinstance(binding['session'], str) or not HANDLE.fullmatch(binding['session'])
            or not isinstance(binding['generation'], str) or not HANDLE.fullmatch(binding['generation'])
            or type(binding['revoked']) is not bool):
        raise RequestDenied('invalid_binding')
    for field in ('capability', 'configuration_digest', 'registry_digest'):
        if not isinstance(binding[field], str) or not CAPABILITY.fullmatch(binding[field]):
            raise RequestDenied('invalid_binding')
    for field in ('checkout', 'git_dir', 'common_dir'):
        if not isinstance(binding[field], str) or not Path(binding[field]).is_absolute():
            raise RequestDenied('invalid_binding')
    if not isinstance(binding['canonical_name'], str) or not isinstance(binding['boot_id'], str):
        raise RequestDenied('invalid_binding')
    for field in ('checked_at', 'checked_wall'):
        if type(binding[field]) not in (int, float) or not math.isfinite(binding[field]):
            raise RequestDenied('invalid_binding')
    return binding


def check_live_binding(path, expected, access):
    binding = read_binding(path)
    if binding != expected or binding['revoked'] or binding['mode'] != 'verified':
        raise RequestDenied('admission_revoked')
    if (binding['boot_id'] != Path('/proc/sys/kernel/random/boot_id').read_text().strip()
            or not 0 <= time.monotonic() - binding['checked_at'] < 600
            or not 0 <= time.time() - binding['checked_wall'] < 600):
        raise RequestDenied('admission_expired')
    try:
        _, registry = access.load_registry()
    except access.AccessDenied:
        raise RequestDenied('admission_revoked') from None
    if registry is None or hashlib.sha256(registry).hexdigest() != binding['registry_digest']:
        raise RequestDenied('admission_revoked')
    return binding


def consume_generation(control, binding):
    used = control / 'used'
    private_directory(used, create=True)
    identifiers = ['generation-' + binding['session'] + '-' + binding['generation'],
                   'capability-' + hashlib.sha256(binding['capability'].encode()).hexdigest()]
    for identifier in identifiers:
        try:
            descriptor = os.open(used / identifier, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
        except FileExistsError:
            raise RequestDenied('generation_consumed') from None
        os.close(descriptor)


def send_error(connection, code):
    header = json.dumps({'status': {'action_unavailable': 503, 'session_busy': 429}.get(code, 403),
                         'content_type': 'application/json', 'body_length': 0, 'code': code}).encode()
    try:
        connection.settimeout(IDLE_SECONDS)
        connection.sendall(struct.pack('!I', len(header)) + header)
    except OSError:
        return False
    return True


def serve(decision, socket_path, *, ready=None, stop=None):
    decision, socket_path = Path(decision).absolute(), Path(socket_path).absolute()
    runtime = Path(os.environ.get('XDG_RUNTIME_DIR', f'/run/user/{os.getuid()}'))
    private_directory(runtime)
    control = runtime / 'greywall-agents/git-access'
    if decision.parent != control or not HANDLE.fullmatch(decision.stem) or decision.suffix != '.json':
        raise RequestDenied('decision_path_denied')
    expected_socket = runtime / 'greywall-git-channel' / decision.stem / 'broker.sock'
    if socket_path != expected_socket:
        raise RequestDenied('channel_path_denied')
    binding = read_binding(decision)
    if binding['session'] != decision.stem:
        raise RequestDenied('invalid_binding')
    access = load_access()
    check_live_binding(decision, binding, access)
    report = access.inspect_access(Path(binding['checkout']), purpose='admission')
    if (report.get('mode') != 'verified' or report.get('identity_verified') is not True
            or report.get('rate_verified') is not True
            or report.get('credential_roles_verified') != ['read', 'forward']
            or any(report.get(field) != binding[field] for field in BINDING_IDENTITY)):
        raise RequestDenied('admission_denied')
    check_live_binding(decision, binding, access)
    consume_generation(control, binding)
    private_directory(socket_path.parent.parent, create=True)
    private_directory(socket_path.parent, create=True)
    spool = control / 'spool'
    private_directory(spool, create=True)
    stop = stop if stop is not None else threading.Event()
    capacity = threading.BoundedSemaphore(2)
    internal_failure = threading.Event()
    lock = threading.Lock()
    workers, connections = set(), set()

    def authorize(header):
        check_live_binding(decision, binding, access)
        if not hmac.compare_digest(header['capability'], binding['capability']):
            raise RequestDenied('capability_denied')

    def handle(connection):
        try:
            with tempfile.TemporaryFile(dir=spool) as body:
                read_request(connection, body, deadline=time.monotonic() + TRANSFER_SECONDS, authorize=authorize)
                send_error(connection, 'action_unavailable')
        except RequestDenied as error:
            send_error(connection, str(error))
        except OSError:
            send_error(connection, 'request_io_failed')
        except Exception:
            print('git-broker: internal_request_failure', file=sys.stderr)
            internal_failure.set()
            stop.set()
            send_error(connection, 'internal_error')
        finally:
            connection.close()
            with lock:
                connections.discard(connection)
                workers.discard(threading.current_thread())
            capacity.release()

    with socket.socket(socket.AF_UNIX) as listener:
        listener.bind(str(socket_path))
        os.chmod(socket_path, 0o600)
        identity = socket_path.lstat()
        listener.listen(2)
        listener.settimeout(0.1)
        termination = None
        try:
            if ready is not None:
                ready()
            while not stop.is_set():
                try:
                    check_live_binding(decision, binding, access)
                except RequestDenied as error:
                    termination = str(error)
                    break
                except OSError:
                    termination = 'binding_unavailable'
                    break
                try:
                    current = socket_path.lstat()
                except FileNotFoundError:
                    termination = 'channel_replaced'
                    break
                if (current.st_dev, current.st_ino) != (identity.st_dev, identity.st_ino):
                    termination = 'channel_replaced'
                    break
                try:
                    connection, _ = listener.accept()
                except TimeoutError:
                    continue
                if not capacity.acquire(blocking=False):
                    send_error(connection, 'session_busy')
                    connection.close()
                    continue
                worker = threading.Thread(target=handle, args=(connection,))
                with lock:
                    workers.add(worker)
                    connections.add(connection)
                worker.start()
        finally:
            stop.set()
            with lock:
                active, pending = list(connections), list(workers)
            for connection in active:
                try:
                    connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    continue
            for worker in pending:
                worker.join()
            if socket_path.exists() and socket_path.lstat().st_ino == identity.st_ino:
                socket_path.unlink()
    return 'internal_error' if internal_failure.is_set() else termination


def protect_parent():
    if sys.platform != 'linux' or os.getppid() == 1:
        raise RequestDenied('parent_guard_unavailable')
    parent = os.getppid()
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(1, signal.SIGKILL, 0, 0, 0) != 0 or os.getppid() != parent:
        raise RequestDenied('parent_guard_unavailable')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['serve'])
    parser.add_argument('--decision', type=Path, required=True)
    parser.add_argument('--socket', type=Path, required=True)
    arguments = parser.parse_args()
    try:
        protect_parent()
        termination = serve(arguments.decision, arguments.socket,
                            ready=lambda: print(json.dumps({'socket': str(arguments.socket)}), flush=True))
        if termination is not None:
            print('git-broker: ' + termination, file=sys.stderr)
            return 77
    except (RequestDenied, OSError):
        print('git-broker: broker_channel_unavailable', file=sys.stderr)
        return 77
    except Exception:
        print('git-broker: internal_channel_failure', file=sys.stderr)
        return 77
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
