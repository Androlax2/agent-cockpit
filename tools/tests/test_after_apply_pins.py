import http.server
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import unittest


REPO = Path(__file__).resolve().parents[2]


def git(directory, *arguments):
    subprocess.run(["git", "-C", str(directory), "-c", "user.email=t@t", "-c", "user.name=t", *arguments],
                   check=True, capture_output=True)


VERSION_LINE = {"greywall": "  Commit:  {commit}", "greyproxy": "  commit: {commit}"}


class AfterApplyPinTests(unittest.TestCase):
    """make after-apply in a copy of the repository, with stow, systemctl, greywall and greyproxy faked."""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        root = Path(self.directory.name)
        self.repository = root / "repository"
        self.repository.mkdir()
        for name in ["Makefile", "packages"]:
            source = REPO / name
            (shutil.copytree if source.is_dir() else shutil.copy)(source, self.repository / name)
        (self.repository / "tools").mkdir()
        shutil.copy(REPO / "tools/migrate-cockpit-names.py", self.repository / "tools")
        (self.repository / "config/bin").mkdir(parents=True)
        git(self.repository, "init", "-q", "-b", "main")
        git(self.repository, "add", ".")
        git(self.repository, "commit", "-qm", "start")
        self.before = subprocess.run(["git", "-C", str(self.repository), "rev-parse", "HEAD"],
                                     check=True, capture_output=True, text=True).stdout.strip()
        self.bin = root / "bin"
        self.bin.mkdir()
        self.log = root / "calls"
        self.home = root / "home"
        (self.home / ".local/bin").mkdir(parents=True)
        for command in ["stow"]:
            self.fake(self.bin / command, "exit 0")
        self.fake(self.bin / "systemctl", 'case "$2" in show) echo not-found; exit 0;; is-active|is-enabled) exit 1;; esac; exit 0')
        self.fake(self.bin / "curl", """printf '{"conversations":{"enabled":false}}'""")
        runtime = root / "run"
        runtime.mkdir()
        state = self.home / ".local/state"
        state.mkdir(parents=True)
        self.environment = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}", "HOME": str(self.home),
                            "XDG_RUNTIME_DIR": str(runtime), "XDG_STATE_HOME": str(state), "COCKPIT_STATE_HOME": str(state),
                            "XDG_DATA_HOME": str(self.home / ".local/share")}
        for tool in VERSION_LINE:
            self.install(tool, self.pinned(tool))

    def tearDown(self):
        self.directory.cleanup()

    def fake(self, path, body):
        path.write_text(f'#!/bin/sh\necho "$(basename "$0") $*" >> {self.log}\n{body}\n')
        path.chmod(0o755)

    def pinned(self, tool):
        build = (self.repository / f"packages/{tool}/build.sh").read_text()
        return next(line.split("=")[1][:40] for line in build.splitlines() if line.startswith(f"{tool}_commit="))

    def install(self, tool, commit):
        line = VERSION_LINE[tool].format(commit=commit)
        self.fake(self.home / f".local/bin/{tool}", f"printf '{line}\\n'")

    def move_the_pin(self, tool):
        # The real build downloads and compiles; this one installs a binary naming the new pin.
        new_pin = "f" * 40
        line = VERSION_LINE[tool].format(commit=new_pin)
        (self.repository / f"packages/{tool}/build.sh").write_text(
            f"#!/bin/sh\n{tool}_commit={new_pin}\necho rebuilt {tool} >> {self.log}\n"
            f"printf '#!/bin/sh\\nprintf \"{line}\\\\n\"\\n' > \"$HOME/.local/bin/{tool}\"\n")
        git(self.repository, "commit", "-qam", f"Move the {tool} pin")

    def after_apply(self):
        return subprocess.run(["make", "-s", "-C", str(self.repository), "after-apply", f"BEFORE={self.before}"],
                              env=self.environment, capture_output=True, text=True)

    def calls(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def assert_moved_pin_rebuilds_only(self, tool, other):
        self.move_the_pin(tool)
        result = self.after_apply()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"rebuilt {tool}", self.calls())
        self.assertNotIn(f"rebuilt {other}", self.calls())
        self.assertIn(f"{tool} is the pinned build (fffffff)", result.stdout)

    def test_a_moved_greywall_pin_rebuilds_greywall_alone(self):
        self.assert_moved_pin_rebuilds_only("greywall", "greyproxy")

    def test_a_moved_greyproxy_pin_rebuilds_greyproxy_alone(self):
        self.assert_moved_pin_rebuilds_only("greyproxy", "greywall")

    def test_unchanged_pins_rebuild_nothing(self):
        git(self.repository, "commit", "-q", "--allow-empty", "-m", "Something else")
        result = self.after_apply()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse([call for call in self.calls() if call.startswith("rebuilt")], self.calls())

    def test_another_sessions_open_guide_does_not_block_an_unrelated_deployment(self):
        runtime = Path(self.environment["XDG_RUNTIME_DIR"])
        requests = runtime / "cockpit-requests"
        requests.mkdir()
        request = requests / "012345abcdef.guide"
        content = json.dumps({"answers": "project-one", "session": "session-one", "title": "Approve test app"})
        request.write_text(content)
        progress = runtime / "cockpit-answers/project-one/guides/012345abcdef.progress"
        progress.parent.mkdir(parents=True)
        progress.write_text('{"step":2}')
        self.environment["OPENCODE_SESSION_ID"] = "session-two"
        git(self.repository, "commit", "-q", "--allow-empty", "-m", "Unrelated deployment")
        for attempt in range(2):
            with self.subTest(attempt=attempt):
                result = self.after_apply()
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(request.read_text(), content)
                self.assertEqual(progress.read_text(), '{"step":2}')
                self.assertFalse(progress.with_suffix(".result").exists())

    def test_every_apply_checks_greyproxy_conversation_tracking_is_off(self):
        git(self.repository, "commit", "-q", "--allow-empty", "-m", "Something else")
        result = self.after_apply()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("greyproxy conversation tracking is off", result.stdout)

    def assert_behind_its_pin_fails(self, tool):
        self.install(tool, "0" * 40)
        git(self.repository, "commit", "-q", "--allow-empty", "-m", "Something else")
        result = self.after_apply()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f"make -C ~/agent-cockpit {tool}-patch", result.stderr)

    def test_a_greywall_behind_its_pin_fails_the_apply_loudly(self):
        self.assert_behind_its_pin_fails("greywall")

    def test_a_greyproxy_behind_its_pin_fails_the_apply_loudly(self):
        self.assert_behind_its_pin_fails("greyproxy")


