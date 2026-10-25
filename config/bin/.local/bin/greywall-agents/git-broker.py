#!/usr/bin/python3 -I
"""Host-only Git channel with fresh protection checks before each write."""

import argparse
from contextlib import nullcontext
import ctypes
import hashlib
import hmac
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import secrets
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import threading
import time


MAX_HEADER_BYTES = 16 * 1024
MAX_GIT_BODY_BYTES = 1024 ** 3
MAX_COMMAND_BYTES = 1024 ** 2
MAX_UPDATED_REFS = 1024
TRANSFER_SECONDS = 15 * 60
IDLE_SECONDS = 30
CAPABILITY = re.compile(r'[0-9a-f]{64}\Z')
HANDLE = re.compile(r'[0-9a-f]{32}\Z')
BINDING_FIELDS = {'version', 'mode', 'session', 'generation', 'capability', 'revoked', 'checkout',
                  'repository_id', 'canonical_name', 'git_dir', 'common_dir', 'configuration_digest',
                  'registry_digest', 'checked_at', 'checked_wall', 'boot_id'}
BINDING_IDENTITY = ('checkout', 'repository_id', 'canonical_name', 'git_dir', 'common_dir', 'configuration_digest')
CONTAINMENT_CODES = {'write_check_denied', 'admission_denied', 'admission_revoked', 'admission_expired',
                     'git_accounting_changed', 'upstream_throttled', 'rate_quota_unknown', 'rate_quota_exhausted',
                     'rate_headroom_reserved', 'rate_ledger_untrusted', 'rate_ledger_invalid', 'rate_ledger_unavailable',
                     'rate_headers_uninspectable', 'rate_resource_mismatch', 'rate_clock_changed'}


class RequestDenied(Exception):
    """Stable public error code; never includes untrusted request data."""

    def __init__(self, code, *, requires_containment=False):
        super().__init__(code)
        self.requires_containment = requires_containment


def is_admission_failure(error):
    return ((isinstance(error, RequestDenied) and error.requires_containment)
            or str(error) in CONTAINMENT_CODES)


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
    if header['action'] in load_actions().COLLABORATION_ACTIONS:
        if header.keys() != required:
            raise RequestDenied('invalid_header')
        if header['body_length'] > MAX_COMMAND_BYTES:
            raise RequestDenied('body_too_large')
        return header
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


def validate_push(body, *, default_branch):
    """Validate only receive commands; ancestry remains GitHub's responsibility."""
    body.seek(0)
    refs = []
    seen = set()
    prefix_bytes = 0
    allowed_capabilities = {b'report-status', b'report-status-v2', b'side-band-64k',
                            b'quiet', b'atomic', b'ofs-delta', b'object-format=sha1'}
    try:
        while True:
            length_bytes = body.read(4)
            prefix_bytes += 4
            if not re.fullmatch(b'[0-9a-fA-F]{4}', length_bytes):
                raise RequestDenied('invalid_receive_commands')
            length = int(length_bytes, 16)
            if prefix_bytes > MAX_COMMAND_BYTES:
                raise RequestDenied('command_prefix_too_large')
            if length == 0:
                if not refs and (prefix_bytes != 4 or body.read(1)):
                    raise RequestDenied('invalid_receive_commands')
                return refs
            if not 4 < length <= 65520 or len(refs) >= MAX_UPDATED_REFS:
                raise RequestDenied('invalid_receive_commands')
            prefix_bytes += length - 4
            if prefix_bytes > MAX_COMMAND_BYTES:
                raise RequestDenied('command_prefix_too_large')
            payload = body.read(length - 4)
            if len(payload) != length - 4:
                raise RequestDenied('invalid_receive_commands')
            if not refs and re.fullmatch(b'shallow [0-9a-f]{40}\n?', payload):
                continue
            command, separator, capabilities = payload.partition(b'\0')
            if separator:
                if refs:
                    raise RequestDenied('invalid_receive_commands')
                tokens = capabilities.removeprefix(b' ').split(b' ')
                if len(set(tokens)) != len(tokens):
                    raise RequestDenied('unsupported_push_capability')
                for token in tokens:
                    if token not in allowed_capabilities and not re.fullmatch(b'agent=[!-~]+', token):
                        raise RequestDenied('unsupported_push_capability')
            match = re.fullmatch(b'([0-9a-f]{40}) ([0-9a-f]{40}) (refs/heads/[^\x00]+)', command)
            if match is None:
                raise RequestDenied('invalid_receive_commands')
            _, new, encoded_ref = match.groups()
            try:
                ref = encoded_ref.decode('utf-8')
            except UnicodeError:
                raise RequestDenied('invalid_receive_commands') from None
            if (new == b'0' * 40 or ref == 'refs/heads/' + default_branch or ref in seen
                    or ref.endswith('.') or '..' in ref or '@{' in ref
                    or any(ord(character) < 33 or ord(character) == 127 or character in '~^:?*[\\'
                           for character in ref)
                    or any(not part or part.startswith('.') or part.endswith('.lock') for part in ref.split('/'))):
                raise RequestDenied('push_ref_denied')
            refs.append(ref)
            seen.add(ref)
    finally:
        body.seek(0)


