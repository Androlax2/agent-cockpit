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
        self.assertIsNotNone(setup, "hosted runners need admission setup before make ci")
        self.assertLess(setup.end(), workflow.index("        run: make ci"))
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

        tool_block = re.search(r"tool: >-\n((?:[ ]{12}\S[^\n]*\n)+)", workflow)
        self.assertIsNotNone(tool_block, "expected explicit pinned installer tools")
        tools = dict(line.strip().split("@") for line in tool_block[1].splitlines())
        self.assertEqual(set(tools), set(reviewed["tools"]))
        for name, version in tools.items():
            with self.subTest(tool=name):
                metadata = reviewed["tools"][name]
                self.assertEqual(version, metadata["version"])
                self.assertIn(metadata["platform"], ("x86_64_linux_gnu", "x86_64_linux_musl"))
                self.assertRegex(metadata["hash"], r"^[0-9a-f]{64}$")
