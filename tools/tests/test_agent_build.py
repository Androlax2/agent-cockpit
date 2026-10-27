import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[2]
RUNNER = REPO / "config/bin/.local/bin/agent-build"
PROBE = ("import json, os; print(json.dumps({'cpus': sorted(os.sched_getaffinity(0)), "
         "'nice': os.getpriority(os.PRIO_PROCESS, 0), 'idle': os.sched_getscheduler(0) == os.SCHED_IDLE}))")
LOWEST_NICE = 19


class AgentBuildTests(unittest.TestCase):
    def setUp(self):
        self.environment = os.environ.copy()
        self.environment.pop("AGENT_BUILD_CPUS", None)
        # A session's own build shims would apply the installed runner, not the one under test.
        self.environment["PATH"] = os.pathsep.join(
            directory for directory in self.environment["PATH"].split(os.pathsep)
            if not (Path(directory) / "agent-build").exists())
        self.available_cpus = sorted(os.sched_getaffinity(0))[:4]

    def run_build(self, *arguments, environment=None, **options):
        return subprocess.run(
            [sys.executable, str(RUNNER), *arguments],
            env=self.environment if environment is None else environment,
            preexec_fn=lambda: os.sched_setaffinity(0, self.available_cpus),
            capture_output=True, text=True, **options,
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
        self.assertEqual(json.loads(result.stdout),
                         {"cpus": self.available_cpus, "nice": LOWEST_NICE, "idle": True})

    def test_cpu_list_left_by_older_sessions_no_longer_pins_builds(self):
        environment = self.environment | {"AGENT_BUILD_CPUS": str(self.available_cpus[0])}
        result = self.run_build(sys.executable, "-c", PROBE, environment=environment)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["cpus"], self.available_cpus)

    def test_streams_arguments_working_directory_and_exit_status_are_preserved(self):
        script = "import os, sys; print(os.getcwd()); print(repr(sys.argv[1:])); print(sys.stdin.read()); print('diagnostic', file=sys.stderr); sys.exit(27)"
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_build(sys.executable, "-c", script, "argument with spaces", "$(not a shell)",
                                    cwd=directory, input="build input")
            self.assertEqual(result.returncode, 27)
            self.assertEqual(result.stdout.splitlines(), [directory, "['argument with spaces', '$(not a shell)']", "build input"])
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
            result = subprocess.run([str(shims / "cargo"), "test"], env=environment,
                                    preexec_fn=lambda: os.sched_setaffinity(0, self.available_cpus),
                                    capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout),
                             {"cpus": self.available_cpus, "nice": LOWEST_NICE, "idle": True})

    def test_host_make_builds_run_at_the_lowest_priority(self):
        with tempfile.TemporaryDirectory() as directory:
            tool = Path(directory) / "cargo"
            tool.write_text(f"#!{sys.executable}\n{PROBE}\n")
            tool.chmod(0o755)
            result = subprocess.run(["make", "--no-print-directory", "build"], cwd=REPO,
                                    env=self.environment | {"PATH": f"{directory}:{self.environment['PATH']}"},
                                    preexec_fn=lambda: os.sched_setaffinity(0, self.available_cpus),
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            observed = json.loads(result.stdout.splitlines()[-1])
            self.assertEqual(observed, {"cpus": self.available_cpus, "nice": LOWEST_NICE, "idle": True})


if __name__ == "__main__":
    unittest.main()
