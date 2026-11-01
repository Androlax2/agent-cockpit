import importlib.util
import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/ci-jobs.py"
EVERY_JOB = {"greenfield", "platform", "docs"}
PLATFORM_CFG = re.compile(r"#!?\[cfg\([^\]]*\b(target_os|target_family|unix|windows)\b")


def platform_crates():
    spec = importlib.util.spec_from_file_location("ci_jobs", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return set(module.PLATFORM_CRATES)


def selected_jobs(*arguments, changed_paths=()):
    result = subprocess.run(
        ["python3", str(SCRIPT), *arguments],
        input="".join(f"{path}\n" for path in changed_paths),
        capture_output=True,
        text=True,
        check=True,
    )
    outputs = dict(line.split("=") for line in result.stdout.splitlines())
    return {job for job, runs in outputs.items() if runs == "true"}


class CiJobSelectionTests(unittest.TestCase):
    def test_each_change_runs_only_the_jobs_whose_paths_it_touches(self):
        cases = {
            "crates/cockpit-workspace-rules/src/lib.rs": {"greenfield"},
            "crates/cockpit-workspace-rules/README.md": {"greenfield", "docs"},
            "crates/cockpit-tasks/src/lib.rs": {"greenfield"},
            "crates/cockpit-daemon/src/main.rs": {"greenfield", "platform"},
            "crates/cockpit-tui/tests/e2e/harness.rs": {"greenfield", "platform"},
            ".agents/rules/rust.md": {"docs"},
            "Cargo.toml": {"greenfield", "platform"},
            "Cargo.lock": {"greenfield", "platform"},
            "Makefile": {"greenfield"},
            ".github/workflows/ci.yml": {"greenfield", "platform"},
            ".github/workflows/release.yml": {"greenfield"},
            "docs/DEVELOPMENT.md": {"docs"},
            "docs/design/greenfield/20-refactoring-plan.md": {"docs"},
            "README.md": {"docs"},
        }
        for path, expected_jobs in cases.items():
            with self.subTest(path=path):
                self.assertEqual(selected_jobs(changed_paths=[path]), expected_jobs)

    def test_a_change_set_runs_the_union_of_its_paths_jobs(self):
        self.assertEqual(
            selected_jobs(changed_paths=["crates/a/src/lib.rs", "docs/DECISIONS.md"]),
            {"greenfield", "docs"},
        )

    def test_all_runs_every_job_whatever_changed(self):
        self.assertEqual(selected_jobs("--all"), EVERY_JOB)

    def test_a_path_no_job_claims_runs_every_job(self):
        for path in ["rust-toolchain.toml", ".cargo/deny.toml", "assets/runtime/icon.svg"]:
            with self.subTest(path=path):
                self.assertEqual(
                    selected_jobs(changed_paths=["crates/a/src/lib.rs", path]),
                    EVERY_JOB,
                )

    def test_legacy_paths_have_no_ci_job_and_run_everything_as_unclaimed(self):
        for path in [
            "apps/cockpit/src/lib.rs",
            "config/bin/.local/bin/agent-build",
            "packaging/arch/PKGBUILD",
        ]:
            with self.subTest(path=path):
                self.assertEqual(selected_jobs(changed_paths=[path]), EVERY_JOB)

    def test_an_empty_change_set_runs_every_job(self):
        self.assertEqual(selected_jobs(), EVERY_JOB)

    def test_every_crate_with_a_platform_cfg_runs_the_platform_jobs(self):
        listed = platform_crates()
        for crate in sorted((ROOT / "crates").iterdir()):
            sources = [*crate.glob("src/**/*.rs"), *crate.glob("tests/**/*.rs")]
            if any(PLATFORM_CFG.search(source.read_text()) for source in sources):
                with self.subTest(crate=crate.name):
                    self.assertIn(crate.name, listed)

    def test_every_crate_a_platform_job_tests_runs_the_platform_jobs(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        tested = set(re.findall(r"cargo nextest run [^\n]*?-p (cockpit-[a-z-]+)", workflow))
        self.assertTrue(tested, "expected the platform jobs' cargo nextest runs")
        self.assertLessEqual(tested, platform_crates())


if __name__ == "__main__":
    unittest.main()
