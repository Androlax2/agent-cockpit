"""Exercise scanner event guards; these fixtures do not execute hosted scanners."""

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


if __name__ == "__main__":
    unittest.main()