def git_route(header, canonical_name):
    header = request_header(json.dumps(header).encode())
    if (not isinstance(canonical_name, str)
            or not re.fullmatch(r'[A-Za-z0-9_-][A-Za-z0-9._-]*/[A-Za-z0-9_-][A-Za-z0-9._-]*', canonical_name)):
        raise RequestDenied('repository_denied')
    advertise = header['action'] == 'git.advertise'
    service = header['service'] if advertise else ('receive-pack' if header['action'] == 'git.push' else 'upload-pack')
    if service == 'receive-pack' and header.get('protocol', 0) != 0:
        raise RequestDenied('unsupported_git_protocol')
    suffix = '/info/refs?service=git-' + service if advertise else '/git-' + service
    return {'method': 'GET' if advertise else 'POST',
            'url': 'https://github.com/' + canonical_name + '.git' + suffix,
            'content_type': 'application/x-git-' + service + '-request',
            'response_type': 'application/x-git-' + service + ('-advertisement' if advertise else '-result')}


def git_authority(access, binding, header):
    registry, serialized = access.load_registry()
    if serialized is None or hashlib.sha256(serialized).hexdigest() != binding['registry_digest']:
        raise RequestDenied('admission_revoked')
    record = next((entry for entry in registry['repositories']
                   if entry['repository_id'] == binding['repository_id']
                   and entry['canonical_name'] == binding['canonical_name']), None)
    if record is None:
        raise RequestDenied('action_unavailable')
    role = 'forward' if header['action'] == 'git.push' or header.get('service') == 'receive-pack' else 'read'
    profile = access.runtime_credential_profiles(record, registry)[role]
    operation = header['action'] + (':' + header['service'] if header['action'] == 'git.advertise' else '')
    costs = profile.get('git_rate_costs', {}).get(operation)
    rate = access.load_rate()
    if costs == {}:
        evidence = profile.get('git_unmetered_evidence', {}).get(operation)
        if (not isinstance(evidence, dict) or evidence.get('mode') != 'no_primary_headers'
                or type(evidence.get('reviewed_at')) is not int or type(evidence.get('valid_until')) is not int
                or not 0 < evidence['reviewed_at'] <= time.time() < evidence['valid_until']
                or not isinstance(evidence.get('evidence'), str) or not CAPABILITY.fullmatch(evidence['evidence'])):
            raise RequestDenied('git_rate_setup_required')
    if (profile.get('git_rate_setup_approved') is not True or not isinstance(costs, dict)
            or any(not rate.valid_bucket(bucket) or not bucket.startswith(access.runtime_principal(profile) + ':')
                   or type(cost) is not int or cost <= 0 for bucket, cost in costs.items())):
        raise RequestDenied('git_rate_setup_required')
    if profile['permissions'].get('contents') != ('write' if role == 'forward' else 'read'):
        raise RequestDenied('git_role_denied')
    return record, profile, costs


