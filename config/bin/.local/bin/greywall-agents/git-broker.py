#!/usr/bin/python3 -I
"""Untrusted request framing only; no admission, listener or forwarding yet."""

import json
import re
import struct
import sys
import time


MAX_HEADER_BYTES = 16 * 1024
MAX_GIT_BODY_BYTES = 1024 ** 3
TRANSFER_SECONDS = 15 * 60
IDLE_SECONDS = 30
CAPABILITY = re.compile(r'[0-9a-f]{64}\Z')


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


def read_request(connection, body, *, deadline):
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
        return header
    except TimeoutError:
        raise RequestDenied('request_timeout') from None
    except OSError:
        raise RequestDenied('request_io_failed') from None


if __name__ == '__main__':
    print('git-broker: broker_channel_unavailable', file=sys.stderr)
    raise SystemExit(77)
