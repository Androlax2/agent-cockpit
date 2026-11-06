"""The greenfield tests run in shards, and the required `greenfield` check gates them (#997)."""

import os
import re
import subprocess
import unittest
from pathlib import Path

from test_ci_bootstrap import ci_job

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = (ROOT / ".github/workflows/ci.yml").read_text()


def required_check_script():
    steps = ci_job(WORKFLOW, "greenfield")
    script = steps.split("        run: |\n", 1)[1]
    return "\n".join(line.removeprefix("          ") for line in script.splitlines())


def run_required_check(changes, selected, lint, tests):
    results = {"CHANGES": changes, "SELECTED": selected, "LINT": lint, "TESTS": tests}
    # GitHub runs `run:` scripts with `bash -e`.
    return subprocess.run(
        ["/bin/bash", "-e", "-c", required_check_script()],
        env={"PATH": os.environ["PATH"], **results},
        capture_output=True,
        text=True,
        check=False,
    ).returncode


class ShardedGreenfieldTests(unittest.TestCase):
    def test_the_shards_cover_every_partition_of_the_workspace_tests(self):
        steps = ci_job(WORKFLOW, "greenfield-test")
        shards = re.search(r"^        shard: \[([0-9, ]+)\]$", steps, re.MULTILINE)
        self.assertIsNotNone(shards, "greenfield-test lists its shards in its matrix")
        numbers = [int(number) for number in shards[1].split(",")]
        partition = re.search(
            r"run: make crates-test TEST_PARTITION=hash:\$\{\{ matrix\.shard \}\}/(\d+)\n", steps
        )
        self.assertIsNotNone(partition, "each shard runs one hash partition of crates-test")
        self.assertEqual(numbers, list(range(1, int(partition[1]) + 1)))

    def test_a_failing_shard_does_not_cancel_the_others(self):
        self.assertIn("      fail-fast: false\n", ci_job(WORKFLOW, "greenfield-test"))

    def test_the_required_check_needs_every_greenfield_job(self):
        steps = ci_job(WORKFLOW, "greenfield")
        self.assertIn("    needs: [changes, greenfield-lint, greenfield-test]\n", steps)
        condition = re.search(r"^    if: (.*)$", steps, re.MULTILINE)[1]
        self.assertIn("!cancelled()", condition, "a failed shard must not skip the check")
        self.assertNotIn("needs.changes.outputs", condition, "the script decides, not a skip")

    def test_the_required_check_passes_only_when_lint_and_every_shard_pass(self):
        cases = [
            ("success", "true", "success", "success", 0),
            ("success", "true", "failure", "success", 1),
            ("success", "true", "success", "failure", 1),
            ("success", "true", "success", "cancelled", 1),
            ("success", "true", "skipped", "success", 1),
            ("success", "true", "success", "skipped", 1),
            ("success", "false", "skipped", "skipped", 0),
            ("success", "", "skipped", "skipped", 1),
            ("failure", "", "skipped", "skipped", 1),
            ("skipped", "", "skipped", "skipped", 1),
        ]
        for changes, selected, lint, tests, expected in cases:
            with self.subTest(changes=changes, selected=selected, lint=lint, tests=tests):
                self.assertEqual(run_required_check(changes, selected, lint, tests), expected)

    def test_one_shard_each_runs_the_second_user_and_live_probe_steps(self):
        steps = ci_job(WORKFLOW, "greenfield-test")
        for name, shard in [
            ("Run the daemon's sandbox tests, with the ignored one for #628", 1),
            ("Probe the native sandbox live", 2),
        ]:
            with self.subTest(step=name):
                step = steps.split(name, 1)[1].split("\n      - ", 1)[0]
                self.assertIn(f"        if: matrix.shard == {shard}\n", step)


if __name__ == "__main__":
    unittest.main()
