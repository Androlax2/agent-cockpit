import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = Path(__file__).parent / "fixtures/ci-install-action-linux.json"


class CiBootstrapTests(unittest.TestCase):
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
