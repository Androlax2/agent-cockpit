"""Exercise scanner event guards; these fixtures do not execute hosted scanners."""

import re
import unittest

from test_workflow_drafts import WORKFLOWS, allows, context, job_conditions


class ScannerEventTests(unittest.TestCase):
    def condition(self, filename, job):
        path = WORKFLOWS / filename
        self.assertTrue(path.exists(), f"missing scanner workflow: {filename}")
        return job_conditions(path.read_text())[job]

    def test_codeql_runs_ready_prs_and_main_but_refuses_privileged_pr_events(self):
        expression = self.condition("codeql.yml", "analyze")
        for event, draft, ref, expected in (
            ("pull_request", False, "refs/pull/1/merge", True),
            ("pull_request", True, "refs/pull/1/merge", False),
            ("pull_request_target", False, "refs/heads/main", False),
            ("workflow_dispatch", False, "refs/heads/main", False),
            ("push", False, "refs/heads/main", True),
            ("push", False, "refs/heads/feature", False),
            ("push", False, "refs/tags/v0.1.0", False),
            ("schedule", False, "refs/heads/main", True),
        ):
            with self.subTest(event=event, draft=draft, ref=ref):
                self.assertEqual(allows(expression, context(event, draft, ref=ref)), expected)

    def test_scorecard_refuses_pr_and_non_main_contexts(self):
        expression = self.condition("scorecard.yml", "scorecard")
        for event, ref, expected in (
            ("push", "refs/heads/main", True),
            ("schedule", "refs/heads/main", True),
            ("pull_request", "refs/pull/1/merge", False),
            ("pull_request_target", "refs/heads/main", False),
            ("workflow_dispatch", "refs/heads/main", False),
            ("push", "refs/heads/feature", False),
            ("push", "refs/tags/v0.1.0", False),
        ):
            with self.subTest(event=event, ref=ref):
                self.assertEqual(allows(expression, context(event, ref=ref)), expected)

    # api.scorecard.dev refuses published results (and so the README badge) from a job with
    # run steps, other actions, or write permissions beyond the job's own id-token (#382).
    def test_scorecard_job_keeps_the_shape_scorecard_accepts_for_publication(self):
        text = (WORKFLOWS / "scorecard.yml").read_text()
        top_level, jobs = text.split("\njobs:\n", 1)
        self.assertIn("permissions:\n  contents: read\n", top_level)
        self.assertNotRegex(top_level, r"(?m)^(env|defaults):")
        self.assertNotRegex(jobs, r"(?m)^\s+(-\s+)?(run|env|defaults|container|services):")
        self.assertEqual(
            set(re.findall(r"uses: ([\w./-]+)@", jobs)),
            {"actions/checkout", "ossf/scorecard-action", "actions/upload-artifact"},
        )
        self.assertIn("permissions:\n      contents: read\n      id-token: write\n", jobs)
        self.assertIn("publish_results: true", jobs)


class ActionPinningTests(unittest.TestCase):
    # Repository setting sha_pinning_required fails any run whose action is not pinned to a
    # full commit SHA; tools/repository_settings.py turns it on only while this holds (#382).
    def test_every_action_is_pinned_to_a_full_commit_sha(self):
        github = WORKFLOWS.parent
        files = sorted([*github.rglob("*.yml"), *github.rglob("*.yaml")])
        self.assertIn(WORKFLOWS / "ci.yml", files)
        for path in files:
            for reference in re.findall(r"(?<![\w-])uses: *['\"]?([^\s#'\"]+)", path.read_text()):
                if reference.startswith("./"):
                    continue
                with self.subTest(path=str(path.relative_to(github)), reference=reference):
                    self.assertRegex(reference, r"^[\w.-]+/[\w./-]+@[0-9a-f]{40}$")


if __name__ == "__main__":
    unittest.main()
