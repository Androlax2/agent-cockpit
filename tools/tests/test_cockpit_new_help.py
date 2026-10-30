"""Exercise new-session help without allowing a real session launch."""

import subprocess
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
COCKPIT = REPO / "config/bin/.local/bin/cockpit"


class CockpitNewHelpTests(unittest.TestCase):
    def run_cockpit(self, arguments):
        temporary = tempfile.TemporaryDirectory(prefix="cockpit-new-help-")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        for name in ("home", "runtime", "state", "bin"):
            (root / name).mkdir()
        calls = root / "calls"
        for name in ("git", "make", "tmux", "osd-show", "hyprctl", "kitty", "claude", "opencode"):
            executable = root / "bin" / name
            executable.write_text('#!/bin/sh\nprintf "%s\\n" "$0 $*" >> "$CALL_LOG"\nexit 99\n')
            executable.chmod(0o755)
        result = subprocess.run(
            ["bash", str(COCKPIT), *arguments],
            cwd=root,
            env={
                "HOME": str(root / "home"),
                "XDG_RUNTIME_DIR": str(root / "runtime"),
                "XDG_STATE_HOME": str(root / "state"),
                "PATH": f"{root / 'bin'}:/usr/bin:/bin",
                "CALL_LOG": str(calls),
            },
            capture_output=True,
            text=True,
            timeout=10,
        )
        return result, calls, root / "state/cockpit/log"

    def test_new_help_succeeds_without_validation_errors_or_session_commands(self):
        for prefix in ([], ["--agent", "opencode"]):
            for option in ("--help", "-h"):
                arguments = [*prefix, "new", option]
                with self.subTest(arguments=arguments):
                    result, calls, log = self.run_cockpit(arguments)

                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertIn("usage: cockpit new", result.stdout)
                    self.assertEqual(result.stderr, "")
                    self.assertFalse(calls.exists(), "help invoked a host/session command")
                    self.assertNotIn("ERROR", log.read_text())

    def test_a_nonexistent_directory_still_fails_before_session_launch(self):
        result, calls, _ = self.run_cockpit(["new", "/nonexistent-cockpit-help-fixture"])

        self.assertEqual(result.returncode, 1)
        self.assertIn("refusing directory", result.stderr)
        self.assertFalse(calls.exists(), "invalid directory invoked a host/session command")


if __name__ == "__main__":
    unittest.main()
