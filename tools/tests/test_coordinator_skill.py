import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SKILL = REPO / ".agents/skills/coordinator"
WATCHER = SKILL / "scripts/coordinator_watch.py"
END_PRUNE = SKILL / "scripts/end_prune.sh"
APPLIER = SKILL / "scripts/apply_issue_changes.py"
START_WORKER = SKILL / "scripts/start_worker.sh"


class FakeCommandsTestCase(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.calls = self.root / "calls"
        self.environment = {
            **os.environ,
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "FAKE_CALLS": str(self.calls),
        }

    def tearDown(self):
        self.directory.cleanup()

    def shim(self, name, body):
        path = self.bin / name
        path.write_text(f"#!/bin/sh\n{body}\n")
        path.chmod(0o755)

    def recorded_calls(self):
        return self.calls.read_text().splitlines() if self.calls.exists() else []


class EndPruneTests(FakeCommandsTestCase):
    def setUp(self):
        super().setUp()
        self.shim("torlek", 'echo "torlek $*" >> "$FAKE_CALLS"')
        self.environment["END_PRUNE_SETTLE_SECONDS"] = "0"
        self.clones = self.root / "clones"
        self.clones.mkdir()

    def clone_with_target(self, name):
        (self.clones / name / "target").mkdir(parents=True)
        (self.clones / name / "target/artifact").write_text("build output")
        return self.clones / name

    def end_prune(self, *arguments):
        return subprocess.run(
            [str(END_PRUNE), *arguments], env=self.environment, capture_output=True, text=True
        )

    def test_it_ends_each_session_and_prunes_the_idle_clones_target(self):
        clone = self.clone_with_target("greenfield-1-first")

        result = self.end_prune(str(self.clones), "s1", "s2", "--", "greenfield-1-first")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.recorded_calls(), ["torlek session end s1", "torlek session end s2"])
        self.assertFalse((clone / "target").exists())
        self.assertIn("pruned greenfield-1-first/target", result.stdout)

    def test_it_still_ends_the_other_sessions_and_prunes_when_one_end_fails(self):
        clone = self.clone_with_target("greenfield-4-after")
        self.shim("torlek", 'echo "torlek $*" >> "$FAKE_CALLS"; [ "$3" != bad ]')

        result = self.end_prune(str(self.clones), "bad", "good", "--", "greenfield-4-after")

        self.assertEqual(result.returncode, 1)
        self.assertEqual(
            self.recorded_calls(), ["torlek session end bad", "torlek session end good"]
        )
        self.assertFalse((clone / "target").exists())

    def test_it_keeps_the_target_when_the_clones_root_is_given_through_a_symlink(self):
        clone = self.clone_with_target("greenfield-5-linked")
        link = self.root / "link"
        link.symlink_to(self.clones)
        worker = subprocess.Popen(["sleep", "30"], cwd=clone)
        self.addCleanup(worker.wait)
        self.addCleanup(worker.kill)

        result = self.end_prune(str(link), "s1", "--", "greenfield-5-linked")

        self.assertTrue((clone / "target/artifact").exists())
        self.assertIn("a process works in it", result.stdout)

    def test_it_keeps_the_target_of_a_clone_a_process_works_in(self):
        clone = self.clone_with_target("greenfield-2-busy")
        worker = subprocess.Popen(["sleep", "30"], cwd=clone / "target")
        self.addCleanup(worker.wait)
        self.addCleanup(worker.kill)

        result = self.end_prune(str(self.clones), "s1", "--", "greenfield-2-busy")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((clone / "target/artifact").exists())
        self.assertIn("skip greenfield-2-busy: a process works in it", result.stdout)

    def test_it_skips_a_clone_without_a_target(self):
        (self.clones / "greenfield-3-clean").mkdir()

        result = self.end_prune(str(self.clones), "s1", "--", "greenfield-3-clean")

        self.assertIn("skip greenfield-3-clean: no target/", result.stdout)

    def test_it_refuses_a_clone_name_that_leaves_the_clones_root(self):
        outside = self.root / "outside"
        (outside / "target").mkdir(parents=True)

        result = self.end_prune(str(self.clones), "s1", "--", "../outside")

        self.assertEqual(result.returncode, 2)
        self.assertTrue((outside / "target").exists())

    def test_it_fails_with_usage_when_the_clones_root_is_missing(self):
        result = self.end_prune(str(self.clones / "missing"), "s1")

        self.assertEqual(result.returncode, 2)
        self.assertIn("usage:", result.stderr)
        self.assertEqual(self.recorded_calls(), [])


