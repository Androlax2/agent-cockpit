import json
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "ci-metrics.py"


class CiMetricsTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="ci metrics ")
        self.addCleanup(self.directory.cleanup)
        self.cache = Path(self.directory.name)
        self.pull = {
            "number": 7,
            "headRefName": "greenfield/7-test",
            "createdAt": "2026-11-01T00:00:00Z",
            "mergedAt": "2026-11-01T01:00:00Z",
            "mergeCommit": {"oid": "merged-sha"},
        }
        self.runs = [
            {
                "id": 10,
                "name": "CI",
                "event": "pull_request",
                "head_branch": "greenfield/7-test",
                "head_sha": "head-sha",
                "pull_requests": [{"number": 7}],
                "run_attempt": 2,
                "conclusion": "success",
                "status": "completed",
                "created_at": "2026-11-01T00:05:00Z",
            },
            {
                "id": 11,
                "name": "CI",
                "event": "push",
                "head_branch": "main",
                "head_sha": "merged-sha",
                "pull_requests": [],
                "run_attempt": 1,
                "conclusion": "success",
                "status": "completed",
                "created_at": "2026-11-01T01:00:01Z",
            },
        ]
        self.jobs = [self.job(101, 1, "cancelled"), self.job(102, 2, "success")]
        self.write("runs.json", self.runs)
        self.write("sample-runs.json", self.runs)
        self.write("sample-pulls.json", [self.pull])
        self.write("jobs-10.json", {"total_count": 2, "jobs": self.jobs})
        main_job = self.job(103, 1, "success")
        main_job.update(run_id=11, name="greenfield-wsl2", labels=["windows-2025"])
        self.write("jobs-11.json", {"total_count": 1, "jobs": [main_job]})
        self.write("timeline-7.json", [])

    def job(self, job_id, attempt, conclusion):
        return {
            "id": job_id,
            "run_id": 10,
            "run_attempt": attempt,
            "name": "build",
            "labels": ["macos-26"],
            "status": "completed",
            "conclusion": conclusion,
            "started_at": "2026-11-01T00:00:00Z",
            "completed_at": "2026-11-01T00:01:01Z",
        }

    def write(self, name, value):
        (self.cache / name).write_text(json.dumps(value))

    def run_metrics(self):
        return subprocess.run(
            ["python3", str(SCRIPT), str(self.cache)],
            capture_output=True,
            text=True,
            check=False,
        )

    def summary(self):
        result = self.run_metrics()
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_counts_cancelled_attempts_and_rounds_each_job_before_weighting(self):
        pull = self.summary()["pulls"][0]
        self.assertEqual(pull["pr_runner_seconds"], 122)
        self.assertEqual(pull["main_runner_seconds"], 61)
        self.assertEqual(pull["pr_weighted_minutes"], 40)
        self.assertEqual(pull["main_weighted_minutes"], 4)

    def test_reruns_are_distinct_from_new_workflow_runs(self):
        pull = self.summary()["pulls"][0]
        self.assertEqual(pull["pr_runs"], 1)
        self.assertEqual(pull["pr_reruns"], 1)

    def test_unfinished_run_is_refused_even_when_all_visible_jobs_completed(self):
        self.runs[0].update(status="in_progress", conclusion=None)
        self.write("sample-runs.json", self.runs)
        result = self.run_metrics()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("run 10: unfinished workflow", result.stderr)

    def test_readiness_uses_the_last_ready_event_after_draft_cycles(self):
        self.write(
            "timeline-7.json",
            [
                {"event": "ready_for_review", "created_at": "2026-11-01T00:10:00Z"},
                {"event": "convert_to_draft", "created_at": "2026-11-01T00:20:00Z"},
                {"event": "ready_for_review", "created_at": "2026-11-01T00:40:00Z"},
            ],
        )
        pull = self.summary()["pulls"][0]
        self.assertEqual(pull["ready_to_merge_seconds"], 1200)
        self.assertEqual(pull["ready_source"], "ready_for_review")

    def test_no_draft_events_uses_creation_as_the_documented_readiness_proxy(self):
        pull = self.summary()["pulls"][0]
        self.assertEqual(pull["ready_to_merge_seconds"], 3600)
        self.assertEqual(pull["ready_source"], "createdAt_proxy")

    def test_skipped_jobs_add_no_runner_time(self):
        skipped = self.job(104, 2, "skipped")
        skipped.update(started_at=None, completed_at=None, labels=[])
        self.write("jobs-10.json", {"total_count": 3, "jobs": [*self.jobs, skipped]})
        self.assertEqual(self.summary()["pulls"][0]["pr_runner_seconds"], 122)

    def test_incomplete_evidence_is_refused_instead_of_reporting_a_lower_cost(self):
        for reason, jobs in {
            "truncated": {"total_count": 3, "jobs": self.jobs},
            "missing attempt": {"total_count": 1, "jobs": self.jobs[1:]},
            "duplicate job": {"total_count": 3, "jobs": [*self.jobs, self.jobs[0]]},
        }.items():
            with self.subTest(reason=reason):
                self.write("jobs-10.json", jobs)
                result = self.run_metrics()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("jobs-10.json", result.stderr)

    def test_unknown_platform_and_unfinished_jobs_are_not_treated_as_free(self):
        for override in [
            {"labels": ["unrecognized-runner"]},
            {"completed_at": None},
            {"completed_at": "2026-10-31T00:00:00Z"},
        ]:
            with self.subTest(override=override):
                jobs = [{**job, **override} for job in self.jobs]
                self.write("jobs-10.json", {"total_count": 2, "jobs": jobs})
                self.assertNotEqual(self.run_metrics().returncode, 0)

    def test_full_timeline_page_is_refused_until_pagination_is_resolved(self):
        self.write("timeline-7.json", [{"event": "commented"}] * 100)
        self.assertNotEqual(self.run_metrics().returncode, 0)

    def test_run_without_unique_pr_attribution_is_refused(self):
        self.runs[0].update(pull_requests=[], head_branch="unrelated")
        self.write("sample-runs.json", self.runs)
        self.assertNotEqual(self.run_metrics().returncode, 0)

    def test_reused_branch_is_attributed_by_pr_lifetime_when_links_are_empty(self):
        earlier = {
            **self.pull,
            "number": 6,
            "createdAt": "2026-10-31T00:00:00Z",
            "mergedAt": "2026-10-31T01:00:00Z",
            "mergeCommit": {"oid": "earlier-merge"},
        }
        self.write("sample-pulls.json", [earlier, self.pull])
        self.write("timeline-6.json", [])
        self.runs[0]["pull_requests"] = []
        earlier_runs = [
            {**self.runs[0], "id": 12, "created_at": "2026-10-31T00:05:00Z"},
            {**self.runs[1], "id": 13, "head_sha": "earlier-merge"},
        ]
        self.write("sample-runs.json", [*self.runs, *earlier_runs])
        self.write(
            "jobs-12.json",
            {"total_count": 2, "jobs": [{**job, "run_id": 12} for job in self.jobs]},
        )
        self.write(
            "jobs-13.json",
            {"total_count": 1, "jobs": [{**self.job(105, 1, "success"), "run_id": 13}]},
        )
        self.assertEqual([pull["pr_runs"] for pull in self.summary()["pulls"]], [1, 1])


if __name__ == "__main__":
    unittest.main()
