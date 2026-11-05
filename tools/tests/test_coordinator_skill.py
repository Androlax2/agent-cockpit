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
        for script in (WATCHER, END_PRUNE, APPLIER):
            self.assertTrue(os.access(script, os.X_OK), script)


if __name__ == "__main__":
    unittest.main()
