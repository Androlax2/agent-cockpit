import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
LOCK = REPO / "tools/full-test-lock.py"


class FullTestLockTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.root = Path(self.folder.name)
        self.lock_folder = self.root / "test-lock"
        self.addCleanup(self.folder.cleanup)

    def start(self, holder, script):
        return subprocess.Popen(
            [
                sys.executable,
                str(LOCK),
                str(self.lock_folder),
                holder,
                sys.executable,
                "-c",
                script,
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

    def start_holder(self, holder="/clones/first"):
        started = self.root / "started"
        release = self.root / "release"
        running = self.start(
            holder,
            f"import os, time\nopen({str(started)!r}, 'w').close()\n"
            f"while not os.path.exists({str(release)!r}): time.sleep(0.02)",
        )
        self.addCleanup(running.communicate)
        # Lets the command finish too when a test killed the lock holder above it.
        self.addCleanup(release.touch)
        self.wait_for(started.exists)
        return running, release

    def wait_for(self, condition, seconds=10):
        deadline = time.monotonic() + seconds
        while not condition():
            self.assertLess(time.monotonic(), deadline, "timed out")
            time.sleep(0.02)

    def test_a_second_full_run_waits_and_names_the_clone_holding_the_lock(self):
        holding, release = self.start_holder("/clones/first")
        waiting = self.start("/clones/second", "print('second ran')")
        time.sleep(0.5)
        self.assertIsNone(waiting.poll(), "the second run did not wait")
        release.touch()
        self.assertEqual(holding.wait(10), 0)
        output, errors = waiting.communicate(timeout=10)
        self.assertEqual(waiting.returncode, 0, errors)
        self.assertIn("Waiting for the test lock (held by /clones/first)", errors)
        self.assertEqual(output, "second ran\n")

    def test_a_run_that_dies_releases_the_lock(self):
        holding, _ = self.start_holder()
        os.kill(holding.pid, signal.SIGKILL)
        holding.wait(10)
        waiting = self.start("/clones/second", "print('second ran')")
        output, errors = waiting.communicate(timeout=10)
        self.assertEqual((waiting.returncode, output), (0, "second ran\n"), errors)

    def test_an_uncontended_run_prints_nothing_extra_and_keeps_its_exit_status(self):
        running = self.start("/clones/only", "import sys; print('ran'); sys.exit(7)")
        output, errors = running.communicate(timeout=10)
        self.assertEqual((running.returncode, output, errors), (7, "ran\n", ""))

    def test_an_unreachable_lock_folder_refuses_to_start_the_command(self):
        self.lock_folder = Path("/proc/full-test-lock-unreachable")
        started = self.root / "command-started"
        running = self.start(
            "/clones/old-session", f"from pathlib import Path; Path({str(started)!r}).touch()"
        )
        output, errors = running.communicate(timeout=10)
        self.assertFalse(started.exists(), "the command ran without the shared lock")
        self.assertEqual((running.returncode, output), (126, ""), errors)
        self.assertIn(f"cannot reach {self.lock_folder}", errors)
        self.assertIn("refusing to run", errors)
        self.assertIn("Restart the session", errors)

    def test_a_planted_symlink_is_refused_without_touching_its_target(self):
        target = self.root / "bashrc"
        target.write_text("keep me\n")
        self.lock_folder.mkdir()
        (self.lock_folder / "full-test.lock").symlink_to(target)
        running = self.start("/clones/host", "print('EXECUTED')")
        output, errors = running.communicate(timeout=10)
        self.assertNotEqual(running.returncode, 0)
        self.assertNotIn("EXECUTED", output)
        self.assertIn("full-test-lock:", errors)
        self.assertEqual(target.read_text(), "keep me\n")


if __name__ == "__main__":
    unittest.main()
