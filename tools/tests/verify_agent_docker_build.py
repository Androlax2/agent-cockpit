#!/usr/bin/env python3
"""Inspect every Docker-save layer for a supplied interception CA and build trust."""

import argparse
import base64
import io
import json
from pathlib import Path, PurePosixPath
import ssl
import sys
import tarfile


TRUST_PATH = b'/run/agent-build-trust'


def archive_file(archive, name):
    path = PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts:
        raise ValueError(f'invalid archive reference: {name}')
    member = archive.getmember(name)
    if not member.isfile():
        raise ValueError(f'archive reference is not a regular file: {name}')
    return archive.extractfile(member)


def inspect_contents(stream, certificate, location):
    encoded = base64.b64encode(certificate)
    tail = b''
    encoded_tail = b''
    overlap = max(len(certificate), len(TRUST_PATH))
    while block := stream.read(1024 * 1024):
        contents = tail + block
        if certificate in contents:
            raise ValueError(f'{location}: interception certificate persisted (DER)')
        if TRUST_PATH in contents:
            raise ValueError(f'{location}: build trust configuration persisted')
        normalized = encoded_tail + block.translate(None, b' \t\r\n')
        if encoded in normalized:
            raise ValueError(f'{location}: interception certificate persisted (PEM/base64)')
        tail = contents[-overlap:]
        encoded_tail = normalized[-len(encoded):]


def inspect_image(image, ca):
    certificate = ssl.PEM_cert_to_DER_cert(ca.read_text())
    layers = set()
    with tarfile.open(image, 'r:*') as archive:
        with archive_file(archive, 'manifest.json') as stream:
            manifests = json.load(stream)
        if not isinstance(manifests, list) or not manifests:
            raise ValueError('archive has no Docker-save image manifest')
        for manifest in manifests:
            with archive_file(archive, manifest['Config']) as stream:
                config = json.load(stream)
            # History legitimately records RUN commands; only runtime settings persist trust.
            for name in ('config', 'container_config'):
                settings = config.get(name)
                for value in runtime_strings(settings):
                    inspect_contents(io.BytesIO(value.encode()), certificate,
                                     f'{manifest["Config"]}: runtime configuration')
            for layer_name in manifest['Layers']:
                if layer_name in layers:
                    continue
                layers.add(layer_name)
                with archive_file(archive, layer_name) as stream, tarfile.open(fileobj=stream, mode='r|*') as layer:
                    for member in layer:
                        location = f'{layer_name}:{member.name}'
                        if TRUST_PATH.decode() in member.linkname:
                            raise ValueError(f'{location}: link to build trust persisted')
                        if member.isfile():
                            with layer.extractfile(member) as contents:
                                inspect_contents(contents, certificate, location)
    print(f'PASS: {len(layers)} layers and runtime configuration contain no supplied CA or build trust overrides')


def runtime_strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for entry in value.values():
            yield from runtime_strings(entry)
    elif isinstance(value, list):
        for entry in value:
            yield from runtime_strings(entry)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image', type=Path)
    parser.add_argument('ca', type=Path, help='single interception CA certificate, not the public-root bundle')
    arguments = parser.parse_args()
    try:
        inspect_image(arguments.image, arguments.ca)
    except (OSError, ValueError, KeyError, tarfile.TarError) as error:
        print(f'image trust check: {error}', file=sys.stderr)
        sys.exit(1)
