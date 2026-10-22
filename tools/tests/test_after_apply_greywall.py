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


class AfterApplyGreywallTests(unittest.TestCase):
    """make after-apply in a copy of the repository, with stow, systemctl and greywall faked."""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        root = Path(self.directory.name)
        self.repository = root / "repository"
        self.repository.mkdir()
        for name in ["Makefile", "packages"]:
            source = REPO / name
            (shutil.copytree if source.is_dir() else shutil.copy)(source, self.repository / name)
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
        for command in ["stow", "systemctl"]:
            self.fake(self.bin / command, "exit 0")
        self.environment = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}", "HOME": str(self.home)}

    def tearDown(self):
        self.directory.cleanup()

    def fake(self, path, body):
        path.write_text(f'#!/bin/sh\necho "$(basename "$0") $*" >> {self.log}\n{body}\n')
        path.chmod(0o755)

    def pinned(self):
        build = (self.repository / "packages/greywall/build.sh").read_text()
        return next(line.split("=")[1][:40] for line in build.splitlines() if line.startswith("greywall_commit="))

    def install_greywall(self, commit):
        self.fake(self.home / ".local/bin/greywall", f"printf '  Commit:  {commit}\\n'")

    def move_the_pin(self):
        # The real build downloads and compiles; this one installs a greywall naming the new pin.
        new_pin = "f" * 40
        build = self.repository / "packages/greywall/build.sh"
        build.write_text(f"#!/bin/sh\ngreywall_commit={new_pin}\necho rebuilt >> {self.log}\n"
                         f"printf '#!/bin/sh\\nprintf \"  Commit:  {new_pin}\\\\n\"\\n' > \"$HOME/.local/bin/greywall\"\n")
        git(self.repository, "commit", "-qam", "Move the greywall pin")

    def after_apply(self):
        return subprocess.run(["make", "-s", "-C", str(self.repository), "after-apply", f"BEFORE={self.before}"],
                              env=self.environment, capture_output=True, text=True)

    def calls(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def test_a_moved_pin_rebuilds_greywall_and_checks_the_new_build(self):
        self.install_greywall(self.pinned())
        self.move_the_pin()
        result = self.after_apply()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("rebuilt", self.calls())
        self.assertIn("greywall is the pinned build (fffffff)", result.stdout)

    def test_an_unchanged_pin_rebuilds_nothing(self):
        self.install_greywall(self.pinned())
        git(self.repository, "commit", "-q", "--allow-empty", "-m", "Something else")
        result = self.after_apply()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("rebuilt", self.calls())

    def test_a_greywall_behind_its_pin_fails_the_apply_loudly(self):
        self.install_greywall("0" * 40)
        git(self.repository, "commit", "-q", "--allow-empty", "-m", "Something else")
        result = self.after_apply()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("make -C ~/agent-cockpit greywall-patch", result.stderr)


if __name__ == "__main__":
    unittest.main()
