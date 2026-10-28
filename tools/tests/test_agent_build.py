import importlib.machinery
import importlib.util
import json
import os
import select
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
RUNNER = REPO / "config/bin/.local/bin/agent-build"
PROBE = (
    "import json, os; print(json.dumps({'cpus': sorted(os.sched_getaffinity(0)), "
    "'nice': os.getpriority(os.PRIO_PROCESS, 0), 'idle': os.sched_getscheduler(0) == os.SCHED_IDLE}))"
)
LOWEST_NICE = 19
BOOT_ID = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
LOADER = importlib.machinery.SourceFileLoader("agent_build", str(RUNNER))
SPEC = importlib.util.spec_from_loader("agent_build", LOADER)
agent_build = importlib.util.module_from_spec(SPEC)
LOADER.exec_module(agent_build)


def write_gate(home, state, **fields):
    """A status as the host game guard writes it, fresh unless fields say otherwise."""
    path = home / ".cache/agent-shims/heavy-gate.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    now = time.time()
    path.write_text(json.dumps({"version": 1, "state": state, "reason": f"test {state}", "since": now,
                                "updated_at": now, "boot_id": BOOT_ID} | fields))
    return path


def fresh_environment(environment):
    """A job started by the agent itself: no admitted ancestor (make test runs these tests inside
    one, whose marker they would otherwise inherit)."""
    return {key: value for key, value in environment.items() if key != "AGENT_BUILD_ADMITTED"}


def marker_of_this_process():
    text = Path(f"/proc/{os.getpid()}/stat").read_text()
    return f"{os.getpid()}:{text[text.rindex(')') + 2:].split()[19]}"


def read_until(stream, text, seconds=15):
    """What the job printed up to text, without a fixed sleep."""
    seen, deadline = "", time.monotonic() + seconds
    while text not in seen:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([stream], [], [], remaining)[0]:
            raise AssertionError(f"{text!r} not printed; got {seen!r}")
        chunk = os.read(stream.fileno(), 4096).decode()
        if not chunk:
            raise AssertionError(f"stream closed before {text!r}; got {seen!r}")
        seen += chunk
    return seen


