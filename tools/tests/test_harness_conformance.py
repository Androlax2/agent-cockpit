"""The nightly harness conformance tool: fake npm and cargo, a fake GitHub, a bare Git remote.

Nothing here reaches npm, GitHub or a provider account (#732).
"""

import contextlib
import copy
import importlib
import io
import json
import os
import re
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
sys.path.insert(0, str(ROOT / "tools" / "tests"))
conformance = importlib.import_module("harness_conformance")
release_pr = conformance.release_pr
from test_release_pr import git, ssh_fixture_environment  # noqa: E402

WORKFLOW = (ROOT / ".github/workflows/harness-conformance.yml").read_text()
BOT_USER = {"login": release_pr.BOT, "type": "Bot", "id": release_pr.BOT_ID}
ACTIONS_USER = {"login": "github-actions[bot]", "type": "Bot", "id": 41898282}


class Repository:
    """A checkout of three adapters' tested lists, with a bare remote as origin."""

    def __init__(self, parent):
        self.root = parent / "checkout"
        self.remote = parent / "remote.git"
        git(parent, "init", "--bare", "-q", str(self.remote))
        self.root.mkdir()
        git(self.root, "init", "-q", "-b", "main")
        git(self.root, "config", "user.name", "Fixture")
        git(self.root, "config", "user.email", "fixture@example.invalid")
        git(self.root, "config", "commit.gpgSign", "false")
        git(self.root, "remote", "add", "origin", str(self.remote))
        for adapter, version in (
            ("claude", "2.1.316"),
            ("codex", "0.166.0"),
            ("opencode", "1.19.5"),
        ):
            path = self.root / conformance.tested_versions_path(adapter)
            path.parent.mkdir(parents=True)
            path.write_text(f"# Tested.\n{version}\n")
        git(self.root, "add", ".")
        git(self.root, "commit", "-q", "-m", "fixture")
        git(self.root, "push", "-q", "origin", "main")
        self.base = git(self.root, "rev-parse", "HEAD")

    def listed(self, adapter, ref):
        text = git(self.remote, "show", f"{ref}:{conformance.tested_versions_path(adapter)}")
        return conformance.listed_versions(text)


class FakeGitHub:
    def __init__(self, issues=(), pulls=()):
        self.issues = [copy.deepcopy(issue) for issue in issues]
        self.pulls = [copy.deepcopy(pull) for pull in pulls]
        self.writes = []

    def call(self, method, path, data=None, paginate=False):
        path = path.removeprefix(f"repos/{release_pr.REPOSITORY}/")
        if method == "GET" and path.startswith("issues?"):
            return [copy.deepcopy(self.issues)]
        if method == "GET" and path.startswith("pulls?"):
            return [copy.deepcopy(self.pulls)]
        self.writes.append((method, path, data))
        if method == "POST" and path == "pulls":
            return {"number": 40 + len(self.pulls)}
        return None

    def written(self, method, path):
        return [data for (m, p, data) in self.writes if (m, p) == (method, path)]


def issue(number, adapter, text, user=ACTIONS_USER):
    return {
        "number": number,
        "user": user,
        "body": f"{conformance.issue_marker(adapter)}\n{text}",
    }


def pull(adapter, version, user=BOT_USER):
    return {
        "number": 7,
        "user": user,
        "head": {"ref": f"{conformance.BRANCH_PREFIX}{adapter}-{version}"},
    }


def result(adapter, version, passed=True, stage=None):
    found = {"adapter": adapter, "version": version, "passed": passed}
    if stage:
        found["stage"] = stage
    return found


class Harness(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="harness-conformance-")
        self.addCleanup(directory.cleanup)
        self.parent = Path(directory.name)
        self.repository = Repository(self.parent)

    def run_tool(self, arguments, github=None, environment=None):
        """(exit code, standard output, standard error) of the tool's main()."""
        github = github or FakeGitHub()
        out, err = io.StringIO(), io.StringIO()
        with (
            patch.dict(os.environ, environment or {}),
            patch.object(release_pr, "api", github.call),
            patch.object(release_pr, "ROOT", self.repository.root),
            contextlib.redirect_stdout(out),
            contextlib.redirect_stderr(err),
        ):
            code = conformance.main(arguments)
        return code, out.getvalue(), err.getvalue()

    def write_results(self, *results):
        path = self.parent / "results.json"
        path.write_text(json.dumps(list(results)))
        return str(path)


