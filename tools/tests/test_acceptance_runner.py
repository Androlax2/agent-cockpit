import os
import re
import shutil
import socket
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "tools/acceptance/run.sh"
GUIDES = (
    REPO / "docs/acceptance/m1.md",
    REPO / "docs/acceptance/m1-idle.md",
    REPO / "docs/acceptance/m2.md",
    REPO / "docs/acceptance/m3.md",
)
# The guides handed over with `torlek ask guide` rather than ask-to-guide.
TORLEK_GUIDES = (REPO / "docs/acceptance/m2.md",)
# `torlek ask guide`'s limits (crates/cockpit-approvals/src/guides.rs).
TORLEK_GUIDE_MAX_STEPS = 100
TORLEK_GUIDE_MAX_BYTES = 64 * 1024
TORLEK_GUIDE_LINE_MAX_CHARS = 1000
BINARIES = ("torlek", "torlek-daemon", "torlek-tui", "torlek-claude-hook")
# ask-to-guide's limit (apps/cockpit/src/bin/ask-to-guide.rs MAX_STEPS).
MAX_STEPS = 60
BASH = shutil.which("bash")
# What `run.sh start` runs, for the tests that give it a PATH without the host's claude programs.
START_TOOLS = (
    "awk", "bash", "cat", "chmod", "dirname", "env", "git", "grep", "id", "ln", "mkdir",
    "readlink", "realpath", "setsid", "sleep", "touch",
)
RUNNER_VERBS = {
    "start",
    "tui",
    "tui-window",
    "torlek",
    "session",
    "kill-daemon",
    "stop",
    "clean",
    "fill-target",
    "prune-prepare",
    "big-needs",
    "todo-image",
    "code-change",
}


def guide_steps(text, repeatable=()):
    """The guide's steps as ask-to-guide reads them (its parse_steps): unindented lines other
    than `# ` and `## ` headings, each with the attributes and text indented under it.
    `torlek ask guide` also takes several `copy:` and `open:` lines on one step."""
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
            if attribute[1] in steps[-1] and attribute[1] not in repeatable:
                raise AssertionError(f"second {attribute[1]}: on one step: {line}")
            steps[-1][attribute[1]] = attribute[2]
        elif not line.startswith(("# ", "## ")):
            steps.append({"text": line})
    return steps


def repeatable_attributes(guide):
    return ("copy", "open") if guide in TORLEK_GUIDES else ()


class GuideTests(unittest.TestCase):
    def test_the_guides_fit_ask_to_guide(self):
        for guide in GUIDES:
            with self.subTest(guide=guide.name):
                steps = guide_steps(guide.read_text(), repeatable_attributes(guide))
                self.assertGreater(len(steps), 0)
                self.assertLessEqual(len(steps), MAX_STEPS)

    def test_the_torlek_guides_parse_as_torlek_ask_guide_reads_them(self):
        """Its parse_guide: a `# Title` first, then `## Section` headings, unindented step lines
        and indented `open:`, `copy:`, `ask:` or `run:` lines; no continuation lines."""
        for guide in TORLEK_GUIDES:
            with self.subTest(guide=guide.name):
                text = guide.read_text()
                self.assertLessEqual(len(text.encode()), TORLEK_GUIDE_MAX_BYTES)
                lines = [line for line in text.splitlines() if line.strip()]
                self.assertTrue(lines[0].startswith("# "))
                steps = 0
                for line in lines[1:]:
                    self.assertLessEqual(len(line.strip()), TORLEK_GUIDE_LINE_MAX_CHARS, line)
                    if line.startswith("## "):
                        continue
                    self.assertFalse(line.startswith("#"), line)
                    if line[0] in " \t":
                        self.assertRegex(line.strip(), r"^(open|copy|ask|run):\s*\S", line)
                    else:
                        steps += 1
                self.assertLessEqual(steps, TORLEK_GUIDE_MAX_STEPS)

    def test_every_run_step_goes_through_the_runner(self):
        for guide in GUIDES:
            for step in guide_steps(guide.read_text(), repeatable_attributes(guide)):
                if "run" not in step:
                    continue
                with self.subTest(guide=guide.name, run=step["run"]):
                    words = step["run"].split()
                    self.assertEqual(words[:2], ["bash", "tools/acceptance/run.sh"])
                    self.assertIn(words[2], RUNNER_VERBS)


