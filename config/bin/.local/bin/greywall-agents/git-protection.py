#!/usr/bin/python3 -I
"""Host-only additive protection plans and explicitly confirmed remediation."""

import copy
import argparse
import importlib.util
import json
import hashlib
import os
from pathlib import Path
import time
import subprocess
import sys


def floor_body(role):
    if role == 'history':
        name = 'agents: preserve all branch history'
        include = '~ALL'
        rules = [{'type': 'deletion'}, {'type': 'non_fast_forward'}]
    elif role == 'default_pr':
        name = 'agents: require default branch pull requests'
        include = '~DEFAULT_BRANCH'
        rules = [{'type': 'pull_request', 'parameters': {
            'required_approving_review_count': 0, 'dismiss_stale_reviews_on_push': False,
            'require_code_owner_review': False, 'require_last_push_approval': False,
            'required_review_thread_resolution': False,
        }}]
    else:
        raise ValueError('unknown_baseline_role')
    return {'name': name, 'target': 'branch', 'enforcement': 'active', 'bypass_actors': [],
            'conditions': {'ref_name': {'include': [include], 'exclude': []}}, 'rules': rules}


def plan_baselines(repository, existing, owned_ids):
    """Return mutations only for numeric-ID-owned baselines; never adopt by name."""
    if (not isinstance(existing, list) or not isinstance(owned_ids, dict)
            or set(owned_ids) - {'history', 'default_pr'}
            or any(type(value) is not int or value <= 0 for value in owned_ids.values())
            or len(set(owned_ids.values())) != len(owned_ids)):
        raise ValueError('invalid_baseline_ownership')
    if any(not isinstance(entry, dict) or type(entry.get('id')) is not int for entry in existing):
        raise ValueError('uninspectable_rulesets')
    if len({entry['id'] for entry in existing}) != len(existing):
        raise ValueError('ruleset_id_ambiguous')
    plan = []
    for role in ['history', 'default_pr']:
        floor = floor_body(role)
        named = [entry for entry in existing if entry.get('name') == floor['name']]
        if len(named) > 1:
            raise ValueError('ruleset_name_ambiguous')
        owned = next((entry for entry in existing if entry['id'] == owned_ids.get(role)), None)
        if named and named[0] is not owned:
            raise ValueError('unowned_ruleset_name_collision')
        if owned is None:
            if role in owned_ids:
                raise ValueError('owned_ruleset_missing')
            plan.append({'role': role, 'method': 'POST', 'body': floor})
            continue
        if (owned.get('source_type') != 'Repository' or owned.get('source') != repository
                or owned.get('name') != floor['name'] or owned.get('target') != 'branch'
                or owned.get('conditions') != floor['conditions']
                or owned.get('enforcement') not in {'active', 'disabled', 'evaluate'}
                or not isinstance(owned.get('bypass_actors'), list)
                or not isinstance(owned.get('rules'), list)):
            raise ValueError('owned_ruleset_requires_review')
        rules = owned['rules']
        if (any(not isinstance(rule, dict) or not isinstance(rule.get('type'), str) for rule in rules)
                or len({rule['type'] for rule in rules}) != len(rules)):
            raise ValueError('uninspectable_rules')
        body = copy.deepcopy({key: owned[key] for key in floor})
        body.update(enforcement='active', bypass_actors=[])
        for required in floor['rules']:
            current = next((rule for rule in rules if rule['type'] == required['type']), None)
            if current is None:
                body['rules'].append(copy.deepcopy(required))
            elif required['type'] == 'pull_request':
                parameters = current.get('parameters')
                if (not isinstance(parameters, dict)
                        or type(parameters.get('required_approving_review_count')) is not int
                        or not 0 <= parameters['required_approving_review_count'] <= 10
                        or any(type(parameters.get(key)) is not bool for key in required['parameters']
                               if key != 'required_approving_review_count')):
                    raise ValueError('uninspectable_pull_request_rule')
        if any(body[key] != owned[key] for key in body):
            plan.append({'role': role, 'method': 'PUT', 'id': owned['id'], 'body': body})
    return plan