class GreyproxyRestartTests(unittest.TestCase):
    """packages/greyproxy/restart.sh with systemctl and curl faked."""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        root = Path(self.directory.name)
        self.bin = root / "bin"
        self.bin.mkdir()
        self.log = root / "calls"
        self.environment = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}", "HOME": str(root),
                            "GREYPROXY_HEALTH_WAIT_S": "2"}

    def tearDown(self):
        self.directory.cleanup()

    def fake(self, command, body):
        path = self.bin / command
        path.write_text(f'#!/bin/sh\necho "{command} $*" >> {self.log}\n{body}\n')
        path.chmod(0o755)

    def restart(self):
        return subprocess.run([str(REPO / "packages/greyproxy/restart.sh")], env=self.environment, capture_output=True, text=True)

    def test_a_healthy_restart_succeeds(self):
        self.fake("systemctl", "exit 0")
        self.fake("curl", "printf 200")
        result = self.restart()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("systemctl --user restart greyproxy", self.log.read_text())

    def test_a_service_that_never_answers_fails_with_the_rollback(self):
        self.fake("systemctl", "exit 0")
        self.fake("curl", "printf 000")
        result = self.restart()
        self.assertEqual(result.returncode, 1)
        self.assertIn("Roll back: cp --preserve=mode", result.stderr)
        self.assertIn("greyproxy.previous", result.stderr)

    def test_a_failed_restart_fails_with_the_rollback(self):
        self.fake("systemctl", "exit 1")
        self.fake("curl", "printf 200")
        result = self.restart()
        self.assertEqual(result.returncode, 1)
        self.assertIn("the restart failed. Roll back", result.stderr)


class FakeGreyproxySettings(http.server.BaseHTTPRequestHandler):
    """greyproxy's /api/settings: GET answers the conversation tracking state, PUT changes it."""

    def log_message(self, *arguments):
        pass

    def answer(self, status, body):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(body).encode())

    def do_GET(self):
        self.server.requests.append(("GET", self.headers.get("Authorization"), None))
        self.answer(200, {"conversations": {"enabled": self.server.tracking}, "theme": "system"})

    def do_PUT(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.server.requests.append(("PUT", self.headers.get("Authorization"), body))
        if self.server.refuses:
            self.answer(500, {"error": "no"})
            return
        self.server.tracking = body["conversations"]["enabled"]
        self.answer(200, {"conversations": {"enabled": self.server.tracking}})


class GreyproxySettingsTests(unittest.TestCase):
    """packages/greyproxy/settings.sh against a fake greyproxy API."""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.data_home = Path(self.directory.name) / "greyproxy"
        self.data_home.mkdir()
        self.server = http.server.HTTPServer(("127.0.0.1", 0), FakeGreyproxySettings)
        self.server.tracking, self.server.refuses, self.server.requests = True, False, []
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.environment = {**os.environ, "GREYPROXY_DATA_HOME": str(self.data_home),
                            "GREYPROXY_API_URL": f"http://127.0.0.1:{self.server.server_port}/api"}

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.directory.cleanup()

    def settings(self):
        return subprocess.run([str(REPO / "packages/greyproxy/settings.sh")], env=self.environment, capture_output=True, text=True)

    def test_tracking_left_on_is_turned_off(self):
        result = self.settings()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.server.tracking)
        self.assertIn(("PUT", None, {"conversations": {"enabled": False}}), self.server.requests)

    def test_tracking_already_off_is_left_alone(self):
        self.server.tracking = False
        result = self.settings()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([method for method, _, _ in self.server.requests], ["GET"])

    def test_the_api_token_is_sent_when_greyproxy_has_one(self):
        (self.data_home / "api-token").write_text("abc123")
        self.settings()
        self.assertEqual({token for _, token, _ in self.server.requests}, {"Bearer abc123"})

    def test_a_refused_change_fails_the_apply_loudly(self):
        self.server.refuses = True
        result = self.settings()
        self.assertEqual(result.returncode, 1)
        self.assertIn("refused to turn conversation tracking off", result.stderr)

    def test_an_unreachable_greyproxy_fails_the_apply_loudly(self):
        self.environment["GREYPROXY_API_URL"] = "http://127.0.0.1:9/api"
        result = self.settings()
        self.assertEqual(result.returncode, 1)
        self.assertIn("Conversation tracking may still be on", result.stderr)


if __name__ == "__main__":
    unittest.main()
