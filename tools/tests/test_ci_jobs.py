import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/ci-jobs.py"


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
            ".agents/rules/rust.md": {"docs"},
            "Cargo.toml": {"greenfield"},
            "Cargo.lock": {"greenfield"},
            "apps/cockpit/src/lib.rs": {"legacy"},
            "apps/Cargo.toml": {"legacy"},
            "config/bin/.local/bin/agent-build": {"legacy"},
            "tools/quality.py": {"legacy"},
            "packaging/arch/PKGBUILD": {"legacy"},
            "Makefile": {"greenfield", "legacy"},
            ".github/workflows/ci.yml": {"greenfield", "legacy"},
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
        self.assertEqual(selected_jobs("--all"), {"greenfield", "legacy", "docs"})

    def test_a_path_no_job_claims_runs_every_job(self):
        for path in ["rust-toolchain.toml", ".cargo/deny.toml", "assets/runtime/icon.svg"]:
            with self.subTest(path=path):
                self.assertEqual(
                    selected_jobs(changed_paths=["crates/a/src/lib.rs", path]),
                    {"greenfield", "legacy", "docs"},
                )

    def test_an_empty_change_set_runs_every_job(self):
        self.assertEqual(selected_jobs(), {"greenfield", "legacy", "docs"})


if __name__ == "__main__":
    unittest.main()
