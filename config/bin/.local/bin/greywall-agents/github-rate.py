#!/usr/bin/python3 -I
"""Host-only principal/resource budgets. No assumed quotas or write-result cache."""

from contextlib import contextmanager
import fcntl
import json
import math
import os
from pathlib import Path
import re
import secrets
import stat
import tempfile
import time


class RateDenied(Exception):
    """A stable, secret-free budget refusal."""


def deny(reason):
    raise RateDenied(reason)


def ledger_path():
    return Path.home() / '.local/state/greywall-agents/github-rate.json'


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            deny('rate_ledger_invalid')
        result[key] = value
    return result


def finite_number(value):
    return type(value) in {int, float} and math.isfinite(value) and value >= 0


def valid_evidence(evidence, repository_id):
    return (isinstance(evidence, dict) and set(evidence) == {'repository_id', 'default_branch', 'evidence_digest'}
            and type(evidence['repository_id']) is int and evidence['repository_id'] == repository_id
            and isinstance(evidence['default_branch'], str) and bool(evidence['default_branch'])
            and isinstance(evidence['evidence_digest'], str) and re.fullmatch(r'[0-9a-f]{64}', evidence['evidence_digest']))


def validate_state(state):
    if (not isinstance(state, dict) or type(state.get('version')) is not int or state['version'] != 1
            or not isinstance(state.get('boot'), str)
            or any(not isinstance(state.get(key), dict) for key in ['buckets', 'reservations', 'scheduled'])
            or any(not finite_number(state.get(key)) for key in ['wall', 'monotonic'])):
        deny('rate_ledger_invalid')
    for bucket, observed in state['buckets'].items():
        if (not valid_bucket(bucket) or not isinstance(observed, dict)
                or any(type(observed.get(key)) is not int for key in ['remaining', 'server_remaining', 'limit', 'reset'])
                or not 0 <= observed['remaining'] <= observed['limit'] or observed['limit'] <= 0
                or not 0 <= observed['server_remaining'] <= observed['limit']
                or observed['reset'] <= 0 or not finite_number(observed.get('blocked_until'))):
            deny('rate_ledger_invalid')
    for identifier, reservation in state['reservations'].items():
        if (not re.fullmatch(r'[0-9a-f]{32}', identifier) or not isinstance(reservation, dict)
                or reservation.get('purpose') not in ('inspection', 'admission', 'scheduled', 'write')
                or type(reservation.get('repository_id')) is not int or reservation['repository_id'] <= 0
                or not finite_number(reservation.get('deadline')) or not isinstance(reservation.get('costs'), dict)
                or not isinstance(reservation.get('windows'), dict) or set(reservation['windows']) != set(reservation['costs'])
                or any(type(reset) is not int or reset <= 0 for reset in reservation['windows'].values())
                or any(not valid_bucket(bucket) or type(cost) is not int or cost < 0
                       for bucket, cost in reservation['costs'].items())
                or (reservation.get('purpose') == 'scheduled'
                    and (not isinstance(reservation.get('key'), str) or not re.fullmatch(r'[0-9a-f]{64}', reservation['key'])))):
            deny('rate_ledger_invalid')
    for repository, cached in state['scheduled'].items():
        if (not re.fullmatch(r'[1-9][0-9]*', repository) or not isinstance(cached, dict)
                or not finite_number(cached.get('at')) or cached['at'] > state['monotonic']
                or not isinstance(cached.get('key'), str) or not re.fullmatch(r'[0-9a-f]{64}', cached['key'])
                or not valid_evidence(cached.get('evidence'), int(repository))):
            deny('rate_ledger_invalid')


