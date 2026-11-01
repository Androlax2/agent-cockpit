import importlib.machinery
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "config/bin/.local/bin/cockpit-game-guard"
LOADER = importlib.machinery.SourceFileLoader("cockpit_game_guard", str(SCRIPT))
SPEC = importlib.util.spec_from_loader("cockpit_game_guard", LOADER)
guard = importlib.util.module_from_spec(SPEC)
LOADER.exec_module(guard)

PLENTY = {"available_kb": 10 * 1024 * 1024, "psi_some_avg10": 0.0, "psi_some_avg60": 0.0}

# This host's layout (Ryzen 7 7800X3D): core n is CPUs n and n+8, CPPC highest_perf per core.
RYZEN_RANKS = [171, 196, 166, 191, 181, 196, 176, 186]

# A child at SCHED_IDLE, like agent-build's, with a second thread; prints its pid, then on each
# line read starts a grandchild (inheriting affinity and policy) and prints its pid.
IDLE_CHILD = r"""
import os, subprocess, sys, threading, time
os.sched_setscheduler(0, os.SCHED_IDLE, os.sched_param(0))
threading.Thread(target=lambda: time.sleep(600), daemon=True).start()
print(os.getpid(), flush=True)
children = []
for line in sys.stdin:
    children.append(subprocess.Popen([sys.executable, "-c", "import time; time.sleep(600)"]))
    print(children[-1].pid, flush=True)
"""


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def fake_sysfs(
    root, cores=8, threads_per_core=2, ranks=None, l3_domains=1, packages=1, hybrid=False
):
    cpu_root = root / "devices/system/cpu"
    total = cores * threads_per_core
    write(cpu_root / "online", f"0-{total - 1}")
    for cpu in range(total):
        core = cpu % cores
        siblings = ",".join(str(core + cores * thread) for thread in range(threads_per_core))
        base = cpu_root / f"cpu{cpu}"
        write(base / "topology/thread_siblings_list", siblings)
        write(base / "topology/physical_package_id", str(core * packages // cores))
        domain = core * l3_domains // cores
        write(base / "cache/index3/level", "3")
        write(base / "cache/index3/shared_cpu_list", f"domain-{domain}")
        if ranks:
            write(base / "acpi_cppc/highest_perf", str(ranks[core]))
    if hybrid:
        (root / "devices/cpu_core").mkdir(parents=True)
    return cpu_root


class Hyprland:
    """Answers hyprctl like Hyprland would, from a window list and a submap."""

    def __init__(self, windows, submap="default", fails=False):
        self.windows, self.submap, self.fails = windows, submap, fails

    def __call__(self, command, **options):
        if self.fails:
            return subprocess.CompletedProcess(command, 1, "", "Couldn't connect")
        if command[1:] == ["-j", "clients"]:
            return subprocess.CompletedProcess(command, 0, json.dumps(self.windows), "")
        return subprocess.CompletedProcess(command, 0, self.submap + "\n", "")


class GameDetectionTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.runtime = Path(self.folder.name)
        write(self.runtime / "hypr/instance-a/.socket.sock", "")

    def test_the_game_list_matches_the_daemons(self):
        registry = (REPO / "apps/crates/cockpit-daemon/src/registry.rs").read_text()
        daemon_pattern = re.search(
            r'static GAME_CLASS: LazyLock<Regex> =\s*LazyLock::new\(\|\| regex\(r"([^"]+)"\)\);',
            registry,
        ).group(1)
        self.assertEqual(guard.GAME_CLASS.pattern, daemon_pattern)

    def test_cases(self):
        cases = {
            "an unfocused game window (alt-tab to the cockpit) is a game": (
                Hyprland(
                    [
                        {"class": "kitty", "focusHistoryID": 0},
                        {"class": "steam_app_1245620", "focusHistoryID": 1},
                    ]
                ),
                True,
            ),
            "the gamemode submap is a game": (
                Hyprland([{"class": "kitty"}], submap="gamemode"),
                True,
            ),
            "no game window is no game": (Hyprland([{"class": "kitty"}, {"class": "zen"}]), False),
            "hyprctl failing is unknown, not over": (Hyprland([], fails=True), None),
        }
        for name, (hyprland, expected) in cases.items():
            with self.subTest(name):
                self.assertIs(guard.game_seen(self.runtime, run=hyprland), expected)

    def test_no_hyprland_socket_means_no_game(self):
        (self.runtime / "hypr/instance-a/.socket.sock").unlink()
        self.assertIs(
            guard.game_seen(self.runtime, run=Hyprland([{"class": "steam_app_1"}])), False
        )


class GameLifetimeTests(unittest.TestCase):
    def protecting_over(self, timeline):
        state, results = {}, []
        for now, seen in timeline:
            results.append(guard.next_protecting(state, seen, now))
        return results

    def test_protection_lasts_until_30_seconds_after_the_last_game_window(self):
        self.assertEqual(
            self.protecting_over([(0, True), (5, True), (10, False), (39, False), (40, False)]),
            [True, True, True, True, False],
        )

    def test_a_game_window_back_within_30_seconds_keeps_protection(self):
        self.assertEqual(
            self.protecting_over([(0, True), (10, False), (30, True), (55, False), (61, False)]),
            [True, True, True, True, True],
        )

    def test_an_unknown_answer_keeps_protection_then_releases_after_ten_minutes(self):
        self.assertEqual(
            self.protecting_over([(0, True), (5, None), (300, None), (605, None)]),
            [True, True, True, False],
        )

    def test_an_unknown_answer_never_starts_protection(self):
        self.assertEqual(self.protecting_over([(0, None), (5, None)]), [False, False])


class TopologyTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)

    def test_this_host_gives_builds_its_two_slowest_cores_but_not_cpu_0s(self):
        cpus, why = guard.choose_build_cpus(fake_sysfs(self.root, ranks=RYZEN_RANKS))
        self.assertEqual(cpus, [2, 6, 10, 14])
        self.assertIn("CPPC", why)

    def test_without_cppc_ranks_builds_get_the_highest_numbered_cores(self):
        cpus, _ = guard.choose_build_cpus(fake_sysfs(self.root))
        self.assertEqual(cpus, [6, 7, 14, 15])

    def test_without_smt_builds_get_two_threads(self):
        cpus, _ = guard.choose_build_cpus(fake_sysfs(self.root, threads_per_core=1))
        self.assertEqual(cpus, [6, 7])

    def test_unsupported_topologies_are_refused_with_their_reason(self):
        cases = {
            "two L3 cache domains": (dict(cores=16, l3_domains=2), "L3"),
            "two packages": (dict(packages=2), "packages"),
            "too few cores": (dict(cores=4), "physical cores"),
            "hybrid": (dict(hybrid=True), "hybrid"),
        }
        for name, (layout, reason) in cases.items():
            with self.subTest(name), tempfile.TemporaryDirectory() as folder:
                with self.assertRaisesRegex(guard.UnsupportedTopology, reason):
                    guard.choose_build_cpus(fake_sysfs(Path(folder), **layout))


class ScanTests(unittest.TestCase):
    AGENT = "/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-aaa.scope"
    KITTY = "/user.slice/user-1000.slice/user@1000.service/app.slice/kitty-1-0.scope"

    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.proc = Path(self.folder.name)

    def process(self, pid, cgroup, policies, ticks=0, comm="cargo"):
        base = self.proc / str(pid)
        write(base / "cgroup", f"0::{cgroup}\n")
        write(base / "comm", comm + "\n")
        for offset, policy in enumerate(policies):
            fields = ["S"] + ["0"] * 40
            fields[11] = str(ticks)
            fields[19] = str(1000 + pid + offset)
            fields[38] = str(policy)
            write(
                base / "task" / str(pid + offset) / "stat",
                f"{pid + offset} ({comm}) " + " ".join(fields),
            )
            if offset == 0:
                write(base / "stat", f"{pid} ({comm}) " + " ".join(fields))

    def test_only_idle_threads_of_agent_sessions_are_heavy(self):
        self.process(10, self.AGENT, [guard.SCHED_IDLE, guard.SCHED_IDLE])
        self.process(20, self.AGENT, [0], comm="node")
        self.process(30, self.KITTY, [guard.SCHED_IDLE], comm="tracker")
        self.process(40, self.KITTY, [0], comm="game")
        heavy, others = guard.scan(self.proc, os.getuid())
        self.assertEqual(heavy, [(10, 1010), (11, 1011)])
        self.assertEqual(
            {pid: why for pid, (_, _, why) in others.items()},
            {20: "not started through agent-build", 30: "SCHED_IDLE outside agent sessions"},
        )

    def test_other_users_processes_are_never_selected(self):
        self.process(10, self.AGENT, [guard.SCHED_IDLE])
        self.assertEqual(guard.scan(self.proc, os.getuid() + 1), ([], {}))

    def test_work_over_one_core_that_is_not_pinned_is_reported_as_untracked(self):
        before = {
            20: ("node", 0, "not started through agent-build"),
            30: ("tracker", 0, "SCHED_IDLE outside agent sessions"),
        }
        after = {
            20: ("node", 3 * guard.CLOCK_TICKS, "not started through agent-build"),
            30: ("tracker", 1, "SCHED_IDLE outside agent sessions"),
        }
        self.assertEqual(
            guard.untracked_report(before, after, 2),
            [{"pid": 20, "comm": "node", "cores": 1.5, "why": "not started through agent-build"}],
        )


class IdleChildCase(unittest.TestCase):
    """Real threads of a child this test starts; never any other process."""

    def setUp(self):
        available = sorted(os.sched_getaffinity(0))
        self.build_cpus = available[-1:]
        self.default_cpus = available
        self.original = available[:-1]
        self.child = subprocess.Popen(
            [sys.executable, "-c", IDLE_CHILD],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            preexec_fn=lambda: os.sched_setaffinity(0, self.original),
        )
        self.addCleanup(self.stop_child)
        self.assertEqual(int(self.child.stdout.readline()), self.child.pid)
        self.wait_for(lambda: len(self.tids(self.child.pid)) == 2)

    def stop_child(self):
        for pid in getattr(self, "grandchildren", []):
            try:
                os.kill(pid, 9)
            except ProcessLookupError:
                pass
        self.child.kill()
        self.child.communicate()

    def start_grandchild(self):
        self.child.stdin.write("start\n")
        self.child.stdin.flush()
        pid = int(self.child.stdout.readline())
        self.grandchildren = getattr(self, "grandchildren", []) + [pid]
        return pid

    def wait_for(self, condition, seconds=10):
        deadline = time.monotonic() + seconds
        while not condition():
            self.assertLess(time.monotonic(), deadline, "timed out")
            time.sleep(0.02)

    def tids(self, pid):
        return sorted(int(tid) for tid in os.listdir(f"/proc/{pid}/task"))

    def heavy(self, *pids):
        return [
            (tid, guard.start_time(guard.stat_fields("/proc", tid)))
            for pid in pids
            for tid in self.tids(pid)
        ]

    def affinities(self, *pids):
        return {tid: sorted(os.sched_getaffinity(tid)) for pid in pids for tid in self.tids(pid)}


@unittest.skipUnless(len(os.sched_getaffinity(0)) >= 2, "needs two CPU threads")
class AffinityTests(IdleChildCase):
    def test_pinning_moves_every_thread_and_restoring_gives_back_the_original(self):
        state = {}
        pinned, failed = guard.pin(state, self.heavy(self.child.pid), self.build_cpus, "/proc")
        self.assertEqual((pinned, failed), (2, []))
        self.assertEqual(
            set(map(tuple, self.affinities(self.child.pid).values())), {tuple(self.build_cpus)}
        )
        self.assertEqual(guard.restore(state, [], self.build_cpus, self.default_cpus, "/proc"), [])
        self.assertEqual(
            set(map(tuple, self.affinities(self.child.pid).values())), {tuple(self.original)}
        )
        self.assertEqual(state["tasks"], {})

    def test_a_child_born_while_pinned_gets_the_default_set_back(self):
        state = {}
        guard.pin(state, self.heavy(self.child.pid), self.build_cpus, "/proc")
        grandchild = self.start_grandchild()
        self.assertEqual(sorted(os.sched_getaffinity(grandchild)), self.build_cpus)
        guard.pin(state, self.heavy(self.child.pid, grandchild), self.build_cpus, "/proc")
        self.assertIsNone(state["tasks"][str(grandchild)]["original"])
        guard.restore(state, [], self.build_cpus, self.default_cpus, "/proc")
        self.assertEqual(sorted(os.sched_getaffinity(grandchild)), self.default_cpus)
        self.assertEqual(sorted(os.sched_getaffinity(self.child.pid)), self.original)

    def test_a_child_born_while_pinned_and_never_scanned_is_restored_too(self):
        state = {}
        guard.pin(state, self.heavy(self.child.pid), self.build_cpus, "/proc")
        grandchild = self.start_grandchild()
        guard.restore(state, self.heavy(grandchild), self.build_cpus, self.default_cpus, "/proc")
        self.assertEqual(sorted(os.sched_getaffinity(grandchild)), self.default_cpus)

    def test_threads_that_refuse_or_do_not_take_the_set_are_reported_as_failed(self):
        def refuse(tid, cpus):
            raise PermissionError(1, "Operation not permitted")

        cases = {
            "refused": dict(setaffinity=refuse),
            "not taken": dict(setaffinity=lambda tid, cpus: None),
        }
        for name, options in cases.items():
            with self.subTest(name):
                pinned, failed = guard.pin(
                    {}, self.heavy(self.child.pid), self.build_cpus, "/proc", **options
                )
                self.assertEqual(pinned, 0)
                self.assertEqual(len(failed), 2)
                self.assertEqual(sorted(os.sched_getaffinity(self.child.pid)), self.original)

    def test_records_of_exited_threads_are_dropped_while_the_game_goes_on(self):
        compiler = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(600)"])
        self.addCleanup(compiler.kill)
        state = {}
        guard.pin(state, self.heavy(self.child.pid, compiler.pid), self.build_cpus, "/proc")
        self.assertIn(str(compiler.pid), state["tasks"])
        compiler.kill()
        compiler.wait()
        guard.pin(state, self.heavy(self.child.pid), self.build_cpus, "/proc")
        self.assertNotIn(str(compiler.pid), state["tasks"])
        self.assertIn(str(self.child.pid), state["tasks"])

    def test_an_original_set_with_no_online_cpu_falls_back_to_the_default_set(self):
        state = {}
        guard.pin(state, self.heavy(self.child.pid), self.build_cpus, "/proc")
        for record in state["tasks"].values():
            record["original"] = [4095]
        self.assertEqual(guard.restore(state, [], self.build_cpus, self.default_cpus, "/proc"), [])
        self.assertEqual(sorted(os.sched_getaffinity(self.child.pid)), self.default_cpus)

    def test_a_reused_thread_id_is_not_touched(self):
        os.sched_setaffinity(self.child.pid, self.build_cpus)
        start = guard.start_time(guard.stat_fields("/proc", self.child.pid))
        state = {
            "tasks": {str(self.child.pid): {"start": start + 1, "original": self.default_cpus}}
        }
        guard.restore(state, [], self.build_cpus, self.default_cpus, "/proc")
        self.assertEqual(sorted(os.sched_getaffinity(self.child.pid)), self.build_cpus)
        self.assertEqual(state["tasks"], {})


@unittest.skipUnless(len(os.sched_getaffinity(0)) >= 2, "needs two CPU threads")
class GuardServiceTests(IdleChildCase):
    """The service loop, driven with a fake Hyprland and a scanner limited to the test's child."""

    def setUp(self):
        super().setUp()
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.runtime = Path(self.folder.name)
        self.now = 0.0
        self.game_open = True
        self.sleeps = []

    def make_guard(self, stop_after_sleeps=None):
        def sleep(seconds):
            self.sleeps.append(seconds)
            self.now += seconds
            if stop_after_sleeps is not None and len(self.sleeps) >= stop_after_sleeps:
                service.stopping = True

        service = guard.Guard(
            self.runtime,
            topology=lambda: (self.build_cpus, "test"),
            scanner=lambda: (self.heavy(self.child.pid), {}),
            detect=lambda: self.game_open,
            clock=lambda: self.now,
            sleep=sleep,
            memory=lambda: PLENTY,
            gate_path=self.runtime / "heavy-gate.json",
            manual_directory=self.runtime,
        )
        return service

    def saved_state(self):
        return json.loads((self.runtime / "cockpit-game-guard.json").read_text())

    def test_a_game_pins_and_its_end_restores_after_30_seconds(self):
        service = self.make_guard()
        service.tick()
        self.assertEqual(sorted(os.sched_getaffinity(self.child.pid)), self.build_cpus)
        self.assertTrue(self.saved_state()["protecting"])
        self.game_open = False
        for self.now in (10, 39):
            service.tick()
            self.assertEqual(sorted(os.sched_getaffinity(self.child.pid)), self.build_cpus)
        self.now = 40
        service.tick()
        self.assertEqual(sorted(os.sched_getaffinity(self.child.pid)), self.original)
        self.assertFalse(self.saved_state()["protecting"])

    def test_stopping_the_service_restores(self):
        service = self.make_guard(stop_after_sleeps=1)
        service.run()
        self.assertEqual(sorted(os.sched_getaffinity(self.child.pid)), self.original)
        self.assertEqual(self.saved_state()["tasks"], {})

    def test_a_start_after_a_crash_restores_what_the_crashed_run_pinned(self):
        crashed = self.make_guard()
        crashed.tick()
        self.assertEqual(sorted(os.sched_getaffinity(self.child.pid)), self.build_cpus)
        self.game_open = False
        restarted = self.make_guard()
        restarted.stopping = True
        restarted.run()
        self.assertEqual(sorted(os.sched_getaffinity(self.child.pid)), self.original)

    def test_without_a_game_the_guard_only_asks_hyprland_every_five_seconds(self):
        self.game_open = False
        scans = []
        service = self.make_guard(stop_after_sleeps=12)
        service.scan = lambda: scans.append(1) or ([], {})
        service.run()
        self.assertEqual(scans, [1], "only the restore on stop scans")
        self.assertEqual(set(self.sleeps), {guard.DETECT_EVERY_S})
        self.assertEqual(service.detections, 12)

    def test_while_gaming_the_guard_scans_every_two_seconds_at_most(self):
        service = self.make_guard(stop_after_sleeps=10)
        service.run()
        self.assertEqual(set(self.sleeps), {guard.PIN_EVERY_S})
        self.assertLessEqual(service.scans, len(self.sleeps) + 1)

    def test_a_malformed_state_file_is_dropped_record_by_record(self):
        start = guard.start_time(guard.stat_fields("/proc", self.child.pid))
        os.sched_setaffinity(self.child.pid, self.build_cpus)
        (self.runtime / "cockpit-game-guard.json").write_text(
            json.dumps(
                {
                    "tasks": {
                        str(self.child.pid): {"start": start, "original": self.original},
                        "x": {"start": 1, "original": None},
                        "7": {"start": "late"},
                        "8": [1],
                    }
                }
            )
        )
        restarted = self.make_guard()
        restarted.stopping = True
        restarted.run()
        self.assertEqual(sorted(os.sched_getaffinity(self.child.pid)), self.original)
        self.assertEqual(self.saved_state()["tasks"], {})

    def test_a_state_file_that_is_not_an_object_starts_empty(self):
        (self.runtime / "cockpit-game-guard.json").write_text("[1, 2]")
        restarted = self.make_guard()
        restarted.stopping = True
        restarted.run()
        self.assertFalse(self.saved_state()["protecting"])

    def test_status_names_untracked_work(self):
        service = self.make_guard()
        reports = iter(
            [
                {20: ("node", 0, "not started through agent-build")},
                {20: ("node", 4 * guard.CLOCK_TICKS, "not started through agent-build")},
            ]
        )
        service.scan = lambda: (self.heavy(self.child.pid), next(reports))
        service.tick()
        self.assertEqual(self.saved_state()["untracked"][0]["comm"], "node")


AGENT_BUILD = REPO / "config/bin/.local/bin/agent-build"
BUILD_LOADER = importlib.machinery.SourceFileLoader("agent_build_for_guard", str(AGENT_BUILD))
agent_build = importlib.util.module_from_spec(
    importlib.util.spec_from_loader("agent_build_for_guard", BUILD_LOADER)
)
BUILD_LOADER.exec_module(agent_build)
GIB = 1024 * 1024


def memory(available_gib, avg10=0.0, avg60=0.0):
    return {
        "available_kb": int(available_gib * GIB),
        "psi_some_avg10": avg10,
        "psi_some_avg60": avg60,
    }


class MemoryHoldTests(unittest.TestCase):
    def holds_over(self, timeline):
        state = {}
        return [guard.next_memory_hold(state, sample, now) for now, sample in timeline]

    def test_cases(self):
        cases = {
            "plenty of memory never holds": ([(0, memory(10)), (5, memory(8))], [False, False]),
            "under 3 GiB available holds": ([(0, memory(2.9))], [True]),
            "pressure at 5 % over 10 s holds": ([(0, memory(10, avg10=5.0))], [True]),
            "release needs a full minute at 5 GiB and low pressure": (
                [(0, memory(2)), (10, memory(5)), (69, memory(6)), (70, memory(6))],
                [True, True, True, False],
            ),
            "a dip during the calm minute restarts it": (
                [
                    (0, memory(2)),
                    (10, memory(6)),
                    (40, memory(4.9)),
                    (80, memory(6)),
                    (139, memory(6)),
                    (140, memory(6)),
                ],
                [True, True, True, True, True, False],
            ),
            "pressure over the last minute keeps the hold": (
                [(0, memory(2)), (10, memory(6, avg60=1.0)), (100, memory(6, avg60=1.0))],
                [True, True, True],
            ),
            "between 3 and 5 GiB keeps the current state": (
                [(0, memory(4)), (10, memory(4))],
                [False, False],
            ),
            "an unreadable sample holds": ([(0, None)], [True]),
        }
        for name, (timeline, expected) in cases.items():
            with self.subTest(name):
                self.assertEqual(self.holds_over(timeline), expected)

    def test_memory_is_read_from_meminfo_and_pressure(self):
        with tempfile.TemporaryDirectory() as folder:
            proc = Path(folder)
            write(proc / "meminfo", "MemTotal: 32452248 kB\nMemAvailable:   10739372 kB\n")
            write(
                proc / "pressure/memory",
                "some avg10=1.50 avg60=0.25 avg300=0.00 total=1\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=1\n",
            )
            self.assertEqual(
                guard.read_memory(proc),
                {"available_kb": 10739372, "psi_some_avg10": 1.5, "psi_some_avg60": 0.25},
            )
            (proc / "pressure/memory").unlink()
            self.assertIsNone(guard.read_memory(proc))


class GateTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)
        self.runtime = self.root / "run"
        self.runtime.mkdir()
        self.gate = self.root / "home/.cache/agent-shims/heavy-gate.json"
        self.game_open = False
        self.sample = memory(10)
        self.now = 1000.0

    def make_guard(self):
        return guard.Guard(
            self.runtime,
            topology=lambda: ([sorted(os.sched_getaffinity(0))[-1]], "test"),
            scanner=lambda: ([], {}),
            detect=lambda: self.game_open,
            clock=lambda: self.now,
            wall=lambda: self.now,
            sleep=lambda seconds: None,
            memory=lambda: self.sample,
            gate_path=self.gate,
            manual_directory=self.runtime,
        )

    def written(self):
        return json.loads(self.gate.read_text())

    def admitted(self):
        """What agent-build would decide on the file the guard wrote."""
        gate, problem = agent_build.read_gate(self.gate)
        return agent_build.admission(gate, problem, self.now, agent_build.boot_id())[0]

    def test_precedence_of_the_gate_states(self):
        cases = [
            ({"off": True, "hold": True}, True, True, "off"),
            ({"hold": True}, True, True, "held"),
            ({}, True, True, "memory"),
            ({}, True, False, "gaming"),
            ({}, False, True, "memory"),
            ({}, False, False, "clear"),
        ]
        for manual, protecting, memory_hold, expected in cases:
            with self.subTest(manual=manual, protecting=protecting, memory_hold=memory_hold):
                self.assertEqual(
                    guard.gate_state(manual, protecting, memory_hold, memory(10), 0)[0], expected
                )

    def test_low_memory_during_a_game_still_holds_new_jobs(self):
        service = self.make_guard()
        self.game_open = True
        self.sample = memory(1)
        service.tick()
        self.assertEqual(self.written()["state"], "memory")
        self.assertFalse(self.admitted())

    def test_the_guard_publishes_clear_gaming_and_memory_that_agent_build_reads(self):
        service = self.make_guard()
        service.tick()
        self.assertEqual(self.written()["state"], "clear")
        self.assertTrue(self.admitted())
        self.game_open = True
        service.tick()
        self.assertEqual(self.written()["state"], "gaming")
        self.assertTrue(self.admitted(), "a game alone pins new jobs, it does not hold them")
        self.game_open = False
        service.tick()
        self.now += 30
        service.tick()
        self.assertEqual(self.written()["state"], "clear")
        self.sample = memory(1)
        service.tick()
        self.assertEqual(self.written()["state"], "memory")
        self.assertIn("MiB available", self.written()["reason"])
        self.assertFalse(self.admitted())

    def test_the_status_is_rewritten_every_detection_while_idle(self):
        service = self.make_guard()
        sequences = []
        for _ in range(3):
            service.tick()
            sequences.append(self.written()["sequence"])
            self.now += 5
        self.assertEqual(sequences, sorted(set(sequences)))
        self.assertEqual(self.written()["boot_id"], agent_build.boot_id())

    def test_the_status_is_readable_by_sessions_and_written_atomically(self):
        self.make_guard().tick()
        self.assertEqual(self.gate.stat().st_mode & 0o777, 0o644)
        self.assertEqual([path.name for path in self.gate.parent.iterdir()], ["heavy-gate.json"])

    def test_a_dead_guards_status_goes_stale_and_holds(self):
        self.make_guard().tick()
        self.now += 31
        self.assertFalse(self.admitted())

    def manual(self, *arguments):
        return guard.manual_command(
            self.runtime,
            arguments[0],
            list(arguments[1:]),
            home=self.root / "home",
            wall=lambda: self.now,
        )

    def test_theos_release_works_with_no_guard_running_and_expires(self):
        self.make_guard().tick()
        self.now += 600
        self.assertFalse(self.admitted())
        self.assertEqual(self.manual("release", "30"), 0)
        self.assertTrue(self.admitted())
        self.assertEqual(
            self.written()["updated_at"], 1000.0, "a release must not make a dead guard look alive"
        )
        self.now += 30 * 60 + 1
        self.assertFalse(self.admitted())

    def test_theos_hold_and_off_and_on(self):
        service = self.make_guard()
        service.tick()
        self.manual("hold")
        self.assertEqual(self.written()["state"], "held")
        service.tick()
        self.assertEqual(self.written()["state"], "held", "the running guard keeps Theo's hold")
        self.assertFalse(self.admitted())
        self.manual("off")
        self.assertEqual(self.written()["state"], "off")
        self.assertTrue(self.admitted())
        self.manual("on")
        service.tick()
        self.assertEqual(self.written()["state"], "clear")

    def test_a_running_guard_keeps_a_release_until_it_expires(self):
        service = self.make_guard()
        self.sample = memory(1)
        service.tick()
        self.manual("release", "10")
        service.tick()
        self.assertTrue(self.admitted())
        self.now += 10 * 60 + 1
        service.tick()
        self.assertFalse(self.admitted())

    def test_a_stop_or_restore_mid_game_never_publishes_a_fresh_status(self):
        service = self.make_guard()
        self.game_open = True
        service.tick()
        written = self.gate.read_text()
        service.stopping = True
        service.run()
        self.make_guard().release("restore")
        self.assertEqual(self.gate.read_text(), written)
        self.now += 31
        self.assertFalse(self.admitted(), "a stopped guard's status must go stale and hold")

    def test_a_failing_status_write_keeps_the_pin_records(self):
        service = self.make_guard()
        service.state["tasks"] = {"4242": {"start": 1, "original": [0]}}
        self.gate.parent.mkdir(parents=True)
        self.gate.parent.chmod(0o500)
        self.addCleanup(self.gate.parent.chmod, 0o700)
        service.tick()
        self.assertIn(
            "4242", json.loads((self.runtime / "cockpit-game-guard.json").read_text())["tasks"]
        )

    def test_hold_turns_admission_back_on(self):
        self.make_guard().tick()
        self.manual("off")
        self.manual("hold")
        self.assertEqual(self.written()["state"], "held")
        self.assertFalse(self.admitted())

    def test_a_release_from_an_earlier_boot_counts_only_once_the_guard_republishes_it(self):
        self.sample = memory(1)
        self.manual("release", "120")
        gate = self.written() | {"boot_id": "earlier-boot"}
        self.gate.write_text(json.dumps(gate))
        self.assertFalse(self.admitted(), "after a reboot the old file alone must not release")
        self.make_guard().tick()
        self.assertTrue(self.admitted(), "the guard republishes the saved release for this boot")
        self.now += 120 * 60 + 1
        self.make_guard().tick()
        self.assertFalse(self.admitted(), "the release ends at its absolute time")

    def account_home_from_getent(self):
        entry = subprocess.run(
            ["getent", "passwd", str(os.getuid())], capture_output=True, text=True, check=True
        )
        return Path(entry.stdout.strip().split(":")[5])

    def test_status_and_manual_files_come_from_the_account_home_whatever_home_and_xdg_say(self):
        changed = {
            "HOME": "/tmp/elsewhere",
            "XDG_STATE_HOME": "/tmp/xdg-state",
            "XDG_CACHE_HOME": "/tmp/xdg-cache",
        }
        saved = {key: os.environ.get(key) for key in changed}
        os.environ.update(changed)
        try:
            gate, manual = guard.gate_file(), guard.manual_dir()
        finally:
            for key, value in saved.items():
                os.environ.pop(key, None) if value is None else os.environ.__setitem__(key, value)
        home = self.account_home_from_getent()
        self.assertEqual(gate, home / ".cache/agent-shims/heavy-gate.json")
        self.assertEqual(manual, home / ".local/state/cockpit")

    def test_a_release_under_another_home_writes_the_account_files_together(self):
        account = self.root / "account"
        original_lookup, original_home = guard.account_home, os.environ.get("HOME")
        guard.account_home = lambda: account
        os.environ["HOME"] = str(self.root / "elsewhere")
        try:
            self.assertEqual(guard.main(["release", "5"]), 0)
        finally:
            guard.account_home = original_lookup
            os.environ["HOME"] = original_home
        self.assertTrue((account / ".cache/agent-shims/heavy-gate.json").exists())
        self.assertTrue((account / ".local/state/cockpit/game-guard-manual.json").exists())
        self.assertFalse((self.root / "elsewhere").exists())

    def test_a_missing_account_fails_closed(self):
        def missing(uid):
            raise KeyError(uid)

        original = guard.pwd.getpwuid
        guard.pwd.getpwuid = missing
        try:
            with self.assertRaisesRegex(OSError, "no password entry"):
                guard.gate_file()
        finally:
            guard.pwd.getpwuid = original

    def test_a_restarted_guard_publishes_nothing_before_its_first_detection(self):
        self.game_open = True
        self.make_guard().tick()
        written = self.gate.read_text()
        restarted = self.make_guard()
        restarted.stopping = True
        restarted.run()
        self.assertEqual(self.gate.read_text(), written)

    def test_release_minutes_are_bounded(self):
        for minutes in ["0", "1441", "soon"]:
            with self.subTest(minutes):
                self.assertEqual(self.manual("release", minutes), 2)


