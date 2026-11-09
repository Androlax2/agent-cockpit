import argparse
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SKILL = REPO / ".agents/skills/coordinator"
WATCHER = SKILL / "scripts/coordinator_watch.py"
PRUNE_TARGET = SKILL / "scripts/prune_target.sh"
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


class PruneTargetTests(FakeCommandsTestCase):
    def setUp(self):
        super().setUp()
        self.clones = self.root / "clones"
        self.clones.mkdir()

    def clone_with_target(self, name):
        (self.clones / name / "target").mkdir(parents=True)
        (self.clones / name / "target/artifact").write_text("build output")
        return self.clones / name

    def prune_target(self, *arguments):
        return subprocess.run(
            [str(PRUNE_TARGET), *arguments], env=self.environment, capture_output=True, text=True
        )

    def test_it_prunes_the_idle_clones_target(self):
        clone = self.clone_with_target("greenfield-1-first")

        result = self.prune_target(str(self.clones), "greenfield-1-first")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((clone / "target").exists())
        self.assertIn("pruned greenfield-1-first/target", result.stdout)

    def test_it_keeps_the_rest_of_the_clone(self):
        clone = self.clone_with_target("greenfield-1-first")
        (clone / "notes.md").write_text("uncommitted work")

        self.prune_target(str(self.clones), "greenfield-1-first")

        self.assertTrue((clone / "notes.md").exists())

    def test_it_prunes_every_named_clone(self):
        first = self.clone_with_target("greenfield-1-first")
        second = self.clone_with_target("greenfield-2-second")

        self.prune_target(str(self.clones), "greenfield-1-first", "greenfield-2-second")

        self.assertFalse((first / "target").exists())
        self.assertFalse((second / "target").exists())

    def test_it_refuses_a_clone_that_is_a_symlink(self):
        outside = self.root / "outside"
        (outside / "target").mkdir(parents=True)
        (self.clones / "greenfield-6-link").symlink_to(outside)

        result = self.prune_target(str(self.clones), "greenfield-6-link")

        self.assertEqual(result.returncode, 2)
        self.assertIn("refusing symlinked clone", result.stderr)
        self.assertTrue((outside / "target").exists())

    def test_it_keeps_the_target_when_the_clones_root_is_given_through_a_symlink(self):
        clone = self.clone_with_target("greenfield-5-linked")
        link = self.root / "link"
        link.symlink_to(self.clones)
        worker = subprocess.Popen(["sleep", "30"], cwd=clone)
        self.addCleanup(worker.wait)
        self.addCleanup(worker.kill)

        result = self.prune_target(str(link), "greenfield-5-linked")

        self.assertTrue((clone / "target/artifact").exists())
        self.assertIn("a process works in it", result.stdout)

    def test_it_keeps_the_target_of_a_clone_a_process_works_in(self):
        clone = self.clone_with_target("greenfield-2-busy")
        worker = subprocess.Popen(["sleep", "30"], cwd=clone / "target")
        self.addCleanup(worker.wait)
        self.addCleanup(worker.kill)

        result = self.prune_target(str(self.clones), "greenfield-2-busy")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((clone / "target/artifact").exists())
        self.assertIn("skip greenfield-2-busy: a process works in it", result.stdout)

    def test_it_skips_a_clone_without_a_target(self):
        (self.clones / "greenfield-3-clean").mkdir()

        result = self.prune_target(str(self.clones), "greenfield-3-clean")

        self.assertIn("skip greenfield-3-clean: no target/", result.stdout)

    def test_it_refuses_a_clone_name_that_leaves_the_clones_root(self):
        outside = self.root / "outside"
        (outside / "target").mkdir(parents=True)

        result = self.prune_target(str(self.clones), "../outside")

        self.assertEqual(result.returncode, 2)
        self.assertTrue((outside / "target").exists())

    def test_it_fails_with_usage_when_the_clones_root_is_missing(self):
        result = self.prune_target(str(self.clones / "missing"), "greenfield-1-first")

        self.assertEqual(result.returncode, 2)
        self.assertIn("usage:", result.stderr)


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

    def test_it_reads_a_last_section_that_ends_without_a_trailing_newline(self):
        self.report.write_text(
            "## Changes for each linked issue\n\n### #11 First\n\n> One.\n\n### #12 Second\n\n> Two."
        )

        result = self.apply()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("#12: 4 characters", result.stdout)

    def test_it_fails_without_posting_when_an_issue_has_no_proposed_text(self):
        self.report.write_text(
            "## Changes for each linked issue\n\n### #11 First\n\n> One.\n\n### #12 Second"
        )

        result = self.apply("--go")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no proposed text for issues ['12']", result.stderr)
        self.assertEqual(self.recorded_calls(), [])

    def test_it_drops_the_report_only_sentence(self):
        result = self.apply()

        self.assertIn("Shared context.", result.stdout)
        self.assertNotIn("the owner applies it", result.stdout)

    def test_it_strips_the_blockquote_markers(self):
        result = self.apply()

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

    def session(self, name, state, doing="", question="", dialog="", root=None):
        workspace = f"{root or self.worker_root}/{name}"
        fields = f'doing="{doing}"'
        fields += f' needs-input="{question}"' if question else ""
        fields += f' dialog="{dialog}"' if dialog else ""
        return "\t".join(["s-" + name, "running", "-", state, workspace, fields])

    def watch(self, *session_lines, first_run=False, extra_arguments=(), timeout=30):
        self.sessions_file.write_text("\n".join(session_lines) + "\n")
        if first_run:
            self.state_file.unlink()
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
                "--idle-grace-seconds",
                "0",
                "--stream-lag-seconds",
                "0",
                *extra_arguments,
            ],
            env=self.environment,
            capture_output=True,
            text=True,
            timeout=timeout,
        )

    def pull_with_checks(self, head, *checks):
        return {
            "number": 7,
            "state": "OPEN",
            "headRefName": "greenfield/7-feature",
            "headRefOid": head,
            "statusCheckRollup": [
                {"name": name, "conclusion": conclusion, "detailsUrl": f"https://ci/{name}"}
                for name, conclusion in checks
            ],
        }

    def test_it_reports_a_workers_question_with_its_branch(self):
        result = self.watch(
            self.session("greenfield-5-thing", "waiting", "asking", question="Which option?")
        )

        self.assertIn(
            "QUESTION s-greenfield-5-thing (greenfield/5-thing): Which option?", result.stdout
        )

    def test_it_reports_a_workers_open_dialog_with_its_branch(self):
        result = self.watch(
            self.session(
                "greenfield-5-thing",
                "waiting",
                "coding",
                dialog="Codex is waiting for you in its terminal",
            )
        )

        self.assertIn(
            "DIALOG s-greenfield-5-thing (greenfield/5-thing): "
            "Codex is waiting for you in its terminal",
            result.stdout,
        )

    def test_it_reports_an_unchanged_dialog_once(self):
        waiting_with_dialog = self.session(
            "greenfield-5-thing", "waiting", "coding", dialog="Approve the command?"
        )
        still_waiting_with_dialog = self.session(
            "greenfield-5-thing", "waiting", "coding again", dialog="Approve the command?"
        )

        result = self.watch(waiting_with_dialog, still_waiting_with_dialog)

        self.assertEqual(result.stdout.count("DIALOG"), 1, result.stdout)

    def test_it_reports_a_new_dialog_after_the_previous_one_cleared(self):
        result = self.watch(
            self.session("greenfield-5-thing", "waiting", "coding", dialog="First prompt"),
            self.session("greenfield-5-thing", "working", "coding"),
            self.session("greenfield-5-thing", "waiting", "coding", dialog="Second prompt"),
        )

        self.assertIn(
            "DIALOG s-greenfield-5-thing (greenfield/5-thing): First prompt", result.stdout
        )
        self.assertIn(
            "DIALOG s-greenfield-5-thing (greenfield/5-thing): Second prompt", result.stdout
        )

    def test_it_reports_a_changed_dialog_text(self):
        result = self.watch(
            self.session("greenfield-5-thing", "waiting", "coding", dialog="First prompt"),
            self.session("greenfield-5-thing", "waiting", "coding", dialog="Second prompt"),
        )

        self.assertEqual(result.stdout.count("DIALOG"), 2, result.stdout)

    def test_it_reports_a_dialog_already_open_when_a_fresh_watcher_starts(self):
        result = self.watch(
            self.session("greenfield-5-thing", "waiting", "coding", dialog="Approve?"),
            first_run=True,
        )

        self.assertIn("DIALOG s-greenfield-5-thing", result.stdout)

    def test_it_reports_a_question_and_a_dialog_that_appear_together(self):
        result = self.watch(
            self.session(
                "greenfield-5-thing",
                "waiting",
                "asking",
                question="Which option?",
                dialog="Approve?",
            )
        )

        self.assertIn("QUESTION s-greenfield-5-thing", result.stdout)
        self.assertIn("DIALOG s-greenfield-5-thing", result.stdout)

    def test_it_ignores_a_dialog_of_a_session_outside_the_worker_root(self):
        result = self.watch(
            self.session("greenfield-5-thing", "waiting", dialog="Mine", root="/elsewhere")
        )

        self.assertNotIn("DIALOG", result.stdout)

    def test_it_does_not_call_a_worker_idle_while_its_dialog_is_open(self):
        result = self.watch(
            self.session("greenfield-6-idle", "working", "coding"),
            self.session("greenfield-6-idle", "waiting", "coding", dialog="Approve?"),
        )

        self.assertIn("DIALOG s-greenfield-6-idle", result.stdout)
        self.assertNotIn("IDLE", result.stdout)

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

    def test_it_reports_a_ci_failure_once_per_head_and_check(self):
        pull = self.pull_with_checks(self.HEAD, ("greenfield", "FAILURE"), ("docs", "SUCCESS"))
        self.pulls_file.write_text(json.dumps([pull]))

        result = self.watch(self.session("greenfield-7-feature", "working", "coding"))

        failures = [line for line in result.stdout.splitlines() if line.startswith("CI ")]
        self.assertEqual(len(failures), 1, result.stdout)
        self.assertIn(
            "PR #7 (greenfield/7-feature) head abcdef12: greenfield https://ci/greenfield",
            failures[0],
        )

    def test_it_skips_a_failure_of_the_macos_job(self):
        pull = self.pull_with_checks(self.HEAD, ("greenfield-macos", "FAILURE"))
        self.pulls_file.write_text(json.dumps([pull]))

        result = self.watch(self.session("greenfield-7-feature", "working", "coding"))

        self.assertNotIn("CI ", result.stdout)

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

    def open_a_pull_request_on_the_second_poll(self):
        pull = {
            "number": 9,
            "state": "OPEN",
            "headRefName": "greenfield/9-y",
            "headRefOid": "2" * 16,
        }
        self.shim(
            "gh",
            f"""
case "$*" in
*--search*)
    count=$(cat "$FAKE_PULLS.count" 2>/dev/null || echo 0)
    echo $((count + 1)) > "$FAKE_PULLS.count"
    case $count in 0) echo '[]' ;; *) echo '{json.dumps([pull])}' ;; esac ;;
esac
""",
        )

    def test_it_prints_nothing_for_a_doing_change_by_default(self):
        result = self.watch(
            self.session("greenfield-9-y", "working", "reading"),
            self.session("greenfield-9-y", "working", "editing"),
        )

        seen_sessions = json.loads(self.state_file.read_text())["sessions"]
        self.assertEqual(seen_sessions["s-greenfield-9-y"]["doing"], "editing")
        self.assertNotIn("SUMMARY", result.stdout)
        self.assertNotIn("editing", result.stdout)

    def test_it_prints_nothing_for_a_pr_opening_by_default(self):
        self.open_a_pull_request_on_the_second_poll()

        result = self.watch(self.session("greenfield-9-y", "working", "coding"))

        seen_pull_requests = json.loads(self.state_file.read_text())["prs"]
        self.assertEqual(seen_pull_requests["greenfield/9-y"]["number"], 9)
        self.assertNotIn("SUMMARY", result.stdout)
        self.assertNotIn("opened", result.stdout)

    def test_it_prints_a_summary_of_changes_when_summary_seconds_is_set(self):
        self.open_a_pull_request_on_the_second_poll()

        result = self.watch(
            self.session("greenfield-9-y", "working", "reading"),
            self.session("greenfield-9-y", "working", "editing"),
            extra_arguments=("--summary-seconds", "0.5"),
        )

        self.assertIn("SUMMARY (no action needed):", result.stdout)
        self.assertIn("working: editing", result.stdout)
        self.assertIn("PR #9 opened (greenfield/9-y)", result.stdout)

    def test_it_ignores_pull_requests_outside_the_branch_prefix(self):
        failing = self.pull_with_checks(self.HEAD, ("greenfield", "FAILURE"))
        stray = {**failing, "headRefName": "feature/other"}
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

    def test_it_reports_a_question_already_open_when_a_fresh_watcher_starts(self):
        result = self.watch(
            self.session("greenfield-5-thing", "waiting", "asking", question="Which option?"),
            first_run=True,
            extra_arguments=("--seed-seconds", "5"),
        )

        self.assertIn("QUESTION s-greenfield-5-thing", result.stdout)

    def test_it_reports_a_second_idle_spell_with_the_same_doing_text(self):
        result = self.watch(
            self.session("greenfield-6-idle", "working", "coding"),
            self.session("greenfield-6-idle", "waiting", "waiting on CI"),
            self.session("greenfield-6-idle", "working", "coding"),
            self.session("greenfield-6-idle", "waiting", "waiting on CI"),
            extra_arguments=("--working-seconds", "0"),
        )

        self.assertEqual(result.stdout.count("IDLE without merged PR"), 2, result.stdout)

    def test_it_reports_one_idle_line_when_a_short_working_blip_separates_two_idles(self):
        result = self.watch(
            self.session("greenfield-6-idle", "working", "coding"),
            self.session("greenfield-6-idle", "waiting", "waiting on CI"),
            self.session("greenfield-6-idle", "working", "waiting on CI"),
            self.session("greenfield-6-idle", "waiting", "waiting on CI"),
        )

        self.assertEqual(result.stdout.count("IDLE without merged PR"), 1, result.stdout)

    def test_it_reports_a_second_idle_after_a_working_spell_longer_than_the_threshold(self):
        working = self.session("greenfield-6-idle", "working", "waiting on CI")
        waiting = self.session("greenfield-6-idle", "waiting", "waiting on CI")
        self.shim(
            "torlek",
            f"printf '%s\\n' '{waiting}'; sleep 0.3; printf '%s\\n' '{working}'; sleep 1.2; "
            f"printf '%s\\n' '{waiting}'; sleep 1.5; exit 3",
        )

        result = self.watch(extra_arguments=("--working-seconds", "1"))

        self.assertEqual(result.stdout.count("IDLE without merged PR"), 2, result.stdout)

    def stream_sessions(self, *steps, hold_seconds=3):
        """Makes `torlek agent sessions --watch` print each line after its delay, then end."""
        commands = [f"sleep {delay}; printf '%s\\n' '{line}'" for delay, line in steps]
        self.shim("torlek", "; ".join(commands) + f"; sleep {hold_seconds}; exit 3")

    def test_it_stays_quiet_when_a_worker_works_again_within_the_idle_grace_period(self):
        self.stream_sessions(
            (0, self.session("greenfield-6-idle", "waiting", "waiting for /code-review")),
            (0.3, self.session("greenfield-6-idle", "working", "applying review fixes")),
        )

        result = self.watch(extra_arguments=("--idle-grace-seconds", "1"))

        self.assertNotIn("IDLE", result.stdout)

    def test_it_reports_a_worker_still_idle_after_the_idle_grace_period(self):
        self.stream_sessions((0, self.session("greenfield-6-idle", "waiting", "watching CI")))

        result = self.watch(extra_arguments=("--idle-grace-seconds", "0.5"))

        self.assertEqual(result.stdout.count("IDLE without merged PR"), 1, result.stdout)

    def test_it_does_not_report_an_idle_worker_before_the_idle_grace_period_ends(self):
        self.stream_sessions(
            (0, self.session("greenfield-6-idle", "waiting", "watching CI")), hold_seconds=1.5
        )

        result = self.watch(extra_arguments=("--idle-grace-seconds", "5"))

        self.assertNotIn("IDLE", result.stdout)

    def test_it_reports_the_idle_spell_that_follows_one_cut_short_by_the_grace_period(self):
        self.stream_sessions(
            (0, self.session("greenfield-6-idle", "waiting", "waiting for /code-review")),
            (0.2, self.session("greenfield-6-idle", "working", "applying review fixes")),
            (0.2, self.session("greenfield-6-idle", "waiting", "watching CI")),
        )

        result = self.watch(extra_arguments=("--idle-grace-seconds", "0.6"))

        self.assertEqual(result.stdout.count("IDLE without merged PR"), 1, result.stdout)
        self.assertIn("watching CI", result.stdout)

    def test_it_stays_quiet_when_an_idle_worker_asks_a_question_within_the_grace_period(self):
        self.stream_sessions(
            (0, self.session("greenfield-6-idle", "waiting", "coding")),
            (0.2, self.session("greenfield-6-idle", "waiting", "asking", question="Which?")),
        )

        result = self.watch(extra_arguments=("--idle-grace-seconds", "0.6"))

        self.assertIn("QUESTION s-greenfield-6-idle", result.stdout)
        self.assertNotIn("IDLE", result.stdout)

    def test_it_gives_up_on_a_hung_gh_and_says_the_merge_state_is_unknown(self):
        self.shim(
            "gh",
            """
case "$*" in
*--search*) echo '[]' ;;
*) exec sleep 30 ;;
esac
""",
        )

        result = self.watch(
            self.session("greenfield-6-idle", "working", "coding"),
            self.session("greenfield-6-idle", "waiting", "coding"),
            extra_arguments=("--gh-timeout-seconds", "0.5"),
        )

        self.assertIn("IDLE, merge state unknown", result.stdout)

    def test_it_reports_a_failing_pr_poll_once_not_on_every_poll(self):
        self.shim(
            "gh",
            """
case "$*" in
*--search*) exit 1 ;;
esac
""",
        )

        result = self.watch(self.session("greenfield-7-feature", "working", "coding"))

        self.assertEqual(result.stdout.count("PR POLL FAILED"), 1, result.stdout)

    def test_it_ends_the_watch_and_kills_the_session_listing_when_a_helper_thread_dies(self):
        self.shim(
            "torlek",
            'echo $$ > "$FAKE_CALLS.pid"; cat "$FAKE_SESSIONS"; exec sleep 30',
        )
        self.shim(
            "gh",
            """
case "$*" in
*--search*) echo '[{"number": 1}]' ;;
esac
""",
        )

        result = self.watch(self.session("greenfield-7-feature", "working", "coding"))

        self.assertEqual(result.returncode, 1)
        self.assertIn("WATCH ENDED: poll_prs crashed", result.stdout)
        listing_pid = int(Path(f"{self.calls}.pid").read_text())
        for _ in range(20):
            try:
                os.kill(listing_pid, 0)
            except ProcessLookupError:
                return
            time.sleep(0.1)
        self.fail("the session listing is still running")

    def test_it_exits_nonzero_and_says_why_when_the_session_watch_ends(self):
        result = self.watch(self.session("greenfield-8-gone", "working", "coding"))

        self.assertEqual(result.returncode, 3)
        self.assertIn(
            "WATCH ENDED: torlek agent sessions --watch exited 3: no grant", result.stdout
        )

    def test_it_reports_a_question_from_a_worker_that_was_already_waiting(self):
        result = self.watch(
            self.session("greenfield-9-ask", "waiting", "waiting on CI"),
            self.session("greenfield-9-ask", "waiting", "waiting on CI", question="Rebase?"),
        )

        self.assertIn("QUESTION s-greenfield-9-ask (greenfield/9-ask): Rebase?", result.stdout)

    def fake_session_lists(self, watched, listed):
        """The watch prints `watched` then ends; the full list prints `listed`."""
        self.shim(
            "torlek",
            f"""
case "$*" in
"agent sessions --watch") printf '%s\\n' '{watched}'; sleep 2; exit 3 ;;
"agent sessions") printf '%s\\n' '{listed}' ;;
esac
""",
        )

    def test_it_reports_a_question_the_session_watch_never_delivered(self):
        self.fake_session_lists(
            self.session("greenfield-9-ask", "working", "coding"),
            self.session("greenfield-9-ask", "waiting", "coding", question="Rebase?"),
        )

        result = self.watch(extra_arguments=("--full-list-seconds", "0.5"))

        self.assertIn(
            "QUESTION s-greenfield-9-ask (greenfield/9-ask), missed by the session watch: Rebase?",
            result.stdout,
        )
        self.assertEqual(result.stdout.count("QUESTION"), 1, result.stdout)

    def test_it_does_not_repeat_a_question_the_watch_already_reported(self):
        asking = self.session("greenfield-9-ask", "waiting", "coding", question="Rebase?")
        self.fake_session_lists(asking, asking)

        result = self.watch(extra_arguments=("--full-list-seconds", "0.5"))

        self.assertEqual(result.stdout.count("QUESTION"), 1, result.stdout)

    def test_it_waits_for_the_watch_to_deliver_a_question_the_full_list_shows_first(self):
        working = self.session("greenfield-9-ask", "working", "coding")
        asking = self.session("greenfield-9-ask", "waiting", "coding", question="Rebase?")
        self.shim(
            "torlek",
            f"""
case "$*" in
"agent sessions --watch") printf '%s\\n' '{working}'; sleep 0.8
    printf '%s\\n' '{asking}'; sleep 1.5; exit 3 ;;
"agent sessions") printf '%s\\n' '{asking}' ;;
esac
""",
        )

        result = self.watch(
            extra_arguments=("--full-list-seconds", "0.5", "--stream-lag-seconds", "1")
        )

        self.assertIn("QUESTION s-greenfield-9-ask (greenfield/9-ask): Rebase?", result.stdout)
        self.assertNotIn("missed", result.stdout)

    def test_it_does_not_repeat_a_question_a_watcher_reported_before_the_full_list_check(self):
        asking = self.session("greenfield-9-ask", "waiting", "coding", question="Rebase?")
        self.state_file.write_text(
            json.dumps(
                {
                    "sessions": {
                        "s-greenfield-9-ask": {
                            "id": "s-greenfield-9-ask",
                            "lifecycle": "running",
                            "state": "waiting",
                            "branch": "greenfield/9-ask",
                            "doing": "coding",
                            "question": "Rebase?",
                        }
                    }
                }
            )
        )
        self.fake_session_lists(asking, asking)

        result = self.watch(extra_arguments=("--full-list-seconds", "0.5"))

        self.assertNotIn("QUESTION", result.stdout)

    def test_it_reports_a_failing_full_list_check_once_not_on_every_check(self):
        self.shim(
            "torlek",
            """
case "$*" in
"agent sessions --watch") sleep 2; exit 3 ;;
*) echo "daemon unreachable" >&2; exit 126 ;;
esac
""",
        )

        result = self.watch(extra_arguments=("--full-list-seconds", "0.3"))

        self.assertEqual(result.stdout.count("SESSIONS CHECK FAILED"), 1, result.stdout)


