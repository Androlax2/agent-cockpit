#!/usr/bin/python3 -I
"""Host-side repository identity inspection. Local identity never grants writes."""

import argparse
import base64
import copy
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from urllib.parse import parse_qs, quote, urlsplit


CHECK_DEADLINE_SECONDS = 30
PROBE_BRANCH = 'agents/protection-probe/future'
APP_TOKENS = {}
REPOSITORY_NAME = re.compile(r'[A-Za-z0-9_-][A-Za-z0-9._-]*/[A-Za-z0-9_-][A-Za-z0-9._-]*\Z')
GITHUB_REMOTE = re.compile(
    r'(?:https://github\.com/|git@github\.com:|ssh://git@github\.com/)'
    r'([A-Za-z0-9_-][A-Za-z0-9._-]*/[A-Za-z0-9_-][A-Za-z0-9._-]*?)(?:\.git)?\Z'
)


class AccessDenied(Exception):
    """A public, secret-free reason code, not a subprocess or config error body."""


def deny(reason):
    raise AccessDenied(reason)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            deny('invalid_registry')
        result[key] = value
    return result


def identity_path(value):
    if any(ord(character) < 32 for character in str(value)):
        deny('unsupported_identity_path')
    path = Path(value).absolute()
    for ancestor in [*reversed(path.parents), path]:
        if ancestor.is_symlink():
            deny('symlink_identity_path')
    return path.resolve(strict=True)


def registry_path():
    return Path.home() / '.config/greywall-agents/git-repositories.json'


def load_registry():
    path = registry_path()
    if not path.exists() and not path.is_symlink():
        return {'version': 1, 'repositories': [], 'inspector_apps': []}, None
    identity_path(path)
    for protected in [path.parent, path]:
        metadata = protected.stat()
        if metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
            deny('untrusted_registry_permissions')
    if not stat.S_ISREG(path.stat().st_mode):
        deny('invalid_registry')
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, 'rb') as registry_file:
        metadata = os.fstat(registry_file.fileno())
        if metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
            deny('untrusted_registry_permissions')
        if not stat.S_ISREG(metadata.st_mode):
            deny('invalid_registry')
        serialized = registry_file.read()
    try:
        registry = json.loads(serialized, object_pairs_hook=unique_object)
    except (ValueError, UnicodeError):
        deny('invalid_registry')
    if (not isinstance(registry, dict) or type(registry.get('version')) is not int
            or registry['version'] != 1 or not isinstance(registry.get('repositories'), list)):
        deny('invalid_registry')
    seen_projects, seen_repositories, seen_checkouts = set(), set(), set()
    for record in registry['repositories']:
        if not isinstance(record, dict):
            deny('invalid_registry')
        project = record.get('project')
        repository_id = record.get('repository_id')
        name = record.get('canonical_name')
        if (not isinstance(project, str) or not re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9._-]*', project)
                or type(repository_id) is not int or repository_id <= 0
                or not isinstance(name, str) or not REPOSITORY_NAME.fullmatch(name)
                or project in seen_projects or repository_id in seen_repositories):
            deny('invalid_registry')
        seen_projects.add(project)
        seen_repositories.add(repository_id)
        checkouts = record.get('checkouts')
        edges = record.get('mirror_edges')
        if not isinstance(checkouts, list) or not checkouts or not isinstance(edges, list):
            deny('invalid_registry')
        roots = set()
        for checkout in checkouts:
            if not isinstance(checkout, dict):
                deny('invalid_registry')
            for key in ['path', 'git_dir', 'common_dir']:
                value = checkout.get(key)
                if (not isinstance(value, str) or not Path(value).is_absolute()
                        or any(ord(character) < 32 for character in value)
                        or str(Path(os.path.normpath(value))) != value):
                    deny('invalid_registry')
            root = checkout['path']
            if root in seen_checkouts:
                deny('invalid_registry')
            seen_checkouts.add(root)
            roots.add(root)
        edge_sources = set()
        for edge in edges:
            if (not isinstance(edge, dict)
                    or not isinstance(edge.get('checkout'), str) or not isinstance(edge.get('origin'), str)
                    or edge['checkout'] not in roots or edge['origin'] not in roots
                    or edge['checkout'] in edge_sources):
                deny('invalid_registry')
            edge_sources.add(edge['checkout'])
    apps = registry.get('inspector_apps', [])
    if not isinstance(apps, list):
        deny('invalid_registry')
    seen_installations = set()
    for app in apps:
        if (not isinstance(app, dict) or type(app.get('app_id')) is not int
                or type(app.get('installation_id')) is not int
                or not isinstance(app.get('owner_login'), str)
                or not isinstance(app.get('approved_repository_ids'), list)
                or any(type(repository_id) is not int or repository_id <= 0
                       for repository_id in app['approved_repository_ids'])):
            deny('invalid_registry')
        identity = (app['app_id'], app['installation_id'])
        if identity in seen_installations:
            deny('invalid_registry')
        seen_installations.add(identity)
    return registry, serialized


