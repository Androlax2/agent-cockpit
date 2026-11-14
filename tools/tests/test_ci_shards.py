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


def run_required_check(changes, selected, lint, tests, sandbox):
    results = {
        "CHANGES": changes,
        "SELECTED": selected,
        "LINT": lint,
        "TESTS": tests,
        "SANDBOX": sandbox,
    }
    # GitHub runs `run:` scripts with `bash -e`.
    return subprocess.run(
        ["/bin/bash", "-e", "-c", required_check_script()],
        env={"PATH": os.environ["PATH"], **results},
        capture_output=True,
        text=True,
        check=False,
    ).returncode


class ShardedGreenfieldTests(unittest.TestCase):
    def test_the_shards_cover_every_slice_of_the_workspace_tests(self):
        steps = ci_job(WORKFLOW, "greenfield-test")
        shards = re.search(r"^        shard: \[([0-9, ]+)\]$", steps, re.MULTILINE)
        self.assertIsNotNone(shards, "greenfield-test lists its shards in its matrix")
        numbers = [int(number) for number in shards[1].split(",")]
        plan = re.search(
            r"ci-shards\.py plan --shards (\d+) --shard \$\{\{ matrix\.shard \}\}", steps
        )
        self.assertIsNotNone(plan, "each shard cuts its own slice with the same shard count")
        self.assertEqual(numbers, list(range(1, int(plan[1]) + 1)))
        self.assertIn('run: make crates-test TEST_FILTER="$TEST_FILTER"\n', steps)
        self.assertIn("TEST_FILTER: ${{ steps.plan.outputs.filter }}\n", steps)

    def test_a_failing_shard_does_not_cancel_the_others(self):
        self.assertIn("      fail-fast: false\n", ci_job(WORKFLOW, "greenfield-test"))

    def test_the_required_check_needs_every_greenfield_job(self):
        steps = ci_job(WORKFLOW, "greenfield")
        self.assertIn(
            "    needs: [changes, greenfield-lint, greenfield-test, greenfield-sandbox]\n", steps
        )
        condition = re.search(r"^    if: (.*)$", steps, re.MULTILINE)[1]
        self.assertIn("!cancelled()", condition, "a failed shard must not skip the check")
        self.assertNotIn("needs.changes.outputs", condition, "the script decides, not a skip")

    def test_the_required_check_passes_only_when_every_greenfield_job_passes(self):
        passed = ("success", "true", "success", "success", "success")
        cases = [(passed, 0)]
        for position, result in [(2, "failure"), (3, "failure"), (4, "failure"), (3, "cancelled")]:
            cases.append((passed[:position] + (result,) + passed[position + 1 :], 1))
        for position in (2, 3, 4):
            cases.append((passed[:position] + ("skipped",) + passed[position + 1 :], 1))
        skipped = ("skipped", "skipped", "skipped")
        cases += [
            (("success", "false", *skipped), 0),
            (("success", "", *skipped), 1),
            (("failure", "", *skipped), 1),
            (("skipped", "", *skipped), 1),
        ]
        for results, expected in cases:
            with self.subTest(results=results):
                self.assertEqual(run_required_check(*results), expected)

    def test_the_sandbox_steps_run_beside_the_shards_on_the_shards_build(self):
        sandbox = ci_job(WORKFLOW, "greenfield-sandbox")
        for name in [
            "Run the daemon's sandbox tests, with the ignored one for #628",
            "Probe the native sandbox live",
            "Install rootless Docker's programs for a project's engine",
            "Set up a project's own engine as its owner does and probe its containers",
        ]:
            with self.subTest(step=name):
                self.assertIn(name, sandbox)
                self.assertNotIn(name, ci_job(WORKFLOW, "greenfield-test"))
        self.assertIn(
            "cargo test --no-run --locked --manifest-path Cargo.toml --workspace\n", sandbox
        )
        self.assertNotIn("matrix.", sandbox)


if __name__ == "__main__":
    unittest.main()
