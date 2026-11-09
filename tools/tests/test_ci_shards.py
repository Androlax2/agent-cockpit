"""The greenfield tests are built once and run in shards; the required `greenfield` check gates them (#997, #1261)."""

import os
import re
import subprocess
import unittest
from pathlib import Path

from test_ci_bootstrap import ci_job

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = (ROOT / ".github/workflows/ci.yml").read_text()
MAKEFILE = (ROOT / "Makefile").read_text()


def required_check_script():
    steps = ci_job(WORKFLOW, "greenfield")
    script = steps.split("        run: |\n", 1)[1]
    return "\n".join(line.removeprefix("          ") for line in script.splitlines())


def run_required_check(changes, selected, lint, build, tests, sandbox):
    results = {
        "CHANGES": changes,
        "SELECTED": selected,
        "LINT": lint,
        "BUILD": build,
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
    def test_the_shards_come_from_the_build_jobs_plan_of_its_own_archive(self):
        build = ci_job(WORKFLOW, "greenfield-build")
        self.assertIn("      shards: ${{ steps.plan.outputs.matrix }}\n", build)
        self.assertIn("run: make crates-test-archive\n", build)
        plan = build.index('python3 tools/ci-shards.py plan >> "$GITHUB_OUTPUT"')
        self.assertGreater(plan, build.index("make crates-test-archive"), "plan the built archive")
        tests = ci_job(WORKFLOW, "greenfield-test")
        self.assertIn(
            "      matrix: ${{ fromJSON(needs.greenfield-build.outputs.shards) }}\n", tests
        )
        self.assertIn('run: make crates-test-from-archive TEST_FILTER="$TEST_FILTER"\n', tests)
        self.assertIn("TEST_FILTER: ${{ matrix.filter }}\n", tests)

    def test_no_job_but_the_build_job_compiles_the_workspace(self):
        for job in ("greenfield-test", "greenfield-sandbox"):
            with self.subTest(job=job):
                steps = ci_job(WORKFLOW, job)
                self.assertIn("name: greenfield-tests\n", steps)
                self.assertNotIn("cargo test", steps)
                self.assertNotIn("make crates-test\n", steps)
                self.assertNotIn("make crates-test ", steps)
        self.assertIn("COCKPIT_PREBUILT_PROGRAMS=1", MAKEFILE)

    def test_a_failing_shard_does_not_cancel_the_others(self):
        self.assertIn("      fail-fast: false\n", ci_job(WORKFLOW, "greenfield-test"))

    def test_the_required_check_needs_every_greenfield_job(self):
        steps = ci_job(WORKFLOW, "greenfield")
        self.assertIn(
            "    needs: [changes, greenfield-lint, greenfield-build, greenfield-test, greenfield-sandbox]\n",
            steps,
        )
        condition = re.search(r"^    if: (.*)$", steps, re.MULTILINE)[1]
        self.assertIn("!cancelled()", condition, "a failed shard must not skip the check")
        self.assertNotIn("needs.changes.outputs", condition, "the script decides, not a skip")

    def test_the_required_check_passes_only_when_every_greenfield_job_passes(self):
        passed = ("success", "true", "success", "success", "success", "success")
        cases = [(passed, 0)]
        for position, result in [
            (2, "failure"),
            (3, "failure"),
            (4, "failure"),
            (5, "failure"),
            (4, "cancelled"),
        ]:
            cases.append((passed[:position] + (result,) + passed[position + 1 :], 1))
        for position in (2, 3, 4, 5):
            cases.append((passed[:position] + ("skipped",) + passed[position + 1 :], 1))
        skipped = ("skipped", "skipped", "skipped", "skipped")
        cases += [
            (("success", "false", *skipped), 0),
            (("success", "", *skipped), 1),
            (("failure", "", *skipped), 1),
            (("skipped", "", *skipped), 1),
        ]
        for results, expected in cases:
            with self.subTest(results=results):
                self.assertEqual(run_required_check(*results), expected)

    def test_the_sandbox_steps_run_beside_the_shards_on_the_shards_archive(self):
        sandbox = ci_job(WORKFLOW, "greenfield-sandbox")
        for name in [
            "Run the daemon's sandbox tests, with the ignored one for #628",
            "Probe the native sandbox live",
        ]:
            with self.subTest(step=name):
                self.assertIn(name, sandbox)
                self.assertNotIn(name, ci_job(WORKFLOW, "greenfield-test"))
        self.assertIn("cargo nextest list --archive-file greenfield-tests.tar.zst", sandbox)
        self.assertNotIn("matrix.", sandbox)


if __name__ == "__main__":
    unittest.main()
