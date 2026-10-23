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
        self.assertFalse(old.is_symlink(), "old sandbox masks protect this physical directory")
        self.assertTrue(new.is_symlink())
        self.assertEqual(new.resolve(), old)
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

    def test_pending_approvals_keep_their_original_request_through_migration(self):
        self.seed(self.state / "claude-remote/devices.json")
        self.seed(self.runtime / "claude-runs/012345abcdef.asking", json.dumps({"command": "echo hello"}))
        output = self.run_migration()
        self.assertEqual(output.returncode, 0, output.stderr)
        self.assertEqual((self.runtime / "claude-runs/012345abcdef.asking").read_bytes(),
                         (self.runtime / "cockpit-runs/012345abcdef.asking").read_bytes())
        self.assertFalse((self.state / "claude-remote").is_symlink())

    def test_an_unfinished_guide_survives_path_migration_without_losing_progress(self):
        request = self.seed(self.runtime / "claude-requests/012345abcdef.guide",
                            json.dumps({"answers": "project-one", "session": "session-one", "steps": []}))
        progress = self.seed(self.runtime / "claude-answers/project-one/guides/012345abcdef.progress", '{"step":2}')
        inodes = (request.stat().st_ino, progress.stat().st_ino)
        output = self.run_migration()
        self.assertEqual(output.returncode, 0, output.stderr)
        canonical_request = self.runtime / "cockpit-requests" / request.name
        canonical_progress = self.runtime / "cockpit-answers/project-one/guides" / progress.name
        self.assertEqual((canonical_request.stat().st_ino, canonical_progress.stat().st_ino), inodes)
        self.assertEqual(canonical_progress.read_text(), '{"step":2}')
        self.assertFalse(progress.with_suffix(".result").exists(), "migration must not answer the guide")
        self.assertEqual(self.run_migration().returncode, 0)

    def test_only_the_daemon_restart_is_blocked_and_identifies_the_guide_owner(self):
        self.seed(self.runtime / "cockpit-requests/012345abcdef.guide",
                  json.dumps({"answers": "project-one", "session": "session-one", "title": "Approve test app"}))
        output = self.run_migration("--check-guides")
        self.assertNotEqual(output.returncode, 0)
        self.assertIn("in-flight guide", output.stderr)
        self.assertIn("session-one", output.stderr)
        self.assertIn("Approve test app", output.stderr)
        self.assertIn("daemon restart", output.stderr)

    def test_a_guide_without_a_valid_answer_folder_still_blocks_the_restart(self):
        self.seed(self.runtime / "cockpit-requests/012345abcdef.guide", '{"answers":"../outside"}')
        output = self.run_migration("--check-guides")
        self.assertNotEqual(output.returncode, 0)
        self.assertIn("no valid answer folder", output.stderr)

    def test_a_completed_guide_does_not_block_deployment(self):
        self.seed(self.runtime / "claude-requests/012345abcdef.guide", json.dumps({"answers": "project-one"}))
        self.seed(self.runtime / "claude-answers/project-one/guides/012345abcdef.result", '{"status":"done"}')
        output = self.run_migration()
        self.assertEqual(output.returncode, 0, output.stderr)

    def test_queue_consumption_cannot_remove_a_compatibility_symlink_or_block_a_retry(self):
        old = self.seed(self.runtime / "claude-todo-updates.jsonl", '{"action":"add","text":"once"}\n')
        output = self.run_migration()
        self.assertEqual(output.returncode, 0, output.stderr)
        self.assertFalse(old.is_symlink(), "a consumer renames and deletes this queue")
        self.assertEqual(old.read_text(), '{"action":"add","text":"once"}\n')
        old.rename(old.with_suffix(".consumed"))
        new = self.seed(self.runtime / "cockpit-todo-updates.jsonl", "next update")
        new.rename(new.with_suffix(".consumed"))
        output = self.run_migration()
        self.assertEqual(output.returncode, 0, output.stderr)

    def test_legacy_and_canonical_queues_can_coexist_without_a_migration_collision(self):
        old = self.seed(self.runtime / "claude-todo-updates.jsonl", "legacy update")
        new = self.seed(self.runtime / "cockpit-todo-updates.jsonl", "canonical update")
        output = self.run_migration()
        self.assertEqual(output.returncode, 0, output.stderr)
        self.assertEqual(old.read_text(), "legacy update")
        self.assertEqual(new.read_text(), "canonical update")

    def test_consuming_the_canonical_queue_does_not_leave_a_dangling_migration_alias(self):
        output = self.run_migration()
        self.assertEqual(output.returncode, 0, output.stderr)
        queue = self.seed(self.runtime / "cockpit-todo-updates.jsonl", "consumed update")
        queue.rename(queue.with_suffix(".consumed"))
        output = self.run_migration()
        self.assertEqual(output.returncode, 0, output.stderr)

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
action, unit = args[:2]
state = units.get(unit, {"active": False, "enabled": False})
if action == "show":
    prop = next(arg.split("=", 1)[1] for arg in args if arg.startswith("--property="))
    if prop == "LoadState": print("loaded" if unit in units else "not-found")
    elif prop == "ActiveState": print(state.get("state", "active" if state["active"] else "inactive"))
    else: sys.exit(9)
    sys.exit(0)
if action in ["is-active", "is-enabled"]:
    flag = state["active" if action == "is-active" else "enabled"]
    print(("active" if action == "is-active" else "enabled") if flag else "disabled")
    sys.exit(0 if flag else 1)
if action == "start" and os.environ.get("TEST_FAIL_START") and Path(os.environ["TEST_FAIL_START"]).exists():
    print("injected start failure", file=sys.stderr)
    sys.exit(1)
units[unit] = state
if action in ["start", "stop"]:
    state["active"] = action == "start"
    if "state" in state: state["state"] = "active" if state["active"] else "inactive"
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
        self.assertEqual(again["calls"].count(["start", "cockpit-remote.service"]), 1, "an already-migrated service was restarted")

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

    def test_activating_legacy_services_are_stopped_before_the_new_services_start(self):
        self.service_fixture({"claude-remote.service": {"active": False, "state": "activating", "enabled": True},
                              "claude-health-check.service": {"active": False, "state": "activating", "enabled": False}})
        units = self.apply_service_migration()
        self.assertEqual(units["claude-remote.service"]["state"], "inactive")
        self.assertTrue(units["cockpit-remote.service"]["active"])
        self.assertEqual(units["claude-health-check.service"]["state"], "inactive")
        self.assertTrue(units["cockpit-health-check.service"]["active"])
        calls = units["calls"]
        self.assertLess(calls.index(["stop", "claude-remote.service"]), calls.index(["start", "cockpit-remote.service"]))

    def test_a_failed_start_retains_original_intent_for_a_successful_retry(self):
        self.service_fixture({"claude-remote.service": {"active": True, "enabled": True}})
        failure = self.seed(self.root / "fail-start")
        self.environment["TEST_FAIL_START"] = str(failure)
        for mode in ["--prepare", "--apply"]:
            output = self.run_migration(mode)
            self.assertEqual(output.returncode, 0, output.stderr)
        output = self.run_migration("--services")
        self.assertNotEqual(output.returncode, 0)
        self.assertTrue((self.runtime / "cockpit-name-services.json").exists())
        failure.unlink()
        units = self.apply_service_migration()
        self.assertTrue(units["cockpit-remote.service"]["active"])
        self.assertTrue(units["cockpit-remote.service"]["enabled"])
        self.assertFalse(units["claude-remote.service"]["active"])
        self.assertFalse((self.runtime / "cockpit-name-services.json").exists())


if __name__ == "__main__":
    unittest.main()