def checkout_root(directory):
    directory = identity_path(directory)
    if not directory.is_dir():
        deny('not_a_repository')
    for candidate in [directory, *directory.parents]:
        marker = candidate / '.git'
        if marker.exists() or marker.is_symlink():
            return candidate
    deny('not_a_repository')


def pointer_target(path, base, prefix=''):
    path = identity_path(path)
    if not stat.S_ISREG(path.stat().st_mode):
        deny('unsupported_git_metadata')
    value = path.read_text().removesuffix('\n')
    if not value.startswith(prefix):
        deny('unsupported_git_metadata')
    value = value[len(prefix):]
    if not value or any(ord(character) < 32 for character in value):
        deny('unsupported_git_metadata')
    return identity_path(base / value)


def git_environment():
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(('GIT_', 'GH_', 'GITHUB_'))}
    environment.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL='/dev/null',
                       GIT_TERMINAL_PROMPT='0', GIT_OPTIONAL_LOCKS='0', LC_ALL='C')
    return environment


def git_output(arguments, deadline):
    executable = shutil.which('git', path=os.defpath)
    if not executable:
        deny('git_unavailable')
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        deny('inspection_timeout')
    result = subprocess.run([executable, *arguments], env=git_environment(),
                            capture_output=True, timeout=remaining)
    if result.returncode:
        deny('unsupported_git_configuration')
    return result.stdout.decode('utf-8')


def read_config(path, deadline):
    path = identity_path(path)
    if not stat.S_ISREG(path.stat().st_mode):
        deny('unsupported_git_configuration')
    before = path.read_bytes()
    entries = []
    for entry in git_output(['config', '--no-includes', '--null', '--file', str(path), '--list'], deadline).split('\0'):
        if not entry:
            continue
        key, separator, value = entry.partition('\n')
        if not separator:
            value = 'true'
        lower_key = key.lower()
        if (lower_key.startswith(('include.', 'includeif.', 'url.'))
                or (lower_key.startswith('remote.') and lower_key.endswith('.pushurl'))):
            deny('unsupported_git_configuration')
        entries.append((key, value))
    if path.read_bytes() != before:
        deny('git_metadata_changed')
    return entries, hashlib.sha256(before).hexdigest()


def metadata_locations(root):
    marker = identity_path(root / '.git')
    git_dir = marker if marker.is_dir() else pointer_target(marker, root, 'gitdir: ')
    common_pointer = git_dir / 'commondir'
    common_dir = pointer_target(common_pointer, git_dir) if common_pointer.exists() else git_dir
    return git_dir, common_dir


def inspect_metadata(root, binding, deadline):
    git_dir, common_dir = metadata_locations(root)
    if str(git_dir) != binding['git_dir'] or str(common_dir) != binding['common_dir']:
        deny('git_metadata_changed')
    if not git_dir.is_dir() or not common_dir.is_dir():
        deny('unsupported_git_metadata')
    identity_path(git_dir / 'HEAD')
    entries, digest = read_config(common_dir / 'config', deadline)
    digests = [digest]
    worktree_config = git_dir / 'config.worktree'
    if worktree_config.exists() or worktree_config.is_symlink():
        extra, digest = read_config(worktree_config, deadline)
        entries.extend(extra)
        digests.append(digest)
    for key, value in entries:
        if key.lower() == 'core.worktree' and identity_path(git_dir / value) != root:
            deny('git_metadata_changed')
    # Git's discovery is safe only after includes/rewrites and worktree pointers are checked.
    discovered = git_output(['-C', str(root), '-c', 'core.fsmonitor=false', '-c', 'core.hooksPath=/dev/null',
                             'rev-parse', '--show-toplevel', '--absolute-git-dir', '--git-common-dir'], deadline).splitlines()
    if (len(discovered) != 3 or identity_path(discovered[0]) != root
            or identity_path(discovered[1]) != git_dir or identity_path(root / discovered[2]) != common_dir):
        deny('git_metadata_changed')
    return entries, digests