def load_actions():
    spec = importlib.util.spec_from_file_location('cockpit_github_actions', Path(__file__).with_name('github-actions.py'))
    actions = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(actions)
    return actions


def action_pending_path(binding, action, fields):
    directory = Path.home() / '.local/state/greywall-agents/github-actions'
    private_directory(directory, create=True)
    digest = hashlib.sha256(json.dumps({'repository_id': binding['repository_id'], 'action': action,
                                       'fields': fields}, sort_keys=True).encode()).hexdigest()
    return directory / (digest + '.pending')


def action_intent(binding, action, fields):
    path = action_pending_path(binding, action, fields)
    request_id = secrets.token_hex(16)
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    except FileExistsError:
        raise RequestDenied('action_reconciliation_required') from None
    with os.fdopen(descriptor, 'w') as journal:
        json.dump({'request_id': request_id, 'repository_id': binding['repository_id'], 'action': action,
                   'digest': path.stem, 'created_at': int(time.time()), 'target_number': fields.get('number'),
                   'head_oid': fields.get('expected_head_sha'),
                   'body_digest': hashlib.sha256(fields['body'].encode()).hexdigest() if 'body' in fields else None,
                   'title_digest': hashlib.sha256(fields['title'].encode()).hexdigest() if 'title' in fields else None}, journal)
        journal.flush()
        os.fsync(journal.fileno())
    sync_action_directory(path.parent)
    return path, request_id


