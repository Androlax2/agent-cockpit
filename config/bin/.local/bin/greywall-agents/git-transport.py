#!/usr/bin/python3 -I
"""Private, killable Git HTTPS worker; credentials arrive only on inherited FDs."""

import base64
import importlib.util
import json
import os
from pathlib import Path
import re
import ssl
import sys
import urllib.error
import urllib.request


MAX_BODY_BYTES = 1024 ** 3


class TransportDenied(Exception):
    """Secret-free transport failure."""

    def __init__(self, code, *, status=None, headers=None):
        super().__init__(code)
        self.status = status
        self.headers = {} if headers is None else headers


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, source, code, message, headers, target):
        return None


def load_broker():
    spec = importlib.util.spec_from_file_location('cockpit_transport_broker', Path(__file__).with_name('git-broker.py'))
    broker = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(broker)
    return broker


def rate_headers(headers):
    return {key.lower(): value for key, value in headers.items()
            if key.lower() in {'x-ratelimit-resource', 'x-ratelimit-limit', 'x-ratelimit-remaining',
                               'x-ratelimit-reset', 'retry-after'}}


def transfer(control, body, output):
    if not isinstance(control, dict) or set(control) != {'header', 'canonical_name', 'token'}:
        raise TransportDenied('transport_control_invalid')
    token = control['token']
    if not isinstance(token, str) or not token or any(not 33 <= ord(character) <= 126 for character in token):
        raise TransportDenied('transport_control_invalid')
    broker = load_broker()
    try:
        route = broker.git_route(control['header'], control['canonical_name'])
    except broker.RequestDenied:
        raise TransportDenied('transport_control_invalid') from None
    header = control['header']
    authorization = base64.b64encode(('x-access-token:' + token).encode('ascii')).decode('ascii')
    headers = {'Authorization': 'Basic ' + authorization, 'Accept': route['response_type'],
               'Accept-Encoding': 'identity', 'Content-Length': str(header['body_length'])}
    if route['method'] == 'POST':
        headers['Content-Type'] = route['content_type']
    if 'protocol' in header:
        headers['Git-Protocol'] = 'version=' + str(header['protocol'])
    context = ssl.create_default_context(cafile=os.environ.get('CURL_CA_BUNDLE') or os.environ.get('SSL_CERT_FILE'))
    opener = urllib.request.build_opener(NoRedirect(), urllib.request.HTTPSHandler(context=context))
    request = urllib.request.Request(route['url'], data=body if route['method'] == 'POST' else None,
                                     headers=headers, method=route['method'])
    try:
        with opener.open(request, timeout=30) as response:
            if response.status != 200 or response.headers.get('Content-Type') != route['response_type']:
                raise TransportDenied('upstream_response_denied', status=response.status, headers=rate_headers(response.headers))
            if response.headers.get('Content-Encoding', 'identity') != 'identity':
                raise TransportDenied('upstream_response_denied')
            declared = response.headers.get('Content-Length')
            if declared is not None and (not re.fullmatch(r'[0-9]{1,10}', declared) or int(declared) > MAX_BODY_BYTES):
                raise TransportDenied('upstream_response_denied')
            length = 0
            while chunk := response.read(64 * 1024):
                length += len(chunk)
                if length > MAX_BODY_BYTES:
                    raise TransportDenied('upstream_body_too_large')
                output.write(chunk)
                output.flush()
            if declared is not None and length != int(declared):
                raise TransportDenied('upstream_response_denied')
            return {'status': 200, 'content_type': route['response_type'], 'body_length': length,
                    'rate_headers': rate_headers(response.headers)}
    except urllib.error.HTTPError as error:
        raise TransportDenied('upstream_response_denied', status=error.code, headers=rate_headers(error.headers)) from None
    except (urllib.error.URLError, OSError, ValueError):
        raise TransportDenied('upstream_transport_failed') from None


def main():
    try:
        parent, control_fd, body_fd, output_fd, result_fd = map(int, sys.argv[1:])
        broker = load_broker()
        if os.getppid() != parent:
            return 77
        broker.protect_parent()
        if os.getppid() != parent:
            return 77
        with os.fdopen(control_fd, 'rb') as source, os.fdopen(body_fd, 'rb') as body, \
                os.fdopen(output_fd, 'wb') as output, os.fdopen(result_fd, 'wb') as result:
            try:
                encoded = source.read(16 * 1024 + 1)
                if len(encoded) > 16 * 1024:
                    raise TransportDenied('transport_control_invalid')
                control = json.loads(encoded, object_pairs_hook=broker.unique_fields, parse_constant=broker.reject_constant)
                # Parent buffered validation can leave the inherited descriptor past its logical position.
                body.seek(0)
                response = transfer(control, body, output)
            except TransportDenied as error:
                response = {'code': 'upstream_transport_failed'}
                if error.status in (403, 429):
                    response = {'status': error.status, 'content_type': 'application/json', 'body_length': 0,
                                'rate_headers': error.headers}
            except Exception:
                response = {'code': 'upstream_transport_failed'}
            result.write(json.dumps(response).encode())
            return 0 if 'code' not in response else 77
    except Exception:
        return 77


if __name__ == '__main__':
    raise SystemExit(main())
