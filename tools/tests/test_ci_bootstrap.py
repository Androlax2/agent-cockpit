import importlib.machinery
import importlib.util
import json
import re
import shlex
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from admission_harness import load_runner

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = Path(__file__).parent / "fixtures/ci-install-action-linux.json"


def ci_job(workflow, name):
    job = re.search(
        rf"\n  {re.escape(name)}:\n(.*?)(?=\n  (?:#|[a-z0-9-]+:\n)|\Z)", workflow, re.DOTALL
    )
    if job is None:
        raise AssertionError(f"expected the {name} job in ci.yml")
    return job[1]


class CiBootstrapTests(unittest.TestCase):
    def test_hosted_runner_initializes_admission_before_quality_gate(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        setup = re.search(
            r"      - name: Configure hosted runner admission\n"
            r"        if: runner.environment == 'github-hosted'\n"
            r"(?:        #[^\n]*\n)*"
            r"        run: ([^\n]+)\n",
            workflow,
        )
        self.assertIsNotNone(setup, "hosted runners need admission setup before make greenfield-ci")
        self.assertLess(setup.end(), workflow.index("        run: make greenfield-ci"))
        command = shlex.split(setup[1])
        self.assertEqual(command[:2], ["python3", "config/bin/.local/bin/cockpit-game-guard"])
        loader = importlib.machinery.SourceFileLoader("ci_game_guard", str(ROOT / command[1]))
        guard = importlib.util.module_from_spec(
            importlib.util.spec_from_loader(loader.name, loader)
        )
        loader.exec_module(guard)
        runner = load_runner()
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            with patch.object(guard, "account_home", return_value=home):
                self.assertEqual(guard.main(command[2:]), 0)
            gate = json.loads((home / ".cache/agent-shims/heavy-gate.json").read_text())
            may_start, reason, _warning = runner.admission(
                gate, None, time.time(), guard.current_boot_id()
            )
            self.assertTrue(may_start, reason)

    def test_installer_pin_matches_reviewed_linux_tool_metadata(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        reviewed = json.loads(FIXTURE.read_text())
        installer = re.search(r"taiki-e/install-action@([0-9a-f]{40})", workflow)
        self.assertIsNotNone(installer, "installer must use an immutable commit")
        self.assertEqual(installer[1], reviewed["action_commit"])

        tool_lists = re.findall(r"tool: ([^\n>]+)\n", workflow)
        self.assertTrue(tool_lists, "expected explicit pinned installer tools")
        tools = {}
        for tool_list in tool_lists:
            for entry in tool_list.split(","):
                name, version = entry.strip().split("@")
                self.assertEqual(tools.setdefault(name, version), version, f"{name} pinned twice")
        self.assertEqual(set(tools), set(reviewed["tools"]))
        for name, version in tools.items():
            with self.subTest(tool=name):
                metadata = reviewed["tools"][name]
                self.assertEqual(version, metadata["version"])
                self.assertIn(metadata["platform"], ("x86_64_linux_gnu", "x86_64_linux_musl"))
                self.assertRegex(metadata["hash"], r"^[0-9a-f]{64}$")

    def test_cached_jobs_restore_the_pinned_rust_cache_saved_only_on_main(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        for job, first_build in [
            ("greenfield", "run: make greenfield-ci"),
            ("greenfield-macos", "cargo nextest run"),
        ]:
            with self.subTest(job=job):
                steps = ci_job(workflow, job)
                cache_step = re.search(
                    r"      - uses: Swatinem/rust-cache@d1527c8f67ef9af357290475d4af4b0fab8f12e0"
                    r" # v2\.9\.3\n"
                    r"        with:\n"
                    r"          save-if: \$\{\{ github\.ref == 'refs/heads/main' \}\}\n"
                    r"          cache-bin: false\n",
                    steps,
                )
                self.assertIsNotNone(cache_step, "the reviewed cache pin, saved only from main")
                self.assertLess(cache_step.end(), steps.index(first_build))
        self.assertEqual(workflow.count("Swatinem/rust-cache@"), 2, "reviewed for these two jobs")

    def test_docs_job_installs_the_reviewed_lychee_release_instead_of_compiling_it(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        steps = ci_job(workflow, "docs")
        self.assertIn(
            "https://github.com/lycheeverse/lychee/releases/download/lychee-v0.24.2/"
            "lychee-x86_64-unknown-linux-gnu.tar.gz",
            steps,
        )
        self.assertRegex(
            steps,
            r"\n +1f4e0ef7f6554a6ed33dd7ac144fb2e1bbed98598e7af973042fc5cd43951c9a  lychee\.tar\.gz\n",
        )
        self.assertNotIn("cargo install", workflow)

    def test_wsl_job_installs_the_reviewed_nextest_release(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        reviewed = json.loads(FIXTURE.read_text())["tools"]["cargo-nextest"]
        download = re.search(
            r"cargo-nextest-([0-9.]+)/cargo-nextest-\1-x86_64-unknown-linux-gnu\.tar\.gz", workflow
        )
        self.assertIsNotNone(download, "the WSL job downloads a pinned cargo-nextest release")
        self.assertEqual(download[1], reviewed["version"])
        self.assertRegex(workflow, rf"\n +{reviewed['hash']}  nextest\.tar\.gz\n")
