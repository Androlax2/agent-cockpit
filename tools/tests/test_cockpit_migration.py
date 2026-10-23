"""The host migration must retain data and reject collisions before changing anything."""

import json
import os
from pathlib import Path
import subprocess
import socket
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[2]
MIGRATION = REPO / "tools/migrate-cockpit-names.py"


class MigrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="cockpit-migration-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.home = self.root / "home"
        self.state = self.home / ".local/state"
        self.runtime = self.root / "run"
        self.state.mkdir(parents=True)
        self.runtime.mkdir()
        self.environment = dict(os.environ, HOME=str(self.home), XDG_STATE_HOME=str(self.state),
                                COCKPIT_STATE_HOME=str(self.state), XDG_RUNTIME_DIR=str(self.runtime),
                                XDG_DATA_HOME=str(self.home / ".local/share"))

    def run_migration(self, mode="--apply"):
        return subprocess.run(["python3", MIGRATION, mode], env=self.environment, capture_output=True, text=True)

    def seed(self, path, text="private-data"):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path

    def test_phone_state_survives_with_private_permissions_and_a_legacy_alias(self):
        old = self.state / "claude-remote"
        self.seed(old / "devices.json", '{"devices":[{"sha256":"kept"}]}')
        self.seed(old / "push.json", '{"subscriptions":["kept"]}')
        old.chmod(0o700)
        (old / "devices.json").chmod(0o600)
        output = self.run_migration()
        self.assertEqual(output.returncode, 0, output.stderr)
        new = self.state / "cockpit-remote"
        self.assertEqual((new / "devices.json").read_bytes(), b'{"devices":[{"sha256":"kept"}]}')
        self.assertEqual((new / "push.json").read_bytes(), b'{"subscriptions":["kept"]}')
        self.assertEqual(new.stat().st_mode & 0o777, 0o700)
        self.assertEqual((new / "devices.json").stat().st_mode & 0o777, 0o600)
        self.assertTrue(old.is_symlink())
        self.assertEqual(old.resolve(), new)
        second = self.run_migration()
        self.assertEqual(second.returncode, 0, second.stderr)

    def test_conflicting_directories_leave_every_source_untouched(self):
        untouched = self.seed(self.state / "claude-remote/devices.json")
        self.seed(self.home / ".cache/claude/runs/old.json")
        self.seed(self.home / ".cache/cockpit/runs/new.json")
        output = self.run_migration()
        self.assertNotEqual(output.returncode, 0)
        self.assertIn("conflict", output.stderr.lower())
        self.assertFalse(untouched.parent.is_symlink())
        self.assertFalse((self.state / "cockpit-remote").exists())

    def test_an_unexpected_symlink_is_not_followed(self):
        outside = self.root / "outside"
        outside.mkdir()
        self.seed(outside / "devices.json")
        (self.state / "claude-remote").symlink_to(outside)
        output = self.run_migration()
        self.assertNotEqual(output.returncode, 0)
        self.assertEqual((outside / "devices.json").read_text(), "private-data")

    def test_check_mode_leaves_old_state_unchanged(self):
        old = self.seed(self.state / "claude-remote/devices.json")
        output = self.run_migration("--check")
        self.assertEqual(output.returncode, 0, output.stderr)
        self.assertTrue(old.exists())
        self.assertFalse(old.parent.is_symlink())
        self.assertFalse((self.state / "cockpit-remote").exists())

    def test_a_report_directory_retains_its_inode_for_existing_sandbox_mounts(self):
        old = self.runtime / "claude-report"
        old.mkdir()
        inode = old.stat().st_ino
        output = self.run_migration()
        self.assertEqual(output.returncode, 0, output.stderr)
        self.assertEqual((self.runtime / "cockpit-report").stat().st_ino, inode)
        self.assertEqual(old.stat().st_ino, inode)

    def test_an_open_report_listener_is_reachable_through_both_paths_after_migration(self):
        old = self.runtime / "claude-report"
        old.mkdir()
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(old / "osd-report.sock"))
            listener.listen()
            result = self.run_migration()
            self.assertEqual(result.returncode, 0, result.stderr)
            for folder in ["claude-report", "cockpit-report"]:
                with socket.socket(socket.AF_UNIX) as client:
                    client.connect(str(self.runtime / folder / "osd-report.sock"))
                    with listener.accept()[0] as connection:
                        client.sendall(b"session report")
                        self.assertEqual(connection.recv(100), b"session report")

    def test_an_existing_timer_stamp_is_preserved(self):
        old = self.seed(self.home / ".local/share/systemd/timers/stamp-claude-health-check.timer", "last run")
        timestamp = old.stat().st_mtime_ns
        result = self.run_migration()
        self.assertEqual(result.returncode, 0, result.stderr)
        new = old.with_name("stamp-cockpit-health-check.timer")
        self.assertEqual(new.stat().st_mtime_ns, timestamp)
        self.assertEqual(new.read_text(), "last run")

    def test_existing_cache_children_stay_under_the_old_parent_mount(self):
        old = self.seed(self.home / ".cache/claude/runs/012345abcdef.waiting")
        inode = old.parent.stat().st_ino
        result = self.run_migration()
        self.assertEqual(result.returncode, 0, result.stderr)
        canonical = self.home / ".cache/cockpit/runs"
        self.assertFalse(old.parent.is_symlink(), "old parent-mounted sandboxes must not follow a link outside their mount")
        self.assertTrue(canonical.is_symlink())
        self.assertEqual(canonical.stat().st_ino, inode)
        self.assertEqual((canonical / old.name).read_text(), "private-data")
        self.assertEqual(self.run_migration().returncode, 0)

    def test_repeated_apply_does_not_block_on_requests_that_need_no_path_migration(self):
        result = self.run_migration()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.seed(self.runtime / "cockpit-runs/012345abcdef.asking", "a new approval")
        result = self.run_migration()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_custom_override_folders_are_not_moved(self):
        custom = self.seed(self.root / "custom/devices.json")
        self.environment["CLAUDE_REMOTE_DIR"] = str(custom.parent)
        output = self.run_migration()
        self.assertEqual(output.returncode, 0, output.stderr)
        self.assertTrue(custom.exists())
        self.assertFalse(custom.parent.is_symlink())

    def test_in_flight_requests_refuse_migration_without_changing_state(self):
        self.seed(self.state / "claude-remote/devices.json")
        self.seed(self.runtime / "claude-runs/012345abcdef.asking", json.dumps({"command": "echo hello"}))
        output = self.run_migration()
        self.assertNotEqual(output.returncode, 0)
        self.assertIn("in-flight", output.stderr)
        self.assertFalse((self.state / "claude-remote").is_symlink())

    def service_fixture(self, units):
        self.units = self.root / "units.json"
        self.units.write_text(json.dumps(units))
        shim = self.root / "bin/systemctl"
        shim.parent.mkdir()
        shim.write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