@contextmanager
def locked_state():
    path = ledger_path()
    directory = path.parent
    if any(ancestor.is_symlink() for ancestor in [directory, *directory.parents]):
        deny('rate_ledger_untrusted')
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    metadata = directory.stat()
    if metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
        deny('rate_ledger_untrusted')
    descriptor = os.open(str(path) + '.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, 'rb') as lock:
        metadata = os.fstat(lock.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
            deny('rate_ledger_untrusted')
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            deny('rate_ledger_busy')
        wall, monotonic = time.time(), time.monotonic()
        boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
        state = None
        if path.is_symlink():
            deny('rate_ledger_untrusted')
        if path.exists() or path.is_symlink():
            descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
            with os.fdopen(descriptor, 'rb') as saved:
                metadata = os.fstat(saved.fileno())
                if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
                    deny('rate_ledger_untrusted')
                try:
                    state = json.load(saved, object_pairs_hook=unique_object)
                except (ValueError, UnicodeError):
                    deny('rate_ledger_invalid')
            validate_state(state)
        if state is None or state['boot'] != boot:
            state = {'version': 1, 'boot': boot, 'buckets': {}, 'reservations': {}, 'scheduled': {},
                     'wall': wall, 'monotonic': monotonic}
        if wall < state['wall'] or monotonic < state['monotonic']:
            deny('rate_clock_changed')
        state.update(wall=wall, monotonic=monotonic)
        state['reservations'] = {key: value for key, value in state['reservations'].items() if value['deadline'] > monotonic}
        state['scheduled'] = {key: value for key, value in state['scheduled'].items() if monotonic - value['at'] < 300}
        yield state
        replacement = None
        try:
            with tempfile.NamedTemporaryFile('w', dir=directory, prefix='.github-rate.', delete=False) as pending:
                replacement = Path(pending.name)
                json.dump(state, pending, sort_keys=True)
                pending.write('\n')
                pending.flush()
                os.fsync(pending.fileno())
            os.replace(replacement, path)
        finally:
            if replacement is not None and replacement.exists():
                replacement.unlink()


def valid_bucket(bucket):
    return isinstance(bucket, str) and re.fullmatch(r'(?:installation|user|app):[1-9][0-9]*:[a-z_][a-z0-9_]*', bucket)


def observe(principal, headers, *, status=200):
    if not isinstance(principal, str) or not re.fullmatch(r'(?:installation|user|app):[1-9][0-9]*', principal):
        deny('rate_principal_invalid')
    if not isinstance(headers, dict) or type(status) is not int:
        deny('rate_headers_uninspectable')
    resource = headers.get('x-ratelimit-resource')
    if not isinstance(resource, str) or not valid_bucket(principal + ':' + resource):
        deny('rate_headers_uninspectable')
    values = []
    for field in ['x-ratelimit-limit', 'x-ratelimit-remaining', 'x-ratelimit-reset']:
        value = headers.get(field)
        if not isinstance(value, str) or not re.fullmatch(r'[0-9]{1,20}', value):
            deny('rate_headers_uninspectable')
        values.append(int(value))
    limit, remaining, reset = values
    if limit <= 0 or remaining > limit or reset <= time.time():
        deny('rate_headers_uninspectable')
    retry = headers.get('retry-after')
    if retry is not None and (not isinstance(retry, str) or not re.fullmatch(r'[0-9]{1,10}', retry)):
        deny('rate_headers_uninspectable')
    bucket = principal + ':' + resource
    with locked_state() as state:
        previous = state['buckets'].get(bucket)
        server_remaining = remaining
        blocked_until = state['wall'] + int(retry) if retry is not None else 0
        if status in {403, 429} and retry is None:
            blocked_until = reset
        if previous and previous['reset'] > state['wall']:
            remaining = min(remaining, previous['remaining'])
            server_remaining = min(server_remaining, previous['server_remaining'])
            reset = max(reset, previous['reset'])
            blocked_until = max(blocked_until, previous['blocked_until'])
        outstanding = sum(reservation['costs'].get(bucket, 0) for reservation in state['reservations'].values()
                          if reservation['windows'].get(bucket) == reset)
        remaining = min(remaining, max(0, server_remaining - outstanding))
        state['buckets'][bucket] = {'remaining': remaining, 'limit': limit, 'reset': reset,
                                    'server_remaining': server_remaining, 'blocked_until': blocked_until}
    return bucket


def require_funds(state, costs, *, purpose):
    for bucket, cost in costs.items():
        if not valid_bucket(bucket) or type(cost) is not int or cost <= 0:
            deny('rate_cost_invalid')
        observed = state['buckets'].get(bucket)
        if observed is None or observed['reset'] <= state['wall']:
            deny('rate_quota_unknown')
        if observed['blocked_until'] > state['wall'] or observed['remaining'] < cost:
            deny('rate_quota_exhausted')
        if purpose != 'inspection' and observed['remaining'] - cost < (observed['limit'] + 4) // 5:
            deny('rate_headroom_reserved')


def reserve(costs, *, purpose, repository_id, deadline, evidence_key=None):
    if (purpose not in {'inspection', 'admission', 'scheduled', 'write'}
            or type(repository_id) is not int or repository_id <= 0
            or not isinstance(costs, dict) or not costs):
        deny('rate_reservation_invalid')
    if any(not valid_bucket(bucket) or type(cost) is not int or cost <= 0 for bucket, cost in costs.items()):
        deny('rate_cost_invalid')
    if type(deadline) not in {int, float} or not math.isfinite(deadline) or deadline <= time.monotonic():
        deny('rate_deadline_expired')
    if purpose == 'scheduled' and (not isinstance(evidence_key, str) or not re.fullmatch(r'[0-9a-f]{64}', evidence_key)):
        deny('rate_evidence_invalid')
    with locked_state() as state:
        if deadline <= state['monotonic']:
            deny('rate_deadline_expired')
        if purpose == 'scheduled':
            cached = state['scheduled'].get(str(repository_id))
            if cached and cached['key'] == evidence_key:
                return {'reservation': None, 'cached': cached['evidence'], 'checked_at': cached['at'], 'expires_at': cached['at'] + 300}
            if any(entry['purpose'] == 'scheduled' and entry['repository_id'] == repository_id
                   for entry in state['reservations'].values()):
                deny('scheduled_check_in_progress')
        require_funds(state, costs, purpose=purpose)
        for bucket, cost in costs.items():
            state['buckets'][bucket]['remaining'] -= cost
        identifier = secrets.token_hex(16)
        state['reservations'][identifier] = {'purpose': purpose, 'repository_id': repository_id,
                                            'deadline': deadline, 'key': evidence_key, 'costs': costs.copy(),
                                            'windows': {bucket: state['buckets'][bucket]['reset'] for bucket in costs}}
        return {'reservation': identifier, 'cached': None}


def require_dispatch_funds(state, reservation, bucket, cost):
    if not valid_bucket(bucket) or type(cost) is not int or cost <= 0:
        deny('rate_cost_invalid')
    observed = state['buckets'].get(bucket)
    if observed is None or observed['reset'] <= state['wall']:
        deny('rate_quota_unknown')
    if observed['blocked_until'] > state['wall'] or observed['server_remaining'] < cost:
        deny('rate_quota_exhausted')
    if reservation['purpose'] != 'inspection' and observed['server_remaining'] - cost < (observed['limit'] + 4) // 5:
        deny('rate_headroom_reserved')
    available = (reservation['costs'].get(bucket, 0)
                 if reservation['windows'].get(bucket) == observed['reset'] else 0)
    extra = max(0, cost - available)
    if extra:
        require_funds(state, {bucket: extra}, purpose=reservation['purpose'])
    return available, extra


def ready(identifier, costs):
    if not isinstance(costs, dict) or not costs:
        deny('rate_cost_invalid')
    with locked_state() as state:
        reservation = state['reservations'].get(identifier)
        if reservation is None:
            deny('rate_reservation_expired')
        allocations = {}
        for bucket, cost in costs.items():
            allocations[bucket] = require_dispatch_funds(state, reservation, bucket, cost)
        for bucket, (available, extra) in allocations.items():
            state['buckets'][bucket]['remaining'] -= extra
            reservation['costs'][bucket] = available + extra
            reservation['windows'][bucket] = state['buckets'][bucket]['reset']


def charge(identifier, bucket, cost=1):
    with locked_state() as state:
        reservation = state['reservations'].get(identifier)
        if reservation is None:
            deny('rate_reservation_expired')
        available, extra = require_dispatch_funds(state, reservation, bucket, cost)
        observed = state['buckets'][bucket]
        if extra:
            state['buckets'][bucket]['remaining'] -= extra
        observed['server_remaining'] -= cost
        reservation['costs'][bucket] = max(0, available - cost)
        reservation['windows'][bucket] = observed['reset']


def invalidate_scheduled(repository_id):
    if type(repository_id) is not int or repository_id <= 0:
        deny('rate_evidence_invalid')
    with locked_state() as state:
        state['scheduled'].pop(str(repository_id), None)


def finish(identifier, *, evidence=None):
    with locked_state() as state:
        reservation = state['reservations'].get(identifier)
        if reservation is None:
            deny('rate_reservation_expired')
        repository_id = reservation['repository_id']
        if evidence is not None:
            if reservation['purpose'] != 'scheduled' or not valid_evidence(evidence, repository_id):
                deny('rate_evidence_invalid')
            state['scheduled'][str(repository_id)] = {'key': reservation['key'], 'at': state['monotonic'], 'evidence': evidence}
        elif reservation['purpose'] == 'scheduled':
            state['scheduled'].pop(str(repository_id), None)
        del state['reservations'][identifier]
        if evidence is not None:
            return {'checked_at': state['monotonic'], 'expires_at': state['monotonic'] + 300}


def estimate_hourly(*, sessions=0, repositories=0, admissions=0, writes=0, advertisements=0,
                    extra_checks=0, other_reads=0, merge_reads=0, mint_requests=0,
                    check_cost=7, poll_seconds=300, coalesced=True):
    counts = [sessions, repositories, admissions, writes, advertisements, extra_checks, other_reads, merge_reads, mint_requests]
    if (any(type(count) is not int or count < 0 for count in counts)
            or type(check_cost) is not int or check_cost <= 0 or type(poll_seconds) is not int
            or poll_seconds <= 0 or 3600 % poll_seconds):
        deny('rate_estimate_invalid')
    scheduled = (3600 // poll_seconds) * (repositories if coalesced else sessions)
    installation = check_cost * (scheduled + admissions + writes + advertisements + extra_checks)
    user = writes + other_reads + merge_reads
    return {'installation': installation, 'user': user, 'app_auth': mint_requests,
            'total': installation + user + mint_requests}