class ApplyIssueChangesTests(FakeCommandsTestCase):
    REPORT = """# Report

## Changes for each linked issue

Proposed text; the owner applies it. Nothing here edits an issue. Shared context.

### #11 First issue

> Add the first line.
> Add the second line.

### #12 Second issue

> Reword the title.

### Not changed

- #13: nothing to do.
"""

    def setUp(self):
        super().setUp()
        self.shim(
            "gh",
            """
echo "gh $1 $2 $3 $4 $5" >> "$FAKE_CALLS"
printf '%s' "$7" > "$FAKE_CALLS.body.$3"
""",
        )
        self.checkout = self.root / "checkout"
        (self.checkout / "docs").mkdir(parents=True)
        subprocess.run(["git", "init", "-q"], cwd=self.checkout, check=True)
        self.report = self.checkout / "docs/report.md"
        self.report.write_text(self.REPORT)

    def comment_body(self, issue):
        return Path(f"{self.calls}.body.{issue}").read_text()

    def apply(self, *arguments, report=None):
        return subprocess.run(
            [
                sys.executable,
                str(APPLIER),
                str(report or self.report),
                "--decision-issue",
                "9",
                "--summary",
                "owner, Q1 A",
                "--repo",
                "example/shop",
                *arguments,
            ],
            env=self.environment,
            capture_output=True,
            text=True,
        )

    def test_it_posts_nothing_without_go(self):
        result = self.apply()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.recorded_calls(), [])
        self.assertIn("#11:", result.stdout)
        self.assertIn("Add the first line.\nAdd the second line.", result.stdout)

    def test_it_posts_one_comment_per_issue_with_go(self):
        result = self.apply("--go")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self.recorded_calls(),
            ["gh issue comment 11 -R example/shop", "gh issue comment 12 -R example/shop"],
        )
        body = self.comment_body(11)
        self.assertIn("Decided in #9 (owner, Q1 A;", body)
        self.assertIn("[report](https://github.com/example/shop/blob/main/docs/report.md)", body)

    def test_it_refuses_an_only_number_the_report_does_not_list(self):
        result = self.apply("--go", "--only", "99")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not list", result.stderr)
        self.assertEqual(self.recorded_calls(), [])

    def test_it_reads_a_last_heading_without_a_trailing_newline(self):
        self.report.write_text(
            "## Changes for each linked issue\n\n### #11 First\n\n> One.\n\n### #12 Second"
        )

        result = self.apply()

        self.assertIn("#12: 0 characters", result.stdout)

    def test_it_drops_the_report_only_sentence_and_the_blockquote_markers(self):
        result = self.apply()

        self.assertIn("Shared context.", result.stdout)
        self.assertNotIn("the owner applies it", result.stdout)
        self.assertNotIn("> Add", result.stdout)

    def test_it_posts_only_the_issues_named_by_only(self):
        result = self.apply("--go", "--only", "12")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.recorded_calls(), ["gh issue comment 12 -R example/shop"])

    def test_it_does_not_post_the_not_changed_section(self):
        result = self.apply("--go")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("nothing to do", self.comment_body(12))

    def test_it_fails_loudly_when_the_report_has_no_changes_section(self):
        self.report.write_text("# Report\n\nNo section.\n")

        result = self.apply("--go")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no '## Changes for each linked issue' section", result.stderr)
        self.assertEqual(self.recorded_calls(), [])


