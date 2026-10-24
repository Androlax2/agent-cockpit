import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


HELPERS = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents'


class SessionEntryTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix='git-entry-', dir='/tmp/opencode')
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.shims = self.root / 'shims'
        self.shims.mkdir()
        # The exact set the launcher copies into the sandbox's shims folder.
        for name in ['git-session-entry.py', 'git-broker.py', 'git-adapter.py', 'github-actions.py', 'gh-compat.py']:
            shutil.copy(HELPERS / name, self.shims / name)
        self.channel = self.root / 'channel'
        self.channel.mkdir(mode=0o700)
        (self.channel / 'capability').write_text('c' * 64)
        (self.channel / 'capability').chmod(0o600)
        self.seen = self.root / 'seen.json'
        self.agent = [sys.executable, '-c', f'import json, os; json.dump(dict(os.environ), open({str(self.seen)!r}, "w"))']

    def run_entry(self, environment):
        base = {'PATH': os.environ['PATH'], 'HOME': str(self.root), 'TMPDIR': str(self.root),
                'GH_TOKEN': 'SYNTHETIC_WRITE', 'GITHUB_TOKEN': 'SYNTHETIC_WRITE', 'GIT_CONFIG_COUNT': '1',
                'GIT_CONFIG_KEY_0': 'credential.helper', 'GIT_CONFIG_VALUE_0': 'store'} | environment
        return subprocess.run([sys.executable, '-I', str(self.shims / 'git-session-entry.py'), *self.agent],
                              env=base, capture_output=True, text=True, timeout=30)

    def broker_environment(self):
        return {'COCKPIT_GIT_SOCKET': str(self.channel / 'broker.sock'),
                'COCKPIT_GIT_CAPABILITY_FILE': str(self.channel / 'capability'),
                'COCKPIT_GITHUB_REPOSITORY': 'acme/project'}

    def routes(self):
        seen = json.loads(self.seen.read_text())
        return {(seen[f'GIT_CONFIG_KEY_{index}'], seen[f'GIT_CONFIG_VALUE_{index}'])
                for index in range(int(seen.get('GIT_CONFIG_COUNT', 0)))}

    def test_verified_session_starts_adapter_and_routes_only_the_enrolled_repository(self):
        result = self.run_entry(self.broker_environment())
        self.assertEqual(result.returncode, 0, result.stderr)
        rewrites = {value: key for key, value in self.routes() if key.endswith('.insteadOf')}
        self.assertIn('https://github.com/acme/project.git', rewrites)
        self.assertRegex(rewrites['https://github.com/acme/project.git'],
                         r'^url\.http://127\.0\.0\.1:[0-9]+/repo\.git\.insteadOf$')
        self.assertFalse(any('other' in value for value in rewrites))
        self.assertIn(('credential.helper', ''), self.routes())
        self.assertNotIn(('credential.helper', 'store'), self.routes())

    def test_write_tokens_never_reach_the_agent(self):
        for environment in [self.broker_environment(), {}]:
            with self.subTest(verified=bool(environment)):
                self.run_entry(environment)
                seen = json.loads(self.seen.read_text())
                for name in ['GH_TOKEN', 'GITHUB_TOKEN', 'GH_ENTERPRISE_TOKEN', 'GITHUB_ENTERPRISE_TOKEN', 'GH_PROJECT_TOKEN']:
                    self.assertNotIn(name, seen)

    def test_read_only_session_starts_no_adapter_and_no_redirect(self):
        result = self.run_entry({})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any(key.endswith('.insteadOf') for key, _ in self.routes()))

    def test_adapter_failure_keeps_the_agent_usable_without_write_routing(self):
        (self.channel / 'capability').write_text('not a capability')
        result = self.run_entry(self.broker_environment() | {'COCKPIT_GIT_ADAPTER_READY_SECONDS': '2'})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Git push through the broker is unavailable', result.stderr)
        self.assertFalse(any(key.endswith('.insteadOf') for key, _ in self.routes()))

    def test_adapter_loopback_bypasses_the_sandbox_proxy(self):
        result = self.run_entry(self.broker_environment() | {'http_proxy': 'http://proxy:1', 'HTTP_PROXY': 'http://proxy:1',
                                                             'NO_PROXY': 'internal.example', 'no_proxy': 'internal.example'})
        self.assertEqual(result.returncode, 0, result.stderr)
        seen = json.loads(self.seen.read_text())
        for name in ['NO_PROXY', 'no_proxy']:
            entries = seen[name].split(',')
            self.assertIn('127.0.0.1', entries)
            self.assertIn('internal.example', entries)
        self.assertEqual(seen['http_proxy'], 'http://proxy:1')

    def test_agent_exit_status_is_preserved(self):
        self.agent = [sys.executable, '-c', 'import sys; sys.exit(9)']
        self.assertEqual(self.run_entry(self.broker_environment()).returncode, 9)


class AdapterSpoolTests(unittest.TestCase):
    def test_adapter_spools_privately_not_beside_the_read_only_capability(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location('spool_adapter', HELPERS / 'git-adapter.py')
        adapter = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(adapter)
        with tempfile.TemporaryDirectory(dir='/tmp/opencode') as scratch:
            channel = Path(scratch) / 'channel'
            channel.mkdir(mode=0o700)
            capability = channel / 'capability'
            capability.write_text('c' * 64)
            capability.chmod(0o600)
            channel.chmod(0o500)
            try:
                server = adapter.make_server(channel / 'broker.sock', capability)
                try:
                    self.assertNotEqual(Path(server.spool), channel)
                    with tempfile.TemporaryFile(dir=server.spool) as spooled:
                        spooled.write(b'0000')
                    self.assertEqual(os.stat(server.spool).st_mode & 0o077, 0)
                finally:
                    server.server_close()
            finally:
                channel.chmod(0o700)


if __name__ == '__main__':
    unittest.main()
