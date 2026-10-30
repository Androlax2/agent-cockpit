import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class ArchRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory(prefix="cockpit package ")
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)
        self.runtime = self.root / "usr/lib/agent-cockpit"
        self.commands = self.runtime / "config/bin/.local/bin"
        self.commands.mkdir(parents=True)
        source = ROOT / "config/bin/.local/bin"
        shutil.copyfile(source / "cockpit", self.commands / "cockpit")
        shutil.copytree(source / "cockpit-agents", self.commands / "cockpit-agents")
        self.home = self.root / "home"
        self.home.mkdir()
        self.shims = self.root / "shims"
        self.shims.mkdir()
        self.project = self.root / "project with spaces"
        self.project.mkdir()
        self.environment = {
            name: value
            for name, value in os.environ.items()
            if not name.startswith(("COCKPIT_", "CLAUDE_", "CODEX_", "GREYWALL_"))
        }
        self.environment.update(
            {
                "HOME": str(self.home),
                "XDG_STATE_HOME": str(self.root / "state"),
                "XDG_RUNTIME_DIR": str(self.root / "run"),
                "PATH": str(self.shims) + os.pathsep + os.environ["PATH"],
            }
        )

    def launcher(self, path, identity):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            "#!/usr/bin/env python3\nimport json, os, sys\nfrom pathlib import Path\n"
            f"record = {{'launcher':{identity!r},'arguments':sys.argv[1:],'directory':os.getcwd()}}\n"
            f"Path({str(self.root / 'provider-call.json')!r}).write_text(json.dumps(record))\n"
            "print(json.dumps(record))\n"
        )
        path.chmod(0o755)

    def catalog(self):
        result = subprocess.run(
            ["bash", str(self.commands / "cockpit"), "opencode-models", str(self.project)],
            env=self.environment,
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_packaged_catalog_uses_its_private_launcher_without_home_installation(self):
        (self.runtime / "release.json").write_text('{"version":"0.0.0.test"}')
        self.launcher(self.commands / "greywall-agents/opencode", "package")
        self.assertEqual(
            self.catalog(),
            {
                "launcher": "package",
                "arguments": ["api", "--standalone", "get", "/api/model"],
                "directory": str(self.project),
            },
        )

    def test_checkout_catalog_keeps_the_home_launcher(self):
        self.launcher(self.home / ".local/bin/greywall-agents/opencode", "home")
        self.assertEqual(self.catalog()["launcher"], "home")

    def test_explicit_catalog_launcher_override_keeps_priority_in_package_mode(self):
        (self.runtime / "release.json").write_text('{"version":"0.0.0.test"}')
        override = self.root / "custom launcher"
        self.launcher(override, "override")
        self.environment["COCKPIT_OPENCODE"] = str(override)
        self.assertEqual(self.catalog()["launcher"], "override")

    def codex_background(self):
        tmux = self.shims / "tmux"
        tmux.write_text(
            "#!/usr/bin/env python3\nimport os, subprocess, sys\narguments = sys.argv[1:]\n"
            "if arguments[0] == 'display-message':\n    print('100 40')\n"
            "elif arguments[0] == 'new-session':\n"
            "    os.chdir(arguments[arguments.index('-c') + 1])\n"
            "    raise SystemExit(subprocess.run(arguments[arguments.index('env'):]).returncode)\n"
            "else:\n    raise SystemExit('Unexpected tmux command')\n"
        )
        tmux.chmod(0o755)
        result = subprocess.run(
            [
                "bash",
                str(self.commands / "cockpit"),
                "--agent",
                "codex",
                "background",
                str(self.project),
                "task with spaces",
            ],
            env=self.environment,
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads((self.root / "provider-call.json").read_text())

    def test_packaged_codex_background_uses_its_private_launcher(self):
        (self.runtime / "release.json").write_text('{"version":"0.0.0.test"}')
        self.launcher(self.commands / "greywall-agents/codex", "package-codex")
        self.assertEqual(
            self.codex_background(),
            {
                "launcher": "package-codex",
                "arguments": ["task with spaces"],
                "directory": str(self.project),
            },
        )

    def test_checkout_codex_background_keeps_the_home_launcher(self):
        self.launcher(self.home / ".local/bin/greywall-agents/codex", "home-codex")
        self.assertEqual(self.codex_background()["launcher"], "home-codex")

    def test_explicit_codex_launcher_override_keeps_priority_in_package_mode(self):
        (self.runtime / "release.json").write_text('{"version":"0.0.0.test"}')
        override = self.root / "custom codex launcher"
        self.launcher(override, "override-codex")
        self.environment["COCKPIT_CODEX"] = str(override)
        self.assertEqual(self.codex_background()["launcher"], "override-codex")


if __name__ == "__main__":
    unittest.main()
