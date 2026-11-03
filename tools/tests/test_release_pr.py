"""History-preserving proposal tests: real Git/bare remotes, fake GitHub and generators."""

import contextlib
import copy
import importlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
proposal = importlib.import_module("release_pr")
github_api = proposal.api


def git(root, *args):
    return subprocess.run(
        ["git", "-c", "core.hooksPath=/dev/null", *args],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def fixture_environment(parent):
    environment = dict(os.environ)
    for key in list(environment):
        if key.startswith("GIT_") or key in (
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_DEBUG",
            "PROPOSAL_GIT_SSH_KEY",
            "SSH_AUTH_SOCK",
        ):
            environment.pop(key)
    # Keep the installed compiler reachable, while isolating Cargo config/cache/credentials.
    rustup = Path(environment.get("RUSTUP_HOME", str(Path.home() / ".rustup"))).resolve()
    environment.update(
        HOME=str(parent / "home"),
        CARGO_HOME=str(parent / "cargo"),
        RUSTUP_HOME=str(rustup),
        RUSTUP_AUTO_INSTALL="0",
        CARGO_NET_OFFLINE="true",
        GIT_CONFIG_GLOBAL="/dev/null",
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_COUNT="0",
    )
    return environment


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
        (self.root / "tools").mkdir()
        (self.root / "docs").mkdir()
        (self.root / "docs/KNOWN-GAPS.md").write_text("# Known gaps\n")
        shutil.copyfile(ROOT / "tools/changelog.py", self.root / "tools/changelog.py")
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


def ssh_fixture_environment(fixture, parent):
    directory = parent / "ssh transport"
    directory.mkdir(exist_ok=True)
    executable = directory / "ssh"
    executable.write_text(
        f"#!{sys.executable}\n"
        "import json, os, pathlib, sys\n"
        "arguments = sys.argv[1:]\n"
        "assert not os.environ.get('GH_TOKEN')\n"
        "assert not os.environ.get('GITHUB_TOKEN')\n"
        "assert not os.environ.get('PROPOSAL_GIT_SSH_KEY')\n"
        "assert arguments[-2:] == ['git@github.com', \"git-receive-pack 'Androlax2/agent-cockpit.git'\"]\n"
        "assert arguments[arguments.index('-F') + 1] == '/dev/null'\n"
        "for required in ('StrictHostKeyChecking=yes', 'IdentitiesOnly=yes', 'IdentityAgent=none', 'ForwardAgent=no', 'BatchMode=yes', 'GlobalKnownHostsFile=/dev/null', 'HostKeyAlgorithms=ssh-ed25519'):\n"
        "    assert required in arguments, required\n"
        "private = pathlib.Path(arguments[arguments.index('-i') + 1])\n"
        "assert private.read_text() == 'fixture-ssh-key\\n'\n"
        "assert private.stat().st_mode & 0o777 == 0o600\n"
        "known = pathlib.Path(next(value.split('=', 1)[1] for value in arguments if value.startswith('UserKnownHostsFile=')))\n"
        "assert known.read_text() == 'github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl\\n'\n"
        "pathlib.Path(os.environ['FIXTURE_SSH_RECORD']).write_text(json.dumps([str(private), str(known)]))\n"
        "if os.environ.get('FIXTURE_SSH_FAIL') == '1':\n"
        "    print('fixture-ssh-key', file=sys.stderr)\n"
        "    sys.exit(1)\n"
        "os.execvp('git-receive-pack', ['git-receive-pack', os.environ['FIXTURE_GIT_REMOTE']])\n"
    )
    executable.chmod(0o700)
    return {
        "PATH": str(directory) + os.pathsep + os.environ["PATH"],
        "GIT_CONFIG_COUNT": "0",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "PROPOSAL_GIT_SSH_KEY": "fixture-ssh-key\n",
        "FIXTURE_GIT_REMOTE": str(fixture.remote),
        "FIXTURE_SSH_RECORD": str(parent / "ssh-record.json"),
    }


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
                "user": {"login": proposal.BOT, "type": "Bot", "id": 351595931},
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
                {"body": data["body"], "user": {"login": proposal.BOT, "id": 351595931}}
            )
            self.writes.append(("comment", number))
            return {}
        raise AssertionError((method, path, data))


