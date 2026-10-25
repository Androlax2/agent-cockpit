import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import unittest
from unittest.mock import patch


HELPERS = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents'


def load(name, alias):
    spec = importlib.util.spec_from_file_location(alias, HELPERS / name)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class PrepareTests(unittest.TestCase):
    def setUp(self):
        self.access = load('git-access.py', 'prepare_access')
        self.broker = load('git-broker.py', 'prepare_broker')
        self.scratch = tempfile.TemporaryDirectory(prefix='git-prepare-', dir='/tmp/opencode')
        self.addCleanup(self.scratch.cleanup)
        root = Path(self.scratch.name)
        self.runtime, self.home, self.checkout = root / 'run', root / 'home', root / 'checkout'
        for directory in [self.runtime, self.home, self.checkout]:
            directory.mkdir(mode=0o700)
        registry = self.home / '.config/greywall-agents/git-repositories.json'
        registry.parent.mkdir(parents=True, mode=0o700)
        registry.write_text('{"version": 1, "repositories": [], "inspector_apps": []}')
        registry.chmod(0o600)
        environment = patch.dict(os.environ, {'HOME': str(self.home), 'XDG_RUNTIME_DIR': str(self.runtime)})
        environment.start()
        self.addCleanup(environment.stop)
        self.report = {'mode': 'verified', 'identity_verified': True, 'rate_verified': True,
                       'credential_roles_verified': ['read', 'forward'], 'checkout': str(self.checkout),
                       'repository_id': 42, 'canonical_name': 'acme/project', 'git_dir': str(self.checkout / '.git'),
                       'common_dir': str(self.checkout / '.git'), 'configuration_digest': '3' * 64,
                       'default_branch': 'main'}

    def prepare(self):
        output = io.StringIO()
        with patch.object(self.access, 'inspect_access', return_value=self.report) as inspect, \
                patch.object(sys, 'argv', ['git-access.py', 'prepare', '--checkout', str(self.checkout)]), \
                contextlib.redirect_stdout(output):
            status = self.access.main()
        return status, output.getvalue(), inspect

    def test_verified_admission_writes_a_binding_the_broker_accepts(self):
        status, output, inspect = self.prepare()
        self.assertEqual(status, 0)
        self.assertEqual(inspect.call_args.kwargs, {'purpose': 'admission'})
        result = json.loads(output)
        self.assertEqual((result['mode'], result['canonical_name']), ('verified', 'acme/project'))
        decision = self.runtime / 'greywall-agents/git-access' / (result['session'] + '.json')
        binding = self.broker.read_binding(decision)
        self.assertEqual((binding['repository_id'], binding['mode'], binding['revoked']), (42, 'verified', False))
        channel = Path(result['channel_dir'])
        self.assertEqual(channel, self.runtime / 'greywall-git-channel' / result['session'])
        self.assertEqual((channel / 'capability').read_text(), binding['capability'])
        for path in [decision, channel / 'capability']:
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(channel.stat().st_mode), 0o700)

    def test_capability_and_credentials_never_appear_in_output(self):
        _, output, _ = self.prepare()
        result = json.loads(output)
        capability = (Path(result['channel_dir']) / 'capability').read_text()
        self.assertNotIn(capability, output)
        self.assertEqual(set(result), {'mode', 'session', 'channel_dir', 'canonical_name'})

    def test_denied_or_incomplete_admission_creates_no_channel(self):
        for change in [{'mode': 'read_only', 'reason': 'protection_uninspectable'}, {'identity_verified': False},
                       {'credential_roles_verified': ['read']}, {'rate_verified': None}]:
            with self.subTest(change=change):
                self.report = self.report | change
                status, output, _ = self.prepare()
                self.assertEqual(status, 0)
                result = json.loads(output)
                self.assertEqual(result['mode'], 'read_only')
                self.assertRegex(result['reason'], r'^[a-z_]+$')
                self.assertFalse((self.runtime / 'greywall-git-channel').exists())
                self.assertFalse((self.runtime / 'greywall-agents/git-access').exists())

    def test_each_launch_gets_a_fresh_session_and_capability(self):
        first, second = json.loads(self.prepare()[1]), json.loads(self.prepare()[1])
        self.assertNotEqual(first['session'], second['session'])
        self.assertNotEqual((Path(first['channel_dir']) / 'capability').read_text(),
                            (Path(second['channel_dir']) / 'capability').read_text())


if __name__ == '__main__':
    unittest.main()