def sync_action_directory(directory):
    descriptor = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def dispatch_collaboration(access, binding, header, body, output, *, deadline, check_live):
    actions = load_actions()
    rate = access.load_rate()
    reservation = None
    dispatched = False
    try:
        body.seek(0)
        try:
            fields = json.loads(body.read(MAX_COMMAND_BYTES + 1), object_pairs_hook=unique_fields,
                                parse_constant=reject_constant)
        except (ValueError, UnicodeError, RecursionError):
            raise RequestDenied('invalid_action_fields') from None
        actions.action_fields(header['action'], fields)
        registry, serialized = access.load_registry()
        if serialized is None or hashlib.sha256(serialized).hexdigest() != binding['registry_digest']:
            raise RequestDenied('admission_revoked')
        record = next((entry for entry in registry['repositories']
                       if entry['repository_id'] == binding['repository_id']
                       and entry['canonical_name'] == binding['canonical_name']), None)
        if record is None:
            raise RequestDenied('action_unavailable')
        profiles = access.runtime_credential_profiles(record, registry)
        request_id = None
        intent = None

        def request(method, path, payload=None, *, permission='issues'):
            nonlocal reservation, dispatched, request_id, intent
            check_live()
            profile = profiles['read' if method == 'GET' else 'forward']
            if profile['permissions'].get(permission) != ('read' if method == 'GET' else 'write'):
                raise RequestDenied('collaboration_role_denied')
            bucket = access.runtime_principal(profile) + ':core'
            costs = {bucket: 1}
            needs_head_guard = method != 'GET' and (header['action'].startswith('review.')
                                                    or header['action'] == 'pull_request.update_branch')
            if needs_head_guard:
                guard_bucket = access.runtime_principal(profiles['read']) + ':core'
                costs[guard_bucket] = costs.get(guard_bucket, 0) + 1
            if method == 'GET':
                reservation = rate.reserve(costs, purpose='admission', repository_id=binding['repository_id'],
                                           deadline=min(deadline, time.monotonic() + 30))['reservation']
            else:
                report = access.inspect_access(Path(binding['checkout']), purpose='write', operation_costs=costs)
                reservation = report.get('rate_reservation')
                if (report.get('mode') != 'verified' or report.get('identity_verified') is not True
                        or report.get('rate_verified') is not True
                        or report.get('credential_roles_verified') != ['read', 'forward']
                        or any(report.get(field) != binding[field] for field in BINDING_IDENTITY)
                        or reservation is None or report.get('rate_operation_costs') != costs):
                    raise RequestDenied('write_check_denied')
            token = access.read_runtime_credential(record, profile, min(deadline, time.monotonic() + 30), verify_remote=False)
            check_live()
            rate.ready(reservation, costs)
            if needs_head_guard:
                guard_token = access.read_runtime_credential(record, profiles['read'],
                    min(deadline, time.monotonic() + 30), verify_remote=False)
                rate.charge(reservation, guard_bucket)
                guard_status, current, guard_headers = access.github_request('GET',
                    '/repos/' + record['canonical_name'] + '/pulls/' + str(fields['number']),
                    guard_token, min(deadline, time.monotonic() + 30))
                observed = rate.observe(access.runtime_principal(profiles['read']), guard_headers, status=guard_status)
                if observed != guard_bucket:
                    raise RequestDenied('rate_resource_mismatch')
                if guard_status != 200:
                    raise RequestDenied('review_head_changed')
                current = actions.verified_pull_request(record, fields['number'], current)
                if (current['head']['sha'] != fields['expected_head_sha']
                        or (header['action'] in ('review.create', 'review.submit', 'pull_request.update_branch')
                            and current['state'] != 'open')):
                    raise RequestDenied('review_head_changed')
                check_live()
                rate.ready(reservation, {bucket: 1})
            if method != 'GET':
                rate.admit_mutation()
                intent, request_id = action_intent(binding, header['action'], fields)
            rate.charge(reservation, bucket)
            if method != 'GET':
                dispatched = True
            status, value, headers = access.github_request(method, path, token, min(deadline, time.monotonic() + 30), payload)
            observed = rate.observe(access.runtime_principal(profile), headers, status=status)
            if observed != bucket:
                raise RequestDenied('rate_resource_mismatch')
            if status in (403, 429) or 'retry-after' in headers:
                raise RequestDenied('upstream_throttled')
            rate.finish(reservation)
            reservation = None
            return status, value, headers

        with rate.repository_write(binding['repository_id']):
            pending = action_pending_path(binding, header['action'], fields)
            if pending.exists() or pending.is_symlink():
                raise RequestDenied('action_reconciliation_required')
            result = actions.perform_action(record, header['action'], fields, request,
                                            actor_id=profiles['forward']['actor_id'])
            result['request_id'] = request_id
            if intent is not None and result.get('outcome') == 'succeeded':
                os.replace(intent, intent.with_name(request_id + '.completed'))
                sync_action_directory(intent.parent)
            encoded = json.dumps(result).encode()
            output.write(encoded)
            output.seek(0)
            return {'status': 200, 'content_type': 'application/json', 'body_length': len(encoded)}
    except (actions.ActionDenied, access.AccessDenied, rate.RateDenied, RequestDenied, OSError) as error:
        if dispatched:
            raise RequestDenied('action_outcome_unknown', requires_containment=is_admission_failure(error)) from None
        if isinstance(error, OSError):
            raise RequestDenied('action_storage_unavailable') from None
        raise RequestDenied(str(error)) from None
    finally:
        if reservation is not None:
            rate.finish(reservation)