class ChangelogTests(unittest.TestCase):
    def test_prose_is_escaped_without_changing_code_spans(self):
        cases = [
            ("- <repo> and `cockpit <repo>`\n", "- &lt;repo&gt; and `cockpit <repo>`\n"),
            ("- ``a ` b <repo>`` <text>\n", "- ``a ` b <repo>`` &lt;text&gt;\n"),
            ("- unmatched ` <repo>\n", "- unmatched ` &lt;repo&gt;\n"),
            ("- \\` <repo> `\n", "- \\` &lt;repo&gt; `\n"),
            ("- \\\\`<repo>`\n", "- \\\\`<repo>`\n"),
            ("- `<repo>\n- <repo>`\n", "- `&lt;repo&gt;\n- &lt;repo&gt;`\n"),
            ("- \\``<repo>`\n", "- \\``<repo>`\n"),
            ("- <https://example.invalid>\n", "- <https://example.invalid>\n"),
            ("- <owner@example.invalid>\n", "- <owner@example.invalid>\n"),
            ("- [file](<docs/a b.md>)\n", "- [file](<docs/a b.md>)\n"),
            ("- \\<repo\\>\n", "- \\<repo\\>\n"),
            ("- fix mapping [key]: <value>\n", "- fix mapping [key]: &lt;value&gt;\n"),
            (
                "- [file](<docs/`a.md>) and `<repo>`\n",
                "- [file](<docs/`a.md>) and `<repo>`\n",
            ),
            ("- `a` [key]: <value>\n", "- `a` [key]: &lt;value&gt;\n"),
            ("[key]: <docs/value.md>\n", "[key]: <docs/value.md>\n"),
            ("- literal ](<value>)\n", "- literal ](&lt;value&gt;)\n"),
            ("- [<label>](<file>)\n", "- [&lt;label&gt;](<file>)\n"),
            ("- \\[label](<value>)\n", "- \\[label](&lt;value&gt;)\n"),
        ]
        for source, expected in cases:
            with self.subTest(source=source):
                result = subprocess.run(
                    [sys.executable, str(ROOT / "tools/changelog.py")],
                    input=source,
                    text=True,
                    capture_output=True,
                    check=True,
                )
                self.assertEqual(result.stdout, expected)