class CommandTests(unittest.TestCase):
    def run_guard(self, *arguments, runtime):
        return subprocess.run(
            [sys.executable, str(SCRIPT), *arguments],
            capture_output=True,
            text=True,
            env=os.environ
            | {
                "XDG_RUNTIME_DIR": str(runtime),
                "HOME": str(runtime),
                "XDG_STATE_HOME": str(runtime),
            },
            timeout=30,
        )

    def test_status_without_state_says_the_guard_has_not_run(self):
        with tempfile.TemporaryDirectory() as runtime:
            result = self.run_guard("status", runtime=runtime)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("has not run", result.stdout)

    def test_an_unknown_command_prints_the_usage(self):
        with tempfile.TemporaryDirectory() as runtime:
            result = self.run_guard("pause", runtime=runtime)
        self.assertEqual(result.returncode, 2)
        self.assertIn("usage: cockpit-game-guard", result.stderr)

    def test_the_unit_restores_on_stop_and_does_not_restart_on_an_unsupported_topology(self):
        unit = (REPO / "config/systemd/.config/systemd/user/cockpit-game-guard.service").read_text()
        self.assertIn("ExecStopPost=%h/.local/bin/cockpit-game-guard restore", unit)
        self.assertIn("RestartPreventExitStatus=3", unit)

    def test_the_unit_is_not_enabled_by_the_install(self):
        wants = REPO / "config/systemd/.config/systemd/user/default.target.wants"
        self.assertFalse((wants / "cockpit-game-guard.service").exists())


if __name__ == "__main__":
    unittest.main()
