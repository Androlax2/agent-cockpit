#!/usr/bin/python3 -I
"""Sandbox entry: route the enrolled repository through the broker adapter, then become the agent.

Runs inside the sandbox before the agent. Write tokens are dropped either way. Without a broker
channel (read-only admission) the agent starts with no redirect; GitHub writes then have no
credentials. Usage: git-session-entry.py <agent command...>
"""

import importlib.util
import json
import os
from pathlib import Path
import select
import subprocess
import sys


WRITE_TOKENS = ('GH_TOKEN', 'GITHUB_TOKEN', 'GH_ENTERPRISE_TOKEN', 'GITHUB_ENTERPRISE_TOKEN', 'GH_PROJECT_TOKEN')


def broker_routing(environment):
    """Start the adapter and return the Git overlay environment, or None when routing is unavailable."""
    adapter_path = Path(__file__).with_name('git-adapter.py')
    spec = importlib.util.spec_from_file_location('cockpit_session_adapter', adapter_path)
    adapter = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(adapter)
    ready_seconds = float(environment.get('COCKPIT_GIT_ADAPTER_READY_SECONDS', '15'))
    # The adapter outlives this exec: its parent becomes the agent, and it exits with it.
    process = subprocess.Popen([sys.executable, '-I', str(adapter_path), 'serve',
                                '--socket', environment['COCKPIT_GIT_SOCKET'],
                                '--capability-file', environment['COCKPIT_GIT_CAPABILITY_FILE']],
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    if not select.select([process.stdout], [], [], ready_seconds)[0]:
        process.kill()
        return None
    line = process.stdout.readline()
    try:
        url = json.loads(line)['url']
        return adapter.git_overlay(environment, environment['COCKPIT_GITHUB_REPOSITORY'], url)
    except (ValueError, KeyError, TypeError, adapter.AdapterDenied):
        process.kill()
        return None


def main():
    command = sys.argv[1:]
    if not command:
        print('git-session-entry: no agent command', file=sys.stderr)
        return 2
    environment = {key: value for key, value in os.environ.items() if key not in WRITE_TOKENS}
    if all(environment.get(name) for name in
           ['COCKPIT_GIT_SOCKET', 'COCKPIT_GIT_CAPABILITY_FILE', 'COCKPIT_GITHUB_REPOSITORY']):
        routed = broker_routing(environment)
        if routed is None:
            print('git-session-entry: Git push through the broker is unavailable for this session; '
                  'reads still work. Relaunch to retry.', file=sys.stderr)
        else:
            environment = routed
    os.execvpe(command[0], command, environment)


if __name__ == '__main__':
    raise SystemExit(main())