class DryRunTests(Harness):
    def test_a_new_pass_is_reported_as_a_pull_request_that_would_open(self):
        results = self.write_results(result("claude", "2.1.400"))

        code, out, _ = self.run_tool(
            ["propose", "--base", self.repository.base, "--results", results, "--dry-run"]
        )

        self.assertEqual(code, 0)
        self.assertIn(
            "would open pull request harness-conformance/claude-2.1.400 adding 2.1.400 to "
            "crates/cockpit-claude/tested-versions.txt",
            out,
        )

    def test_a_failure_is_reported_as_an_issue_that_would_open(self):
        results = self.write_results(result("codex", "0.170.0", False, "conformance"))

        code, out, _ = self.run_tool(["report", "--results", results, "--dry-run"])

        self.assertEqual(code, 0)
        self.assertIn("would open issue: codex 0.170.0 fails at conformance", out)

    def test_a_dry_run_says_what_a_real_run_would_do_with_an_open_issue(self):
        body = conformance.failure_body("codex", "0.170.0", "conformance")
        github = FakeGitHub(
            issues=[
                {"number": 5, "user": ACTIONS_USER, "body": body},
                issue(6, "claude", "2.1.400 fails."),
            ]
        )
        results = self.write_results(
            result("codex", "0.170.0", False, "conformance"), result("claude", "2.1.401")
        )

        _, out, _ = self.run_tool(["report", "--results", results, "--dry-run"], github)

        self.assertIn("codex 0.170.0 is already reported in #5", out)
        self.assertIn("dry run: would close #6", out)
        self.assertEqual(github.writes, [])

    def test_a_dry_run_writes_nothing(self):
        github = FakeGitHub()
        results = self.write_results(
            result("claude", "2.1.400"), result("codex", "0.170.0", False, "install")
        )

        self.run_tool(["report", "--results", results, "--dry-run"], github)
        self.run_tool(
            ["propose", "--base", self.repository.base, "--results", results, "--dry-run"], github
        )

        self.assertEqual(github.writes, [])
        self.assertEqual(self.repository.listed("claude", "main"), ["2.1.316"])


