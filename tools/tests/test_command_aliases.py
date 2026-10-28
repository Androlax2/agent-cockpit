"""Legacy entry points must forward transparently to the canonical commands."""

import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
COMMANDS = [
    "todo",
    "remote",
    "design",
    "guide",
    "fade",
    "run-approve",
    "demo",
    "needs-input",
    "session-label",
    "health-check",
    "idle-prompt",
    "sessions",
]


class CommandAliasesTests(unittest.TestCase):
    def test_legacy_commands_preserve_arguments_and_exit_codes(self):
        for name in COMMANDS:
            with self.subTest(command=name), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                legacy = root / f"claude-{name}"
                source = REPO / "config/bin/.local/bin" / legacy.name
                self.assertTrue(source.exists(), f"missing compatibility command: {source.name}")
                shutil.copy2(source, legacy)
                canonical = root / ("cockpit" if name == "sessions" else f"cockpit-{name}")
                canonical.write_text(
                    "#!/usr/bin/env python3\nimport json, sys\nprint(json.dumps(sys.argv[1:]))\nsys.exit(37)\n"
                )
                canonical.chmod(0o755)
                output = subprocess.run(
                    [legacy, "one spaced argument", "", "--flag"], capture_output=True, text=True
                )
                self.assertEqual(output.returncode, 37, output.stderr)
                self.assertEqual(json.loads(output.stdout), ["one spaced argument", "", "--flag"])


if __name__ == "__main__":
    unittest.main()