path = Path(os.environ["TEST_UNITS"])
units = json.loads(path.read_text())
args = [arg for arg in sys.argv[1:] if arg not in ["--user", "--quiet"]]
action, unit = args
state = units.setdefault(unit, {"active": False, "enabled": False})
if action in ["is-active", "is-enabled"]:
    flag = state["active" if action == "is-active" else "enabled"]
    print(("active" if action == "is-active" else "enabled") if flag else "disabled")
    sys.exit(0 if flag else 1)
if action in ["start", "stop"]: state["active"] = action == "start"
elif action in ["enable", "disable"]: state["enabled"] = action == "enable"
else: sys.exit(9)
units.setdefault("calls", []).append([action, unit])
path.write_text(json.dumps(units))
''')
        shim.chmod(0o755)
        self.environment.update(TEST_UNITS=str(self.units), PATH=f"{shim.parent}:{os.environ['PATH']}")

    def apply_service_migration(self):
        for mode in ["--prepare", "--apply", "--services"]:
            output = self.run_migration(mode)
            self.assertEqual(output.returncode, 0, output.stderr)
        return json.loads(self.units.read_text())

    def test_active_legacy_phone_and_timer_transfer_without_duplicate_units(self):
        self.service_fixture({"claude-remote.service": {"active": True, "enabled": True},
                              "claude-health-check.timer": {"active": True, "enabled": True}})
        units = self.apply_service_migration()
        self.assertEqual(units["cockpit-remote.service"], {"active": True, "enabled": True})
        self.assertEqual(units["cockpit-health-check.timer"], {"active": True, "enabled": True})
        self.assertEqual(units["claude-remote.service"], {"active": False, "enabled": False})
        calls = units["calls"]
        self.assertLess(calls.index(["stop", "claude-remote.service"]), calls.index(["start", "cockpit-remote.service"]))
        again = self.apply_service_migration()
        self.assertEqual(again["calls"], calls, "an already-migrated service was restarted")

    def test_inactive_disabled_services_stay_inactive_and_disabled(self):
        self.service_fixture({})
        units = self.apply_service_migration()
        self.assertFalse(units.get("cockpit-remote.service", {}).get("active", False))
        self.assertFalse(units.get("cockpit-health-check.timer", {}).get("enabled", False))
        self.assertNotIn("calls", units)

    def test_service_preparation_refuses_a_path_conflict_before_stopping_anything(self):
        self.service_fixture({"claude-remote.service": {"active": True, "enabled": True}})
        self.seed(self.state / "claude-remote/devices.json")
        self.seed(self.state / "cockpit-remote/devices.json")
        output = self.run_migration("--prepare")
        self.assertNotEqual(output.returncode, 0)
        units = json.loads(self.units.read_text())
        self.assertTrue(units["claude-remote.service"]["active"])
        self.assertNotIn("calls", units)


if __name__ == "__main__":
    unittest.main()