class ProposeTests(Harness):
    def propose(self, results, github, **environment):
        return self.run_tool(
            ["propose", "--base", self.repository.base, "--results", results],
            github,
            {**ssh_fixture_environment(self.repository, self.parent), **environment},
        )

    def test_a_new_pass_pushes_a_branch_that_adds_one_line_and_opens_a_pull_request(self):
        github = FakeGitHub()

        code, _, err = self.propose(self.write_results(result("claude", "2.1.400")), github)

        self.assertEqual((code, err), (0, ""))
        branch = "harness-conformance/claude-2.1.400"
        self.assertEqual(self.repository.listed("claude", branch), ["2.1.316", "2.1.400"])
        changed = git(self.repository.remote, "diff", "--numstat", "main", branch)
        self.assertEqual(changed.split(), ["1", "0", "crates/cockpit-claude/tested-versions.txt"])
        [created] = github.written("POST", "pulls")
        self.assertEqual((created["head"], created["base"]), (branch, "main"))
        self.assertNotIn("draft", created)
        self.assertIn("2.1.400", created["title"])
        self.assertEqual(self.repository.listed("claude", "main"), ["2.1.316"])

    def test_the_commit_is_the_proposal_bots_and_the_branch_starts_at_the_base(self):
        self.propose(self.write_results(result("codex", "0.170.0")), FakeGitHub())

        branch = "harness-conformance/codex-0.170.0"
        self.assertEqual(
            git(self.repository.remote, "rev-parse", f"{branch}^"), self.repository.base
        )
        self.assertEqual(
            git(self.repository.remote, "log", "-1", "--format=%an", branch), release_pr.BOT
        )

    def test_two_adapters_open_two_pull_requests_each_from_the_base(self):
        github = FakeGitHub()

        self.propose(
            self.write_results(result("claude", "2.1.400"), result("opencode", "1.20.0")), github
        )

        self.assertEqual(len(github.written("POST", "pulls")), 2)
        for adapter, version in (("claude", "2.1.400"), ("opencode", "1.20.0")):
            branch = f"harness-conformance/{adapter}-{version}"
            self.assertEqual(
                git(self.repository.remote, "rev-parse", f"{branch}^"), self.repository.base
            )

    def test_a_version_already_tested_opens_nothing(self):
        github = FakeGitHub()

        _, out, _ = self.propose(self.write_results(result("claude", "2.1.316")), github)

        self.assertIn("already tested", out)
        self.assertEqual(github.writes, [])

    def test_a_failure_opens_no_pull_request(self):
        github = FakeGitHub()

        self.propose(self.write_results(result("claude", "2.1.400", False, "conformance")), github)

        self.assertEqual(github.writes, [])

    def test_an_open_pull_request_for_the_adapter_is_not_followed_by_another(self):
        github = FakeGitHub(pulls=[pull("claude", "2.1.399")])

        _, out, _ = self.propose(self.write_results(result("claude", "2.1.400")), github)

        self.assertIn("already has an open conformance pull request", out)
        self.assertEqual(github.writes, [])

    def test_a_pull_request_that_is_not_the_bots_does_not_block_the_adapter(self):
        stranger = {"login": "someone", "type": "User", "id": 1}
        github = FakeGitHub(pulls=[pull("claude", "2.1.399", user=stranger)])

        self.propose(self.write_results(result("claude", "2.1.400")), github)

        self.assertEqual(len(github.written("POST", "pulls")), 1)

    def test_an_existing_branch_is_never_overwritten(self):
        github = FakeGitHub()
        results = self.write_results(result("claude", "2.1.400"))
        self.propose(results, github)
        before = git(self.repository.remote, "rev-parse", "harness-conformance/claude-2.1.400")
        github.pulls = []

        _, out, _ = self.propose(results, github)

        self.assertIn("already exists", out)
        self.assertEqual(
            git(self.repository.remote, "rev-parse", "harness-conformance/claude-2.1.400"), before
        )
        self.assertEqual(len(github.written("POST", "pulls")), 1)

    def test_a_dirty_checkout_is_refused(self):
        (self.repository.root / "stray.txt").write_text("x")

        code, _, err = self.propose(self.write_results(result("claude", "2.1.400")), FakeGitHub())

        self.assertEqual(code, 1)
        self.assertIn("clean checkout", err)

    def test_a_checkout_that_is_not_at_the_base_is_refused(self):
        code, _, err = self.run_tool(
            [
                "propose",
                "--base",
                "0" * 40,
                "--results",
                self.write_results(result("claude", "2.1.400")),
            ]
        )

        self.assertEqual(code, 1)
        self.assertIn("clean checkout at the base", err)

    def test_the_deploy_key_is_required(self):
        code, _, err = self.run_tool(
            [
                "propose",
                "--base",
                self.repository.base,
                "--results",
                self.write_results(result("claude", "2.1.400")),
            ],
            environment={"PROPOSAL_GIT_SSH_KEY": ""},
        )

        self.assertEqual(code, 1)
        self.assertIn("PROPOSAL_GIT_SSH_KEY", err)


