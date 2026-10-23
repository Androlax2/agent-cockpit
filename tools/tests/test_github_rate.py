import importlib.util
from contextlib import ExitStack
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


MODULE = Path(__file__).resolve().parents[2] / 'config/bin/.local/bin/greywall-agents/github-rate.py'


class GithubRateTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(MODULE.is_file(), 'shared principal budget ledger is not implemented')
        spec = importlib.util.spec_from_file_location('cockpit_github_rate', MODULE)
        self.rate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.rate)
        self.scratch = tempfile.TemporaryDirectory(prefix='github-rate-', dir='/tmp/opencode')
        self.addCleanup(self.scratch.cleanup)
        self.environment = patch.dict(os.environ, {'HOME': self.scratch.name})
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.bucket = 'installation:77:core'
        self.key = 'a' * 64

    def observe(self, principal='installation:77', remaining=100, resource='core', limit=5000):
        return self.rate.observe(principal, {'x-ratelimit-resource': resource, 'x-ratelimit-limit': str(limit),
                                            'x-ratelimit-remaining': str(remaining),
                                            'x-ratelimit-reset': str(int(time.time()) + 3600)})

    def reserve(self, costs=None, purpose='inspection', repository_id=42):
        return self.rate.reserve(costs or {self.bucket: 7}, purpose=purpose, repository_id=repository_id,
                                 deadline=time.monotonic() + 30, evidence_key=self.key)

    def test_unknown_or_insufficient_budget_never_admits_a_complete_check(self):
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_quota_unknown$'):
            self.reserve()
        self.observe(remaining=6)
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_quota_exhausted$'):
            self.reserve()

    def test_only_eight_upstream_requests_can_hold_slots(self):
        self.assertTrue(hasattr(self.rate, 'request_slot'), 'shared upstream slots are missing')
        with ExitStack() as slots:
            for _ in range(8):
                slots.enter_context(self.rate.request_slot())
            with self.assertRaisesRegex(self.rate.RateDenied, '^upstream_busy$'):
                with self.rate.request_slot():
                    self.fail('ninth upstream request was admitted')
        with self.rate.request_slot():
            pass

    def test_first_request_can_record_quota_in_a_fresh_home(self):
        previous_umask = os.umask(0o022)
        try:
            with self.rate.request_slot():
                self.assertEqual(self.observe(), 'installation:77:core')
        finally:
            os.umask(previous_umask)

    def test_repository_write_lock_is_shared_with_another_process(self):
        self.assertTrue(hasattr(self.rate, 'repository_write'), 'repository write lock is missing')
        script = ('import importlib.util\n'
                  f'spec = importlib.util.spec_from_file_location("rate", {str(MODULE)!r})\n'
                  'rate = importlib.util.module_from_spec(spec)\nspec.loader.exec_module(rate)\n'
                  'try:\n with rate.repository_write(42): pass\n'
                  'except rate.RateDenied as error:\n print(str(error))\n')
        with self.rate.repository_write(42):
            result = subprocess.run([sys.executable, '-I', '-c', script], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), 'repository_write_busy')
            with self.rate.repository_write(99):
                pass
        with self.rate.repository_write(42):
            pass

    def test_mutation_spacing_and_minute_limit_are_global(self):
        self.assertTrue(hasattr(self.rate, 'admit_mutation'), 'mutation pacing is missing')
        now = time.time()
        monotonic = time.monotonic()
        with patch.object(self.rate.time, 'time', return_value=now), \
                patch.object(self.rate.time, 'monotonic', return_value=monotonic):
            self.rate.admit_mutation()
            with self.assertRaisesRegex(self.rate.RateDenied, '^mutation_paced$'):
                self.rate.admit_mutation()
        for second in range(1, 30):
            with patch.object(self.rate.time, 'time', return_value=now + second), \
                    patch.object(self.rate.time, 'monotonic', return_value=monotonic + second):
                self.rate.admit_mutation()
        with patch.object(self.rate.time, 'time', return_value=now + 30), \
                patch.object(self.rate.time, 'monotonic', return_value=monotonic + 30):
            with self.assertRaisesRegex(self.rate.RateDenied, '^mutation_paced$'):
                self.rate.admit_mutation()

    def test_hour_limit_survives_boot_change(self):
        self.assertTrue(hasattr(self.rate, 'admit_mutation'), 'mutation pacing is missing')
        now, monotonic = time.time(), time.monotonic()
        for index in range(300):
            with patch.object(self.rate.time, 'time', return_value=now + index * 3), \
                    patch.object(self.rate.time, 'monotonic', return_value=monotonic + index * 3):
                self.rate.admit_mutation()
        path = self.rate.ledger_path()
        saved = json.loads(path.read_text())
        saved['boot'] = 'old-boot'
        path.write_text(json.dumps(saved))
        with patch.object(self.rate.time, 'time', return_value=now + 1000), \
                patch.object(self.rate.time, 'monotonic', return_value=1):
            with self.assertRaisesRegex(self.rate.RateDenied, '^mutation_paced$'):
                self.rate.admit_mutation()

    def test_mutation_pacing_survives_another_process(self):
        self.assertTrue(hasattr(self.rate, 'admit_mutation'), 'mutation pacing is missing')
        now = time.time()
        with self.rate.locked_state() as state:
            state['mutations'] = [{'wall': now - 2 * index,
                                   'monotonic': state['monotonic'] - 2 * index, 'boot': state['boot']}
                                  for index in reversed(range(30))]
        script = ('import importlib.util\n'
                  f'spec = importlib.util.spec_from_file_location("rate", {str(MODULE)!r})\n'
                  'rate = importlib.util.module_from_spec(spec)\nspec.loader.exec_module(rate)\n'
                  'try:\n rate.admit_mutation()\n'
                  'except rate.RateDenied as error:\n print(str(error))\n')
        result = subprocess.run([sys.executable, '-I', '-c', script], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), 'mutation_paced')

    def test_forward_wall_clock_jump_cannot_evade_mutation_spacing(self):
        self.assertTrue(hasattr(self.rate, 'admit_mutation'), 'mutation pacing is missing')
        now, monotonic = time.time(), time.monotonic()
        with patch.object(self.rate.time, 'time', return_value=now), \
                patch.object(self.rate.time, 'monotonic', return_value=monotonic):
            self.rate.admit_mutation()
        with patch.object(self.rate.time, 'time', return_value=now + 7200), \
                patch.object(self.rate.time, 'monotonic', return_value=monotonic + 0.1):
            with self.assertRaisesRegex(self.rate.RateDenied, '^mutation_paced$'):
                self.rate.admit_mutation()

    def test_git_throttle_blocks_already_reserved_requests_without_quota_headers(self):
        self.assertTrue(hasattr(self.rate, 'backoff'), 'Git secondary backoff is missing')
        self.observe(remaining=5000)
        reservation = self.reserve(purpose='write')['reservation']
        self.rate.backoff([self.bucket], {'retry-after': '60'})
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_quota_exhausted$'):
            self.rate.charge(reservation, self.bucket)

    def test_automated_checks_and_writes_preserve_the_approved_twenty_percent(self):
        self.rate.observe('installation:77', {'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '100',
                                            'x-ratelimit-remaining': '100', 'x-ratelimit-reset': str(int(time.time()) + 3600)})
        for purpose in ['admission', 'scheduled', 'write']:
            with self.subTest(purpose=purpose), self.assertRaisesRegex(self.rate.RateDenied, '^rate_headroom_reserved$'):
                self.reserve({self.bucket: 81}, purpose=purpose)
        self.reserve({self.bucket: 80}, purpose='write')

    def test_read_only_inspection_can_use_the_reserve_without_authorizing_a_write(self):
        self.rate.observe('installation:77', {'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '100',
                                            'x-ratelimit-remaining': '100', 'x-ratelimit-reset': str(int(time.time()) + 3600)})
        self.reserve({self.bucket: 80}, purpose='write')
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_headroom_reserved$'):
            self.reserve({self.bucket: 1}, purpose='write')
        result = self.reserve({self.bucket: 20}, purpose='inspection')
        self.assertIsNone(result['cached'])

    def test_new_server_observations_can_stop_prepaid_dispatch_at_the_reserve(self):
        headers = {'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '100',
                   'x-ratelimit-remaining': '100', 'x-ratelimit-reset': str(int(time.time()) + 3600)}
        self.rate.observe('installation:77', headers)
        reservation = self.reserve({self.bucket: 80}, purpose='write')['reservation']
        self.rate.charge(reservation, self.bucket, 79)
        self.rate.observe('installation:77', headers | {'x-ratelimit-remaining': '20'})
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_headroom_reserved$'):
            self.rate.charge(reservation, self.bucket)

    def test_app_installation_bucket_is_shared_across_repo_tokens(self):
        self.observe(remaining=10)
        self.reserve(repository_id=42)
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_quota_exhausted$'):
            self.reserve(repository_id=99)

    def test_pat_pool_is_shared_across_tokens_but_not_other_users_or_resources(self):
        self.observe('user:88', remaining=2)
        self.observe('user:99', remaining=2)
        self.observe('user:88', remaining=2, resource='graphql')
        self.reserve({'user:88:core': 2})
        with self.assertRaises(self.rate.RateDenied):
            self.reserve({'user:88:core': 1}, repository_id=99)
        self.reserve({'user:99:core': 2, 'user:88:graphql': 2})

    def test_app_mint_cost_is_not_free_and_cannot_borrow_installation_quota(self):
        self.observe()
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_quota_unknown$'):
            self.reserve({self.bucket: 7, 'app:123:core': 1})
        self.observe('app:123', remaining=1)
        reservation = self.reserve({self.bucket: 7, 'app:123:core': 1})
        self.rate.charge(reservation['reservation'], 'app:123:core')
        with self.assertRaises(self.rate.RateDenied):
            self.reserve({'app:123:core': 1})

    def test_each_write_still_spends_a_fresh_inspection(self):
        self.observe(remaining=18, limit=20)
        first = self.reserve(purpose='write')
        second = self.reserve(purpose='write')
        self.assertNotEqual(first['reservation'], second['reservation'])
        self.assertIsNone(first['cached'])
        self.assertIsNone(second['cached'])
        with self.assertRaises(self.rate.RateDenied):
            self.reserve(purpose='write')

    def test_extra_pages_are_charged_before_request_and_never_truncate_into_success(self):
        self.observe(remaining=2)
        reservation = self.reserve({self.bucket: 1})['reservation']
        self.rate.charge(reservation, self.bucket)
        self.rate.charge(reservation, self.bucket)
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_quota_exhausted$'):
            self.rate.charge(reservation, self.bucket)

    def test_failed_multibucket_reservation_does_not_spend_other_pools(self):
        self.observe(remaining=7)
        with self.assertRaises(self.rate.RateDenied):
            self.reserve({self.bucket: 7, 'user:88:core': 1})
        self.reserve()

    def test_server_reconciliation_cannot_restore_reserved_capacity(self):
        self.observe(remaining=10)
        reservation = self.reserve()['reservation']
        self.rate.charge(reservation, self.bucket)
        self.observe(remaining=9)
        with self.assertRaises(self.rate.RateDenied):
            self.reserve({self.bucket: 4})
        self.reserve({self.bucket: 3})

    def test_external_usage_is_subtracted_before_remaining_reserved_requests(self):
        self.observe(remaining=10)
        reservation = self.reserve()['reservation']
        self.rate.charge(reservation, self.bucket)
        self.observe(remaining=8)
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_quota_exhausted$'):
            self.reserve({self.bucket: 3})
        self.reserve({self.bucket: 2})

    def test_scheduled_checks_coalesce_by_repository_not_session(self):
        self.observe(remaining=9, limit=10)
        first = self.reserve(purpose='scheduled')
        evidence = {'repository_id': 42, 'default_branch': 'main', 'evidence_digest': 'b' * 64}
        self.rate.finish(first['reservation'], evidence=evidence)
        cached = self.reserve(purpose='scheduled')
        self.assertEqual(cached['cached'], evidence)
        self.assertIsNone(cached['reservation'])
        self.assertLessEqual(cached['checked_at'], time.monotonic())
        self.assertEqual(cached['expires_at'], cached['checked_at'] + 300)
        with self.assertRaises(self.rate.RateDenied):
            self.reserve(purpose='scheduled', repository_id=99)
        with self.assertRaises(self.rate.RateDenied):
            self.reserve(purpose='write')

    def test_simultaneous_scheduled_scans_are_not_duplicated(self):
        self.observe(limit=100)
        self.reserve(purpose='scheduled')
        with self.assertRaisesRegex(self.rate.RateDenied, '^scheduled_check_in_progress$'):
            self.reserve(purpose='scheduled')

    def test_expired_or_changed_evidence_never_renews_a_session(self):
        self.observe(remaining=9, limit=10)
        first = self.reserve(purpose='scheduled')
        self.rate.finish(first['reservation'], evidence={'repository_id': 42, 'default_branch': 'main', 'evidence_digest': 'b' * 64})
        with patch.object(self.rate.time, 'monotonic', return_value=time.monotonic() + 301):
            with self.assertRaises(self.rate.RateDenied):
                self.reserve(purpose='scheduled')
        self.key = 'c' * 64
        with self.assertRaises(self.rate.RateDenied):
            self.reserve(purpose='scheduled')

    def test_read_snapshot_is_not_merge_evidence(self):
        self.observe(remaining=7)
        first = self.reserve(purpose='inspection')
        self.rate.finish(first['reservation'])
        with self.assertRaises(self.rate.RateDenied):
            self.reserve(purpose='write')

    def test_expired_reservation_cannot_spend_or_publish_success(self):
        self.observe()
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_deadline_expired$'):
            self.rate.reserve({self.bucket: 7}, purpose='scheduled', repository_id=42,
                              deadline=time.monotonic() - 1, evidence_key=self.key)

    def test_a_reservation_that_expires_after_creation_cannot_spend_or_cache(self):
        self.observe(limit=100)
        reservation = self.reserve(purpose='scheduled')['reservation']
        future = time.monotonic() + 31
        with patch.object(self.rate.time, 'monotonic', return_value=future):
            with self.assertRaisesRegex(self.rate.RateDenied, '^rate_reservation_expired$'):
                self.rate.charge(reservation, self.bucket)
            with self.assertRaisesRegex(self.rate.RateDenied, '^rate_reservation_expired$'):
                self.rate.finish(reservation, evidence={'repository_id': 42, 'default_branch': 'main', 'evidence_digest': 'b' * 64})

    def test_rate_exhaustion_never_renews_or_forwards_and_honors_retry_after(self):
        self.observe()
        headers = {'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '5000', 'x-ratelimit-remaining': '99',
                   'x-ratelimit-reset': str(int(time.time()) + 3600), 'retry-after': '60'}
        self.rate.observe('installation:77', headers, status=429)
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_quota_exhausted$'):
            self.reserve()

    def test_server_exhaustion_revokes_already_reserved_requests_before_dispatch(self):
        self.observe()
        reservation = self.reserve()['reservation']
        headers = {'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '5000', 'x-ratelimit-remaining': '0',
                   'x-ratelimit-reset': str(int(time.time()) + 3600), 'retry-after': '60'}
        self.rate.observe('installation:77', headers, status=429)
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_quota_exhausted$'):
            self.rate.charge(reservation, self.bucket)

    def test_success_headers_showing_zero_server_quota_also_stop_reserved_dispatch(self):
        self.observe(remaining=7)
        reservation = self.reserve()['reservation']
        self.rate.charge(reservation, self.bucket)
        self.observe(remaining=0)
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_quota_exhausted$'):
            self.rate.charge(reservation, self.bucket)

    def test_old_reservations_do_not_get_free_credit_after_a_server_window_reset(self):
        now = int(time.time())
        self.rate.observe('installation:77', {'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '5000',
                                            'x-ratelimit-remaining': '7', 'x-ratelimit-reset': str(now + 10)})
        old = self.reserve()['reservation']
        with patch.object(self.rate.time, 'time', return_value=now + 11):
            self.observe(remaining=7)
            self.reserve()
            with self.assertRaisesRegex(self.rate.RateDenied, '^rate_quota_exhausted$'):
                self.rate.charge(old, self.bucket)

    def test_operation_readiness_holds_fresh_credit_if_the_server_window_changed(self):
        now = int(time.time())
        self.rate.observe('installation:77', {'x-ratelimit-resource': 'core', 'x-ratelimit-limit': '5000',
                                            'x-ratelimit-remaining': '7', 'x-ratelimit-reset': str(now + 10)})
        old = self.reserve()['reservation']
        with patch.object(self.rate.time, 'time', return_value=now + 11):
            self.observe(remaining=7)
            self.rate.ready(old, {self.bucket: 7})
            with self.assertRaisesRegex(self.rate.RateDenied, '^rate_quota_exhausted$'):
                self.reserve()

    def test_wall_clock_rollback_cannot_restore_quota_or_expired_evidence(self):
        self.observe()
        with patch.object(self.rate.time, 'time', return_value=time.time() - 60):
            with self.assertRaisesRegex(self.rate.RateDenied, '^rate_clock_changed$'):
                self.reserve()

    def test_corrupt_nested_state_is_a_named_denial_not_a_traceback(self):
        self.observe()
        original = self.rate.ledger_path().read_text()
        for field, content in [('buckets', {self.bucket: {'remaining': 'unlimited'}}),
                               ('reservations', {'123': None}), ('scheduled', {'42': {'at': 'forever'}})]:
            with self.subTest(field=field):
                state = json.loads(original)
                state[field] = content
                self.rate.ledger_path().write_text(json.dumps(state))
                with self.assertRaisesRegex(self.rate.RateDenied, '^rate_ledger_invalid$'):
                    self.reserve()

    def test_world_readable_or_symlinked_ledger_is_not_trusted(self):
        self.observe()
        path = self.rate.ledger_path()
        path.chmod(0o644)
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_ledger_untrusted$'):
            self.reserve()
        path.chmod(0o600)
        target = path.with_name('other.json')
        path.rename(target)
        path.symlink_to(target)
        with self.assertRaisesRegex(self.rate.RateDenied, '^rate_ledger_untrusted$'):
            self.reserve()

    def test_cross_process_reservations_cannot_overspend_the_same_principal(self):
        self.observe(remaining=9, limit=10)
        program = ('import importlib.util,time\n'
                   f'spec=importlib.util.spec_from_file_location("rate", {str(MODULE)!r})\n'
                   'rate=importlib.util.module_from_spec(spec);spec.loader.exec_module(rate)\n'
                   'try:\n'
                   ' rate.reserve({"installation:77:core":7},purpose="write",repository_id=42,deadline=time.monotonic()+30)\n'
                   ' print("reserved")\n'
                   'except rate.RateDenied:\n'
                   ' print("denied")\n')
        processes = [subprocess.Popen([sys.executable, '-I', '-c', program], stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, text=True) for _ in range(8)]
        results = [process.communicate(timeout=10) for process in processes]
        self.assertEqual(sum(output.strip() == 'reserved' for output, error in results), 1, results)
        self.assertTrue(all(process.returncode == 0 for process in processes), results)

    def test_documented_capacity_examples_keep_installation_and_user_pools_separate(self):
        examples = [
            ({'sessions': 20, 'repositories': 5, 'admissions': 20, 'writes': 60, 'advertisements': 60, 'other_reads': 200}, (1400, 260, 1660)),
            ({'sessions': 20, 'repositories': 10, 'admissions': 20, 'writes': 120, 'advertisements': 120, 'other_reads': 300}, (2660, 420, 3080)),
            ({'repositories': 12}, (1008, 0, 1008)),
            ({'repositories': 12, 'admissions': 20, 'writes': 120, 'advertisements': 120, 'extra_checks': 12, 'other_reads': 300, 'merge_reads': 80}, (2912, 500, 3412)),
            ({'sessions': 20, 'repositories': 5, 'admissions': 20, 'writes': 60, 'advertisements': 60, 'other_reads': 200, 'coalesced': False}, (2660, 260, 2920)),
        ]
        for arguments, expected in examples:
            with self.subTest(arguments=arguments):
                result = self.rate.estimate_hourly(**arguments)
                self.assertEqual((result['installation'], result['user'], result['total']), expected)
        self.assertEqual(self.rate.estimate_hourly(**examples[0][0], poll_seconds=60)['total'], 3340)
        self.assertEqual(self.rate.estimate_hourly(**examples[1][0], poll_seconds=60)['total'], 6440)
        self.assertEqual(self.rate.estimate_hourly(repositories=12, poll_seconds=60)['total'], 5040)
        self.assertEqual(self.rate.estimate_hourly(**examples[-1][0], poll_seconds=60)['total'], 9640)
        self.assertEqual(self.rate.estimate_hourly(repositories=60)['total'], 5040)


if __name__ == '__main__':
    unittest.main()