class StartWorkerTests(FakeCommandsTestCase):
    ITEM_TITLE = "#5 Add the thing"

    def setUp(self):
        super().setUp()
        self.environment.update(PROJECT_DIR="/projects/shop", WORKER_REPO="example/shop")
        self.shim("gh", 'echo "Add the thing"')
        self.write_torlek(
            list_rows=f"9\tshop\tdone\t2\tfeature\t{self.ITEM_TITLE}\n"
            f"12\tshop\ttodo\t2\tfeature\t{self.ITEM_TITLE}\n"
            "13\tshop\ttodo\t2\tfeature\t#5 Add the thing, later"
        )

    def write_torlek(self, list_rows):
        self.shim(
            "torlek",
            f"""
case "$1 $2" in
"todo add") echo "$3|$4|$5|$6|$7" >> "$FAKE_CALLS" ;;
"todo list") printf '{list_rows}\\n' ;;
"agent start") shift; printf '%s\\n' "$@" > "$FAKE_CALLS.start"; echo started ;;
esac
""",
        )

    def start_worker(self, *arguments):
        return subprocess.run(
            [str(START_WORKER), *arguments], env=self.environment, capture_output=True, text=True
        )

    def test_it_files_an_item_then_starts_the_worker_with_the_item_id_in_its_prompt(self):
        result = self.start_worker("5", "thing", "sonnet", "medium", "Work issue #5: add it.")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self.recorded_calls(),
            [f"{self.ITEM_TITLE}|--kind|feature|--notes|Worker branch greenfield/5-thing"],
        )
        start = Path(f"{self.calls}.start").read_text().splitlines()
        self.assertEqual(start[:2], ["start", "/projects/shop"])
        self.assertIn("--clone", start)
        self.assertEqual(start[start.index("--clone") + 1], "greenfield/5-thing")
        self.assertEqual(start[start.index("--model") + 1], "sonnet")
        self.assertEqual(start[start.index("--effort") + 1], "medium")
        prompt = start[start.index("--prompt") + 1]
        self.assertTrue(prompt.startswith("Work issue #5: add it. Follow CLAUDE.md"))
        self.assertIn("torlek todo start 12", prompt)
        self.assertIn("torlek todo done 12 --summary", prompt)
        self.assertNotIn("TODO_ID", prompt)

    def test_it_fails_without_starting_when_no_item_with_that_title_exists(self):
        self.write_torlek(list_rows="7\tshop\ttodo\t2\tfeature\tSomething else")

        result = self.start_worker("5", "thing", "sonnet", "medium", "Work issue #5.")

        self.assertEqual(result.returncode, 1)
        self.assertIn("no task item found", result.stderr)
        self.assertFalse(Path(f"{self.calls}.start").exists())

    def test_it_fails_with_usage_when_an_argument_is_missing(self):
        result = self.start_worker("5", "thing")

        self.assertEqual(result.returncode, 2)
        self.assertIn("usage:", result.stderr)