def preview_protection(access, project, deadline=None):
    """Inspect using the dedicated App; return a plan, never write authorization."""
    if deadline is None:
        deadline = time.monotonic() + access.CHECK_DEADLINE_SECONDS
    registry, serialized = access.load_registry()
    record = next((entry for entry in registry['repositories'] if entry['project'] == project), None)
    if record is None:
        access.deny('unregistered_project')
    root = access.checkout_root(Path(record['checkouts'][0]['path']))
    digests = access.inspect_remote_chain(root, record, deadline, set())
    bound = access.bind_inspector(record, registry)
    app = access.inspector_app(bound)
    token, cache_key, minted = access.inspector_token(bound, app, deadline)

    def get(path):
        status, body, headers = access.api_request('GET', path, token, deadline)
        if status in {401, 403}:
            access.APP_TOKENS.pop(cache_key, None)
        return status, body, headers

    base = '/repos/' + record['canonical_name']
    status, repository, headers = get(base)
    if status != 200:
        access.deny(f'api_http_{status}')
    if (not isinstance(repository, dict) or type(repository.get('id')) is not int
            or repository['id'] != record['repository_id']
            or repository.get('full_name') != record['canonical_name']
            or not isinstance(repository.get('owner'), dict)
            or repository['owner'].get('id') != app['owner_id']):
        access.deny('remote_identity_mismatch')
    listed = access.paginated_read(base + '/rulesets?includes_parents=true', get)
    details = []
    for entry in listed:
        if not isinstance(entry, dict) or type(entry.get('id')) is not int or entry['id'] <= 0:
            access.deny('protection_uninspectable')
        status, detail, headers = get(f'{base}/rulesets/{entry["id"]}?includes_parents=true')
        if status != 200:
            access.deny(f'api_http_{status}')
        if not isinstance(detail, dict) or detail.get('id') != entry['id']:
            access.deny('protection_uninspectable')
        details.append(detail)
    mutations = plan_baselines(record['canonical_name'], details, record.get('baseline_rulesets', {}))
    if access.registry_path().read_bytes() != serialized:
        access.deny('registry_changed')
    if access.inspect_remote_chain(root, record, deadline, set()) != digests:
        access.deny('git_metadata_changed')
    access.remaining_seconds(deadline)
    return {'mode': 'preview', 'reason': 'protection_plan', 'project': project,
            'repository_id': record['repository_id'], 'canonical_name': record['canonical_name'],
            'before': details, 'mutations': mutations, 'rollout_enabled': False,
            'registry_digest': hashlib.sha256(serialized).hexdigest(),
            'configuration_digest': hashlib.sha256(''.join(digests).encode()).hexdigest()}


def admin_mutation(access, record, mutation, deadline):
    if (not access.REPOSITORY_NAME.fullmatch(record['canonical_name'])
            or type(record['repository_id']) is not int or record['repository_id'] <= 0
            or mutation.get('method') not in {'POST', 'PUT'}
            or not isinstance(mutation.get('body'), dict)
            or (mutation['method'] == 'PUT' and (type(mutation.get('id')) is not int or mutation['id'] <= 0))):
        access.deny('admin_request_forbidden')
    environment = access.git_environment()
    environment.update(PATH=os.defpath, GH_CONFIG_DIR=str(Path.home() / '.config/gh'), GH_PROMPT_DISABLED='1')
    credential = subprocess.run([access.trusted_program('gh'), 'auth', 'token', '--hostname', 'github.com'],
                                env=environment, capture_output=True, timeout=access.remaining_seconds(deadline))
    if credential.returncode:
        access.deny('admin_authentication_unavailable')
    try:
        token = credential.stdout.decode('ascii').strip()
    except UnicodeError:
        access.deny('admin_authentication_unavailable')
    base = '/repos/' + record['canonical_name']
    status, repository, headers = access.github_request('GET', base, token, deadline)
    if status != 200:
        access.deny(f'api_http_{status}')
    if (not isinstance(repository, dict) or type(repository.get('id')) is not int
            or repository['id'] != record['repository_id'] or repository.get('full_name') != record['canonical_name']):
        access.deny('remote_identity_mismatch')
    path = base + '/rulesets' + (f'/{mutation["id"]}' if mutation['method'] == 'PUT' else '')
    status, result, headers = access.github_request(mutation['method'], path, token, deadline, mutation['body'])
    if status != (201 if mutation['method'] == 'POST' else 200):
        access.deny(f'api_http_{status}')
    return result


