import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "tools/acceptance/run.sh"
GUIDE = REPO / "docs/acceptance/m1.md"
BINARIES = ("cockpit", "cockpit-daemon", "cockpit-tui", "cockpit-claude-hook")
# ask-to-guide's limit (apps/cockpit/src/bin/ask-to-guide.rs MAX_STEPS).
MAX_STEPS = 60
RUNNER_VERBS = {"start", "tui", "cockpit", "session", "kill-daemon", "stop", "clean"}


def guide_steps(text):
    """The guide's steps as ask-to-guide reads them (its parse_steps): unindented lines other
    than `# ` and `## ` headings, each with the attributes and text indented under it."""
    steps = []
    for raw in text.splitlines():
        line = raw.strip()
        if not line:
            continue
        if raw[0] in " \t" and steps:
            attribute = re.match(r"^(open|copy|ask|run):\s*(\S.*)$", line)
            if attribute is None:
                steps[-1]["text"] += "\n" + line
                continue
            if attribute[1] in steps[-1]:
                raise AssertionError(f"second {attribute[1]}: on one step: {line}")
            steps[-1][attribute[1]] = attribute[2]
        elif not line.startswith(("# ", "## ")):
            steps.append({"text": line})
    return steps


class GuideTests(unittest.TestCase):
    def test_the_m1_guide_fits_ask_to_guide(self):
        steps = guide_steps(GUIDE.read_text())
        self.assertGreater(len(steps), 0)
        self.assertLessEqual(len(steps), MAX_STEPS)

    def test_every_run_step_goes_through_the_runner(self):
        for step in guide_steps(GUIDE.read_text()):
            if "run" not in step:
                continue
            with self.subTest(run=step["run"]):
                words = step["run"].split()
                self.assertEqual(words[:2], ["bash", "tools/acceptance/run.sh"])
                self.assertIn(words[2], RUNNER_VERBS)


# Stands in for each program: `cockpit session new` prints a session line for s-new, another
# `cockpit session` verb prints itself, anything else prints its directory and environment.
STAND_IN = """#!/bin/sh
case "$1 $2" in
"session new") printf 's-new\\trunning\\tclaude\\tstart\\t-\\t%s\\t-\\t-\\t-\\n' "$PWD/$3" ;;
session\\ *) echo "$*" ;;
*) pwd; env ;;
esac
"""


class RunnerTests(unittest.TestCase):
    """run.sh copied into a fake clone whose target/release holds stand-ins."""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        root = Path(self.directory.name)
        self.clone = root / "clone"
        self.script = self.clone / "tools/acceptance/run.sh"
        self.script.parent.mkdir(parents=True)
        shutil.copy(SCRIPT, self.script)
        self.bin = self.clone / "target/release"
        self.bin.mkdir(parents=True)
        for name in BINARIES:
            stand_in = self.bin / name
            stand_in.write_text(STAND_IN)
            stand_in.chmod(0o755)
        self.scratch = root / "scratch"
        self.environment = {
            **os.environ,
            "COCKPIT_ACCEPTANCE_HOME": str(self.scratch),
            "XDG_RUNTIME_DIR": "/run/user/owner",
            "XDG_STATE_HOME": "/home/owner/.local/state",
        }

    def tearDown(self):
        self.directory.cleanup()

    def run_script(self, *arguments, **overrides):
        return subprocess.run(
            ["bash", str(self.script), *arguments],
            env={**self.environment, **overrides},
            capture_output=True,
            text=True,
        )

    def test_cockpit_runs_this_clones_build_on_the_scratch_home(self):
        (self.scratch / "projects").mkdir(parents=True)
        result = self.run_script("cockpit", "version")
        self.assertEqual(result.returncode, 0, result.stderr)
        cwd, *lines = result.stdout.splitlines()
        variables = dict(line.split("=", 1) for line in lines if "=" in line)
        self.assertEqual(cwd, str(self.scratch / "projects"))
        for name, place in (
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_RUNTIME_DIR", "runtime"),
            ("TMUX_TMPDIR", "tmux"),
        ):
            with self.subTest(variable=name):
                self.assertEqual(variables[name], str(self.scratch / place))
        self.assertEqual(variables["PATH"].split(":")[0], str(self.bin))

    def test_cockpit_before_start_says_to_start_first(self):
        result = self.run_script("cockpit", "version")
        self.assertEqual(result.returncode, 1)
        self.assertIn("run.sh start first", result.stderr)

    def test_a_missing_build_is_named(self):
        (self.bin / "cockpit-tui").unlink()
        result = self.run_script("tui")
        self.assertEqual(result.returncode, 1)
        self.assertIn("cockpit-tui is not built", result.stderr)

    def test_a_relative_scratch_home_is_refused(self):
        result = self.run_script("stop", COCKPIT_ACCEPTANCE_HOME="scratch")
        self.assertEqual(result.returncode, 2)

    def test_clean_refuses_a_directory_it_did_not_make(self):
        self.scratch.mkdir()
        keep = self.scratch / "keep"
        keep.write_text("owner data")
        result = self.run_script("clean")
        self.assertEqual(result.returncode, 1)
        self.assertTrue(keep.exists())

    def test_session_verbs_act_on_the_session_session_new_started(self):
        (self.scratch / "projects").mkdir(parents=True)
        started = self.run_script("session", "new", "--prompt", "hi")
        self.assertEqual(started.returncode, 0, started.stderr)
        self.assertTrue(started.stdout.startswith("s-new\t"))
        for verb in ("sleep", "wake", "end", "resume"):
            with self.subTest(verb=verb):
                result = self.run_script("session", verb)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), f"session {verb} s-new")

    def test_session_verbs_before_session_new_are_refused(self):
        (self.scratch / "projects").mkdir(parents=True)
        result = self.run_script("session", "sleep")
        self.assertEqual(result.returncode, 1)
        self.assertIn("run.sh session new", result.stderr)

    def test_unknown_verbs_print_the_usage(self):
        for arguments in ((), ("deploy",), ("session", "show")):
            with self.subTest(arguments=arguments):
                result = self.run_script(*arguments)
                self.assertEqual(result.returncode, 2)
                self.assertIn("usage:", result.stderr)


if __name__ == "__main__":
    unittest.main()
