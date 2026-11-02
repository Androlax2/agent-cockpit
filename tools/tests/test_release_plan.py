import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/release.yml"
REQUIRED = {
    "plan",
    "draft",
    "gate",
    "install",
    "install-wsl2",
    "compatibility",
    "responsiveness",
    "upgrade",
    "upgrade-wsl2",
}


def command(*arguments):
    return subprocess.run(
        [sys.executable, str(ROOT / "tools/release.py"), *arguments],
        capture_output=True,
        text=True,
        check=False,
    )


def job(name):
    return re.search(rf"\n  {name}:\n((?:    [^\n]*\n|\n)*)", WORKFLOW.read_text())[1]


class ReleasePlanTests(unittest.TestCase):
    def plan(self, version, channel):
        result = command("release-plan", "--version", version, "--channel", channel)
        self.assertEqual(result.returncode, 0, result.stderr)
        return dict(line.split("=", 1) for line in result.stdout.splitlines())

    def test_alpha1_and_its_dry_runs_schedule_only_linux_archives_and_installs(self):
        for version, channel in [
            ("0.1.0-alpha.1", "alpha"),
            ("0.1.0-alpha.7", "alpha"),
            ("0.1.0-dev.123", "dev"),
        ]:
            with self.subTest(version=version):
                plan = self.plan(version, channel)
                self.assertEqual(json.loads(plan["platforms"]), ["linux-x86_64", "linux-aarch64"])
                for matrix in ("build_matrix", "install_matrix"):
                    entries = json.loads(plan[matrix])["include"]
                    self.assertEqual(
                        [entry["platform"] for entry in entries], ["linux-x86_64", "linux-aarch64"]
                    )
                    self.assertTrue(all(entry["runner"].startswith("ubuntu-") for entry in entries))

    def test_future_beta_definitions_keep_both_macos_slices_without_authorizing_release(self):
        plan = self.plan("0.1.0-beta.1", "beta")
        self.assertEqual(
            json.loads(plan["platforms"]), ["linux-x86_64", "linux-aarch64", "macos-universal"]
        )
        entries = json.loads(plan["install_matrix"])["include"]
        self.assertEqual(
            [entry["runner"] for entry in entries if entry["platform"] == "macos-universal"],
            ["macos-26", "macos-26-intel"],
        )
        result = command("gate-gaps", "beta")
        self.assertEqual(result.returncode, 1)
        self.assertIn("#568", result.stdout)

    def test_alpha1_exception_is_not_applied_to_other_cores_or_mismatched_channels(self):
        for version, channel in [
            ("0.2.0-alpha.1", "alpha"),
            ("0.2.0-dev.1", "dev"),
            ("0.1.0-beta.1", "alpha"),
            ("0.1.0-alpha.1", "dev"),
            ("0.1.0", "stable"),
        ]:
            with self.subTest(version=version, channel=channel):
                result = command("release-plan", "--version", version, "--channel", channel)
                self.assertEqual(result.returncode, 1, result.stderr)

    def test_workflow_uses_the_plan_for_every_platform_consumer(self):
        for name, output in [
            ("build", "build_matrix"),
            ("install", "install_matrix"),
            ("upgrade", "install_matrix"),
        ]:
            with self.subTest(job=name):
                self.assertIn(
                    "matrix: ${{ fromJSON(needs.plan.outputs." + output + ") }}", job(name)
                )
                self.assertNotIn("continue-on-error", job(name))
        self.assertIn("PLATFORMS: ${{ needs.plan.outputs.platforms }}", job("upgrade-baseline"))
        self.assertNotIn(
            "for platform in linux-x86_64 linux-aarch64 macos-universal", job("upgrade-baseline")
        )
        for name in ("install-wsl2", "upgrade-wsl2"):
            self.assertIn("archive-linux-x86_64", job(name))
            self.assertIn("microsoft-standard-WSL2", job(name))
        self.assertIn("release-plan", job("plan"))

    def test_the_actual_plan_step_exports_the_matrices_used_by_downstream_jobs(self):
        script = re.search(r"run: (python3 tools/release.py release-plan[^\n]+)", job("plan"))[1]
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "outputs"
            result = subprocess.run(
                ["bash", "-euo", "pipefail", "-c", script],
                cwd=ROOT,
                env=os.environ
                | {"VERSION": "0.1.0-dev.123", "CHANNEL": "dev", "GITHUB_OUTPUT": str(output)},
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(output.read_text(), result.stdout)
            self.assertEqual(
                dict(line.split("=", 1) for line in result.stdout.splitlines()),
                self.plan("0.1.0-dev.123", "dev"),
            )
        makefile = (ROOT / "Makefile").read_text()
        self.assertIn(
            "-p test_release_plan.py", makefile.split("release-check:", 1)[1].split("\n\n", 1)[0]
        )


class PublishContractTests(unittest.TestCase):
    def verify(self, results, channel="alpha"):
        return command("verify-gates", "--channel", channel, "--results", json.dumps(results))

    def results(self):
        return {name: {"result": "success"} for name in REQUIRED}

    def test_every_required_job_must_succeed_and_cannot_be_skipped_or_absent(self):
        for name in REQUIRED:
            for state in ("skipped", "failure", "cancelled", "missing"):
                with self.subTest(job=name, state=state):
                    results = self.results()
                    if state == "missing":
                        del results[name]
                    else:
                        results[name]["result"] = state
                    result = self.verify(results)
                    self.assertEqual(result.returncode, 1, result.stderr)
                    self.assertIn(name, result.stderr)

    def test_no_deferred_macos_job_is_a_dependency_of_alpha1_publication(self):
        needs = {
            name.strip() for name in re.search(r"needs: \[([^]]+)\]", job("publish"))[1].split(",")
        }
        self.assertEqual(needs, REQUIRED)
        results = self.results() | {"macos-deferred": {"result": "skipped"}}
        result = self.verify(results)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_beta_cannot_use_alpha1_results_to_bypass_unproven_gates(self):
        result = self.verify(self.results(), "beta")
        self.assertEqual(result.returncode, 1)
        self.assertIn("#568", result.stderr)

    def test_malformed_job_results_are_refused(self):
        for encoded in ("not JSON", "null", "[]", '{"install":true}'):
            with self.subTest(encoded=encoded):
                result = command("verify-gates", "--channel", "alpha", "--results", encoded)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertNotIn("Traceback", result.stderr)

    def test_the_actual_publish_step_refuses_failed_gates_before_github(self):
        body = job("publish")
        self.assertIn("NEEDS_JSON: ${{ toJSON(needs) }}", body)
        script = re.search(r"run: (python3 tools/release.py verify-gates[^\n]+)", body)[1]
        for state in ("success", "skipped"):
            with self.subTest(state=state):
                results = self.results()
                results["install-wsl2"]["result"] = state
                result = subprocess.run(
                    ["bash", "-euc", script],
                    cwd=ROOT,
                    env=os.environ | {"CHANNEL": "alpha", "NEEDS_JSON": json.dumps(results)},
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(result.returncode, 0 if state == "success" else 1, result.stderr)
        self.assertNotIn("always()", body)
        self.assertNotIn("continue-on-error", body)
        self.assertLess(body.index("verify-gates"), body.index("gh release edit"))


if __name__ == "__main__":
    unittest.main()