class ReportTests(Harness):
    def test_a_failure_opens_one_issue_naming_the_version_and_stage(self):
        github = FakeGitHub()

        self.run_tool(
            [
                "report",
                "--results",
                self.write_results(result("codex", "0.170.0", False, "conformance")),
            ],
            github,
        )

        [created] = github.written("POST", "issues")
        self.assertIn("codex 0.170.0 fails at conformance", created["title"])
        self.assertIn(conformance.issue_marker("codex"), created["body"])
        self.assertIn("`0.170.0` fails at the conformance stage", created["body"])

    def test_the_same_failure_the_next_night_changes_nothing(self):
        body = conformance.failure_body("codex", "0.170.0", "conformance")
        github = FakeGitHub(issues=[{"number": 5, "user": ACTIONS_USER, "body": body}])

        _, out, _ = self.run_tool(
            [
                "report",
                "--results",
                self.write_results(result("codex", "0.170.0", False, "conformance")),
            ],
            github,
        )

        self.assertIn("already reported in #5", out)
        self.assertEqual(github.writes, [])

    def test_an_issue_edited_to_windows_line_endings_is_still_the_same_failure(self):
        body = conformance.failure_body("codex", "0.170.0", "conformance").replace("\n", "\r\n")
        github = FakeGitHub(issues=[{"number": 5, "user": ACTIONS_USER, "body": body}])

        self.run_tool(
            [
                "report",
                "--results",
                self.write_results(result("codex", "0.170.0", False, "conformance")),
            ],
            github,
        )

        self.assertEqual(github.writes, [])

    def test_a_newer_failing_version_updates_the_open_issue_and_comments(self):
        body = conformance.failure_body("codex", "0.170.0", "conformance")
        github = FakeGitHub(issues=[{"number": 5, "user": ACTIONS_USER, "body": body}])

        self.run_tool(
            [
                "report",
                "--results",
                self.write_results(result("codex", "0.171.0", False, "conformance")),
            ],
            github,
        )

        [updated] = github.written("PATCH", "issues/5")
        self.assertIn("`0.171.0` fails", updated["body"])
        self.assertEqual(len(github.written("POST", "issues/5/comments")), 1)
        self.assertEqual(github.written("POST", "issues"), [])

    def test_a_pass_closes_the_open_issue_of_that_adapter_only(self):
        github = FakeGitHub(
            issues=[
                issue(5, "codex", "0.170.0 fails."),
                issue(6, "claude", "2.1.400 fails."),
            ]
        )

        self.run_tool(
            ["report", "--results", self.write_results(result("codex", "0.171.0"))], github
        )

        self.assertEqual(
            github.written("PATCH", "issues/5"), [{"state": "closed", "state_reason": "completed"}]
        )
        self.assertEqual(github.written("PATCH", "issues/6"), [])

    def test_an_issue_somebody_else_wrote_is_never_updated_or_closed(self):
        stranger = {"login": "someone", "type": "User", "id": 1}
        github = FakeGitHub(issues=[issue(5, "codex", "pretend.", user=stranger)])

        self.run_tool(
            ["report", "--results", self.write_results(result("codex", "0.171.0"))], github
        )
        self.run_tool(
            [
                "report",
                "--results",
                self.write_results(result("codex", "0.172.0", False, "install")),
            ],
            github,
        )

        self.assertEqual(github.written("PATCH", "issues/5"), [])
        self.assertEqual(len(github.written("POST", "issues")), 1)

    def test_a_version_npm_never_answered_is_reported_as_latest(self):
        github = FakeGitHub()

        self.run_tool(
            ["report", "--results", self.write_results(result("opencode", None, False, "install"))],
            github,
        )

        [created] = github.written("POST", "issues")
        self.assertIn("opencode latest fails at install", created["title"])


class ResultsTests(Harness):
    def refusal(self, *results):
        return self.run_tool(["untested", "--results", self.write_results(*results)])[2]

    def test_an_unknown_adapter_is_refused(self):
        self.assertIn("unknown adapter", self.refusal(result("gemini", "1.0.0")))

    def test_a_version_that_is_not_a_version_is_refused(self):
        self.assertIn("not a version", self.refusal(result("claude", "1.0.0; rm -rf /")))

    def test_a_stage_that_is_not_one_of_ours_is_refused(self):
        self.assertIn("not a stage", self.refusal(result("claude", "2.1.400", False, "[x](y)")))

    def test_a_pass_without_a_version_is_refused(self):
        self.assertIn("needs its version", self.refusal(result("claude", None)))

    def test_untested_is_true_only_for_a_passing_version_off_the_list(self):
        for results, expected in (
            ([result("claude", "2.1.400")], "true"),
            ([result("claude", "2.1.316")], "false"),
            ([result("claude", "2.1.400", False, "conformance")], "false"),
            ([result("claude", "2.1.316"), result("codex", "0.170.0")], "true"),
        ):
            with self.subTest(results=results):
                code, out, _ = self.run_tool(
                    ["untested", "--results", self.write_results(*results)]
                )
                self.assertEqual((code, out.strip()), (0, expected))