def inspect_remote_chain(root, record, deadline, visited):
    if root in visited:
        deny('mirror_cycle')
    visited.add(root)
    binding = next((entry for entry in record['checkouts'] if entry['path'] == str(root)), None)
    if binding is None:
        deny('unapproved_local_mirror')
    entries, digests = inspect_metadata(root, binding, deadline)
    origins = [value for key, value in entries if key == 'remote.origin.url']
    if len(origins) != 1:
        deny('missing_origin' if not origins else 'unsupported_git_configuration')
    for key, value in entries:
        if not key.startswith('remote.') or not key.endswith('.url'):
            continue
        matched = GITHUB_REMOTE.fullmatch(value)
        if matched:
            if matched[1].lower() != record['canonical_name'].lower():
                deny('remote_identity_mismatch')
            continue
        if key != 'remote.origin.url' or not value.startswith('/'):
            deny('unsupported_remote')
        target = identity_path(value)
        if {'checkout': str(root), 'origin': str(target)} not in record['mirror_edges']:
            deny('unapproved_local_mirror')
        digests.extend(inspect_remote_chain(target, record, deadline, visited))
    return digests


def remaining_seconds(deadline):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        deny('inspection_timeout')
    return remaining


def trusted_program(name):
    executable = shutil.which(name, path=os.defpath)
    if not executable:
        deny('inspector_dependency_unavailable')
    return executable


def sign_app_jwt(app, deadline):
    result = subprocess.run([trusted_program('secret-tool'), 'lookup', 'service', 'cockpit-github-inspector',
                             'app', app['client_id']], capture_output=True, env=git_environment(),
                            timeout=remaining_seconds(deadline))
    if result.returncode or not result.stdout:
        deny('inspector_key_unavailable')

    def encode(content):
        return base64.urlsafe_b64encode(content).rstrip(b'=')

    now = int(time.time())
    header = encode(b'{"alg":"RS256","typ":"JWT"}')
    payload = encode(json.dumps({'iss': app['client_id'], 'iat': now - 60, 'exp': now + 600},
                                separators=(',', ':')).encode())
    unsigned = header + b'.' + payload
    descriptor = os.memfd_create('cockpit-inspector-key', os.MFD_CLOEXEC)
    with os.fdopen(descriptor, 'w+b') as key_file:
        key_file.write(result.stdout)
        key_file.flush()
        key_file.seek(0)
        signature = subprocess.run([trusted_program('openssl'), 'dgst', '-sha256', '-sign',
                                    f'/proc/self/fd/{descriptor}'], input=unsigned, capture_output=True,
                                   pass_fds=(descriptor,), env=git_environment(), timeout=remaining_seconds(deadline))
    if signature.returncode or not signature.stdout:
        deny('inspector_signing_failed')
    return (unsigned + b'.' + encode(signature.stdout)).decode('ascii')


def api_request(method, path, token, deadline, body=None):
    if (method not in {'GET', 'POST'} or not path.startswith('/') or path.startswith('//')
            or any(ord(character) < 32 for character in path)
            or (method == 'POST' and not re.fullmatch(r'/app/installations/[1-9][0-9]*/access_tokens', path))):
        deny('inspector_request_forbidden')
    return github_request(method, path, token, deadline, body)


