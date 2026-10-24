import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import textwrap
import threading
import time
import unittest
from unittest.mock import patch


HELPERS = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents'


class SupervisorTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location('supervisor_access', HELPERS / 'git-access.py')
        self.access = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.access)
        self.scratch = tempfile.TemporaryDirectory(prefix='git-supervisor-', dir='/tmp/opencode')
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        runtime = self.root / 'run'
        runtime.mkdir(mode=0o700)
        environment = patch.dict(os.environ, {'XDG_RUNTIME_DIR': str(runtime), 'HOME': str(self.root)})
        environment.start()
        self.addCleanup(environment.stop)
        self.session = 'a' * 32
        self.control = runtime / 'greywall-agents/git-access'
        self.channel = runtime / 'greywall-git-channel' / self.session
        for directory in [self.control, self.channel]:
            directory.mkdir(parents=True, mode=0o700)
        self.binding = {'version': 1, 'mode': 'verified', 'session': self.session, 'generation': 'b' * 32,
                        'capability': 'c' * 64, 'revoked': False, 'checkout': str(self.root), 'repository_id': 42,
                        'canonical_name': 'acme/project', 'git_dir': str(self.root / '.git'),
                        'common_dir': str(self.root / '.git'), 'configuration_digest': '3' * 64,
                        'registry_digest': '4' * 64, 'checked_at': time.monotonic(), 'checked_wall': time.time(),
                        'boot_id': 'boot'}
        self.decision = self.control / (self.session + '.json')
        self.decision.write_text(json.dumps(self.binding))
        self.decision.chmod(0o600)
        (self.channel / 'capability').write_text('c' * 64)
        self.broker_log = self.root / 'broker-log'
        self.fake_broker = [sys.executable, '-c', textwrap.dedent(f'''
            import signal, sys, time
            open({str(self.broker_log)!r}, 'a').write('started\\n')
            signal.signal(signal.SIGTERM, lambda *_: (open({str(self.broker_log)!r}, 'a').write('stopped\\n'), sys.exit(0)))
            print('{{"socket": "ready"}}', flush=True)
            time.sleep(float(sys.argv[1]) if len(sys.argv) > 1 else 60)
            sys.exit(3)
        ''')]
        self.report = {'mode': 'verified', 'identity_verified': True, 'rate_verified': True,
                       'credential_roles_verified': ['read', 'forward'],
                       **{field: self.binding[field] for field in ['checkout', 'repository_id', 'canonical_name',
                                                                   'git_dir', 'common_dir', 'configuration_digest']}}
        self.inspections = []

    def inspect(self, checkout, *, purpose):
        self.inspections.append(purpose)
        return self.report

    def supervise(self, command, **options):
        settings = {'check_interval': 0.3, 'inspection_timeout': 1.0, 'hard_expiry': 5.0,
                    'broker_command': self.fake_broker, 'inspect': self.inspect} | options
        return self.access.supervise(self.session, command, **settings)

    def binding_now(self):
        return json.loads(self.decision.read_text())

    def test_sandbox_exit_status_is_returned_and_broker_stopped(self):
        status = self.supervise([sys.executable, '-c', 'import sys; sys.exit(5)'])
        self.assertEqual(status, 5)
        self.assertEqual(self.broker_log.read_text(), 'started\nstopped\n')
        self.assertFalse((self.channel / 'capability').exists())

    def test_passing_scheduled_checks_renew_only_the_check_times(self):
        before = self.binding_now()
        status = self.supervise([sys.executable, '-c', 'import time; time.sleep(1.2)'])
        self.assertEqual(status, 0)
        after = self.binding_now()
        self.assertIn('scheduled', self.inspections)
        self.assertGreater(after['checked_at'], before['checked_at'])
        self.assertEqual({key: value for key, value in after.items() if not key.startswith('checked_')},
                         {key: value for key, value in before.items() if not key.startswith('checked_')})

    def test_first_failed_check_revokes_and_contains_immediately(self):
        self.report = {'mode': 'read_only', 'reason': 'protection_uninspectable'}
        started = time.monotonic()
        status = self.supervise([sys.executable, '-c', 'import time; time.sleep(60)'])
        self.assertEqual(status, 77)
        self.assertLess(time.monotonic() - started, 5)
        self.assertTrue(self.binding_now()['revoked'])

    def test_identity_change_in_a_passing_report_is_treated_as_failure(self):
        self.report = self.report | {'repository_id': 99}
        status = self.supervise([sys.executable, '-c', 'import time; time.sleep(60)'])
        self.assertEqual(status, 77)
        self.assertTrue(self.binding_now()['revoked'])

    def test_hung_inspection_is_contained_at_the_check_deadline(self):
        def hang(checkout, *, purpose):
            time.sleep(30)
            return self.report
        started = time.monotonic()
        status = self.supervise([sys.executable, '-c', 'import time; time.sleep(60)'], inspect=hang,
                                inspection_timeout=0.5)
        self.assertEqual(status, 77)
        self.assertLess(time.monotonic() - started, 5)
        self.assertTrue(self.binding_now()['revoked'])

    def test_inspection_exception_is_a_failure_not_a_renewal(self):
        def broken(checkout, *, purpose):
            raise self.access.AccessDenied('api_network_error')
        status = self.supervise([sys.executable, '-c', 'import time; time.sleep(60)'], inspect=broken)
        self.assertEqual(status, 77)

    def test_unexpected_broker_exit_contains_the_sandbox(self):
        broker = self.fake_broker + ['0.3']
        status = self.supervise([sys.executable, '-c', 'import time; time.sleep(60)'], broker_command=broker,
                                check_interval=30)
        self.assertEqual(status, 77)
        self.assertTrue(self.binding_now()['revoked'])

    def test_broker_that_never_becomes_ready_starts_no_sandbox(self):
        marker = self.root / 'sandbox-started'
        broker = [sys.executable, '-c', 'import time; time.sleep(60)']
        status = self.supervise([sys.executable, '-c', f'open({str(marker)!r}, "w")'], broker_command=broker,
                                broker_ready_timeout=0.5)
        self.assertEqual(status, 77)
        self.assertFalse(marker.exists())

    def test_run_command_without_a_valid_session_starts_nothing(self):
        marker = self.root / 'sandbox-started'
        result = subprocess.run([sys.executable, '-I', str(HELPERS / 'git-access.py'), 'run', '--session', '../x',
                                 '--', sys.executable, '-c', f'open({str(marker)!r}, "w")'],
                                capture_output=True, text=True, env=os.environ.copy())
        self.assertEqual(result.returncode, 77, result.stderr)
        self.assertFalse(marker.exists())

    def test_supervisor_death_kills_the_sandbox(self):
        pid_file = self.root / 'child-pid'
        child = [sys.executable, '-c', f'import os, time; open({str(pid_file)!r}, "w").write(str(os.getpid())); time.sleep(60)']
        settings = json.dumps({'helper': str(HELPERS / 'git-access.py'), 'session': self.session, 'child': child,
                               'broker': self.fake_broker, 'report': self.report})
        script = textwrap.dedent(f'''
            import importlib.util, json
            settings = json.loads({settings!r})
            spec = importlib.util.spec_from_file_location('access', settings['helper'])
            access = importlib.util.module_from_spec(spec); spec.loader.exec_module(access)
            access.supervise(settings['session'], settings['child'], check_interval=30, inspection_timeout=1,
                             hard_expiry=60, broker_command=settings['broker'],
                             inspect=lambda checkout, purpose: settings['report'])
        ''')
        supervisor = subprocess.Popen([sys.executable, '-c', script], env=os.environ.copy())
        self.addCleanup(lambda: supervisor.poll() is None and supervisor.kill())
        deadline = time.monotonic() + 10
        while not pid_file.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        child = int(pid_file.read_text())
        supervisor.kill()
        supervisor.wait()
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                os.kill(child, 0)
            except ProcessLookupError:
                return
            time.sleep(0.05)
        os.kill(child, signal.SIGKILL)
        self.fail('sandbox child survived its supervisor')


if __name__ == '__main__':
    unittest.main()