class WatcherTests(FakeCommandsTestCase):
    HEAD = "abcdef1234567890"

    def setUp(self):
        super().setUp()
        self.worker_root = self.root / "workers"
        self.state_file = self.root / "state.json"
        self.state_file.write_text("{}")
        self.sessions_file = self.root / "sessions"
        self.merged_file = self.root / "merged"
        self.merged_file.write_text("0")
        self.pulls_file = self.root / "pulls"
        self.pulls_file.write_text("[]")
        self.environment.update(
            FAKE_SESSIONS=str(self.sessions_file),
            FAKE_MERGED=str(self.merged_file),
            FAKE_PULLS=str(self.pulls_file),
        )
        self.shim("torlek", 'cat "$FAKE_SESSIONS"; sleep 1.5; echo "no grant" >&2; exit 3')
        self.shim(
            "gh",
            """
case "$*" in
*--search*) cat "$FAKE_PULLS" ;;
*"--state merged"*) cat "$FAKE_MERGED" ;;
esac
""",
        )

    def session(self, name, state, doing="", question="", root=None):
        workspace = f"{root or self.worker_root}/{name}"
        fields = f'doing="{doing}"' + (f' needs-input="{question}"' if question else "")
        return "\t".join(["s-" + name, "running", "-", state, workspace, fields])

    def watch(self, *session_lines):
        self.sessions_file.write_text("\n".join(session_lines) + "\n")
        return subprocess.run(
            [
                sys.executable,
                str(WATCHER),
                str(self.state_file),
                "--repo",
                "example/shop",
                "--worker-root",
                str(self.worker_root),
                "--since",
                "2026-01-01",
                "--seed-seconds",
                "0",
                "--poll-seconds",
                "0.2",
                "--summary-seconds",
                "1000",
            ],
            env=self.environment,
            capture_output=True,
            text=True,
            timeout=30,
        )

    def pull_with_failing_checks(self, head):
        return {
            "number": 7,
            "state": "OPEN",
            "headRefName": "greenfield/7-feature",
            "headRefOid": head,
            "statusCheckRollup": [
                {"name": "greenfield", "conclusion": "FAILURE", "detailsUrl": "https://ci/job/1"},
                {"name": "greenfield-macos", "conclusion": "FAILURE"},
                {"name": "docs", "conclusion": "SUCCESS"},
            ],
        }

    def test_it_reports_a_workers_question_with_its_branch(self):
        result = self.watch(
            self.session("greenfield-5-thing", "waiting", "asking", question="Which option?")
        )

        self.assertIn(
            "QUESTION s-greenfield-5-thing (greenfield/5-thing): Which option?", result.stdout
        )

    def test_it_ignores_sessions_outside_the_worker_root(self):
        result = self.watch(
            self.session("greenfield-5-thing", "waiting", question="Mine?", root="/elsewhere")
        )

        self.assertNotIn("QUESTION", result.stdout)

    def test_it_reports_a_worker_going_idle_without_a_merged_pr(self):
        result = self.watch(
            self.session("greenfield-6-idle", "working", "coding"),
            self.session("greenfield-6-idle", "waiting", "coding"),
        )

        self.assertIn(
            "IDLE without merged PR s-greenfield-6-idle (greenfield/6-idle)", result.stdout
        )

    def test_it_stays_quiet_when_an_idle_worker_has_a_merged_pr(self):
        self.merged_file.write_text("1")

        result = self.watch(
            self.session("greenfield-6-idle", "working", "coding"),
            self.session("greenfield-6-idle", "waiting", "coding"),
        )

        self.assertNotIn("IDLE", result.stdout)

    def test_it_reports_a_ci_failure_once_per_head_and_skips_the_macos_job(self):
        self.pulls_file.write_text(json.dumps([self.pull_with_failing_checks(self.HEAD)]))

        result = self.watch(self.session("greenfield-7-feature", "working", "coding"))

        failures = [line for line in result.stdout.splitlines() if line.startswith("CI ")]
        self.assertEqual(len(failures), 1, result.stdout)
        self.assertIn(
            "PR #7 (greenfield/7-feature) head abcdef12: greenfield https://ci/job/1", failures[0]
        )

    def test_it_reports_a_pr_merge_on_a_later_poll_after_an_empty_first_poll(self):
        pull = {
            "number": 8,
            "state": "OPEN",
            "headRefName": "greenfield/8-x",
            "headRefOid": "1" * 16,
        }
        merged = {**pull, "state": "MERGED"}
        # Poll 1 is empty and poll 2 sees the PR open, so the MERGED on poll 3 is a real event.
        self.pulls_file.write_text(json.dumps([merged]))
        pulls_open = json.dumps([pull])
        self.shim(
            "gh",
            f"""
case "$*" in
*--search*)
    count=$(cat "$FAKE_PULLS.count" 2>/dev/null || echo 0)
    echo $((count + 1)) > "$FAKE_PULLS.count"
    case $count in 0) echo '[]' ;; 1) echo '{pulls_open}' ;; *) cat "$FAKE_PULLS" ;; esac ;;
esac
""",
        )

        result = self.watch(self.session("greenfield-8-x", "working", "coding"))

        self.assertIn("PR #8 MERGED (greenfield/8-x)", result.stdout)

    def test_it_ignores_pull_requests_outside_the_branch_prefix(self):
        stray = {**self.pull_with_failing_checks(self.HEAD), "headRefName": "feature/other"}
        self.pulls_file.write_text(json.dumps([stray]))

        result = self.watch(self.session("greenfield-7-feature", "working", "coding"))

        self.assertNotIn("CI ", result.stdout)

    def test_it_says_the_merge_state_is_unknown_when_gh_fails(self):
        self.shim(
            "gh",
            """
case "$*" in
*--search*) echo '[]' ;;
*) exit 1 ;;
esac
""",
        )

        result = self.watch(
            self.session("greenfield-6-idle", "working", "coding"),
            self.session("greenfield-6-idle", "waiting", "coding"),
        )

        self.assertIn("IDLE, merge state unknown", result.stdout)
        self.assertNotIn("IDLE without merged PR", result.stdout)

    def test_it_exits_nonzero_and_says_why_when_the_session_watch_ends(self):
        result = self.watch(self.session("greenfield-8-gone", "working", "coding"))

        self.assertEqual(result.returncode, 3)
        self.assertIn(
            "WATCH ENDED: torlek agent sessions --watch exited 3: no grant", result.stdout
        )


class SkillContentTests(unittest.TestCase):
    def test_nothing_personal_appears_in_the_skill(self):
        pattern = (
            r"([0-9]{1,3}\.){3}[0-9]{1,3}|\.ts\.net|gh[opsu]_[A-Za-z0-9]{20,}|"
            r"/home/|/Users/|\.local/state|Theo|Androlax2"
        )
        result = subprocess.run(
            ["grep", "-rnE", pattern, str(SKILL)], capture_output=True, text=True
        )

        self.assertEqual(result.stdout, "")

    def test_the_scripts_are_executable(self):
        for script in (WATCHER, END_PRUNE, APPLIER, START_WORKER):
            self.assertTrue(os.access(script, os.X_OK), script)


if __name__ == "__main__":
    unittest.main()