class CheckTests(Harness):
    """`check` with an npm and a cargo that only record what they were asked."""

    def setUp(self):
        super().setUp()
        self.tools = self.parent / "fake tools"
        self.tools.mkdir()
        self.record = self.parent / "cargo-record.json"
        self.write_tool(
            "npm",
            "import os, pathlib, sys\n"
            "args = sys.argv[1:]\n"
            "if os.environ.get('FAKE_NPM_FAIL') == args[0]:\n"
            "    print('npm ERR! fixture'); sys.exit(1)\n"
            "if args[0] == 'view':\n"
            "    print(os.environ['FAKE_LATEST']); sys.exit(0)\n"
            "prefix = pathlib.Path(args[args.index('--prefix') + 1])\n"
            "(prefix / 'node_modules/.bin').mkdir(parents=True, exist_ok=True)\n"
            "program = prefix / 'node_modules/.bin' / os.environ['FAKE_EXECUTABLE']\n"
            "program.write_text('#!/bin/sh\\necho \"' + os.environ['FAKE_PRINTS'] + '\"\\n')\n"
            "program.chmod(0o755)\n",
        )
        self.write_tool(
            "cargo",
            "import json, os, pathlib, sys\n"
            "pathlib.Path(os.environ['FAKE_CARGO_RECORD']).write_text(json.dumps({\n"
            "    'arguments': sys.argv[1:],\n"
            "    'program': {k: v for k, v in os.environ.items() if k.endswith('_PROGRAM')},\n"
            "}))\n"
            "print('fixture test output')\n"
            "sys.exit(int(os.environ.get('FAKE_CARGO_EXIT', '0')))\n",
        )

    def write_tool(self, name, body):
        path = self.tools / name
        path.write_text(f"#!{sys.executable}\n{body}")
        path.chmod(0o755)

    def check(self, adapter, *, latest, prints=None, executable=None, **environment):
        results = self.parent / "check-results.json"
        results.unlink(missing_ok=True)
        fake = {
            "PATH": f"{self.tools}{os.pathsep}{os.environ['PATH']}",
            "FAKE_LATEST": latest,
            "FAKE_PRINTS": prints or f"{latest} (Fixture Code)",
            "FAKE_EXECUTABLE": executable or conformance.ADAPTERS[adapter].executable,
            "FAKE_CARGO_RECORD": str(self.record),
            **environment,
        }
        with patch.dict(os.environ, fake), contextlib.redirect_stdout(io.StringIO()):
            conformance.check(adapter, self.parent / "harness", results)
        return json.loads(results.read_text())

    def test_a_harness_that_passes_is_recorded_with_the_version_npm_installed(self):
        self.assertEqual(
            self.check("claude", latest="2.1.400"),
            [{"adapter": "claude", "version": "2.1.400", "passed": True}],
        )

    def test_the_live_tests_run_for_that_adapter_against_the_installed_program(self):
        self.check("codex", latest="0.170.0")

        recorded = json.loads(self.record.read_text())
        self.assertEqual(
            recorded["arguments"],
            [
                "nextest",
                "run",
                "--locked",
                "-p",
                "cockpit-codex",
                "--test",
                "conformance",
                "--run-ignored",
                "only",
            ],
        )
        program = Path(recorded["program"]["COCKPIT_CODEX_PROGRAM"])
        self.assertEqual(program, self.parent / "harness/node_modules/.bin/codex")

    def test_failing_live_tests_are_recorded_at_the_conformance_stage(self):
        self.assertEqual(
            self.check("opencode", latest="1.20.0", FAKE_CARGO_EXIT="100"),
            [{"adapter": "opencode", "version": "1.20.0", "passed": False, "stage": "conformance"}],
        )

    def test_a_build_failure_is_recorded_at_the_cargo_stage_not_blamed_on_the_harness(self):
        self.assertEqual(
            self.check("claude", latest="2.1.400", FAKE_CARGO_EXIT="101"),
            [{"adapter": "claude", "version": "2.1.400", "passed": False, "stage": "cargo"}],
        )

    def test_a_printed_version_that_only_contains_the_installed_one_fails(self):
        for installed, printed in (
            ("2.1.31", "2.1.316 (Fixture)"),
            ("1.2.0", "11.2.0"),
            ("2.1.4", "v2.1.4-1"),
        ):
            with self.subTest(installed=installed, printed=printed):
                found = self.check("claude", latest=installed, prints=printed)
                self.assertEqual(found[0]["stage"], "version")

    def test_a_printed_version_inside_other_words_passes(self):
        for printed in ("2.1.400 (Fixture Code)", "claude v2.1.400", "Fixture 2.1.400."):
            with self.subTest(printed=printed):
                self.assertTrue(self.check("claude", latest="2.1.400", prints=printed)[0]["passed"])

    def test_a_command_over_its_timeout_is_killed_with_what_it_started(self):
        marker = self.parent / "grandchild-ran"
        started = time.monotonic()

        code, reason = conformance.command(
            ["sh", "-c", f"(sleep 2; touch {marker}) & wait"], timeout=1
        )
        time.sleep(2.5)

        self.assertEqual(code, 124)
        self.assertIn("did not end within 1 seconds", reason)
        self.assertLess(time.monotonic() - started, 4)
        self.assertFalse(marker.exists())

    def test_a_version_that_npm_cannot_install_is_recorded_at_the_install_stage(self):
        self.assertEqual(
            self.check("claude", latest="2.1.400", FAKE_NPM_FAIL="install"),
            [{"adapter": "claude", "version": "2.1.400", "passed": False, "stage": "install"}],
        )
        self.assertFalse(self.record.exists())

    def test_a_registry_that_does_not_answer_is_recorded_without_a_version(self):
        self.assertEqual(
            self.check("claude", latest="2.1.400", FAKE_NPM_FAIL="view"),
            [{"adapter": "claude", "version": None, "passed": False, "stage": "install"}],
        )

    def test_a_latest_that_is_not_a_version_is_not_installed(self):
        found = self.check("claude", latest="latest; rm -rf /")

        self.assertEqual((found[0]["version"], found[0]["stage"]), (None, "install"))

    def test_a_program_that_prints_another_version_fails_before_the_tests_run(self):
        self.assertEqual(
            self.check("claude", latest="2.1.400", prints="2.1.399 (Fixture Code)"),
            [{"adapter": "claude", "version": "2.1.400", "passed": False, "stage": "version"}],
        )
        self.assertFalse(self.record.exists())

    def test_each_adapter_adds_its_own_result_to_the_same_file(self):
        results = self.parent / "check-results.json"
        results.unlink(missing_ok=True)
        for adapter, latest in (("claude", "2.1.400"), ("codex", "0.170.0")):
            fake = {
                "PATH": f"{self.tools}{os.pathsep}{os.environ['PATH']}",
                "FAKE_LATEST": latest,
                "FAKE_PRINTS": latest,
                "FAKE_EXECUTABLE": conformance.ADAPTERS[adapter].executable,
                "FAKE_CARGO_RECORD": str(self.record),
            }
            with patch.dict(os.environ, fake), contextlib.redirect_stdout(io.StringIO()):
                conformance.check(adapter, self.parent / f"harness-{adapter}", results)

        self.assertEqual(
            [found["adapter"] for found in json.loads(results.read_text())], ["claude", "codex"]
        )