class FixtureIsolationTests(unittest.TestCase):
    def test_proposal_fixture_never_runs_inherited_git_hooks(self):
        with tempfile.TemporaryDirectory(prefix="proposal-hook-isolation-") as directory:
            parent = Path(directory)
            hooks = parent / "host-hooks"
            hooks.mkdir()
            marker = parent / "host-hook-ran"
            hook = hooks / "post-commit"
            hook.write_text(
                f"#!{sys.executable}\nfrom pathlib import Path\nPath({str(marker)!r}).touch()\n"
            )
            hook.chmod(0o700)
            environment = dict(os.environ)
            environment.update(
                GIT_CONFIG_COUNT="1",
                GIT_CONFIG_KEY_0="core.hooksPath",
                GIT_CONFIG_VALUE_0=str(hooks),
            )
            completed = subprocess.run(
                [
                    sys.executable,
                    str(Path(__file__).resolve()),
                    "ProposalTests.test_first_draft_carries_the_exact_alpha_version_everywhere",
                ],
                env=environment,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertFalse(marker.exists(), "fixture setup must not execute inherited host hooks")


class ProposalTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="release-pr-tests-")
        self.addCleanup(self.temp.cleanup)
        self.parent = Path(self.temp.name)
        isolated = patch.dict(os.environ, fixture_environment(self.parent), clear=True)
        isolated.start()
        self.addCleanup(isolated.stop)
        self.fixture = Fixture(self.parent)
        self.bin = self.parent / "bin"
        self.bin.mkdir()
        self.fake_program(
            "release-plz",
            "assert sys.argv[1:] == ['update', '--config', 'release-plz.toml']\np = pathlib.Path('Cargo.toml')\np.write_text(p.read_text().replace('0.1.0', '0.1.1'))\np = pathlib.Path('Cargo.lock')\np.write_text(p.read_text().replace('0.1.0', '0.1.1'))\n",
        )
        self.fake_program(
            "git-cliff",
            "import json\n"
            "if '--context' in sys.argv:\n"
            "    print(json.dumps([{'version': sys.argv[sys.argv.index('--tag') + 1], 'timestamp': int(os.environ.get('FIXTURE_CLIFF_TIMESTAMP', '999'))}]))\n"
            "elif '--from-context' in sys.argv:\n"
            "    context = json.loads(pathlib.Path(sys.argv[sys.argv.index('--from-context') + 1]).read_text())\n"
            "    print('# Changelog\\n\\n## ' + context[0]['version'].removeprefix('v') + ' (' + str(context[0]['timestamp']) + ')\\n\\n- Fixture change')\n"
            "else:\n"
            "    print('# Changelog\\n\\n## Unreleased\\n\\n- Fixture change')\n",
        )
        self.fake_program(
            "cargo",
            "assert sys.argv[1:] == ['update', '--workspace', '--offline']\n"
            "import tomllib\n"
            "version = tomllib.loads(pathlib.Path('Cargo.toml').read_text())['workspace']['package']['version']\n"
            "p = pathlib.Path('Cargo.lock')\n"
            "p.write_text(p.read_text().replace('version = \\\"0.1.1\\\"', 'version = \\\"' + version + '\\\"').replace('version = \\\"0.1.0\\\"', 'version = \\\"' + version + '\\\"'))\n",
        )
        environment = ssh_fixture_environment(self.fixture, self.parent)
        environment["PATH"] = str(self.bin) + os.pathsep + environment["PATH"]
        env = patch.dict(os.environ, environment)
        env.start()
        self.addCleanup(env.stop)
        for name, value in [("api", self.fixture.api.call), ("ORIGIN", str(self.fixture.remote))]:
            mock = patch.object(proposal, name, value)
            mock.start()
            self.addCleanup(mock.stop)

    def fake_program(self, name, body):
        path = self.bin / name
        path.write_text(
            '#!/usr/bin/env python3\nimport sys, pathlib, os\nassert not os.environ.get("GH_TOKEN")\nassert not os.environ.get("GITHUB_TOKEN")\nassert not os.environ.get("PROPOSAL_GIT_SSH_KEY")\n'
            + body
        )
        path.chmod(0o755)

    def run_main(self, command, base=None):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = proposal.main(
                [command, "--base", base or self.fixture.base]
                + (["--channel", "alpha"] if command == "prepare" else []),
                cwd=self.fixture.root,
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
        with patch.dict(os.environ, {"FIXTURE_CLIFF_TIMESTAMP": "1234567"}):
            self.assertEqual(
                proposal.main(
                    ["prepare", "--base", self.fixture.base, "--channel", "alpha"], cwd=clone
                ),
                0,
            )
        self.assertEqual(git(clone, "rev-parse", "HEAD"), head)

    def test_first_draft_carries_the_exact_alpha_version_everywhere(self):
        self.prepare()
        self.publish()
        self.assertEqual(
            self.fixture.api.pulls[1]["title"], "chore(release): prepare 0.1.0-alpha.1"
        )
        manifest = tomllib.loads((self.fixture.root / "Cargo.toml").read_text())
        self.assertEqual(manifest["workspace"]["package"]["version"], "0.1.0-alpha.1")
        lock = tomllib.loads((self.fixture.root / "Cargo.lock").read_text())
        self.assertEqual([p["version"] for p in lock["package"]], ["0.1.0-alpha.1"])
        self.assertIn("\n## 0.1.0-alpha.1 (", (self.fixture.root / "CHANGELOG.md").read_text())

    def published_prerelease_body(self):
        self.prepare()
        self.publish()
        return self.fixture.api.pulls[1]["body"]

    def test_draft_shows_the_dry_run_and_release_commands_and_approval(self):
        body = self.published_prerelease_body()
        dispatch = "gh workflow run prerelease.yml -R Androlax2/agent-cockpit --ref main"
        for step in (
            f"`{dispatch} -f dry_run=true`",
            f"`{dispatch} -f dry_run=false`",
            "approve the `release` environment on the run",
            "https://github.com/Androlax2/agent-cockpit/actions/workflows/prerelease.yml",
        ):
            with self.subTest(step=step):
                self.assertIn(step, body)

    def test_draft_names_the_tag_its_dispatch_creates(self):
        body = self.published_prerelease_body()
        self.assertIn(f"would tag {self.fixture.base} as v0.1.0-alpha.1`", body)
        self.assertIn(f"tags `{self.fixture.base}` as `v0.1.0-alpha.1`", body)

    def test_draft_names_what_the_dispatch_checks_before_tagging(self):
        body = self.published_prerelease_body()
        for check in (
            "the Alpha 1 checklist (#52)",
            "the Beta 1 checklist (#57)",
            "`docs/KNOWN-GAPS.md`",
            f"`main` is still `{self.fixture.base}`",
            "`v0.1.0-alpha.1` is the next prerelease",
        ):
            with self.subTest(check=check):
                self.assertIn(check, body)

    def test_draft_keeps_the_do_not_merge_rule(self):
        self.assertIn("Do not merge this draft.", self.published_prerelease_body())

    def test_draft_dispatches_the_prerelease_workflow_input_that_exists(self):
        workflow = (ROOT / ".github/workflows/prerelease.yml").read_text()
        self.assertIn("  workflow_dispatch:\n    inputs:\n      dry_run:\n", workflow)

    def test_draft_published_before_release_steps_is_superseded_and_keeps_its_branch(self):
        old_head = self.prepare()
        old_base = self.fixture.base
        self.publish()
        # The exact body drafts carried before #669, such as #689.
        self.fixture.api.pulls[1]["body"] = (
            "Next prerelease: `v0.1.0-alpha.1`. Generated locally with release-plz update and git-cliff.\n\n"
            "Draft for review only; prerelease dispatch verifies this version and tags its main source. "
            "Do not merge this draft. Separate release approval and all release gates still apply. "
            "The branch and commit are retained when superseded.\n\n"
            f"<!-- cockpit-release-proposal v1 base={old_base} head={old_head} -->"
        )
        self.fixture.next_base()
        self.prepare()
        self.publish()
        self.assertEqual(self.fixture.api.pulls[1]["state"], "closed")
        self.assertEqual(self.fixture.api.pulls[2]["state"], "open")
        self.assertEqual(
            git(self.fixture.remote, "rev-parse", "refs/heads/release-plz/" + old_base), old_head
        )

    def test_selection_refuses_a_draft_without_release_steps(self):
        self.prepare()
        self.publish()
        pull = self.fixture.api.pulls[1]
        pull["body"] = pull["body"].replace(
            proposal.release_steps("0.1.0-alpha.1", self.fixture.base), ""
        )
        code, _, error = self.select_prerelease()
        self.assertEqual(code, 1)
        self.assertIn("edited, non-draft or not App-owned", error)

    def test_push_uses_git_only_key_and_cleans_private_files(self):
        self.prepare()
        with patch.dict(
            os.environ, {"GH_TOKEN": "fixture-api-only", "GITHUB_TOKEN": "fixture-reader"}
        ):
            self.publish()
        record = self.parent / "ssh-record.json"
        self.assertTrue(record.exists(), "publication must use the isolated SSH transport")
        self.assertTrue(all(not Path(path).exists() for path in json.loads(record.read_text())))

    def test_local_git_helpers_receive_no_publication_credentials(self):
        self.prepare()
        record = self.parent / "fsmonitor-environment.json"
        helper = self.parent / "fixture-fsmonitor"
        helper.write_text(
            f"#!{sys.executable}\nimport json, os, pathlib\n"
            f"pathlib.Path({str(record)!r}).write_text(json.dumps({{name: bool(os.environ.get(name)) for name in ('GH_TOKEN', 'GITHUB_TOKEN', 'PROPOSAL_GIT_SSH_KEY')}}))\n"
            "print('fixture\\0', end='')\n"
        )
        helper.chmod(0o700)
        git(self.fixture.root, "config", "core.fsmonitor", str(helper))
        with patch.dict(
            os.environ, {"GH_TOKEN": "fixture-api-only", "GITHUB_TOKEN": "fixture-reader"}
        ):
            self.publish()
        self.assertEqual(
            json.loads(record.read_text()),
            {"GH_TOKEN": False, "GITHUB_TOKEN": False, "PROPOSAL_GIT_SSH_KEY": False},
        )

    def test_missing_push_key_refuses_before_creating_branch_or_draft(self):
        self.prepare()
        with patch.dict(os.environ, {"PROPOSAL_GIT_SSH_KEY": ""}):
            code, _, error = self.run_main("publish")
        self.assertEqual(code, 1)
        self.assertIn("PROPOSAL_GIT_SSH_KEY", error)
        self.assertEqual(
            git(
                self.fixture.remote,
                "for-each-ref",
                "--format=%(refname)",
                "refs/heads/release-plz/",
            ),
            "",
        )
        self.assertFalse(self.fixture.api.writes)

    def test_api_transport_receives_only_api_credentials_during_publication(self):
        self.prepare()
        state = self.parent / "github-state.json"
        executable = self.bin / "gh"
        executable.write_text(
            f"#!{sys.executable}\n"
            + textwrap.dedent(f"""\
                import json, os, pathlib, sys
                from types import SimpleNamespace
                sys.path.insert(0, {str(ROOT / "tools/tests")!r})
                from test_release_pr import FakeGitHub
                assert os.environ['GH_TOKEN'] == 'fixture-api-only'
                assert not os.environ.get('PROPOSAL_GIT_SSH_KEY')
                assert not os.environ.get('GH_DEBUG')
                assert sys.argv[1:3] == ['api', '--method']
                state_path = pathlib.Path({str(state)!r})
                api = FakeGitHub(SimpleNamespace(remote=pathlib.Path({str(self.fixture.remote)!r}), base={self.fixture.base!r}))
                if state_path.exists():
                    saved = json.loads(state_path.read_text())
                    api.pulls = {{int(number): pr for number, pr in saved['pulls'].items()}}
                    api.writes = saved['writes']
                payload = json.load(sys.stdin) if '--input' in sys.argv else None
                result = api.call(sys.argv[3], sys.argv[4], payload, '--paginate' in sys.argv)
                state_path.write_text(json.dumps({{'pulls': api.pulls, 'writes': api.writes}}))
                print(json.dumps(result))
                """)
        )
        executable.chmod(0o700)
        with (
            patch.object(proposal, "api", github_api),
            patch.dict(os.environ, {"GH_TOKEN": "fixture-api-only", "GH_DEBUG": "api"}),
        ):
            self.publish()
        saved = json.loads(state.read_text())
        self.assertEqual(saved["writes"], [["create", 1]])
        self.assertTrue(saved["pulls"]["1"]["draft"])
        self.assertEqual(saved["pulls"]["1"]["user"]["id"], 351595931)

    def test_push_does_not_include_tags_from_inherited_follow_tags_setting(self):
        self.prepare()
        git(
            self.fixture.root,
            "-c",
            "tag.gpgSign=false",
            "tag",
            "-a",
            "fixture-tag",
            "-m",
            "fixture only",
        )
        git(self.fixture.root, "config", "push.followTags", "true")
        self.publish()
        self.assertEqual(git(self.fixture.remote, "tag", "--list"), "")

    def test_failed_push_removes_private_files_and_redacts_diagnostics(self):
        self.prepare()
        with patch.dict(os.environ, {"FIXTURE_SSH_FAIL": "1"}):
            code, output, error = self.run_main("publish")
        self.assertEqual(code, 1)
        self.assertNotIn("fixture-ssh-key", output + error)
        record = self.parent / "ssh-record.json"
        self.assertTrue(record.exists())
        self.assertTrue(all(not Path(path).exists() for path in json.loads(record.read_text())))
        self.assertFalse(self.fixture.api.writes)

    def test_git_url_rewrites_are_refused_before_ssh_authentication(self):
        self.prepare()
        for setting in ("insteadOf", "pushInsteadOf"):
            for scope in ("local", "environment"):
                with self.subTest(setting=setting, scope=scope):
                    name = f"url.ssh://unapproved.invalid/.{setting}"
                    environment = {}
                    if scope == "local":
                        git(self.fixture.root, "config", name, "git@github.com:")
                    else:
                        environment = {
                            "GIT_CONFIG_COUNT": "1",
                            "GIT_CONFIG_KEY_0": name,
                            "GIT_CONFIG_VALUE_0": "git@github.com:",
                        }
                    try:
                        with patch.dict(os.environ, environment):
                            code, _, error = self.run_main("publish")
                        self.assertEqual(code, 1)
                        self.assertIn("URL rewrites", error)
                        self.assertFalse((self.parent / "ssh-record.json").exists())
                        self.assertFalse(self.fixture.api.writes)
                    finally:
                        if scope == "local":
                            git(self.fixture.root, "config", "--unset", name)

    def test_verified_proposal_identity_replaces_release_bot_without_allowlist(self):
        self.prepare()
        self.assertEqual(
            git(self.fixture.root, "show", "-s", "--format=%an"), "agent-cockpit-proposal[bot]"
        )
        self.publish()
        self.fixture.api.pulls[1]["user"]["login"] = "agent-cockpit-release[bot]"
        self.assertEqual(self.select_prerelease()[0], 1)

    def test_changed_bot_id_is_not_accepted_even_with_same_login(self):
        self.prepare()
        self.publish()
        self.fixture.api.pulls[1]["user"]["id"] = 999
        self.assertEqual(self.select_prerelease()[0], 1)

    def select_prerelease(self, beta="open"):
        git(self.fixture.root, "checkout", "-q", "--detach", self.fixture.base)
        out, err = io.StringIO(), io.StringIO()
        with (
            patch.object(proposal.release, "BOOTSTRAP_COMMIT", self.fixture.base),
            contextlib.redirect_stdout(out),
            contextlib.redirect_stderr(err),
        ):
            code = proposal.release.main(
                [
                    "next-prerelease",
                    "--main",
                    "main",
                    "--alpha-checklist",
                    "closed",
                    "--beta-checklist",
                    beta,
                ],
                cwd=self.fixture.root,
            )
        return code, out.getvalue(), err.getvalue()

    def test_selection_reads_the_verified_open_draft_without_remote_writes(self):
        self.prepare()
        self.publish()
        writes = self.fixture.api.writes.copy()
        code, output, error = self.select_prerelease()
        self.assertEqual(code, 0, error)
        self.assertIn("tag=v0.1.0-alpha.1\n", output)
        self.assertIn("proposal_number=1\n", output)
        self.assertIn(f"proposal_head={self.fixture.api.pulls[1]['head']['sha']}\n", output)
        self.assertEqual(self.fixture.api.writes, writes)

    def test_selection_refuses_missing_draft(self):
        code, _, error = self.select_prerelease()
        self.assertEqual(code, 1)
        self.assertIn("exactly one open release proposal", error)
        self.assertFalse(self.fixture.api.writes)

    def test_selection_refuses_duplicate_drafts(self):
        self.prepare()
        self.publish()
        self.fixture.api.pulls[2] = {**copy.deepcopy(self.fixture.api.pulls[1]), "number": 2}
        writes = self.fixture.api.writes.copy()
        code, _, error = self.select_prerelease()
        self.assertEqual(code, 1)
        self.assertIn("exactly one open release proposal", error)
        self.assertEqual(self.fixture.api.writes, writes)

    def test_selection_refuses_a_duplicate_opened_during_verification(self):
        self.prepare()
        self.publish()
        original = self.fixture.api.call

        def open_duplicate(method, path, data=None, paginate=False):
            result = original(method, path, data, paginate)
            if method == "GET" and "pulls?" in path:
                self.fixture.api.pulls[2] = {
                    **copy.deepcopy(self.fixture.api.pulls[1]),
                    "number": 2,
                }
            return result

        writes = self.fixture.api.writes.copy()
        with patch.object(proposal, "api", open_duplicate):
            code, _, error = self.select_prerelease()
        self.assertEqual(code, 1)
        self.assertIn("open release proposal", error)
        self.assertEqual(self.fixture.api.writes, writes)

    def test_selection_refuses_late_edits_and_main_advancement(self):
        self.prepare()
        self.publish()
        original_pr = copy.deepcopy(self.fixture.api.pulls[1])
        original_api = self.fixture.api.call
        writes = self.fixture.api.writes.copy()
        for alteration in ("title", "body", "head", "main"):
            with self.subTest(alteration=alteration):
                self.fixture.api.pulls[1] = copy.deepcopy(original_pr)
                list_calls = 0

                def change_before_final_list(
                    method, path, data=None, paginate=False, *, alteration=alteration
                ):
                    nonlocal list_calls
                    if method == "GET" and "pulls?" in path:
                        list_calls += 1
                        if list_calls == 2:
                            if alteration in ("title", "body"):
                                self.fixture.api.pulls[1][alteration] += " edited"
                            elif alteration == "head":
                                self.fixture.api.pulls[1]["head"]["sha"] = "a" * 40
                            else:
                                self.fixture.next_base()
                    return original_api(method, path, data, paginate)

                with patch.object(proposal, "api", change_before_final_list):
                    code, output, _ = self.select_prerelease()
                self.assertEqual(list_calls, 2)
                self.assertEqual(code, 1)
                self.assertEqual(output, "")
                self.assertEqual(self.fixture.api.writes, writes)

    def test_workflow_freezes_selection_after_token_creation_and_before_tag_request(self):
        workflow = (ROOT / ".github/workflows/prerelease.yml").read_text()
        token_step = workflow.index("      - uses: actions/create-github-app-token@")
        selection_step = workflow.index("      - name: Verify the open draft")
        self.assertLess(token_step, selection_step)

    def run_snapshot_step(self, selection_code, mutation="none", dry_run=False):
        self.prepare()
        self.publish()
        source = self.fixture.base
        selected_code, selected_output, error = self.select_prerelease()
        self.assertEqual(selected_code, 0, error)
        if mutation == "proposal":
            self.fixture.api.pulls[1].update(
                title="Later human edit", body="Later human edit", state="closed"
            )
            self.fixture.api.pulls[1]["head"]["sha"] = "a" * 40
            self.fixture.api.pulls[2] = {
                **copy.deepcopy(self.fixture.api.pulls[1]),
                "number": 2,
                "state": "open",
            }
        workflow = (ROOT / ".github/workflows/prerelease.yml").read_text()
        step = workflow.split("      - name: Verify the open draft", 1)[1].split("\n      - ", 1)[0]
        script = textwrap.dedent(step.split("        run: |\n", 1)[1])
        outputs = self.parent / "outputs"
        requests = self.parent / "requests"
        # The real selector is exercised above and by mutation tests. This adapter
        # controls its completion boundary while executing the actual workflow shell.
        self.fake_program(
            "python3",
            "assert sys.argv[1:3] == ['tools/release.py', 'next-prerelease']\n"
            "assert os.environ['GH_TOKEN'] == 'fixture-reader'\n"
            "import subprocess\n"
            "if os.environ['FIXTURE_MUTATION'] == 'main':\n"
            "    subprocess.run(['git', 'checkout', '-q', 'main'], check=True)\n"
            "    subprocess.run(['git', 'commit', '-q', '--allow-empty', '-m', 'fixture later main'], check=True)\n"
            "if os.environ['FIXTURE_SELECTION_EXIT'] != '0':\n"
            "    sys.exit(int(os.environ['FIXTURE_SELECTION_EXIT']))\n"
            "print(os.environ['FIXTURE_SELECTION'], end='')\n",
        )
        selector = self.bin / "python3"
        # Pin the fixture interpreter so its name cannot recurse through this shim.
        selector.write_text(
            selector.read_text()
            .replace("#!/usr/bin/env python3\n", f"#!{sys.executable}\n", 1)
            .replace('assert not os.environ.get("GH_TOKEN")\n', "")
        )
        self.fake_program(
            "gh",
            "assert os.environ['GH_TOKEN'] == 'fixture-writer'\n"
            "assert sys.argv[1:3] == ['api', 'repos/Androlax2/agent-cockpit/git/refs']\n"
            "pathlib.Path(os.environ['FIXTURE_REQUESTS']).write_text('\\n'.join(sys.argv[1:]))\n",
        )
        github = self.bin / "gh"
        github.write_text(
            github.read_text()
            .replace("#!/usr/bin/env python3\n", f"#!{sys.executable}\n", 1)
            .replace('assert not os.environ.get("GH_TOKEN")\n', "")
        )
        environment = dict(os.environ)
        environment.pop("GITHUB_TOKEN", None)
        environment.pop("PROPOSAL_GIT_SSH_KEY", None)
        environment.update(
            GH_TOKEN="fixture-reader",
            RELEASE_TOKEN="fixture-writer",
            GITHUB_REPOSITORY=proposal.REPOSITORY,
            GITHUB_OUTPUT=str(outputs),
            ALPHA_CHECKLIST="closed",
            BETA_CHECKLIST="open",
            DRY_RUN=str(dry_run).lower(),
            FIXTURE_SELECTION=selected_output,
            FIXTURE_SELECTION_EXIT=str(selection_code),
            FIXTURE_MUTATION=mutation,
            FIXTURE_REQUESTS=str(requests),
        )
        completed = subprocess.run(
            ["bash", "--noprofile", "--norc", "-e", "-o", "pipefail", "-c", script],
            cwd=self.fixture.root,
            env=environment,
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        return completed, requests, outputs, source

    def test_workflow_never_requests_a_tag_when_final_selection_refuses(self):
        completed, requests, _, _ = self.run_snapshot_step(1)
        self.assertNotEqual(completed.returncode, 0)
        self.assertFalse(requests.exists())

    def test_workflow_tags_the_frozen_source_after_main_advances(self):
        completed, requests, outputs, source = self.run_snapshot_step(0, mutation="main")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertNotEqual(git(self.fixture.root, "rev-parse", "main"), source)
        self.assertTrue(requests.exists(), "final selection step must use its captured source")
        self.assertIn(f"sha={source}", requests.read_text().splitlines())
        self.assertIn("proposal_head=", outputs.read_text())
        self.assertEqual(git(self.fixture.root, "tag", "--list"), "")

    def test_workflow_dry_run_records_snapshot_without_requesting_a_tag(self):
        completed, requests, outputs, _ = self.run_snapshot_step(0, dry_run=True)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertFalse(requests.exists())
        self.assertIn("tag=v0.1.0-alpha.1\n", outputs.read_text())

    def test_workflow_keeps_snapshot_after_later_proposal_edits(self):
        completed, requests, outputs, source = self.run_snapshot_step(0, mutation="proposal")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn(f"sha={source}", requests.read_text().splitlines())
        self.assertNotIn("proposal_head=" + "a" * 40, outputs.read_text())
        self.assertIn("proposal_number=1\n", outputs.read_text())
        self.assertEqual(git(self.fixture.root, "tag", "--list"), "")

    def test_selection_refuses_edited_or_foreign_metadata(self):
        self.prepare()
        self.publish()
        original = copy.deepcopy(self.fixture.api.pulls[1])
        writes = self.fixture.api.writes.copy()
        for alteration in (
            "author",
            "repository",
            "head",
            "title",
            "body",
            "base",
            "ready",
            "closed",
        ):
            with self.subTest(alteration=alteration):
                pr = copy.deepcopy(original)
                if alteration == "author":
                    pr["user"]["login"] = "human"
                elif alteration == "repository":
                    pr["head"]["repo"]["full_name"] = "someone/fork"
                elif alteration == "head":
                    pr["head"]["sha"] = "a" * 40
                elif alteration in ("title", "body"):
                    pr[alteration] += " edited"
                elif alteration == "base":
                    pr["body"] = pr["body"].replace(self.fixture.base, "a" * 40)
                    pr["head"]["ref"] = "release-plz/" + "a" * 40
                elif alteration == "ready":
                    pr["draft"] = False
                else:
                    pr["state"] = "closed"
                self.fixture.api.pulls[1] = pr
                self.assertEqual(self.select_prerelease()[0], 1)
                self.assertEqual(self.fixture.api.writes, writes)

    def test_selection_refuses_stale_main_after_draft_creation(self):
        self.prepare()
        self.publish()
        self.fixture.next_base()
        code, _, error = self.select_prerelease()
        self.assertEqual(code, 1)
        self.assertIn("current main", error)

    def test_selection_refuses_wrong_channel(self):
        self.prepare()
        self.publish()
        code, _, error = self.select_prerelease(beta="closed")
        self.assertEqual(code, 1)
        self.assertIn("expected 0.1.0-beta.1", error)

    def test_selection_refuses_inconsistent_or_wrong_version_proposal_content(self):
        for alteration in (
            "lock",
            "manifest",
            "changelog",
            "leading_changelog",
            "duplicate_changelog",
            "number",
            "legacy",
        ):
            with self.subTest(alteration=alteration):
                self.fixture.api.pulls.clear()
                self.prepare()
                version = "0.1.0-alpha.1"
                if alteration in ("number", "legacy"):
                    version = "0.1.0-alpha.9" if alteration == "number" else "0.1.0"
                    paths = ("Cargo.toml", "Cargo.lock", "CHANGELOG.md")
                else:
                    version_change = {
                        "lock": "Cargo.lock",
                        "manifest": "Cargo.toml",
                        "changelog": "CHANGELOG.md",
                        "leading_changelog": "CHANGELOG.md",
                        "duplicate_changelog": "CHANGELOG.md",
                    }
                    paths = (version_change[alteration],)
                for name in paths:
                    path = self.fixture.root / name
                    if alteration in ("leading_changelog", "duplicate_changelog"):
                        heading = "Unreleased" if alteration == "leading_changelog" else version
                        path.write_text(
                            path.read_text().replace(
                                "# Changelog\n", f"# Changelog\n\n## {heading}\n\n- Extra section\n"
                            )
                        )
                        continue
                    path.write_text(
                        path.read_text().replace(
                            "0.1.0-alpha.1",
                            version if alteration in ("number", "legacy") else "0.1.0-alpha.9",
                        )
                    )
                git(self.fixture.root, "add", ".")
                # Construct an untrusted, single-parent proposal before its first publication.
                tree = git(self.fixture.root, "write-tree")
                head = git(
                    self.fixture.root,
                    "commit-tree",
                    tree,
                    "-p",
                    self.fixture.base,
                    "-m",
                    "fixture untrusted proposal",
                )
                branch = "release-plz/" + self.fixture.base
                git(self.fixture.root, "push", "-q", "origin", f"{head}:refs/heads/{branch}")
                self.fixture.api.call(
                    "POST",
                    proposal.endpoint("pulls"),
                    {
                        "title": f"chore(release): prepare {version}",
                        "head": branch,
                        "base": "main",
                        "draft": True,
                        "body": proposal.proposal_body(version, self.fixture.base, head),
                    },
                )
                git(self.fixture.root, "checkout", "-q", "--detach", head)
                writes = self.fixture.api.writes.copy()
                code, _, _ = self.select_prerelease()
                self.fixture.next_base()
                self.assertEqual(code, 1)
                self.assertEqual(self.fixture.api.writes, writes)

    def test_verified_alpha_replaces_a_legacy_stable_shaped_draft(self):
        self.prepare()
        for name in ("Cargo.toml", "Cargo.lock", "CHANGELOG.md"):
            path = self.fixture.root / name
            path.write_text(path.read_text().replace("0.1.0-alpha.1", "0.1.0"))
        git(self.fixture.root, "add", ".")
        tree = git(self.fixture.root, "write-tree")
        head = git(
            self.fixture.root,
            "commit-tree",
            tree,
            "-p",
            self.fixture.base,
            "-m",
            "fixture legacy proposal",
        )
        branch = "release-plz/" + self.fixture.base
        git(self.fixture.root, "push", "-q", "origin", f"{head}:refs/heads/{branch}")
        self.fixture.api.call(
            "POST",
            proposal.endpoint("pulls"),
            {
                "title": "chore(release): prepare 0.1.0",
                "head": branch,
                "base": "main",
                "draft": True,
                "body": proposal.proposal_body("0.1.0", self.fixture.base, head),
            },
        )
        git(self.fixture.root, "checkout", "-q", "--detach", head)
        self.fixture.next_base()
        self.prepare()
        self.publish()
        self.assertEqual(self.fixture.api.pulls[1]["state"], "closed")
        self.assertEqual(git(self.fixture.remote, "rev-parse", "refs/heads/" + branch), head)

    def test_tagged_attempt_advances_the_next_draft(self):
        git(self.fixture.root, "tag", "v0.1.0-alpha.1", self.fixture.base)
        self.fixture.next_base()
        self.prepare()
        self.publish()
        self.assertEqual(
            self.fixture.api.pulls[1]["title"], "chore(release): prepare 0.1.0-alpha.2"
        )

    def test_explicit_beta_channel_starts_at_one_after_alphas(self):
        git(self.fixture.root, "tag", "v0.1.0-alpha.3", self.fixture.base)
        self.fixture.next_base()
        code = proposal.main(
            ["prepare", "--base", self.fixture.base, "--channel", "beta"], cwd=self.fixture.root
        )
        self.assertEqual(code, 0)
        self.publish()
        self.assertEqual(self.fixture.api.pulls[1]["title"], "chore(release): prepare 0.1.0-beta.1")

    def test_same_source_channel_transition_requires_new_main_and_keeps_alpha(self):
        alpha_head = self.prepare()
        self.publish()
        alpha_branch = self.fixture.api.pulls[1]["head"]["ref"]
        code, _, error = self.select_prerelease(beta="closed")
        self.assertEqual(code, 1)
        self.assertIn("expected 0.1.0-beta.1", error)
        with contextlib.redirect_stdout(io.StringIO()):
            code = proposal.main(
                ["prepare", "--base", self.fixture.base, "--channel", "beta"],
                cwd=self.fixture.root,
            )
        self.assertEqual(code, 0)
        writes = self.fixture.api.writes.copy()
        code, _, error = self.run_main("publish")
        self.assertEqual(code, 1)
        self.assertIn("channel transitions require a deliberate new main commit", error)
        self.assertEqual(self.fixture.api.writes, writes)
        self.assertEqual(self.fixture.api.pulls[1]["state"], "open")
        self.assertEqual(
            git(self.fixture.remote, "rev-parse", "refs/heads/" + alpha_branch), alpha_head
        )

    def test_beta_transition_commit_replaces_alpha_and_preserves_its_branch(self):
        alpha_head = self.prepare()
        self.publish()
        alpha_branch = self.fixture.api.pulls[1]["head"]["ref"]
        git(self.fixture.root, "checkout", "-q", "main")
        (self.fixture.root / "docs/beta-transition.md").write_text("Beta transition approved.\n")
        self.fixture.base = self.fixture.commit("docs: prepare beta transition")
        git(self.fixture.root, "push", "-q", "origin", "main")
        git(self.fixture.root, "checkout", "-q", "--detach", self.fixture.base)
        with contextlib.redirect_stdout(io.StringIO()):
            code = proposal.main(
                ["prepare", "--base", self.fixture.base, "--channel", "beta"],
                cwd=self.fixture.root,
            )
        self.assertEqual(code, 0)
        self.publish()
        self.assertEqual(
            self.fixture.api.pulls[2]["head"]["ref"], "release-plz/" + self.fixture.base
        )
        code, output, error = self.select_prerelease(beta="closed")
        self.assertEqual(code, 0, error)
        self.assertIn("tag=v0.1.0-beta.1\n", output)
        self.assertEqual(self.fixture.api.pulls[1]["state"], "closed")
        self.assertEqual(
            git(self.fixture.remote, "rev-parse", "refs/heads/" + alpha_branch), alpha_head
        )

    def test_new_verified_draft_precedes_links_and_close_and_keeps_old_branch(self):
        old_head = self.prepare()
        old_base = self.fixture.base
        self.publish()
        self.fixture.next_base()
        self.prepare()
        self.publish()
        self.assertEqual(self.fixture.api.pulls[1]["state"], "closed")
        self.assertTrue(self.fixture.api.pulls[2]["draft"])
        self.assertEqual(
            self.fixture.api.pulls[2]["title"], "chore(release): prepare 0.1.0-alpha.1"
        )
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
                "--channel",
                "alpha",
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

    def test_conformance_keeps_installed_toolchain_while_isolating_cargo_home(self):
        import release_pr_conformance

        host = self.parent / "fixture-host"
        toolchain = host / ".rustup"
        toolchain.mkdir(parents=True)
        (toolchain / "installed").write_text("fixture toolchain")
        cargo = self.bin / "fixture-cargo"
        cargo.write_text(
            '#!/usr/bin/env python3\nimport os, pathlib\nroot = pathlib.Path(os.environ.get("RUSTUP_HOME", str(pathlib.Path.home() / ".rustup")))\nassert (root / "installed").is_file(), "installed toolchain hidden by temporary HOME"\nassert os.environ["RUSTUP_AUTO_INSTALL"] == "0"\n'
        )
        cargo.chmod(0o755)
        with patch.dict(os.environ, {"HOME": str(host)}, clear=False):
            previous = os.environ.pop("RUSTUP_HOME", None)
            try:
                env = release_pr_conformance.fixture_environment(self.parent / "isolated")
            finally:
                if previous is not None:
                    os.environ["RUSTUP_HOME"] = previous
        subprocess.run([str(cargo)], env=env, check=True)
        self.assertNotEqual(env["HOME"], str(host))
        self.assertEqual(env["CARGO_HOME"], str(self.parent / "isolated/cargo"))


if __name__ == "__main__":
    unittest.main()