def github_request(method, path, token, deadline, body=None):
    if (method not in {'GET', 'POST', 'PUT'} or not path.startswith('/') or path.startswith('//')
            or any(ord(character) < 32 for character in path)):
        deny('api_request_forbidden')
    if not isinstance(token, str) or not token or any(not 33 <= ord(character) <= 126 for character in token):
        deny('inspector_token_invalid')
    remaining = remaining_seconds(deadline)
    descriptor = os.memfd_create('cockpit-inspector-headers', os.MFD_CLOEXEC)
    with os.fdopen(descriptor, 'r+b') as header_file:
        arguments = [trusted_program('curl'), '--disable', '--silent', '--show-error', '--proto', '=https',
                     '--max-time', str(remaining), '--request', method,
                     '--header', '@-', '--header', 'Accept: application/vnd.github+json',
                     '--header', 'Cache-Control: no-cache',
                     '--header', 'X-GitHub-Api-Version: 2026-03-10', '--dump-header',
                     f'/proc/self/fd/{descriptor}', '--write-out', '\n%{http_code}']
        if body is not None:
            arguments += ['--header', 'Content-Type: application/json', '--data-binary', json.dumps(body)]
        arguments += ['--url', 'https://api.github.com' + path]
        result = subprocess.run(arguments, input=f'Authorization: Bearer {token}\n'.encode(), capture_output=True,
                                pass_fds=(descriptor,), env=git_environment(), timeout=remaining)
        header_file.seek(0)
        headers = {}
        for line in header_file.read().decode('iso-8859-1').splitlines():
            if line.startswith('HTTP/'):
                headers = {}
            elif ':' in line:
                key, value = line.split(':', 1)
                headers[key.lower()] = value.strip()
    remaining_seconds(deadline)
    if result.returncode == 28:
        deny('inspection_timeout')
    if result.returncode:
        deny('api_network_error')
    content, separator, status = result.stdout.rpartition(b'\n')
    if not separator or not re.fullmatch(rb'[0-9]{3}', status):
        deny('api_invalid_response')
    status = int(status)
    if status not in {200, 201}:
        return status, None, headers
    try:
        value = json.loads(content, object_pairs_hook=unique_object)
    except (ValueError, UnicodeError, AccessDenied):
        deny('api_invalid_response')
    remaining_seconds(deadline)
    return status, value, headers


def inspector_app(record):
    app = record.get('inspector_app')
    if not isinstance(app, dict):
        deny('inspector_unavailable')
    if app.get('setup_approved') is not True:
        deny('inspector_setup_unapproved')
    if (any(type(app.get(key)) is not int or app[key] <= 0 for key in ['app_id', 'installation_id', 'owner_id'])
            or not isinstance(app.get('client_id'), str) or not re.fullmatch(r'[A-Za-z0-9_]+', app['client_id'])
            or not isinstance(app.get('key_epoch'), str) or not app['key_epoch']):
        deny('invalid_inspector_enrollment')
    permissions = app.get('permissions')
    if permissions == {'metadata': 'read', 'administration': 'read'}:
        return app
    if (permissions == {'metadata': 'read', 'administration': 'write'}
            and app.get('administration_write_approved') is True):
        return app
    deny('inspector_grants_unapproved')


def bind_inspector(record, registry):
    app_id, installation_id = record.get('inspector_app_id'), record.get('installation_id')
    if type(app_id) is not int or type(installation_id) is not int:
        deny('inspector_unavailable')
    matches = [app for app in registry.get('inspector_apps', [])
               if app['app_id'] == app_id and app['installation_id'] == installation_id
               and app['owner_login'].lower() == record['canonical_name'].split('/')[0].lower()]
    if len(matches) != 1:
        deny('inspector_unavailable')
    if record['repository_id'] not in matches[0]['approved_repository_ids']:
        deny('inspector_repository_unapproved')
    bound = record | {'inspector_app': matches[0]}
    inspector_app(bound)
    return bound


