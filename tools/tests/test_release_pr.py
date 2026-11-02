"""History-preserving proposal tests: real Git/bare remotes, fake GitHub and generators."""

import contextlib
import copy
import importlib.util
import io
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("release_pr", ROOT / "tools/release_pr.py")
proposal = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(proposal)


def git(root, *args):
    return subprocess.run(
        ["git", *args], cwd=root, check=True, capture_output=True, text=True
    ).stdout.strip()


class Fixture:
    def __init__(self, parent):
        self.root = parent / "repository with spaces"
        self.root.mkdir()
        self.remote = parent / "remote.git"
        git(parent, "init", "--bare", "-q", str(self.remote))
        git(self.root, "init", "-q", "-b", "main")
        git(self.root, "config", "user.name", "Fixture")
        git(self.root, "config", "user.email", "fixture@example.invalid")
        git(self.root, "config", "commit.gpgSign", "false")
        git(self.root, "remote", "add", "origin", str(self.remote))
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["crates/one"]\nresolver = "2"\n\n[workspace.package]\nversion = "0.1.0"\npublish = false\n'
        )
        (self.root / "crates/one/src").mkdir(parents=True)
        (self.root / "crates/one/Cargo.toml").write_text(
            '[package]\nname = "one"\nversion.workspace = true\npublish.workspace = true\nedition = "2021"\n'
        )
        (self.root / "crates/one/src/lib.rs").write_text("pub fn value() -> u32 { 1 }\n")
        (self.root / "Cargo.lock").write_text(
            'version = 4\n\n[[package]]\nname = "one"\nversion = "0.1.0"\n'
        )
        for name in ("release-plz.toml", "cliff.toml"):
            shutil.copyfile(ROOT / name, self.root / name)
        self.base = self.commit("feat: initial fixture")
        git(self.root, "push", "-q", "origin", "main")
        git(self.root, "checkout", "-q", "--detach", self.base)
        self.api = FakeGitHub(self)

    def commit(self, subject):
        git(self.root, "add", ".")
        git(self.root, "commit", "-qm", subject)
        return git(self.root, "rev-parse", "HEAD")

    def next_base(self):
        git(self.root, "checkout", "-q", "main")
        source = self.root / "crates/one/src/lib.rs"
        source.write_text(source.read_text() + "// next fix\n")
        self.base = self.commit("fix: fixture value")
        git(self.root, "push", "-q", "origin", "main")
        git(self.root, "checkout", "-q", "--detach", self.base)


class FakeGitHub:
    def __init__(self, fixture):
        self.fixture = fixture
        self.pulls = {}
        self.comments = {}
        self.writes = []
        self.fail_create = False
        self.fail_close = False
        self.created_not_draft = False

    def call(self, method, path, data=None, paginate=False):
        prefix = f"repos/{proposal.REPOSITORY}/"
        assert path.startswith(prefix), path
        path = path[len(prefix) :]
        if method == "GET" and path == "git/ref/heads/main":
            return {"object": {"sha": git(self.fixture.remote, "rev-parse", "refs/heads/main")}}
        if method == "GET" and path.startswith("pulls?"):
            return [copy.deepcopy(list(self.pulls.values()))]
        if method == "POST" and path == "pulls":
            if self.fail_create:
                raise proposal.Refusal("fixture create failure")
            number = len(self.pulls) + 1
            head = git(self.fixture.remote, "rev-parse", "refs/heads/" + data["head"])
            pr = {
                "number": number,
                "title": data["title"],
                "state": "open",
                "draft": not self.created_not_draft,
                "body": data["body"],
                "html_url": f"https://github.com/{proposal.REPOSITORY}/pull/{number}",
                "user": {"login": proposal.BOT, "type": "Bot"},
                "head": {
                    "ref": data["head"],
                    "sha": head,
                    "repo": {"full_name": proposal.REPOSITORY},
                },
                "base": {"ref": "main", "repo": {"full_name": proposal.REPOSITORY}},
            }
            self.pulls[number] = pr
            self.writes.append(("create", number))
            return copy.deepcopy(pr)
        if path.startswith("pulls/"):
            number = int(path.split("/")[1])
            if method == "GET":
                return copy.deepcopy(self.pulls[number])
            assert data == {"state": "closed"}
            assert any(
                pr["state"] == "open"
                and pr["number"] != number
                and pr["head"]["ref"] == "release-plz/" + self.fixture.base
                for pr in self.pulls.values()
            )
            if self.fail_close:
                raise proposal.Refusal("fixture close failure")
            self.writes.append(("close", number))
            self.pulls[number]["state"] = "closed"
            return copy.deepcopy(self.pulls[number])
        if path.startswith("issues/") and path.split("?")[0].endswith("/comments"):
            number = int(path.split("/")[1])
            if method == "GET":
                return [copy.deepcopy(self.comments.get(number, []))]
            self.comments.setdefault(number, []).append(
                {"body": data["body"], "user": {"login": proposal.BOT}}
            )
            self.writes.append(("comment", number))
            return {}
        raise AssertionError((method, path, data))


class ProposalTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="release-pr-tests-")
        self.addCleanup(self.temp.cleanup)
        self.parent = Path(self.temp.name)
        self.fixture = Fixture(self.parent)
        self.bin = self.parent / "bin"
        self.bin.mkdir()
        self.fake_program(
            "release-plz",
            "assert sys.argv[1:] == ['update', '--config', 'release-plz.toml']\np = pathlib.Path('Cargo.toml')\np.write_text(p.read_text().replace('0.1.0', '0.1.1'))\np = pathlib.Path('Cargo.lock')\np.write_text(p.read_text().replace('0.1.0', '0.1.1'))\n",
        )
        self.fake_program(
            "git-cliff", "print('# Changelog\\n\\n## Unreleased\\n\\n- Fixture change')\n"
        )
        env = patch.dict(os.environ, {"PATH": str(self.bin) + os.pathsep + os.environ["PATH"]})
        env.start()
        self.addCleanup(env.stop)
        for name, value in [("api", self.fixture.api.call), ("ORIGIN", str(self.fixture.remote))]:
            mock = patch.object(proposal, name, value)
            mock.start()
            self.addCleanup(mock.stop)

    def fake_program(self, name, body):
        path = self.bin / name
        path.write_text(
            '#!/usr/bin/env python3\nimport sys, pathlib, os\nassert not os.environ.get("GH_TOKEN")\nassert not os.environ.get("GITHUB_TOKEN")\n'
            + body
        )
        path.chmod(0o755)

    def run_main(self, command, base=None):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = proposal.main(
                [command, "--base", base or self.fixture.base], cwd=self.fixture.root
            )
        return code, out.getvalue(), err.getvalue()

    def prepare(self):
        code, _, error = self.run_main("prepare")
        self.assertEqual(code, 0, error)
        return git(self.fixture.root, "rev-parse", "HEAD")

    def publish(self):
        code, _, error = self.run_main("publish")
        self.assertEqual(code, 0, error)

    def test_prepare_is_deterministic_and_publish_repeat_makes_no_writes(self):
        with patch.dict(
            os.environ,
            {"GH_TOKEN": "private-fixture-token", "GITHUB_TOKEN": "private-fixture-token"},
        ):
            head = self.prepare()
        self.assertEqual(git(self.fixture.root, "rev-parse", "HEAD^"), self.fixture.base)
        self.publish()
        writes = self.fixture.api.writes.copy()
        self.publish()
        self.assertEqual(self.fixture.api.writes, writes)
        self.assertEqual(
            git(self.fixture.remote, "rev-parse", "refs/heads/release-plz/" + self.fixture.base),
            head,
        )
        # A fresh checkout for the same base produces the same commit, without moving any ref.
        clone = self.parent / "retry"
        git(self.parent, "clone", "-q", "--no-checkout", str(self.fixture.remote), str(clone))
        git(clone, "checkout", "-q", "--detach", self.fixture.base)
        self.assertEqual(proposal.main(["prepare", "--base", self.fixture.base], cwd=clone), 0)
        self.assertEqual(git(clone, "rev-parse", "HEAD"), head)

    def test_new_verified_draft_precedes_links_and_close_and_keeps_old_branch(self):
        old_head = self.prepare()
        old_base = self.fixture.base
        self.publish()
        self.fixture.next_base()
        self.prepare()
        self.publish()
        self.assertEqual(self.fixture.api.pulls[1]["state"], "closed")
        self.assertTrue(self.fixture.api.pulls[2]["draft"])
        writes = self.fixture.api.writes
        self.assertLess(writes.index(("create", 2)), writes.index(("close", 1)))
        self.assertEqual(
            git(self.fixture.remote, "rev-parse", "refs/heads/release-plz/" + old_base), old_head
        )
        self.assertIn("#2", self.fixture.api.comments[1][0]["body"])
        self.assertIn("#1", self.fixture.api.comments[2][0]["body"])

    def test_partial_push_then_create_failure_resumes_without_rewriting_branch(self):
        head = self.prepare()
        self.fixture.api.fail_create = True
        self.assertEqual(self.run_main("publish")[0], 1)
        self.fixture.api.fail_create = False
        self.publish()
        self.assertEqual(len(self.fixture.api.pulls), 1)
        self.assertEqual(
            git(self.fixture.remote, "rev-parse", "refs/heads/release-plz/" + self.fixture.base),
            head,
        )

    def test_partial_close_resumes_without_duplicate_comments(self):
        self.prepare()
        self.publish()
        self.fixture.next_base()
        self.prepare()
        self.fixture.api.fail_close = True
        self.assertEqual(self.run_main("publish")[0], 1)
        comments = copy.deepcopy(self.fixture.api.comments)
        self.fixture.api.fail_close = False
        self.publish()
        self.assertEqual(self.fixture.api.comments, comments)
        self.assertEqual(self.fixture.api.pulls[1]["state"], "closed")

    def test_replacement_verification_failure_never_closes_old_draft(self):
        self.prepare()
        self.publish()
        self.fixture.next_base()
        self.prepare()
        self.fixture.api.created_not_draft = True
        self.assertEqual(self.run_main("publish")[0], 1)
        self.assertEqual(self.fixture.api.pulls[1]["state"], "open")
        self.assertFalse(self.fixture.api.comments)

    def test_human_owned_ready_or_modified_older_drafts_are_retained(self):
        for alteration in ("human", "ready", "head", "body", "title"):
            with self.subTest(alteration=alteration):
                self.fixture.api.pulls.clear()
                self.fixture.api.comments.clear()
                self.prepare()
                self.publish()
                old = self.fixture.api.pulls[1]
                if alteration == "human":
                    old["user"]["login"] = "human"
                elif alteration == "ready":
                    old["draft"] = False
                elif alteration == "body":
                    old["body"] += "\nHuman review: keep this draft open."
                elif alteration == "title":
                    old["title"] = "Keep this proposal for investigation"
                else:
                    old["head"]["sha"] = "a" * 40
                self.fixture.next_base()
                self.prepare()
                self.publish()
                self.assertEqual(old["state"], "open")
                # Reset only the disposable fixture for the next subcase, not pushed history.
                git(self.fixture.root, "checkout", "-q", "--detach", self.fixture.base)

    def test_closed_ready_foreign_or_changed_target_is_not_reused(self):
        self.prepare()
        self.publish()
        original = copy.deepcopy(self.fixture.api.pulls[1])
        for field, value in [
            ("state", "closed"),
            ("draft", False),
            ("user", {"login": "human", "type": "User"}),
        ]:
            self.fixture.api.pulls[1] = {**copy.deepcopy(original), field: value}
            self.assertEqual(self.run_main("publish")[0], 1)
        self.fixture.api.pulls[1] = copy.deepcopy(original)
        self.fixture.api.pulls[1]["head"]["sha"] = "b" * 40
        self.assertEqual(self.run_main("publish")[0], 1)

    def test_stale_main_refuses_before_remote_writes(self):
        prepared = self.prepare()
        previous = self.fixture.base
        self.fixture.next_base()
        git(self.fixture.root, "checkout", "-q", "--detach", prepared)
        code, _, error = self.run_main("publish", base=previous)
        self.assertEqual(code, 1)
        self.assertIn("main advanced", error)
        self.assertFalse(self.fixture.api.writes)

    def test_manual_commit_with_rewritten_marker_is_not_superseded(self):
        original = self.prepare()
        self.publish()
        old = self.fixture.api.pulls[1]
        with (self.fixture.root / "CHANGELOG.md").open("a") as stream:
            stream.write("\nHuman amendment; retain for investigation.\n")
        manual = self.fixture.commit("docs: human amendment")
        git(self.fixture.root, "push", "-q", "origin", manual + ":refs/heads/" + old["head"]["ref"])
        old["head"]["sha"] = manual
        old["body"] = old["body"].replace(original, manual)
        self.fixture.next_base()
        self.prepare()
        self.publish()
        self.assertEqual(old["state"], "open")

    def test_unknown_base_draft_is_not_superseded(self):
        self.prepare()
        self.publish()
        foreign = copy.deepcopy(self.fixture.api.pulls[1])
        foreign["number"] = 2
        foreign["body"] = proposal.proposal_body("0.1.1", "a" * 40, "b" * 40)
        foreign["head"]["ref"] = "release-plz/" + "a" * 40
        foreign["head"]["sha"] = "b" * 40
        self.fixture.api.pulls[2] = foreign
        self.publish()
        self.assertEqual(self.fixture.api.pulls[2]["state"], "open")

    def test_cli_uses_the_disposable_current_directory(self):
        completed = subprocess.run(
            [
                sys.executable,
                str(ROOT / "tools/release_pr.py"),
                "prepare",
                "--base",
                self.fixture.base,
            ],
            cwd=self.fixture.root,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(git(self.fixture.root, "rev-parse", "HEAD^"), self.fixture.base)

    def test_lost_create_response_reuses_the_created_draft(self):
        self.prepare()
        original = self.fixture.api.call

        def lost_response(method, path, data=None, paginate=False):
            result = original(method, path, data, paginate)
            if method == "POST" and path.endswith("/pulls"):
                raise proposal.Refusal("fixture lost response after create")
            return result

        with patch.object(proposal, "api", lost_response):
            self.assertEqual(self.run_main("publish")[0], 1)
        self.publish()
        self.assertEqual(len(self.fixture.api.pulls), 1)

    def test_existing_remote_branch_with_unexpected_head_is_not_overwritten(self):
        head = self.prepare()
        git(
            self.fixture.root,
            "push",
            "-q",
            "origin",
            self.fixture.base + ":refs/heads/release-plz/" + self.fixture.base,
        )
        self.assertEqual(self.run_main("publish")[0], 1)
        self.assertNotEqual(
            git(self.fixture.remote, "rev-parse", "refs/heads/release-plz/" + self.fixture.base),
            head,
        )
        self.assertFalse(self.fixture.api.writes)

    def test_generator_cannot_change_dependency_or_member_manifest(self):
        for path in ("Cargo.toml", "crates/one/Cargo.toml"):
            with self.subTest(path=path):
                self.fake_program(
                    "release-plz",
                    f'with pathlib.Path({path!r}).open("a") as f: f.write("\\n[dependencies]\\nnewdep = \\"1\\"\\n")\n',
                )
                code, _, _ = self.run_main("prepare")
                self.assertEqual(code, 1)
                # Restore fixture working files only; no commits were pushed or rewritten.
                git(self.fixture.root, "restore", "--staged", "--worktree", ".")
                (self.fixture.root / "CHANGELOG.md").unlink(missing_ok=True)

    def test_workflow_never_calls_rewriting_or_publishing_commands(self):
        text = (ROOT / ".github/workflows/release-plz.yml").read_text()
        self.assertIn("cancel-in-progress: false", text)
        self.assertIn("persist-credentials: false", text)
        self.assertIn("permission-pull-requests: write", text)
        self.assertIn("release-plz@0.4.2", text)
        self.assertIn("release-pr-check", text)
        for forbidden in (
            "release-plz release",
            "--force",
            "git tag",
            "gh release",
            "pull_request_target",
        ):
            self.assertNotIn(forbidden, text)

    def test_external_lockfile_change_is_refused(self):
        git(self.fixture.root, "checkout", "-q", "main")
        lock = self.fixture.root / "Cargo.lock"
        lock.write_text(
            lock.read_text()
            + '\n[[package]]\nname = "external-fixture"\nversion = "1.0.0"\nsource = "registry+https://example.invalid/index"\nchecksum = "before"\n'
        )
        self.fixture.base = self.fixture.commit("test: external dependency fixture")
        git(self.fixture.root, "push", "-q", "origin", "main")
        git(self.fixture.root, "checkout", "-q", "--detach", self.fixture.base)
        self.fake_program(
            "release-plz",
            'p = pathlib.Path("Cargo.lock")\np.write_text(p.read_text().replace("before", "after"))\n',
        )
        code, _, error = self.run_main("prepare")
        self.assertEqual(code, 1)
        self.assertIn("external dependencies", error)


if __name__ == "__main__":
    unittest.main()
