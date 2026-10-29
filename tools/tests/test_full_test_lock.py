import fcntl
import json
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
# Test-only: runs the shipped agent-build with only its account home replaced (never installed).
HARNESS = REPO / "tools/tests/admission_harness.py"
BOOT_ID = Path("/proc/sys/kernel/random/boot_id").read_text().strip()


def write_gate(home, state, **fields):
    """A status as the host game guard writes it, fresh unless fields say otherwise."""
    path = home / ".cache/agent-shims/heavy-gate.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    now = time.time()
    path.write_text(
        json.dumps(
            {
                "version": 1,
                "state": state,
                "reason": f"test {state}",
                "since": now,
                "updated_at": now,
                "boot_id": BOOT_ID,
            }
            | fields
        )
    )


def marker_of_this_process():
    """An admitted ancestor for a run this test starts."""
    text = Path(f"/proc/{os.getpid()}/stat").read_text()
    return f"{os.getpid()}:{text[text.rindex(')') + 2 :].split()[19]}"


class FullTestLockTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.root = Path(self.folder.name)
        self.lock_folder = self.root / "test-lock"
        self.addCleanup(self.folder.cleanup)
        self.home = self.root / "home"
        write_gate(self.home, "clear")
        self.lock = self.stage_layout()

    def stage_layout(self):
        """A byte copy of the committed lock helper, next to an adapter in agent-build's place: the
        shipped helper finds it through its own unchanged parents[1] rule, and the adapter runs the
        shipped agent-build with the fixture home as its account home."""
        committed = subprocess.run(
            ["git", "-C", str(REPO), "rev-parse", "HEAD:tools/full-test-lock.py"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
        working = subprocess.run(
            ["git", "hash-object", str(LOCK)], capture_output=True, text=True, check=True
        ).stdout.strip()
        if working != committed:
            raise AssertionError(
                f"tools/full-test-lock.py is {working}, not the committed {committed}: commit or restore it"
            )
        layout = self.root / "layout"
        lock = layout / "tools/full-test-lock.py"
        lock.parent.mkdir(parents=True)
        lock.write_bytes(LOCK.read_bytes())
        copied = subprocess.run(
            ["git", "hash-object", str(lock)], capture_output=True, text=True, check=True
        ).stdout.strip()
        if copied != committed:
            raise AssertionError(f"the copied lock helper is {copied}, not {committed}")
        adapter = layout / "config/bin/.local/bin/agent-build"
        adapter.parent.mkdir(parents=True)
        adapter.write_text(
            f"import os, sys\nos.execv(sys.executable, [sys.executable, {str(HARNESS)!r}, "
            f"{str(self.home)!r}, 'agent-build', *sys.argv[1:]])\n"
        )
        return lock

    def start(self, holder, script, admitted=False):
        return subprocess.Popen(
            [
                sys.executable,
                str(self.lock),
                str(self.lock_folder),
                holder,
                sys.executable,
                "-c",
                script,
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            # A fresh run unless admitted: make test runs these tests inside an admitted run.
            env={key: value for key, value in os.environ.items() if key != "AGENT_BUILD_ADMITTED"}
            | ({"AGENT_BUILD_ADMITTED": marker_of_this_process()} if admitted else {}),
        )

    def lock_is_free(self):
        self.lock_folder.mkdir(exist_ok=True)
        with open(self.lock_folder / "full-test.lock", "a") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return False
            fcntl.flock(lock, fcntl.LOCK_UN)
            return True

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

    def test_a_fresh_run_waits_for_admission_without_holding_the_lock(self):
        write_gate(self.home, "gaming")
        started = self.root / "command-started"
        waiting = self.start(
            "/clones/gaming",
            f"from pathlib import Path; Path({str(started)!r}).touch(); print('ran')",
        )
        self.addCleanup(lambda: waiting.poll() is None and waiting.kill())
        time.sleep(1)
        self.assertIsNone(waiting.poll(), "the run did not wait for admission")
        self.assertFalse(started.exists())
        self.assertTrue(
            self.lock_is_free(), "a run waiting for admission holds the shared test lock"
        )
        write_gate(self.home, "clear")
        output, errors = waiting.communicate(timeout=30)
        self.assertEqual((waiting.returncode, output), (0, "ran\n"), errors)
        self.assertIn("waits", errors)
        self.assertIn("test gaming", errors)

    def test_a_run_queued_for_the_lock_checks_admission_again_and_gives_the_lock_back(self):
        holding, release = self.start_holder("/clones/first")
        started = self.root / "second-started"
        queued = self.start(
            "/clones/second",
            f"from pathlib import Path; Path({str(started)!r}).touch(); print('second ran')",
        )
        self.addCleanup(lambda: queued.poll() is None and queued.kill())
        time.sleep(0.5)
        self.assertIsNone(queued.poll(), "the second run did not queue for the lock")
        write_gate(self.home, "gaming")
        release.touch()
        self.assertEqual(holding.wait(10), 0)
        time.sleep(1)
        self.assertIsNone(queued.poll(), "the queued run started although a game began")
        self.assertFalse(started.exists())
        self.assertTrue(self.lock_is_free(), "the queued run kept the lock while held")
        write_gate(self.home, "clear")
        output, errors = queued.communicate(timeout=30)
        self.assertEqual((queued.returncode, output), (0, "second ran\n"), errors)
        self.assertIn("gave the lock back and waits again", errors)

    def test_a_run_inside_an_admitted_job_does_not_wait(self):
        write_gate(self.home, "gaming")
        running = self.start(
            "/clones/nested", "import sys; print('ran'); sys.exit(5)", admitted=True
        )
        self.addCleanup(lambda: running.poll() is None and running.kill())
        output, errors = running.communicate(timeout=10)
        self.assertEqual((running.returncode, output), (5, "ran\n"), errors)

    def test_builds_started_while_holding_the_lock_never_wait_when_a_game_starts(self):
        go = self.root / "go"
        started = self.root / "suite-started"
        script = (
            f"import os, subprocess, sys, time\n"
            f"open({str(started)!r}, 'w').close()\n"
            f"while not os.path.exists({str(go)!r}): time.sleep(0.02)\n"
            f"sys.exit(subprocess.call([sys.executable, {str(HARNESS)!r}, {str(self.home)!r}, 'agent-build', "
            f"sys.executable, '-c', 'print(\\'nested ran\\')']))"
        )
        running = self.start("/clones/admitted", script)
        self.addCleanup(lambda: running.poll() is None and running.kill())
        self.wait_for(started.exists)
        write_gate(self.home, "gaming")
        go.touch()
        output, errors = running.communicate(timeout=10)
        self.assertEqual((running.returncode, output), (0, "nested ran\n"), errors)
        self.assertNotIn("waits", errors)

    def test_an_unreachable_lock_folder_still_refuses_after_admission(self):
        self.lock_folder = Path("/proc/full-test-lock-unreachable")
        running = self.start("/clones/old-session", "print('EXECUTED')")
        output, errors = running.communicate(timeout=10)
        self.assertEqual((running.returncode, output), (126, ""), errors)


if __name__ == "__main__":
    unittest.main()