def inspector_token(record, app, deadline):
    cache_key = (app['app_id'], app['client_id'], app['installation_id'], app['owner_id'],
                 record['repository_id'], app['key_epoch'], tuple(sorted(app['permissions'].items())))
    cached = APP_TOKENS.get(cache_key)
    if cached and cached['expires'] > time.time() + remaining_seconds(deadline):
        return cached['token'], cache_key, 0
    jwt = sign_app_jwt(app, deadline)
    status, minted, headers = api_request('POST', f'/app/installations/{app["installation_id"]}/access_tokens',
                                         jwt, deadline, {'repository_ids': [record['repository_id']],
                                                         'permissions': app['permissions']})
    if status != 201:
        deny(f'api_http_{status}')
    if (not isinstance(minted, dict) or minted.get('permissions') != app['permissions']
            or not isinstance(minted.get('repositories'), list)
            or len(minted['repositories']) != 1 or not isinstance(minted['repositories'][0], dict)
            or type(minted['repositories'][0].get('id')) is not int
            or minted['repositories'][0]['id'] != record['repository_id']
            or not isinstance(minted.get('token'), str) or not minted['token']
            or any(not 33 <= ord(character) <= 126 for character in minted['token'])):
        deny('inspector_token_invalid')
    try:
        expiry = datetime.fromisoformat(minted['expires_at'].replace('Z', '+00:00'))
        if expiry.tzinfo is None:
            deny('inspector_token_invalid')
        expires = expiry.timestamp()
    except (KeyError, TypeError, ValueError, AttributeError):
        deny('inspector_token_invalid')
    if not time.time() + remaining_seconds(deadline) < expires <= time.time() + 3660:
        deny('inspector_token_invalid')
    APP_TOKENS[cache_key] = {'token': minted['token'], 'expires': expires}
    return minted['token'], cache_key, 1


def paginated_read(path, get):
    rows = []
    page = 1
    while True:
        separator = '&' if '?' in path else '?'
        status, batch, headers = get(f'{path}{separator}per_page=100&page={page}')
        if status != 200:
            deny(f'api_http_{status}')
        if not isinstance(batch, list) or len(batch) > 100:
            deny('api_invalid_response')
        rows.extend(batch)
        links = headers.get('link', '')
        next_links = re.findall(r'<([^>]+)>;\s*rel="next"', links)
        if next_links:
            if len(next_links) != 1:
                deny('api_pagination_invalid')
            target = urlsplit(next_links[0])
            query = parse_qs(target.query)
            if (target.scheme != 'https' or target.netloc != 'api.github.com'
                    or target.path != path.split('?')[0] or target.fragment
                    or query.get('page') != [str(page + 1)]
                    or query.get('per_page') != ['100']):
                deny('api_pagination_invalid')
        elif 'next' in links or ('<' not in links and links):
            deny('api_pagination_invalid')
        if not next_links and len(batch) < 100:
            return rows
        page += 1


def read_protection_snapshot(record, deadline):
    app = inspector_app(record)
    token, cache_key, minted = inspector_token(record, app, deadline)
    counts = {'app_auth': minted, 'installation': 0}

    def get(path):
        counts['installation'] += 1
        status, body, headers = api_request('GET', path, token, deadline)
        if status in {401, 403}:
            APP_TOKENS.pop(cache_key, None)
        return status, body, headers

    base = '/repos/' + record['canonical_name']
    status, repository, headers = get(base)
    if status != 200:
        deny(f'api_http_{status}')
    if (not isinstance(repository, dict) or repository.get('id') != record['repository_id']
            or repository.get('full_name') != record['canonical_name']
            or not isinstance(repository.get('owner'), dict) or repository['owner'].get('id') != app['owner_id']):
        deny('remote_identity_mismatch')
    default = repository.get('default_branch')
    if not isinstance(default, str) or not default:
        deny('protection_uninspectable')
    baseline = record.get('baseline_rulesets')
    if (not isinstance(baseline, dict) or any(type(baseline.get(role)) is not int or baseline[role] <= 0
                                             for role in ['history', 'default_pr'])):
        deny('protection_missing')
    listed = paginated_read(base + '/rulesets?includes_parents=true', get)
    ids = {item.get('id') for item in listed if isinstance(item, dict) and type(item.get('id')) is int}
    if not {baseline['history'], baseline['default_pr']}.issubset(ids):
        deny('protection_missing')
    details = []
    for role in ['history', 'default_pr']:
        status, detail, headers = get(f'{base}/rulesets/{baseline[role]}?includes_parents=true')
        if status != 200:
            deny(f'api_http_{status}')
        details.append(detail)
    effective = {branch: paginated_read(f'{base}/rules/branches/{quote(branch, safe="")}', get)
                 for branch in [default, PROBE_BRANCH]}
    classic_status, classic, headers = get(f'{base}/branches/{quote(default, safe="")}/protection')
    remaining_seconds(deadline)
    return {'complete': True, 'repository': repository, 'rulesets': details, 'effective_rules': effective,
            'classic_protection_status': classic_status, 'classic_protection': classic, 'request_counts': counts}


