import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[2] / "packaging/arch/preview_policy.py"


class ArchPolicyPreviewTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)
        self.source = self.root / "policy.json"
        self.output = self.root / "proposed.json"
        self.policy = {
            "network": {"forwardPorts": [8081]},
            "filesystem": {
                "defaultDenyRead": True,
                "allowRead": ["~/agent-cockpit/config/claude", "/run/user/1000/cockpit-report"],
                "allowWrite": ["/run/user/1000/osd.fifo"],
                "denyRead": ["~/.ssh/id_*"],
                "denyWrite": ["~/agent-cockpit/config/greywall", "~/.config/systemd"],
            },
        }

    def run_preview(self, raw=None):
        self.source.write_text(json.dumps(self.policy) if raw is None else raw)
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--policy",
                str(self.source),
                "--output",
                str(self.output),
                "--checkout-root",
                "/home/example/agent-cockpit",
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )

    def test_exact_paths_change_without_changing_other_policy(self):
        result = self.run_preview()
        self.assertEqual(result.returncode, 0, result.stderr)
        expected = json.loads(json.dumps(self.policy))
        expected["filesystem"]["allowRead"][0] = "/usr/lib/agent-cockpit/config/claude"
        expected["filesystem"]["denyWrite"][0] = "/usr/lib/agent-cockpit/config/greywall"
        self.assertEqual(json.loads(self.output.read_text()), expected)
        self.assertEqual(json.loads(self.source.read_text()), self.policy)
        self.assertEqual(self.output.stat().st_mode & 0o777, 0o600)

    def test_similarly_named_paths_are_untouched(self):
        self.policy["filesystem"]["allowRead"] = [
            "~/agent-cockpit-other/config",
            "/tmp/agent-cockpit/config",
        ]
        result = self.run_preview()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(self.output.read_text())["filesystem"]["allowRead"],
            self.policy["filesystem"]["allowRead"],
        )

    def test_whole_package_grants_require_separate_review(self):
        for index, suffix in enumerate(
            ["", "/", "/**", "/./**", "/**/*", "/?*/**", "/[!.]*/**", "/{config,apps}/**"]
        ):
            with self.subTest(suffix=suffix):
                self.output = self.root / f"proposed-{index}.json"
                self.policy["filesystem"]["allowRead"] = ["~/agent-cockpit" + suffix]
                result = self.run_preview()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("whole-root grant", result.stderr)
                self.assertFalse(self.output.exists())

    def test_parent_traversal_is_rejected(self):
        self.policy["filesystem"]["allowRead"] = ["~/agent-cockpit/../.ssh"]
        result = self.run_preview()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("parent traversal", result.stderr)
        self.assertFalse(self.output.exists())

    def test_malformed_grant_lists_are_rejected(self):
        self.policy["filesystem"]["allowRead"] = "~/agent-cockpit/config/claude"
        result = self.run_preview()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("list of paths", result.stderr)
        self.assertFalse(self.output.exists())

    def test_duplicate_keys_are_rejected(self):
        result = self.run_preview('{"filesystem":{"allowRead":[],"allowRead":["/"]}}')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("duplicate key", result.stderr)
        self.assertFalse(self.output.exists())

    def test_existing_learned_profile_header_comments_are_preserved(self):
        header = '// Learned profile for "claude"\n// Updated by greywall\n'
        result = self.run_preview(header + json.dumps(self.policy))
        self.assertEqual(result.returncode, 0, result.stderr)
        proposed = self.output.read_text()
        self.assertTrue(proposed.startswith(header))
        self.assertEqual(
            json.loads(proposed[len(header) :])["filesystem"]["denyRead"],
            ["~/.ssh/id_*"],
        )

    def test_existing_preview_is_preserved(self):
        self.output.write_text("existing review")
        result = self.run_preview()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exists", result.stderr)
        self.assertEqual(self.output.read_text(), "existing review")


if __name__ == "__main__":
    unittest.main()