def run_git_transport(header, binding, token, body, output, *, deadline, check_live):
    control_fd = os.memfd_create('cockpit-git-control', os.MFD_CLOEXEC)
    result_fd = os.memfd_create('cockpit-git-result', os.MFD_CLOEXEC)
    with os.fdopen(control_fd, 'w+b') as control, os.fdopen(result_fd, 'w+b') as result:
        control.write(json.dumps({'header': header, 'canonical_name': binding['canonical_name'], 'token': token}).encode())
        control.seek(0)
        body.seek(0)
        output.seek(0)
        arguments = [sys.executable, '-I', str(Path(__file__).with_name('git-transport.py')), str(os.getpid()),
                     str(control_fd), str(body.fileno()), str(output.fileno()), str(result_fd)]
        environment = load_access().git_environment()
        check_live()
        with subprocess.Popen(arguments, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                              stderr=subprocess.DEVNULL, env=environment,
                              pass_fds=(control_fd, body.fileno(), output.fileno(), result_fd)) as worker:
            last_progress = time.monotonic()
            progress = (0, 0)
            try:
                while worker.poll() is None:
                    check_live()
                    now = time.monotonic()
                    current = (os.lseek(body.fileno(), 0, os.SEEK_CUR), os.fstat(output.fileno()).st_size)
                    if current != progress:
                        progress, last_progress = current, now
                    if now >= deadline or now - last_progress >= IDLE_SECONDS:
                        raise RequestDenied('upstream_timeout')
                    if current[1] > MAX_GIT_BODY_BYTES or os.fstat(result_fd).st_size > MAX_HEADER_BYTES:
                        raise RequestDenied('upstream_response_denied')
                    time.sleep(0.05)
                check_live()
                if worker.returncode or time.monotonic() >= deadline:
                    raise RequestDenied('upstream_transport_failed')
            finally:
                if worker.poll() is None:
                    worker.kill()
                    worker.wait()
        result.seek(0)
        try:
            response = json.loads(result.read(MAX_HEADER_BYTES + 1), object_pairs_hook=unique_fields,
                                  parse_constant=reject_constant)
        except (ValueError, UnicodeError):
            raise RequestDenied('upstream_response_denied') from None
        if (not isinstance(response, dict) or set(response) != {'status', 'content_type', 'body_length', 'rate_headers'}
                or response['status'] not in (200, 403, 429)
                or response['content_type'] != (git_route(header, binding['canonical_name'])['response_type']
                                                if response['status'] == 200 else 'application/json')
                or type(response['body_length']) is not int
                or not 0 <= response['body_length'] <= MAX_GIT_BODY_BYTES
                or response['body_length'] != os.fstat(output.fileno()).st_size
                or not isinstance(response['rate_headers'], dict)):
            raise RequestDenied('upstream_response_denied')
        output.seek(0)
        return response


