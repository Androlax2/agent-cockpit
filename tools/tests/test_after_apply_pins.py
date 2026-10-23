import os
from pathlib import Path
import shutil
import subprocess
import tempfile
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


if __name__ == "__main__":
    unittest.main()