def verify_protection_snapshot(record, snapshot):
    if not isinstance(snapshot, dict) or snapshot.get('complete') is not True:
        deny('protection_uninspectable')
    repository = snapshot.get('repository')
    if not isinstance(repository, dict):
        deny('protection_uninspectable')
    if (type(repository.get('id')) is not int or repository['id'] != record['repository_id']
            or repository.get('full_name') != record['canonical_name']):
        deny('remote_identity_mismatch')
    default_branch = repository.get('default_branch')
    if not isinstance(default_branch, str) or not default_branch:
        deny('protection_uninspectable')
    baseline_ids = record.get('baseline_rulesets')
    if (not isinstance(baseline_ids, dict)
            or any(type(baseline_ids.get(role)) is not int or baseline_ids[role] <= 0
                   for role in ['history', 'default_pr'])
            or baseline_ids['history'] == baseline_ids['default_pr']):
        deny('protection_missing')
    rulesets = snapshot.get('rulesets')
    if not isinstance(rulesets, list) or any(not isinstance(item, dict) for item in rulesets):
        deny('protection_uninspectable')
    for role, target, required_rules in [
        ('history', '~ALL', {'non_fast_forward', 'deletion'}),
        ('default_pr', '~DEFAULT_BRANCH', {'pull_request'}),
    ]:
        matches = [item for item in rulesets if item.get('id') == baseline_ids[role]]
        if len(matches) != 1:
            deny('protection_missing')
        ruleset = matches[0]
        if (ruleset.get('source_type') != 'Repository' or ruleset.get('source') != record['canonical_name']
                or ruleset.get('target') != 'branch' or ruleset.get('enforcement') != 'active'
                or ruleset.get('conditions') != {'ref_name': {'include': [target], 'exclude': []}}):
            deny('protection_missing')
        bypass = ruleset.get('bypass_actors')
        if not isinstance(bypass, list):
            deny('protection_uninspectable')
        if bypass:
            deny('protection_bypass')
        rules = ruleset.get('rules')
        if not isinstance(rules, list) or any(not isinstance(rule, dict) for rule in rules):
            deny('protection_uninspectable')
        if not required_rules.issubset({rule.get('type') for rule in rules if isinstance(rule.get('type'), str)}):
            deny('protection_missing')
        if role == 'default_pr':
            pull_rules = [rule for rule in rules if rule.get('type') == 'pull_request']
            if len(pull_rules) != 1 or not isinstance(pull_rules[0].get('parameters'), dict):
                deny('protection_uninspectable')
            parameters = pull_rules[0]['parameters']
            reviews = parameters.get('required_approving_review_count')
            if type(reviews) is not int or not 0 <= reviews <= 10:
                deny('protection_uninspectable')
            for key in ['dismiss_stale_reviews_on_push', 'require_code_owner_review',
                        'require_last_push_approval', 'required_review_thread_resolution']:
                if type(parameters.get(key)) is not bool:
                    deny('protection_uninspectable')
    effective = snapshot.get('effective_rules')
    if not isinstance(effective, dict):
        deny('protection_uninspectable')
    for branch, required in [
        (default_branch, {('deletion', baseline_ids['history']), ('non_fast_forward', baseline_ids['history']),
                          ('pull_request', baseline_ids['default_pr'])}),
        (PROBE_BRANCH, {('deletion', baseline_ids['history']), ('non_fast_forward', baseline_ids['history'])}),
    ]:
        rules = effective.get(branch)
        if not isinstance(rules, list):
            deny('protection_not_effective')
        observed = {(rule.get('type'), rule.get('ruleset_id')) for rule in rules if isinstance(rule, dict)
                    and isinstance(rule.get('type'), str) and type(rule.get('ruleset_id')) is int}
        if not required.issubset(observed):
            deny('protection_not_effective')
    if type(snapshot.get('classic_protection_status')) is not int or snapshot['classic_protection_status'] not in {200, 404}:
        deny('protection_uninspectable')
    if snapshot['classic_protection_status'] == 200:
        classic = snapshot.get('classic_protection')
        if (not isinstance(classic, dict) or not isinstance(classic.get('url'), str)
                or not classic['url'].startswith('https://api.github.com/')
                or not isinstance(classic.get('enforce_admins'), dict)
                or type(classic['enforce_admins'].get('enabled')) is not bool):
            deny('protection_uninspectable')
    return default_branch