class AgentBuildTests(unittest.TestCase):
    def setUp(self):
        self.environment = os.environ.copy()
        self.environment.pop("AGENT_BUILD_CPUS", None)
        # A session's own build shims would apply the installed runner, not the one under test.
        self.environment["PATH"] = os.pathsep.join(
            directory
            for directory in self.environment["PATH"].split(os.pathsep)
            if not (Path(directory) / "agent-build").exists()
        )
        self.available_cpus = sorted(os.sched_getaffinity(0))[:4]
        home = tempfile.TemporaryDirectory()
        self.addCleanup(home.cleanup)
        self.home = Path(home.name)
        write_gate(self.home, "clear")
        self.environment["HOME"] = str(self.home)
        self.environment = fresh_environment(self.environment)

    def run_build(self, *arguments, environment=None, **options):
        return subprocess.run(
            [sys.executable, str(RUNNER), *arguments],
            env=self.environment if environment is None else environment,
            preexec_fn=lambda: os.sched_setaffinity(0, self.available_cpus),
            capture_output=True,
            text=True,
            **options,
        )

    def test_builds_keep_every_available_cpu_thread(self):
        result = self.run_build(sys.executable, "-c", PROBE)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["cpus"], self.available_cpus)

    def test_builds_run_at_the_lowest_priority_without_changing_the_caller(self):
        caller_priority = os.getpriority(os.PRIO_PROCESS, 0)
        caller_policy = os.sched_getscheduler(0)
        result = self.run_build(sys.executable, "-c", PROBE)
        self.assertEqual(result.returncode, 0, result.stderr)
        observed = json.loads(result.stdout)
        self.assertEqual(observed["nice"], LOWEST_NICE)
        self.assertTrue(observed["idle"], observed)
        self.assertEqual(os.getpriority(os.PRIO_PROCESS, 0), caller_priority)
        self.assertEqual(os.sched_getscheduler(0), caller_policy)

    def test_nested_builds_stay_at_the_lowest_priority_on_every_cpu_thread(self):
        result = self.run_build(sys.executable, str(RUNNER), sys.executable, "-c", PROBE)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout),
            {"cpus": self.available_cpus, "nice": LOWEST_NICE, "idle": True},
        )

    def test_cpu_list_left_by_older_sessions_no_longer_pins_builds(self):
        environment = self.environment | {"AGENT_BUILD_CPUS": str(self.available_cpus[0])}
        result = self.run_build(sys.executable, "-c", PROBE, environment=environment)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["cpus"], self.available_cpus)

    def test_streams_arguments_working_directory_and_exit_status_are_preserved(self):
        script = "import os, sys; print(os.getcwd()); print(repr(sys.argv[1:])); print(sys.stdin.read()); print('diagnostic', file=sys.stderr); sys.exit(27)"
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_build(
                sys.executable,
                "-c",
                script,
                "argument with spaces",
                "$(not a shell)",
                cwd=directory,
                input="build input",
            )
            self.assertEqual(result.returncode, 27)
            self.assertEqual(
                result.stdout.splitlines(),
                [directory, "['argument with spaces', '$(not a shell)']", "build input"],
            )
            self.assertEqual(result.stderr, "diagnostic\n")

    def test_missing_tool_is_an_explicit_failure(self):
        result = self.run_build("agent-build-test-tool-that-does-not-exist")
        self.assertEqual(result.returncode, 127)
        self.assertIn("agent-build:", result.stderr)

    def test_tool_shim_finds_the_real_tool_without_recursing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shims = root / "shims"
            tools = root / "tools"
            shims.mkdir()
            tools.mkdir()
            (shims / "cargo").symlink_to(RUNNER)
            tool = tools / "cargo"
            tool.write_text(f"#!{sys.executable}\n{PROBE}\n")
            tool.chmod(0o755)
            environment = self.environment | {"PATH": f"{shims}:{tools}:{self.environment['PATH']}"}
            result = subprocess.run(
                [str(shims / "cargo"), "test"],
                env=environment,
                preexec_fn=lambda: os.sched_setaffinity(0, self.available_cpus),
                capture_output=True,
                text=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                json.loads(result.stdout),
                {"cpus": self.available_cpus, "nice": LOWEST_NICE, "idle": True},
            )

    def test_host_make_builds_run_at_the_lowest_priority(self):
        with tempfile.TemporaryDirectory() as directory:
            tool = Path(directory) / "cargo"
            tool.write_text(f"#!{sys.executable}\n{PROBE}\n")
            tool.chmod(0o755)
            result = subprocess.run(
                ["make", "--no-print-directory", "build"],
                cwd=REPO,
                env=self.environment | {"PATH": f"{directory}:{self.environment['PATH']}"},
                preexec_fn=lambda: os.sched_setaffinity(0, self.available_cpus),
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            observed = json.loads(result.stdout.splitlines()[-1])
            self.assertEqual(
                observed, {"cpus": self.available_cpus, "nice": LOWEST_NICE, "idle": True}
            )


class AdmissionTests(unittest.TestCase):
    """A new heavy job through the real runner, with a status file in a temporary home."""

    def setUp(self):
        home = tempfile.TemporaryDirectory()
        self.addCleanup(home.cleanup)
        self.home = Path(home.name)
        self.marker = self.home / "ran"
        self.environment = fresh_environment(os.environ | {"HOME": str(self.home)})

    def start(self, *arguments, preexec_fn=None, environment=None):
        script = f"from pathlib import Path; Path({str(self.marker)!r}).touch(); print('ran'); raise SystemExit(9)"
        command = arguments or (sys.executable, "-c", script)
        job = subprocess.Popen([sys.executable, str(RUNNER), *command], env=environment or self.environment,
                               preexec_fn=preexec_fn, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                               start_new_session=True)
        def stop():
            # The whole group: a nested job still waiting would keep the pipes open forever.
            try:
                os.killpg(job.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            job.communicate(timeout=10)
        self.addCleanup(stop)
        return job

    def assert_waits(self, job):
        said = read_until(job.stderr, "waits")
        self.assertIsNone(job.poll(), "the job did not wait")
        self.assertFalse(self.marker.exists(), "the job ran while held")
        return said

    def test_held_statuses_make_a_new_job_wait_before_it_runs(self):
        stale = time.time() - 120
        cases = {
            "a game is open": lambda: write_gate(self.home, "gaming"),
            "memory is low": lambda: write_gate(self.home, "memory"),
            "Theo holds jobs": lambda: write_gate(self.home, "held"),
            "the status is missing": lambda: None,
            "the status is stale": lambda: write_gate(self.home, "clear", updated_at=stale),
            "the status is from another boot": lambda: write_gate(self.home, "clear", boot_id="earlier-boot"),
            "the status is not JSON": lambda: write_gate(self.home, "clear").write_text("{"),
            "the status is too large": lambda: write_gate(self.home, "clear", padding="x" * 5000),
            "the status has an unknown state": lambda: write_gate(self.home, "maybe"),
            "the state is a list": lambda: write_gate(self.home, []),
            "the state is an object": lambda: write_gate(self.home, {}),
            "the state is a number": lambda: write_gate(self.home, 5),
            "the times are not numbers": lambda: write_gate(self.home, "clear", updated_at="now"),
            "the status is from the future": lambda: write_gate(self.home, "clear", updated_at=time.time() + 3600),
            "a release is too far ahead": lambda: write_gate(self.home, "gaming", manual_release_until=time.time() + 3 * 86400),
            "the status is a symbolic link": self.plant_symlinked_gate,
        }
        for name, prepare in cases.items():
            with self.subTest(name):
                status = self.home / ".cache/agent-shims/heavy-gate.json"
                if status.exists() or status.is_symlink():
                    status.unlink()
                prepare()
                job = self.start()
                said = self.assert_waits(job)
                os.killpg(job.pid, signal.SIGKILL)
                job.communicate(timeout=10)
                self.assertIn("nothing has run yet", said)
                self.assertIn("cockpit-game-guard release", said)

    def plant_symlinked_gate(self):
        real = write_gate(self.home, "clear").rename(self.home / "real-gate.json")
        (self.home / ".cache/agent-shims/heavy-gate.json").symlink_to(real)

    def test_a_held_job_starts_when_the_hold_ends_and_keeps_its_exit_status(self):
        write_gate(self.home, "gaming")
        job = self.start()
        said = self.assert_waits(job)
        write_gate(self.home, "clear")
        output, errors = job.communicate(timeout=30)
        self.assertEqual((job.returncode, output), (9, "ran\n"), errors)
        self.assertIn("test gaming", said + errors)
        self.assertIn("starts after", errors)

    def test_a_held_job_starts_once_a_malformed_status_is_replaced_by_clear(self):
        write_gate(self.home, [])
        job = self.start()
        self.assert_waits(job)
        write_gate(self.home, "clear")
        output, errors = job.communicate(timeout=30)
        self.assertEqual((job.returncode, output), (9, "ran\n"), errors)

    def test_theos_release_starts_jobs_even_with_a_stale_status(self):
        write_gate(self.home, "gaming", updated_at=time.time() - 600, manual_release_until=time.time() + 600)
        output, errors = self.start().communicate(timeout=10)
        self.assertEqual(output, "ran\n", errors)

    def test_an_expired_release_holds_again(self):
        write_gate(self.home, "gaming", manual_release_until=time.time() - 1)
        self.assert_waits(self.start())

    def test_admission_off_starts_jobs_with_a_warning(self):
        write_gate(self.home, "off", updated_at=0, boot_id="earlier-boot")
        output, errors = self.start().communicate(timeout=10)
        self.assertEqual(output, "ran\n", errors)
        self.assertIn("admission is off", errors)

    def test_a_job_inside_an_admitted_job_never_waits(self):
        write_gate(self.home, "gaming")
        admitted = self.environment | {"AGENT_BUILD_ADMITTED": marker_of_this_process()}
        output, errors = self.start(environment=admitted).communicate(timeout=10)
        self.assertEqual(output, "ran\n", errors)
        self.assertNotIn("waits", errors)

    def test_a_marker_that_is_not_an_ancestor_or_has_another_start_time_waits(self):
        write_gate(self.home, "gaming")
        pid, _, start = marker_of_this_process().partition(":")
        for name, marker in {"wrong start time": f"{pid}:{int(start) + 1}", "not an ancestor": "1:1",
                             "garbage": "yes"}.items():
            with self.subTest(name):
                self.marker.unlink(missing_ok=True)
                job = self.start(environment=self.environment | {"AGENT_BUILD_ADMITTED": marker})
                self.assert_waits(job)
                os.killpg(job.pid, signal.SIGKILL)
                job.communicate(timeout=10)

    def test_a_cheap_call_and_its_children_are_not_admitted(self):
        write_gate(self.home, "gaming")
        tools = self.home / "tools"
        tools.mkdir()
        fake_make = tools / "make"
        nested = f"from pathlib import Path; Path({str(self.marker)!r}).touch()"
        fake_make.write_text(f"#!{sys.executable}\nimport subprocess, sys\n"
                             f"sys.exit(subprocess.call([sys.executable, {str(RUNNER)!r}, sys.executable, '-c', {nested!r}]))\n")
        fake_make.chmod(0o755)
        environment = self.environment | {"PATH": f"{tools}:{self.environment['PATH']}"}
        job = self.start("make", "help", environment=environment)
        self.assert_waits(job)

    def test_mixed_cheap_and_heavy_goals_wait(self):
        write_gate(self.home, "gaming")
        for arguments in (["make", "help", "test"], [sys.executable, "--version", "-c", "pass"]):
            with self.subTest(arguments):
                tools = self.home / "mixed"
                tools.mkdir(exist_ok=True)
                (tools / "make").write_text(f"#!{sys.executable}\nfrom pathlib import Path; Path({str(self.marker)!r}).touch()\n")
                (tools / "make").chmod(0o755)
                self.marker.unlink(missing_ok=True)
                job = self.start(*arguments, environment=self.environment | {"PATH": f"{tools}:{self.environment['PATH']}"})
                self.assert_waits(job)
                os.killpg(job.pid, signal.SIGKILL)
                job.communicate(timeout=10)

    def test_scheduling_priority_alone_never_counts_as_admitted(self):
        write_gate(self.home, "gaming")
        def as_idle_job():
            os.sched_setscheduler(0, os.SCHED_IDLE, os.sched_param(0))
        def as_batch_job():
            os.sched_setscheduler(0, os.SCHED_BATCH, os.sched_param(0))
        def as_niced_job():
            os.setpriority(os.PRIO_PROCESS, 0, 19)
        for name, preexec in {"SCHED_IDLE": as_idle_job, "SCHED_BATCH": as_batch_job, "nice 19": as_niced_job}.items():
            with self.subTest(name):
                self.marker.unlink(missing_ok=True)
                try:
                    job = self.start(preexec_fn=preexec)
                except subprocess.SubprocessError:
                    # Only SCHED_BATCH from an idle parent under RLIMIT_NICE 0; the others still run.
                    self.skipTest(f"cannot start a {name} job here (RLIMIT_NICE 0 under SCHED_IDLE)")
                self.assert_waits(job)
                os.killpg(job.pid, signal.SIGKILL)
                job.communicate(timeout=10)

    def test_asking_a_tool_for_its_version_never_waits(self):
        write_gate(self.home, "gaming")
        job = self.start(sys.executable, "--version")
        output, errors = job.communicate(timeout=10)
        self.assertEqual(job.returncode, 0, errors)
        self.assertIn("Python", output)

    def test_after_a_hold_jobs_start_ten_seconds_apart(self):
        ticket = self.home / ".cache/cockpit/test-lock/admission.lock"
        ticket.parent.mkdir(parents=True)
        ticket.write_text(f"{time.time()}\n")
        slept = []
        original_home, original_sleep = os.environ.get("HOME"), agent_build.time.sleep
        os.environ["HOME"] = str(self.home)
        agent_build.time.sleep = slept.append
        try:
            agent_build.take_release_turn()
        finally:
            agent_build.time.sleep = original_sleep
            os.environ["HOME"] = original_home
        self.assertEqual(len(slept), 1)
        self.assertGreater(slept[0], agent_build.RELEASE_SPACING_S - 1)

    def test_a_job_whose_turn_comes_during_a_new_game_waits_again(self):
        decisions = iter([False, True])
        waits = []
        originals = (agent_build.is_admitted, agent_build.wait_for_admission, agent_build.take_release_turn,
                     agent_build.admission, agent_build.read_gate)
        agent_build.is_admitted = lambda: False
        agent_build.wait_for_admission = lambda label: waits.append(label) or True
        agent_build.take_release_turn = lambda: None
        agent_build.read_gate = lambda path: (None, "test")
        agent_build.admission = lambda gate, problem, now, boot: (next(decisions), None, None)
        try:
            agent_build.admit("cargo build")
        finally:
            (agent_build.is_admitted, agent_build.wait_for_admission, agent_build.take_release_turn,
             agent_build.admission, agent_build.read_gate) = originals
        self.assertEqual(len(waits), 2, "after its turn the job saw a game and waited again")

    def test_waiting_jobs_say_why_once_a_minute_not_on_every_poll(self):
        gates = iter([({"version": 1, "state": "gaming", "reason": "a game is open", "since": 0, "updated_at": 0, "boot_id": BOOT_ID}, None)] * 40
                     + [({"version": 1, "state": "clear", "since": 0, "updated_at": 0, "boot_id": BOOT_ID}, None)])
        now = [0.0]
        lines = []
        class Out:
            def write(self, text): lines.append(text)
            def flush(self): pass
        def sleep(seconds): now[0] += seconds
        def read(path):
            gate, problem = next(gates)
            return gate | {"updated_at": now[0]}, problem
        waited = agent_build.wait_for_admission("cargo test", read=read, clock=lambda: now[0], sleep=sleep, out=Out())
        self.assertTrue(waited)
        self.assertEqual(sum("waits" in line for line in lines), 2)
        self.assertTrue(any("starts after 80 s" in line for line in lines))


if __name__ == "__main__":
    unittest.main()
