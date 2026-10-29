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
# Test-only: runs the shipped runner with only its account home replaced (never installed).
HARNESS = REPO / "tools/tests/admission_harness.py"
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


def through_harness(fixture_home, argv0="agent-build"):
    """The command prefix that runs the shipped runner with fixture_home as its account home."""
    return [sys.executable, str(HARNESS), str(fixture_home), argv0]


def write_shim(directory, name, fixture_home):
    """A shim named like a tool, as the launcher's cargo -> agent-build link, through the harness."""
    shim = directory / name
    shim.write_text(f'#!/bin/sh\nexec {sys.executable} {HARNESS} {fixture_home} "$0" "$@"\n')
    shim.chmod(0o755)
    return shim


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
    return path


def fresh_environment(environment):
    """A job started by the agent itself: no admitted ancestor (make test runs these tests inside
    one, whose marker they would otherwise inherit)."""
    return {key: value for key, value in environment.items() if key != "AGENT_BUILD_ADMITTED"}


def marker_of_this_process():
    text = Path(f"/proc/{os.getpid()}/stat").read_text()
    return f"{os.getpid()}:{text[text.rindex(')') + 2 :].split()[19]}"


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
        self.environment = fresh_environment(self.environment)

    def run_build(self, *arguments, environment=None, **options):
        return subprocess.run(
            [*through_harness(self.home), *arguments],
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
        result = self.run_build(*through_harness(self.home), sys.executable, "-c", PROBE)
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
            write_shim(shims, "cargo", self.home)
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
            # make runs the shipped runner, which reads the account's real status: this test runs
            # inside an admitted ancestor (itself) so it checks priority, not admission.
            result = subprocess.run(
                ["make", "--no-print-directory", "build"],
                cwd=REPO,
                env=self.environment
                | {
                    "PATH": f"{directory}:{self.environment['PATH']}",
                    "AGENT_BUILD_ADMITTED": marker_of_this_process(),
                },
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
    """A new heavy job through the shipped runner, with the account home replaced by a temporary
    folder (harness); HOME points elsewhere, so nothing here can come from HOME."""

    def setUp(self):
        home = tempfile.TemporaryDirectory()
        self.addCleanup(home.cleanup)
        self.home = Path(home.name)
        self.marker = self.home / "ran"
        other_home = tempfile.TemporaryDirectory()
        self.addCleanup(other_home.cleanup)
        self.other_home = Path(other_home.name)
        self.environment = fresh_environment(os.environ | {"HOME": str(self.other_home)})

    def start(self, *arguments, preexec_fn=None, environment=None):
        script = f"from pathlib import Path; Path({str(self.marker)!r}).touch(); print('ran'); raise SystemExit(9)"
        command = arguments or (sys.executable, "-c", script)
        job = subprocess.Popen(
            [*through_harness(self.home), *command],
            env=environment or self.environment,
            preexec_fn=preexec_fn,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )

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
            "the status is from another boot": lambda: write_gate(
                self.home, "clear", boot_id="earlier-boot"
            ),
            "the status is not JSON": lambda: write_gate(self.home, "clear").write_text("{"),
            "the status is too large": lambda: write_gate(self.home, "clear", padding="x" * 5000),
            "the status has an unknown state": lambda: write_gate(self.home, "maybe"),
            "the state is a list": lambda: write_gate(self.home, []),
            "the state is an object": lambda: write_gate(self.home, {}),
            "the state is a number": lambda: write_gate(self.home, 5),
            "the times are not numbers": lambda: write_gate(self.home, "clear", updated_at="now"),
            "the status is from the future": lambda: write_gate(
                self.home, "clear", updated_at=time.time() + 3600
            ),
            "a release is too far ahead": lambda: write_gate(
                self.home, "gaming", manual_release_until=time.time() + 3 * 86400
            ),
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
        write_gate(
            self.home,
            "gaming",
            updated_at=time.time() - 600,
            manual_release_until=time.time() + 600,
        )
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
        for name, marker in {
            "wrong start time": f"{pid}:{int(start) + 1}",
            "not an ancestor": "1:1",
            "garbage": "yes",
        }.items():
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
        fake_make.write_text(
            f"#!{sys.executable}\nimport subprocess, sys\n"
            f"sys.exit(subprocess.call([*{through_harness(self.home)!r}, sys.executable, '-c', {nested!r}]))\n"
        )
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
                (tools / "make").write_text(
                    f"#!{sys.executable}\nfrom pathlib import Path; Path({str(self.marker)!r}).touch()\n"
                )
                (tools / "make").chmod(0o755)
                self.marker.unlink(missing_ok=True)
                job = self.start(
                    *arguments,
                    environment=self.environment | {"PATH": f"{tools}:{self.environment['PATH']}"},
                )
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

        for name, preexec in {
            "SCHED_IDLE": as_idle_job,
            "SCHED_BATCH": as_batch_job,
            "nice 19": as_niced_job,
        }.items():
            with self.subTest(name):
                self.marker.unlink(missing_ok=True)
                try:
                    job = self.start(preexec_fn=preexec)
                except subprocess.SubprocessError:
                    # Only SCHED_BATCH from an idle parent under RLIMIT_NICE 0; the others still run.
                    self.skipTest(
                        f"cannot start a {name} job here (RLIMIT_NICE 0 under SCHED_IDLE)"
                    )
                self.assert_waits(job)
                os.killpg(job.pid, signal.SIGKILL)
                job.communicate(timeout=10)

    def test_a_forged_clear_status_under_home_cannot_admit_while_the_account_status_holds(self):
        write_gate(self.home, "gaming")
        write_gate(self.other_home, "clear")
        self.assert_waits(self.start())

    def test_an_empty_home_does_not_hold_a_job_the_account_status_clears(self):
        write_gate(self.home, "clear")
        output, errors = self.start().communicate(timeout=10)
        self.assertEqual(output, "ran\n", errors)

    def test_a_forged_held_status_under_home_does_not_hold_a_cleared_job(self):
        write_gate(self.home, "clear")
        write_gate(self.other_home, "held")
        output, errors = self.start().communicate(timeout=10)
        self.assertEqual(output, "ran\n", errors)

    def test_a_shim_name_follows_the_account_status_not_home(self):
        tools = self.home / "real-tools"
        shims = self.home / "shims"
        tools.mkdir()
        shims.mkdir()
        (tools / "cargo").write_text(
            f"#!{sys.executable}\nfrom pathlib import Path; Path({str(self.marker)!r}).touch(); print('ran')\n"
        )
        (tools / "cargo").chmod(0o755)
        write_shim(shims, "cargo", self.home)
        environment = self.environment | {"PATH": f"{shims}:{tools}:{self.environment['PATH']}"}
        cases = {
            "forged clear under HOME, account gaming": ("gaming", "clear", True),
            "forged held under HOME, account clear": ("clear", "held", False),
        }
        for name, (account, forged, waits) in cases.items():
            with self.subTest(name):
                self.marker.unlink(missing_ok=True)
                write_gate(self.home, account)
                write_gate(self.other_home, forged)
                job = subprocess.Popen(
                    [str(shims / "cargo"), "build"],
                    env=environment,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                    start_new_session=True,
                )
                self.addCleanup(
                    lambda job=job: job.poll() is None and os.killpg(job.pid, signal.SIGKILL)
                )
                if waits:
                    read_until(job.stderr, "waits")
                    self.assertFalse(self.marker.exists())
                    os.killpg(job.pid, signal.SIGKILL)
                    job.communicate(timeout=10)
                else:
                    output, errors = job.communicate(timeout=10)
                    self.assertEqual(output, "ran\n", errors)

    def test_a_future_ticket_under_home_does_not_delay_a_released_job(self):
        ticket = self.other_home / ".cache/cockpit/test-lock/admission.lock"
        ticket.parent.mkdir(parents=True)
        ticket.write_text(f"{time.time() + 5}\n")
        write_gate(self.home, "gaming")
        job = self.start()
        self.assert_waits(job)
        write_gate(self.home, "clear")
        started = time.monotonic()
        output, errors = job.communicate(timeout=30)
        self.assertEqual(output, "ran\n", errors)
        self.assertLess(time.monotonic() - started, 8, "a ticket under HOME delayed the job")

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
        original_home, original_sleep = agent_build.account_home, agent_build.time.sleep
        agent_build.account_home = lambda: self.home
        agent_build.time.sleep = slept.append
        try:
            agent_build.take_release_turn()
        finally:
            agent_build.time.sleep = original_sleep
            agent_build.account_home = original_home
        self.assertEqual(len(slept), 1)
        self.assertGreater(slept[0], agent_build.RELEASE_SPACING_S - 1)

    def test_a_job_whose_turn_comes_during_a_new_game_waits_again(self):
        decisions = iter([False, True])
        waits = []
        originals = (
            agent_build.is_admitted,
            agent_build.wait_for_admission,
            agent_build.take_release_turn,
            agent_build.admission,
            agent_build.read_gate,
        )
        agent_build.is_admitted = lambda: False
        agent_build.wait_for_admission = lambda label: waits.append(label) or True
        agent_build.take_release_turn = lambda: None
        agent_build.read_gate = lambda path: (None, "test")
        agent_build.admission = lambda gate, problem, now, boot: (next(decisions), None, None)
        try:
            agent_build.admit("cargo build")
        finally:
            (
                agent_build.is_admitted,
                agent_build.wait_for_admission,
                agent_build.take_release_turn,
                agent_build.admission,
                agent_build.read_gate,
            ) = originals
        self.assertEqual(len(waits), 2, "after its turn the job saw a game and waited again")

    def test_waiting_jobs_say_why_once_a_minute_not_on_every_poll(self):
        gates = iter(
            [
                (
                    {
                        "version": 1,
                        "state": "gaming",
                        "reason": "a game is open",
                        "since": 0,
                        "updated_at": 0,
                        "boot_id": BOOT_ID,
                    },
                    None,
                )
            ]
            * 40
            + [
                (
                    {
                        "version": 1,
                        "state": "clear",
                        "since": 0,
                        "updated_at": 0,
                        "boot_id": BOOT_ID,
                    },
                    None,
                )
            ]
        )
        now = [0.0]
        lines = []

        class Out:
            def write(self, text):
                lines.append(text)

            def flush(self):
                pass

        def sleep(seconds):
            now[0] += seconds

        def read(path):
            gate, problem = next(gates)
            return gate | {"updated_at": now[0]}, problem

        waited = agent_build.wait_for_admission(
            "cargo test", read=read, clock=lambda: now[0], sleep=sleep, out=Out()
        )
        self.assertTrue(waited)
        self.assertEqual(sum("waits" in line for line in lines), 2)
        self.assertTrue(any("starts after 80 s" in line for line in lines))


class AccountHomeTests(unittest.TestCase):
    """The shipped runner's own lookup, without the harness."""

    def expected_home(self):
        entry = subprocess.run(
            ["getent", "passwd", str(os.getuid())], capture_output=True, text=True, check=True
        )
        return Path(entry.stdout.strip().split(":")[5])

    def test_status_and_ticket_come_from_the_account_home_whatever_home_and_xdg_say(self):
        changed = {
            "HOME": "/tmp/elsewhere",
            "XDG_CACHE_HOME": "/tmp/xdg-cache",
            "XDG_STATE_HOME": "/tmp/xdg-state",
            "USERPROFILE": "/tmp/profile",
        }
        saved = {key: os.environ.get(key) for key in changed}
        os.environ.update(changed)
        try:
            gate, ticket = agent_build.gate_path(), agent_build.release_ticket_path()
        finally:
            for key, value in saved.items():
                os.environ.pop(key, None) if value is None else os.environ.__setitem__(key, value)
        self.assertEqual(gate, self.expected_home() / ".cache/agent-shims/heavy-gate.json")
        self.assertEqual(ticket, self.expected_home() / ".cache/cockpit/test-lock/admission.lock")

    def test_the_shipped_source_reads_no_home_variable(self):
        source = RUNNER.read_text()
        for forbidden in (
            "Path.home",
            "expanduser",
            '"HOME"',
            "'HOME'",
            "getenv",
            'Path("~',
            "Path('~",
        ):
            with self.subTest(forbidden):
                self.assertNotIn(forbidden, source)
        environment_uses = [line.strip() for line in source.splitlines() if "environ[" in line]
        self.assertEqual(environment_uses, ["os.environ[ADMITTED_ENV] = admission_marker()"])

    def test_the_shipped_executable_answers_the_same_under_any_home(self):
        """No harness: the real runner and the real account status, only read. A runner that
        followed HOME would answer 0 under the forged clear and 75 under the forged held."""
        forged = {}
        for state in ("clear", "held"):
            folder = tempfile.TemporaryDirectory()
            self.addCleanup(folder.cleanup)
            write_gate(Path(folder.name), state)
            forged[state] = folder.name
        real_status = self.expected_home() / ".cache/agent-shims/heavy-gate.json"

        def decision_inputs():
            """Everything the real decision depends on, apart from the forged HOMEs: the state,
            boot, manual release and whether the status is fresh. Read only."""
            try:
                gate = json.loads(real_status.read_text())
            except (OSError, ValueError):
                return None
            if not isinstance(gate, dict):
                return None
            age = (
                time.time() - gate.get("updated_at", 0)
                if isinstance(gate.get("updated_at"), (int, float))
                else None
            )
            fresh = age is not None and -5 <= age <= agent_build.GATE_FRESH_S - 5
            stale = age is None or age > agent_build.GATE_FRESH_S + 5 or age < -10
            return (
                gate.get("state"),
                gate.get("boot_id"),
                gate.get("manual_release_until"),
                "fresh" if fresh else "stale" if stale else "near the edge",
            )

        environment = fresh_environment(os.environ)
        answers = None
        for _ in range(10):
            before = decision_inputs()
            if before is not None and before[3] == "near the edge":
                time.sleep(1)
                continue
            pair = [
                subprocess.run(
                    [sys.executable, str(RUNNER), "--check-gate"],
                    timeout=10,
                    env=environment | {"HOME": forged[state]},
                ).returncode
                for state in ("clear", "held")
            ]
            if decision_inputs() == before:
                answers = pair
                break
        if answers is None:
            self.skipTest(
                "the real status kept changing while sampled; no HOME conclusion either way"
            )
        self.assertIn(answers[0], (0, 75))
        self.assertEqual(answers[0], answers[1], f"answers differ under forged homes: {answers}")


class FailClosedTests(unittest.TestCase):
    """The account lookup failing: the shipped run() never runs the command."""

    def run_shipped(self, arguments, lookup):
        original_lookup, original_argv = agent_build.pwd.getpwuid, sys.argv
        original_marker = os.environ.pop("AGENT_BUILD_ADMITTED", None)
        agent_build.pwd.getpwuid = lookup
        sys.argv = ["agent-build", *arguments]
        try:
            return agent_build.run()
        finally:
            agent_build.pwd.getpwuid = original_lookup
            sys.argv = original_argv
            if original_marker is not None:
                os.environ["AGENT_BUILD_ADMITTED"] = original_marker

    def test_cases(self):
        def missing(uid):
            raise KeyError(uid)

        class Entry:
            def __init__(self, home):
                self.pw_dir = home

        lookups = {
            "no password entry": missing,
            "empty home": lambda uid: Entry(""),
            "relative home": lambda uid: Entry("home/theo"),
        }
        with tempfile.TemporaryDirectory() as folder:
            marker = Path(folder) / "ran"
            heavy = [
                sys.executable,
                "-c",
                f"from pathlib import Path; Path({str(marker)!r}).touch()",
            ]
            for name, lookup in lookups.items():
                for arguments in (heavy, ["--check-gate"], ["--wait-gate", "a test run"]):
                    with self.subTest(name=name, arguments=arguments[0]):
                        self.assertEqual(self.run_shipped(arguments, lookup), 126)
                        self.assertFalse(marker.exists(), "the command ran without an account home")


if __name__ == "__main__":
    unittest.main()