def apply_protection(access, project):
    reviewed = copy.deepcopy(preview_protection(access, project))
    print(json.dumps(reviewed, indent=2, sort_keys=True), file=sys.stderr)
    confirmation = f'APPLY {reviewed["canonical_name"]} {reviewed["repository_id"]}'
    print('Host administration only; no sandbox lease. To apply this exact plan, type:\n' + confirmation, file=sys.stderr)
    try:
        if input() != confirmation:
            access.deny('remediation_not_confirmed')
    except EOFError:
        access.deny('remediation_not_confirmed')
    expected = reviewed
    for mutation in reviewed['mutations']:
        deadline = time.monotonic() + access.CHECK_DEADLINE_SECONDS
        current = preview_protection(access, project, deadline)
        if current != expected:
            access.deny('remediation_plan_changed')
        registry, serialized = access.load_registry()
        if hashlib.sha256(serialized).hexdigest() != current['registry_digest']:
            access.deny('registry_changed')
        record = next(entry for entry in registry['repositories'] if entry['project'] == project)
        result = admin_mutation(access, record, mutation, deadline)
        if (not isinstance(result, dict) or type(result.get('id')) is not int
                or result['id'] <= 0
                or result.get('source_type') != 'Repository' or result.get('source') != record['canonical_name']
                or any(result.get(key) != value for key, value in mutation['body'].items())):
            access.deny('remediation_outcome_unverified')
        ruleset_id = result['id']
        if mutation['method'] == 'PUT' and ruleset_id != mutation['id']:
            access.deny('remediation_outcome_unverified')
        expected_registry_digest = current['registry_digest']
        if mutation['method'] == 'POST':
            if any(entry['id'] == ruleset_id for entry in current['before']):
                access.deny('remediation_outcome_unverified')
            record.setdefault('baseline_rulesets', {})[mutation['role']] = ruleset_id
            access.save_registry(registry, serialized)
            updated, serialized = access.load_registry()
            if updated != registry:
                access.deny('registry_changed')
            expected_registry_digest = hashlib.sha256(serialized).hexdigest()
        after = preview_protection(access, project, deadline)
        observed = next((entry for entry in after['before'] if entry['id'] == ruleset_id), None)
        if observed is None or any(observed.get(key) != value for key, value in mutation['body'].items()):
            access.deny('remediation_outcome_unverified')
        if (after['registry_digest'] != expected_registry_digest
                or after['configuration_digest'] != current['configuration_digest']
                or after['mutations'] != current['mutations'][1:]):
            access.deny('remediation_outcome_unverified')
        before_others = [entry for entry in current['before'] if entry['id'] != ruleset_id]
        after_others = [entry for entry in after['before'] if entry['id'] != ruleset_id]
        if before_others != after_others:
            access.deny('remediation_plan_changed')
        expected = after
    registry, serialized = access.load_registry()
    if hashlib.sha256(serialized).hexdigest() != expected['registry_digest']:
        access.deny('registry_changed')
    record = next(entry for entry in registry['repositories'] if entry['project'] == project)
    report = access.inspect_access(Path(record['checkouts'][0]['path']), purpose='inspection')
    if report['mode'] != 'verified':
        access.deny(report['reason'])
    return report | {'rollout_enabled': False}


def main():
    parser = argparse.ArgumentParser(description='Preview additive protection changes; applying requires explicit host confirmation.')
    parser.add_argument('project')
    parser.add_argument('--apply', action='store_true', help='Apply the reviewed plan using the host admin login, then re-inspect.')
    arguments = parser.parse_args()
    path = Path(__file__).with_name('git-access.py')
    spec = importlib.util.spec_from_file_location('cockpit_git_access', path)
    access = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(access)
    try:
        report = (apply_protection(access, arguments.project) if arguments.apply
                  else preview_protection(access, arguments.project))
    except (access.AccessDenied, ValueError) as error:
        report = {'mode': 'read_only', 'reason': str(error), 'rollout_enabled': False}
    except (OSError, UnicodeError):
        report = {'mode': 'read_only', 'reason': 'protection_uninspectable', 'rollout_enabled': False}
    except subprocess.TimeoutExpired:
        report = {'mode': 'read_only', 'reason': 'inspection_timeout', 'rollout_enabled': False}
    if arguments.apply and report['mode'] != 'verified':
        report['upstream_outcome'] = 'unknown_or_partial; reconcile read-only before retrying'
    print(json.dumps(report, sort_keys=True))
    return 0 if report['mode'] in {'preview', 'verified'} else 1


if __name__ == '__main__':
    raise SystemExit(main())
