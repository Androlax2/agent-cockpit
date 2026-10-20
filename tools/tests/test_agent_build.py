import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[2]
RUNNER = REPO / "config/bin/.local/bin/agent-build"
PROBE = "import json, os; print(json.dumps({'cpus': sorted(os.sched_getaffinity(0)), 'nice': os.getpriority(os.PRIO_PROCESS, 0)}))"


class AgentBuildTests(unittest.TestCase):
    def setUp(self):
        self.environment = os.environ.copy()
        self.environment.pop("AGENT_BUILD_CPUS", None)
        self.available_cpus = sorted(os.sched_getaffinity(0))[:4]
        self.expected_cpus = self.available_cpus[:max(1, len(self.available_cpus) // 2)]

    def run_build(self, *arguments, environment=None, **options):
        return subprocess.run(
            [sys.executable, str(RUNNER), *arguments],
            env=self.environment if environment is None else environment,
            preexec_fn=lambda: os.sched_setaffinity(0, self.available_cpus),
            capture_output=True, text=True, **options,
        )

    def test_independent_builds_share_half_the_available_cpu_threads(self):
        for _ in range(2):
            with self.subTest():
                result = self.run_build(sys.executable, "-c", PROBE)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)["cpus"], self.expected_cpus)

    def test_builds_have_low_priority_without_changing_the_caller(self):
        caller_priority = os.getpriority(os.PRIO_PROCESS, 0)
        result = self.run_build(sys.executable, "-c", PROBE)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["nice"], max(10, caller_priority))
        self.assertEqual(os.getpriority(os.PRIO_PROCESS, 0), caller_priority)

    def test_nested_builds_keep_the_initial_cpu_budget(self):
        result = self.run_build(sys.executable, str(RUNNER), sys.executable, "-c", PROBE)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["cpus"], self.expected_cpus)

    def test_existing_lower_priority_is_not_raised(self):
        script = f"import os; os.setpriority(os.PRIO_PROCESS, 0, 15); os.execv({sys.executable!r}, [{sys.executable!r}, {str(RUNNER)!r}, {sys.executable!r}, '-c', {PROBE!r}])"
        result = subprocess.run([sys.executable, "-c", script], env=self.environment,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["nice"], 15)

    def test_one_available_cpu_remains_usable(self):
        self.available_cpus = self.available_cpus[:1]
        result = self.run_build(sys.executable, "-c", PROBE)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["cpus"], self.available_cpus)

    def test_explicit_cpu_list_is_inherited(self):
        environment = self.environment | {"AGENT_BUILD_CPUS": str(self.available_cpus[-1])}
        result = self.run_build(sys.executable, "-c", PROBE, environment=environment)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["cpus"], [self.available_cpus[-1]])

    def test_invalid_cpu_lists_do_not_execute_the_command(self):
        for cpu_list in ["", "oops", "-1", "1,,2", "999999"]:
            with self.subTest(cpu_list=cpu_list):
                result = self.run_build(sys.executable, "-c", "print('EXECUTED')",
                                        environment=self.environment | {"AGENT_BUILD_CPUS": cpu_list})
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("EXECUTED", result.stdout)
                self.assertIn("agent-build:", result.stderr)

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

    def test_cpu_list_discovery_leaves_the_callers_scheduling_unchanged(self):
        caller_cpus = os.sched_getaffinity(0)
        result = self.run_build("--cpu-list")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), ",".join(map(str, self.expected_cpus)))
        self.assertEqual(os.sched_getaffinity(0), caller_cpus)

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
            self.assertEqual(json.loads(result.stdout)["cpus"], self.expected_cpus)

    def test_host_make_builds_use_the_same_budget(self):
        with tempfile.TemporaryDirectory() as directory:
            tool = Path(directory) / "cargo"
            tool.write_text(f"#!{sys.executable}\n{PROBE}\n")
            tool.chmod(0o755)
            result = subprocess.run(["make", "--no-print-directory", "cockpit"], cwd=REPO,
                                    env=self.environment | {"PATH": f"{directory}:{self.environment['PATH']}"},
                                    preexec_fn=lambda: os.sched_setaffinity(0, self.available_cpus),
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            observed = json.loads(result.stdout.splitlines()[-1])
            self.assertEqual(observed["cpus"], self.expected_cpus)
            self.assertGreaterEqual(observed["nice"], 10)


if __name__ == "__main__":
    unittest.main()