# Stands in for each program: `torlek session new` prints a session line for s-new, another
# `torlek session` verb prints itself, anything else prints its directory and environment.
STAND_IN = """#!/bin/sh
case "$1 $2" in
"session new") printf 's-new\\trunning\\tclaude\\tstart\\t-\\t%s\\t-\\t-\\t-\\n' "$PWD/$3" ;;
session\\ *) echo "$*" ;;
"resources status") printf 'platform linux\\nbudget 16000 millicores 31691 MiB\\tin use 0\\n' ;;
"todo add") printf 'item 7 (intent revision 1, state revision 1)\\nproject: demo\\n' ;;
"checkout list") printf '1\\tdemo\\tmain\\t1\\n3\\tdemo\\tparallel:prune-me\\t1\\n' ;;
"todo image"|"workflow start"|"storage prune") echo "$*" ;;
*) pwd; env ;;
esac
"""


class RunnerTests(unittest.TestCase):
    """run.sh copied into a fake clone whose target/release holds stand-ins."""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = root = Path(self.directory.name)
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
        self.state = root / "state"
        # A real tmux falls back to /tmp when TMUX_TMPDIR is missing, where the owner's servers
        # live: the tests only ever see this recorder.
        self.tmux_calls = root / "tmux-calls"
        fake_tmux = root / "fake-tmux/tmux"
        fake_tmux.parent.mkdir()
        fake_tmux.write_text(f'#!/bin/sh\necho "$*" >> {self.tmux_calls}\n')
        fake_tmux.chmod(0o755)
        self.environment = {
            **os.environ,
            "PATH": f"{fake_tmux.parent}:{os.environ['PATH']}",
            "COCKPIT_ACCEPTANCE_HOME": str(self.scratch),
            "COCKPIT_ACCEPTANCE_STATE": str(self.state),
            "TORLEK_PROFILE": "torlek",
            "TORLEK_SESSION": "s-installed",
            "TORLEK_LAUNCH_TOKEN": "installed-token",
            "GH_TOKEN": "ghp_owner",
            "XDG_RUNTIME_DIR": "/run/user/owner",
            "XDG_STATE_HOME": "/home/owner/.local/state",
        }

    def tearDown(self):
        self.directory.cleanup()

    def run_script(self, *arguments, **overrides):
        return subprocess.run(
            [BASH, str(self.script), *arguments],
            env={**self.environment, **overrides},
            capture_output=True,
            text=True,
        )

    def test_torlek_runs_this_clones_build_on_the_scratch_home(self):
        (self.scratch / "projects").mkdir(parents=True)
        result = self.run_script("torlek", "version")
        self.assertEqual(result.returncode, 0, result.stderr)
        cwd, *lines = result.stdout.splitlines()
        variables = dict(line.split("=", 1) for line in lines if "=" in line)
        self.assertEqual(cwd, str(self.scratch / "projects"))
        for name, place in (
            ("XDG_CONFIG_HOME", self.scratch / "config"),
            ("XDG_STATE_HOME", self.state / "state"),
            ("XDG_CACHE_HOME", self.state / "cache"),
            ("XDG_RUNTIME_DIR", self.scratch / "runtime"),
            ("TMUX_TMPDIR", self.scratch / "tmux"),
        ):
            with self.subTest(variable=name):
                self.assertEqual(variables[name], str(place))
        self.assertEqual(
            variables["PATH"].split(":")[:2], [str(self.scratch / "bin"), str(self.bin)]
        )

    def test_torlek_runs_as_profile_acceptance_without_the_installed_sessions_identity(self):
        (self.scratch / "projects").mkdir(parents=True)
        result = self.run_script("torlek", "version")
        self.assertEqual(result.returncode, 0, result.stderr)
        variables = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
        self.assertEqual(variables["TORLEK_PROFILE"], "acceptance")
        for name in ("TORLEK_SESSION", "TORLEK_LAUNCH_TOKEN", "GH_TOKEN"):
            with self.subTest(variable=name):
                self.assertNotIn(name, variables)

    def test_the_state_stays_in_the_clone_by_default(self):
        (self.scratch / "projects").mkdir(parents=True)
        del self.environment["COCKPIT_ACCEPTANCE_STATE"]
        result = self.run_script("torlek", "version")
        self.assertEqual(result.returncode, 0, result.stderr)
        variables = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
        self.assertEqual(variables["XDG_STATE_HOME"], str(self.clone / "target/acceptance/state"))

    def make_program(self, relative, body=""):
        program = self.root / relative
        program.parent.mkdir(parents=True, exist_ok=True)
        program.write_text(f"#!/bin/sh\n{body}\n")
        program.chmod(0o755)
        return program

    def start_with_path(self, *directories, **overrides):
        """run.sh start with the given directories (relative to the temporary directory) first on
        PATH and a daemon stand-in that is ready at once."""
        # Alive a moment after ready, like a real daemon, so start's liveness check holds.
        (self.bin / "torlek-daemon").write_text("#!/bin/sh\necho 'ready stand-in'\nexec sleep 2\n")
        # The host's own PATH holds claude programs: give the script only the tools it uses.
        tools = self.root / "tools"
        tools.mkdir()
        for tool in START_TOOLS:
            (tools / tool).symlink_to(shutil.which(tool))
        path = [str(self.root / directory) for directory in directories] + [str(tools)]
        return self.run_script("start", PATH=":".join(path), **overrides)

    def linked_claude(self):
        return Path(os.readlink(self.scratch / "bin/claude"))

    def test_start_writes_the_walks_configuration_and_demo_project(self):
        self.make_program("real/claude")
        result = self.start_with_path("real")
        self.assertEqual(result.returncode, 0, result.stderr)
        config = (self.scratch / "config/torlek-acceptance/config.toml").read_text()
        self.assertIn('sandbox = "off"', config)
        self.assertIn('target_prune_base_ref = "refs/heads/main"', config)
        demo = self.scratch / "projects/demo"
        tracked = subprocess.run(
            ["git", "-C", str(demo), "ls-files"], capture_output=True, text=True, check=True
        ).stdout.split()
        self.assertEqual(sorted(tracked), [".gitignore", "README.md"])
        self.assertEqual((demo / ".gitignore").read_text(), "target/\n")
        self.assertIn('name = "checks"', (demo / ".torlek/config.toml").read_text())
        status = subprocess.run(
            ["git", "-C", str(demo), "status", "--porcelain"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        self.assertEqual(status, "")

    def test_start_refuses_a_state_directory_with_files_it_did_not_make(self):
        self.make_program("real/claude")
        self.state.mkdir()
        (self.state / "keep").write_text("owner data")
        result = self.start_with_path("real")
        self.assertEqual(result.returncode, 1)
        self.assertIn("was not made by run.sh", result.stderr)
        self.assertFalse((self.state / ".torlek-acceptance").exists())

    def test_start_links_the_first_claude_outside_greywall(self):
        self.make_program("greywall-agents/claude")
        real = self.make_program("real/claude")
        result = self.start_with_path("greywall-agents", "real")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.linked_claude(), real)

    def test_start_skips_a_claude_that_links_to_the_greywall_launcher(self):
        launcher = self.make_program("greywall-agents/claude")
        (self.root / "shim").mkdir()
        (self.root / "shim/claude").symlink_to(launcher)
        real = self.make_program("real/claude")
        result = self.start_with_path("shim", "real")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.linked_claude(), real)

    def test_start_links_the_claude_the_owner_names(self):
        self.make_program("real/claude")
        chosen = self.make_program("chosen/claude")
        result = self.start_with_path("real", COCKPIT_ACCEPTANCE_CLAUDE=str(chosen))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.linked_claude(), chosen)

    def test_start_without_a_claude_outside_greywall_fails(self):
        self.make_program("greywall-agents/claude")
        result = self.start_with_path("greywall-agents")
        self.assertEqual(result.returncode, 1)
        self.assertIn("COCKPIT_ACCEPTANCE_CLAUDE", result.stderr)

    def open_tui_window(self, hyprctl_exit=0):
        """run.sh tui-window with stand-ins for hyprctl and setsid that record their calls."""
        (self.scratch / "projects").mkdir(parents=True)
        calls = self.root / "calls"
        self.make_program("fakes/hyprctl", f'echo "hyprctl $XDG_RUNTIME_DIR $*" >> {calls}\nexit {hyprctl_exit}')
        self.make_program("fakes/setsid", f'echo "setsid ${{XDG_CONFIG_HOME:-unset}} $XDG_RUNTIME_DIR $*" >> {calls}')
        result = self.run_script("tui-window", PATH=f"{self.root / 'fakes'}:{self.environment['PATH']}")
        return result, calls.read_text().splitlines()

    def test_tui_window_opens_the_tui_in_an_opaque_window_of_its_own(self):
        result, (hyprctl, setsid) = self.open_tui_window()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(hyprctl.startswith("hyprctl /run/user/owner eval hl.window_rule("))
        self.assertIn('class = "^(torlek-acceptance)$"', hyprctl)
        self.assertIn('opacity = "1 1"', hyprctl)
        self.assertEqual(
            setsid,
            f"setsid unset /run/user/owner -f kitty --class torlek-acceptance bash {self.script} tui",
        )

    def test_tui_window_opens_nothing_when_hyprctl_fails(self):
        result, calls = self.open_tui_window(hyprctl_exit=1)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(calls), 1)
        self.assertTrue(calls[0].startswith("hyprctl "))

    def test_torlek_before_start_says_to_start_first(self):
        result = self.run_script("torlek", "version")
        self.assertEqual(result.returncode, 1)
        self.assertIn("run.sh start first", result.stderr)

    def test_a_missing_build_is_named(self):
        (self.bin / "torlek-tui").unlink()
        result = self.run_script("tui")
        self.assertEqual(result.returncode, 1)
        self.assertIn("torlek-tui is not built", result.stderr)

    def test_a_relative_scratch_home_is_refused(self):
        result = self.run_script("stop", COCKPIT_ACCEPTANCE_HOME="scratch")
        self.assertEqual(result.returncode, 2)

    def test_clean_removes_the_scratch_and_state_directories_it_made(self):
        for made in (self.scratch, self.state):
            made.mkdir()
            (made / ".torlek-acceptance").touch()
        result = self.run_script("clean")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.scratch.exists())
        self.assertFalse(self.state.exists())

    def test_stop_without_a_scratch_tmux_server_runs_no_tmux(self):
        self.scratch.mkdir()
        result = self.run_script("stop")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.tmux_calls.exists())

    def test_stop_after_the_scratch_directory_is_gone_runs_no_tmux(self):
        # The fallback that reached the installed Torlek: a missing TMUX_TMPDIR (#845).
        result = self.run_script("stop")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.scratch.exists())
        self.assertFalse(self.tmux_calls.exists())

    def test_the_runner_tests_reach_only_the_recording_tmux(self):
        self.assertEqual(
            shutil.which("tmux", path=self.environment["PATH"]),
            str(self.tmux_calls.parent / "fake-tmux/tmux"),
        )

    def test_stop_kills_the_scratch_tmux_servers_by_their_socket_paths(self):
        sockets = self.scratch / f"tmux/tmux-{os.getuid()}"
        sockets.mkdir(parents=True)
        listeners = []
        for name in ("torlek-acceptance", "torlek"):
            listener = socket.socket(socket.AF_UNIX)
            listener.bind(str(sockets / name))
            listeners.append(listener)
        try:
            result = self.run_script("stop")
        finally:
            for listener in listeners:
                listener.close()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self.tmux_calls.read_text().splitlines(),
            [
                f"-S {sockets}/torlek-acceptance kill-server",
                f"-S {sockets}/torlek kill-server",
            ],
        )

    def test_clean_refuses_a_state_directory_it_did_not_make(self):
        self.scratch.mkdir()
        (self.scratch / ".torlek-acceptance").touch()
        self.state.mkdir()
        keep = self.state / "keep"
        keep.write_text("owner data")
        result = self.run_script("clean")
        self.assertEqual(result.returncode, 1)
        self.assertTrue(keep.exists())
        self.assertTrue(self.scratch.exists())

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
        for verb in ("sleep", "wake", "end", "resume", "show"):
            with self.subTest(verb=verb):
                result = self.run_script("session", verb)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), f"session {verb} s-new")

    def test_session_verbs_pass_their_other_arguments_after_the_session(self):
        (self.scratch / "projects").mkdir(parents=True)
        self.run_script("session", "new")
        result = self.run_script("session", "grant", "start-sessions", "--per-hour", "4")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "session grant s-new start-sessions --per-hour 4")

    def make_demo(self):
        demo = self.scratch / "projects/demo"
        demo.mkdir(parents=True)
        return demo

    def test_fill_target_writes_stand_in_build_output_into_the_clone(self):
        clone = self.scratch / "projects/.parallel/demo/prune-me"
        (clone / ".git").mkdir(parents=True)
        result = self.run_script("fill-target", "prune-me")
        self.assertEqual(result.returncode, 0, result.stderr)
        sizes = [entry.stat().st_size for entry in (clone / "target").rglob("*") if entry.is_file()]
        self.assertEqual(sum(sizes), 25165824)

    def test_fill_target_refuses_a_clone_that_does_not_exist(self):
        (self.scratch / "projects").mkdir(parents=True)
        result = self.run_script("fill-target", "absent")
        self.assertEqual(result.returncode, 1)
        self.assertIn("checkout clone demo absent", result.stderr)

    def test_fill_target_finds_a_clone_of_a_branch_with_a_slash(self):
        clone = self.scratch / "projects/.parallel/demo/feature-x"
        (clone / ".git").mkdir(parents=True)
        result = self.run_script("fill-target", "feature/x")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((clone / "target/debug/stand-in-1").exists())

    def test_fill_target_refuses_a_branch_that_leaves_the_demo_clones(self):
        outside = self.scratch / "projects/elsewhere"
        (outside / ".git").mkdir(parents=True)
        result = self.run_script("fill-target", "../../elsewhere")
        self.assertEqual(result.returncode, 2)
        self.assertFalse((outside / "target").exists())

    def test_prune_prepare_prunes_the_checkout_number_of_the_clone(self):
        (self.scratch / "projects").mkdir(parents=True)
        result = self.run_script("prune-prepare", "prune-me")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[-1], "storage prune 3")

    def test_prune_prepare_refuses_a_clone_checkout_list_does_not_show(self):
        (self.scratch / "projects").mkdir(parents=True)
        result = self.run_script("prune-prepare", "absent")
        self.assertEqual(result.returncode, 1)
        self.assertNotIn("storage prune", result.stdout)

    def test_big_needs_reserves_two_fifths_of_the_budget_and_off_removes_it(self):
        demo = self.make_demo()
        result = self.run_script("big-needs", "on")
        self.assertEqual(result.returncode, 0, result.stderr)
        config = (demo / ".torlek/config.toml").read_text()
        self.assertIn("[needs]\ncpu_millis = 6400\n", config)
        self.assertIn('name = "checks"', config)
        result = self.run_script("big-needs", "off")
        self.assertEqual(result.returncode, 0, result.stderr)
        config = (demo / ".torlek/config.toml").read_text()
        self.assertNotIn("[needs]", config)
        self.assertIn('name = "checks"', config)

    def test_todo_image_reads_the_file_from_the_clone(self):
        (self.scratch / "projects").mkdir(parents=True)
        result = self.run_script("todo-image", "1", "assets/note.png")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), f"todo image add 1 {self.clone}/assets/note.png")

    def test_code_change_starts_the_workflow_on_the_item_it_filed(self):
        (self.scratch / "projects").mkdir(parents=True)
        result = self.run_script("code-change", "Add greet.sh", "Print hello.")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[-1], "workflow start 7 --preset code-change")

    def test_session_verbs_before_session_new_are_refused(self):
        (self.scratch / "projects").mkdir(parents=True)
        result = self.run_script("session", "sleep")
        self.assertEqual(result.returncode, 1)
        self.assertIn("run.sh session new", result.stderr)

    def test_unknown_verbs_print_the_usage(self):
        for arguments in ((), ("deploy",), ("session", "list")):
            with self.subTest(arguments=arguments):
                result = self.run_script(*arguments)
                self.assertEqual(result.returncode, 2)
                self.assertIn("usage:", result.stderr)


if __name__ == "__main__":
    unittest.main()
