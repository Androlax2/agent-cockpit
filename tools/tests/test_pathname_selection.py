import os
import socket
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BIN = ROOT / "config/bin/.local/bin"


class PathnameSelectionTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.environment = os.environ | {
            "HOME": str(self.root),
            "XDG_RUNTIME_DIR": str(self.root),
            "HYPRLAND_INSTANCE_SIGNATURE": "inherited",
        }

    def run_section(self, path, start, end, output):
        # Exercise the production selection block without running desktop setup or cleanup.
        source = path.read_text()
        section = source[source.index(start) : source.index(end)]
        result = subprocess.run(
            [
                "bash",
                "-uc",
                'report() { printf "%s\\n" "$1"; }; select_paths() {\n'
                + section
                + output
                + "\n}; select_paths",
            ],
            env=self.environment,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout

    def instance(self, name, age, live=True):
        directory = self.root / "hypr" / name
        directory.mkdir(parents=True)
        if live:
            listener = socket.socket(socket.AF_UNIX)
            self.addCleanup(listener.close)
            listener.bind(str(directory / ".socket.sock"))
        modified = time.time() - age
        os.utime(directory, (modified, modified))

    def test_screenshot_chooses_newest_socket_with_an_intact_path(self):
        self.instance("older", 100)
        self.instance("new space\n[instance]", 50)
        self.instance("newest-without-socket", 0, live=False)
        self.assertEqual(self.selected_instance(), b"new space\n[instance]")

    def selected_instance(self):
        return self.run_section(
            ROOT / "tools/capture-claude.sh",
            "# Started from a terminal",
            "previous_workspace=",
            'printf "%s" "$HYPRLAND_INSTANCE_SIGNATURE"',
        )

    def test_screenshot_keeps_inherited_signature_without_matches(self):
        self.assertEqual(self.selected_instance(), b"inherited")

    def selected_settings(self):
        return self.run_section(
            BIN / "cockpit-agents/opencode.sh",
            "    # Its own folders, from its launcher",
            "    local launcher",
            'printf "%s" "$settings"',
        )

    def test_settings_chooses_newest_complete_filename(self):
        directory = self.root / "greywall-agents"
        directory.mkdir()
        for name, age in [
            ("old.opencode.json", 100),
            ("new \n[settings].opencode.json", 50),
            ("ignored.json", 0),
        ]:
            path = directory / name
            path.write_text("{}")
            modified = time.time() - age
            os.utime(path, (modified, modified))
        self.assertEqual(
            self.selected_settings(), os.fsencode(directory / "new \n[settings].opencode.json")
        )

    def test_settings_without_matches_is_empty(self):
        self.assertEqual(self.selected_settings(), b"")

    def status_report(self):
        return self.run_section(
            BIN / "cockpit",
            "    local recent_facts"
            if "    local recent_facts" in (BIN / "cockpit").read_text()
            else "    local -a recent_facts",
            "    if command -v ask-to-run",
            "",
        )

    def test_status_reads_unusual_filenames_without_splitting(self):
        directory = self.root / ".cache/claude"
        directory.mkdir(parents=True)
        for name in ["statusline-space file.json", "statusline-new\n[line].json"]:
            (directory / name).write_text('{"effort":"high"}')
        self.assertEqual(self.status_report(), b"ok\n")

    def test_status_preserves_filters_and_no_match_result(self):
        directory = self.root / ".cache/claude"
        directory.mkdir(parents=True)
        for name in ["statusline-old.json", "other.json", "nested/statusline-hidden.json"]:
            path = directory / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("{}")
            if "old" in name:
                modified = time.time() - 7200
                os.utime(path, (modified, modified))
        self.assertEqual(self.status_report(), b"ok\n")

    def test_status_missing_effort_still_fails(self):
        directory = self.root / ".cache/claude"
        directory.mkdir(parents=True)
        (directory / "statusline-current.json").write_text("{}")
        self.assertEqual(self.status_report(), b"fail\n")