def inspect_access(checkout, *, purpose):
    if purpose not in {'inspection', 'admission', 'scheduled', 'write'}:
        raise ValueError('invalid inspection purpose')
    report = {'mode': 'read_only', 'identity_verified': False}
    try:
        deadline = time.monotonic() + CHECK_DEADLINE_SECONDS
        root = checkout_root(checkout)
        registry, serialized = load_registry()
        record = next((record for record in registry['repositories']
                       if any(binding['path'] == str(root) for binding in record['checkouts'])), None)
        if record is None:
            deny('unregistered_project')
        digests = inspect_remote_chain(root, record, deadline, set())
        binding = next(binding for binding in record['checkouts'] if binding['path'] == str(root))
        report.update(identity_verified=True, project=record['project'], checkout=str(root),
                      git_dir=binding['git_dir'], common_dir=binding['common_dir'],
                      repository_id=record['repository_id'], canonical_name=record['canonical_name'],
                      configuration_digest=hashlib.sha256(''.join(digests).encode()).hexdigest())
        snapshot = read_protection_snapshot(bind_inspector(record, registry), deadline)
        default_branch = verify_protection_snapshot(record, snapshot)
        if registry_path().read_bytes() != serialized:
            deny('registry_changed')
        if inspect_remote_chain(root, record, deadline, set()) != digests:
            deny('git_metadata_changed')
        remaining_seconds(deadline)
        report.update(mode='verified', reason='protections_verified', default_branch=default_branch)
        if 'request_counts' in snapshot:
            report['request_counts'] = snapshot['request_counts']
    except AccessDenied as error:
        report['reason'] = str(error)
    except (OSError, UnicodeError):
        report['reason'] = 'identity_uninspectable'
    except subprocess.TimeoutExpired:
        report['reason'] = 'inspection_timeout'
    return report