class WatcherStateTests(FakeCommandsTestCase):
    def test_it_rewrites_the_state_file_only_when_the_state_changed(self):
        spec = importlib.util.spec_from_file_location("coordinator_watch", WATCHER)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        state_file = self.root / "state.json"
        watcher = module.Watcher(
            argparse.Namespace(
                repo="example/shop",
                worker_root=self.root,
                branch_prefix="greenfield/",
                since="2026-01-01",
                seed_seconds=0,
                poll_seconds=1,
                working_seconds=120,
                idle_grace_seconds=0,
                summary_seconds=1,
                full_list_seconds=1,
                stream_lag_seconds=0,
                state_file=state_file,
            )
        )
        watcher.save()
        written_inode = state_file.stat().st_ino

        watcher.save()

        self.assertEqual(state_file.stat().st_ino, written_inode)


class WatcherDefaultsTests(unittest.TestCase):
    def test_the_summary_is_off_unless_summary_seconds_is_given(self):
        spec = importlib.util.spec_from_file_location("coordinator_watch", WATCHER)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        base = ["state.json", "--repo", "example/shop", "--worker-root", "workers"]

        default = module.parse_arguments(base)
        opted_in = module.parse_arguments([*base, "--summary-seconds", "600"])

        self.assertEqual(default.summary_seconds, 0)
        self.assertEqual(opted_in.summary_seconds, 600)


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
        for script in (WATCHER, PRUNE_TARGET, APPLIER, START_WORKER):
            self.assertTrue(os.access(script, os.X_OK), script)


if __name__ == "__main__":
    unittest.main()