class WorkflowTests(unittest.TestCase):
    """The workflow's authority: what each job may read, write and use."""

    @staticmethod
    def job(name):
        jobs = WORKFLOW.split("\njobs:\n", 1)[1]
        match = re.search(rf"(?ms)^  {name}:\n(.*?)(?=^  [\w-]+:\n|\Z)", jobs)
        return match[1]

    def test_the_workflow_reads_by_default_and_only_the_report_job_writes(self):
        top = WORKFLOW.split("\njobs:\n", 1)[0]
        self.assertRegex(top, r"(?m)^permissions:\n  contents: read\n\n")
        self.assertEqual(re.findall(r"(?m)^ {6}[\w-]+: write$", WORKFLOW), ["      issues: write"])
        self.assertIn("issues: write", self.job("report"))

    def test_only_the_propose_job_uses_secrets_and_the_proposal_environment(self):
        self.assertEqual(WORKFLOW.count("secrets."), 2)
        for name in ("check", "conformance", "report"):
            with self.subTest(job=name):
                body = self.job(name)
                self.assertNotIn("secrets.", body)
                self.assertNotIn("environment:", body)
        propose = self.job("propose")
        self.assertRegex(propose, r"(?m)^    environment: release-proposal$")
        self.assertIn("permission-contents: read", propose)
        self.assertIn("permission-pull-requests: write", propose)
        self.assertNotIn("permission-contents: write", propose)
        self.assertIn("repositories: agent-cockpit", propose)

    def test_the_job_that_installs_from_npm_holds_neither_secret_nor_write_permission(self):
        conformance_job = self.job("conformance")
        self.assertNotIn("permissions:", conformance_job)
        self.assertNotIn("GH_TOKEN", conformance_job)

    def test_the_app_token_and_deploy_key_only_exist_outside_a_dry_run(self):
        propose = self.job("propose")
        for step in propose.split("      - ")[1:]:
            if "secrets." in step:
                with self.subTest(step=step.splitlines()[0]):
                    self.assertIn("if: github.event_name == 'schedule' || !inputs.dry_run", step)

    def test_nothing_publishes_on_a_pull_request(self):
        for name in ("conformance", "report", "propose"):
            with self.subTest(job=name):
                self.assertIn("github.event_name != 'pull_request'", self.job(name))

    def test_a_manual_run_is_a_dry_run_unless_it_says_otherwise(self):
        self.assertRegex(WORKFLOW, r"(?s)dry_run:.*?type: boolean\n\s+default: true")

    def test_the_proposal_job_proposes_only_when_a_passing_version_is_new(self):
        self.assertIn("needs.conformance.outputs.untested == 'true'", self.job("propose"))


if __name__ == "__main__":
    unittest.main()