def save_registry(registry, original):
    path = registry_path()
    descriptor = os.open(path.parent / 'git-repositories.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, 'rb') as lock:
        metadata = os.fstat(lock.fileno())
        if metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
            deny('untrusted_registry_permissions')
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            deny('registry_busy')
        load_registry()
        if path.read_bytes() != original:
            deny('registry_changed')
        replacement = None
        try:
            with tempfile.NamedTemporaryFile('w', dir=path.parent, prefix='.git-repositories.', delete=False) as pending:
                replacement = Path(pending.name)
                json.dump(registry, pending, indent=2)
                pending.write('\n')
                pending.flush()
                os.fsync(pending.fileno())
            if path.read_bytes() != original:
                deny('registry_changed')
            os.replace(replacement, path)
        finally:
            if replacement is not None and replacement.exists():
                replacement.unlink()


def enroll_repository(arguments):
    report = {'mode': 'read_only', 'enrollment_saved': False, 'rollout_enabled': False}
    try:
        if (not REPOSITORY_NAME.fullmatch(arguments.repository)
                or not re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9._-]*', arguments.project)
                or any(value <= 0 for value in [arguments.repository_id, arguments.inspector_app_id,
                                                arguments.history_ruleset, arguments.default_pr_ruleset])):
            deny('invalid_enrollment')
        registry, original = load_registry()
        if original is not None and registry_path().read_bytes() != original:
            deny('registry_changed')
        apps = [app for app in registry.get('inspector_apps', [])
                if app['app_id'] == arguments.inspector_app_id
                and app['owner_login'].lower() == arguments.repository.split('/')[0].lower()]
        if not apps:
            deny('inspector_unavailable')
        if len(apps) != 1:
            deny('ambiguous_inspector_enrollment')
        app = apps[0]
        if arguments.repository_id not in app['approved_repository_ids']:
            deny('inspector_repository_unapproved')
        root = checkout_root(arguments.checkout)
        existing = None
        for record in registry['repositories']:
            if record['project'] == arguments.project and record['repository_id'] != arguments.repository_id:
                deny('project_already_enrolled')
            if record['repository_id'] == arguments.repository_id:
                if record['project'] != arguments.project:
                    deny('repository_already_enrolled')
                existing = record
            elif any(binding['path'] == str(root) for binding in record['checkouts']):
                deny('checkout_already_enrolled')
        candidate = copy.deepcopy(existing) if existing else {
            'project': arguments.project, 'repository_id': arguments.repository_id, 'checkouts': [], 'mirror_edges': [],
        }
        candidate.pop('inspector_app', None)
        candidate.update(canonical_name=arguments.repository, inspector_app_id=app['app_id'],
                         installation_id=app['installation_id'],
                         baseline_rulesets={'history': arguments.history_ruleset, 'default_pr': arguments.default_pr_ruleset})
        git_dir, common_dir = metadata_locations(root)
        binding = {'path': str(root), 'git_dir': str(git_dir), 'common_dir': str(common_dir)}
        candidate['checkouts'] = [entry for entry in candidate['checkouts'] if entry['path'] != str(root)] + [binding]
        deadline = time.monotonic() + CHECK_DEADLINE_SECONDS
        digests = inspect_remote_chain(root, candidate, deadline, set())
        verify_protection_snapshot(candidate, read_protection_snapshot(bind_inspector(candidate, registry), deadline))
        confirmation = f'ENROLL {candidate["canonical_name"]} {candidate["repository_id"]} {root}'
        print('No write lease is being granted. To save this verified identity, type:\n' + confirmation, file=sys.stderr)
        if input() != confirmation:
            deny('enrollment_not_confirmed')
        if registry_path().read_bytes() != original:
            deny('registry_changed')
        deadline = time.monotonic() + CHECK_DEADLINE_SECONDS
        if inspect_remote_chain(root, candidate, deadline, set()) != digests:
            deny('git_metadata_changed')
        # Human confirmation can take minutes: never persist an earlier successful remote check.
        snapshot = read_protection_snapshot(bind_inspector(candidate, registry), deadline)
        default = verify_protection_snapshot(candidate, snapshot)
        if existing is None:
            registry['repositories'].append(candidate)
        else:
            registry['repositories'][registry['repositories'].index(existing)] = candidate
        save_registry(registry, original)
        report.update(mode='verified', reason='enrolled', enrollment_saved=True, project=candidate['project'],
                      repository_id=candidate['repository_id'], canonical_name=candidate['canonical_name'],
                      checkout=str(root), default_branch=default)
    except AccessDenied as error:
        report['reason'] = str(error)
    except EOFError:
        report['reason'] = 'enrollment_not_confirmed'
    except (OSError, UnicodeError):
        report['reason'] = 'enrollment_uninspectable'
    except subprocess.TimeoutExpired:
        report['reason'] = 'inspection_timeout'
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    inspect = commands.add_parser('inspect')
    inspect.add_argument('--checkout', required=True, type=Path)
    enroll = commands.add_parser('enroll')
    enroll.add_argument('--checkout', required=True, type=Path)
    enroll.add_argument('--repository', required=True)
    enroll.add_argument('--project', required=True)
    enroll.add_argument('--repository-id', required=True, type=int)
    enroll.add_argument('--inspector-app-id', required=True, type=int)
    enroll.add_argument('--history-ruleset', required=True, type=int)
    enroll.add_argument('--default-pr-ruleset', required=True, type=int)
    arguments = parser.parse_args()
    report = (enroll_repository(arguments) if arguments.command == 'enroll'
              else inspect_access(arguments.checkout, purpose='inspection'))
    print(json.dumps(report, sort_keys=True))
    return 0 if report['mode'] == 'verified' else 1


if __name__ == '__main__':
    raise SystemExit(main())