def dispatch_git(access, binding, header, body, output, *, deadline, check_live):
    record, profile, costs = git_authority(access, binding, header)
    rate = access.load_rate()
    requires_write_check = header['action'] == 'git.push' or header.get('service') == 'receive-pack'
    reservation = None
    dispatched = False
    updated_refs = []
    try:
        with rate.repository_write(binding['repository_id']) if requires_write_check else nullcontext():
            check_live()
            report = access.inspect_access(Path(binding['checkout']), purpose='write' if requires_write_check else 'admission',
                                           **(({'operation_costs': costs} | ({'unmetered_operation': True} if not costs else {}))
                                              if requires_write_check else {}))
            reservation = report.get('rate_reservation')
            if (report.get('mode') != 'verified' or report.get('identity_verified') is not True
                    or report.get('rate_verified') is not True
                    or report.get('credential_roles_verified') != ['read', 'forward']
                    or any(report.get(field) != binding[field] for field in BINDING_IDENTITY)):
                raise RequestDenied('write_check_denied' if requires_write_check else 'admission_denied')
            if header['action'] == 'git.push':
                default = report.get('default_branch')
                if not isinstance(default, str) or not default:
                    raise RequestDenied('write_check_denied')
                updated_refs = validate_push(body, default_branch=default)
            if not requires_write_check and costs:
                reservation = rate.reserve(costs, purpose='admission', repository_id=binding['repository_id'],
                                           deadline=min(deadline, time.monotonic() + 30))['reservation']
            elif requires_write_check and (reservation is None or report.get('rate_operation_costs') != costs):
                raise RequestDenied('write_check_denied')
            token = access.read_runtime_credential(record, profile, min(deadline, time.monotonic() + 30), verify_remote=False)
            with rate.request_slot():
                check_live()
                if costs:
                    rate.ready(reservation, costs)
                if updated_refs:
                    rate.admit_mutation()
                for bucket, cost in costs.items():
                    rate.charge(reservation, bucket, cost)
                if reservation is not None:
                    rate.finish(reservation)
                reservation = None
                check_live()
                if time.monotonic() >= deadline:
                    raise RequestDenied('request_timeout')
                dispatched = True
                response = run_git_transport(header, binding, token, body, output, deadline=deadline, check_live=check_live)
                if response['status'] in (403, 429):
                    rate.backoff(costs or [access.runtime_principal(profile) + ':core'], response['rate_headers'])
                    raise RequestDenied('upstream_throttled')
                if response['rate_headers']:
                    if not costs:
                        raise RequestDenied('git_accounting_changed')
                    observed = rate.observe(access.runtime_principal(profile), response['rate_headers'], status=response['status'])
                    if observed not in costs:
                        raise RequestDenied('rate_resource_mismatch')
                return {key: response[key] for key in ['status', 'content_type', 'body_length']}
    except (RequestDenied, rate.RateDenied, access.AccessDenied, OSError) as error:
        if dispatched and header['action'] == 'git.push':
            raise RequestDenied('upstream_outcome_unknown', requires_containment=is_admission_failure(error)) from None
        if isinstance(error, (rate.RateDenied, access.AccessDenied)):
            raise RequestDenied(str(error)) from None
        raise
    finally:
        if reservation is not None:
            rate.finish(reservation)


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
    admission_failure = threading.Event()
    lock = threading.Lock()
    workers, connections = set(), set()

    def authorize(header):
        if admission_failure.is_set():
            raise RequestDenied('admission_revoked')
        check_live_binding(decision, binding, access)
        if not hmac.compare_digest(header['capability'], binding['capability']):
            raise RequestDenied('capability_denied')

    def handle(connection):
        response_started = False
        push_completed = False
        try:
            deadline = time.monotonic() + TRANSFER_SECONDS
            next_check = time.monotonic() + 300

            def check_live():
                nonlocal next_check
                if stop.is_set() or admission_failure.is_set():
                    raise RequestDenied('admission_revoked')
                check_live_binding(decision, binding, access)
                if time.monotonic() >= next_check:
                    current = access.inspect_access(Path(binding['checkout']), purpose='scheduled')
                    if (current.get('mode') != 'verified'
                            or current.get('identity_verified') is not True or current.get('rate_verified') is not True
                            or current.get('credential_roles_verified') != ['read', 'forward']
                            or any(current.get(field) != binding[field] for field in BINDING_IDENTITY)):
                        stop.set()
                        raise RequestDenied('admission_revoked')
                    next_check = time.monotonic() + 300

            with tempfile.TemporaryFile(dir=spool) as body, tempfile.TemporaryFile(dir=spool) as output:
                header = read_request(connection, body, deadline=deadline, authorize=authorize)
                is_collaboration_action = header['action'] in load_actions().COLLABORATION_ACTIONS
                if not is_collaboration_action:
                    git_route(header, binding['canonical_name'])
                if header['action'] == 'git.push':
                    default_branch = report.get('default_branch')
                    if not isinstance(default_branch, str) or not default_branch:
                        raise RequestDenied('admission_denied')
                    validate_push(body, default_branch=default_branch)
                dispatch = dispatch_collaboration if is_collaboration_action else dispatch_git
                response = dispatch(access, binding, header, body, output, deadline=deadline, check_live=check_live)
                push_completed = header['action'] == 'git.push' or is_collaboration_action
                encoded = json.dumps(response).encode()
                check_live()
                connection.settimeout(min(IDLE_SECONDS, max(0.001, deadline - time.monotonic())))
                response_started = True
                connection.sendall(struct.pack('!I', len(encoded)) + encoded)
                while chunk := output.read(64 * 1024):
                    check_live()
                    if time.monotonic() >= deadline:
                        raise RequestDenied('request_timeout')
                    connection.settimeout(min(IDLE_SECONDS, deadline - time.monotonic()))
                    connection.sendall(chunk)
        except RequestDenied as error:
            contains_failure = is_admission_failure(error)
            if contains_failure:
                admission_failure.set()
            if not response_started:
                send_error(connection, 'upstream_outcome_unknown' if push_completed else str(error))
            if contains_failure:
                stop.set()
        except OSError:
            if not response_started:
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
    return ('internal_error' if internal_failure.is_set() else
            'admission_revoked' if admission_failure.is_set() else termination)


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
