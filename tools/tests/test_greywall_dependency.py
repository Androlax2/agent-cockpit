import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


HELPER = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents/git-access.py'


class GreywallDependencyTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(dir='/tmp/opencode', prefix='greywall-dependency-')
        self.addCleanup(self.scratch.cleanup)
        self.home = Path(self.scratch.name)
        self.executable = self.home / '.local/bin/greywall'
        self.executable.parent.mkdir(parents=True)

    def check(self, **environment):
        return subprocess.run([sys.executable, '-I', str(HELPER), 'check-greywall'],
                              capture_output=True, text=True, env=os.environ | {'HOME': str(self.home)} | environment)

    def assert_denied(self, result):
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout)['reason'], 'read_only_dependency_missing')
        self.assertEqual(json.loads(result.stdout)['mode'], 'read_only')
        self.assertEqual(result.stderr, '')

    def test_missing_binary_is_a_named_read_only_dependency_failure(self):
        self.assert_denied(self.check())

    def test_unrecognized_binary_is_not_executed_even_if_it_claims_the_required_version(self):
        marker = self.home / 'executed'
        self.executable.write_text(f'#!/bin/sh\ntouch {marker}\necho "Commit: 4a0601220357e5e29228d20872f5574dd24a45d8"\n')
        self.executable.chmod(0o700)
        self.assert_denied(self.check())
        self.assertFalse(marker.exists())

    def test_environment_cannot_authorize_an_unverified_binary(self):
        self.executable.write_bytes(b'unverified binary')
        self.executable.chmod(0o700)
        self.assert_denied(self.check(GREYWALL_SHA256=hashlib.sha256(self.executable.read_bytes()).hexdigest(),
                                      GREYWALL_READ_ONLY_VERIFIED='1'))

    def test_only_a_trusted_exact_build_can_report_the_dependency_verified(self):
        self.executable.write_bytes(b'synthetic canary-tested fixture binary')
        self.executable.chmod(0o700)
        spec = importlib.util.spec_from_file_location('cockpit_test_greywall_dependency', HELPER)
        access = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(access)
        self.assertTrue(hasattr(access, 'VERIFIED_GREYWALL_BUILDS'), 'no canary-bound build allowlist exists')
        fingerprint = hashlib.sha256(self.executable.read_bytes()).hexdigest()
        os.utime(self.executable, ns=(1, self.executable.stat().st_mtime_ns))
        with patch.dict(access.VERIFIED_GREYWALL_BUILDS, {fingerprint: 'fixture-revision'}, clear=True), \
                patch.dict(os.environ, {'HOME': str(self.home)}), \
                patch.object(sys, 'argv', [str(HELPER), 'check-greywall']), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(access.main(), 0)
        report = json.loads(output.getvalue())
        self.assertEqual(report['mode'], 'verified')
        self.assertEqual(report['greywall_sha256'], fingerprint)
        self.assertEqual(report['greywall_revision'], 'fixture-revision')

    def test_reviewed_bytes_do_not_authorize_symlinks_or_unsafe_permissions(self):
        payload = b'synthetic reviewed build'
        fingerprint = hashlib.sha256(payload).hexdigest()
        spec = importlib.util.spec_from_file_location('cockpit_test_greywall_dependency', HELPER)
        access = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(access)
        target = self.home / 'actual-binary'
        target.write_bytes(payload)
        target.chmod(0o700)
        for scenario in ['not-executable', 'group-writable', 'other-writable', 'symlink']:
            with self.subTest(scenario=scenario):
                if self.executable.exists() or self.executable.is_symlink():
                    self.executable.unlink()
                if scenario == 'symlink':
                    self.executable.symlink_to(target)
                else:
                    self.executable.write_bytes(payload)
                    self.executable.chmod({'not-executable': 0o600, 'group-writable': 0o720,
                                           'other-writable': 0o702}[scenario])
                with patch.dict(access.VERIFIED_GREYWALL_BUILDS, {fingerprint: 'fixture-revision'}, clear=True), \
                        patch.dict(os.environ, {'HOME': str(self.home)}), \
                        patch.object(sys, 'argv', [str(HELPER), 'check-greywall']), \
                        contextlib.redirect_stdout(io.StringIO()) as output:
                    self.assertEqual(access.main(), 1)
                self.assertEqual(json.loads(output.getvalue())['reason'], 'read_only_dependency_missing')


if __name__ == '__main__':
    unittest.main()
