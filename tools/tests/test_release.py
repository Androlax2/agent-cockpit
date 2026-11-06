import contextlib
import importlib.util
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
RELEASE_WORKFLOW = ROOT / ".github/workflows/release.yml"
SPEC = importlib.util.spec_from_file_location("release", ROOT / "tools/release.py")
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)

WORKSPACE_MANIFEST = """[workspace]
members = ["crates/*"]

[workspace.package]
version = "{version}"
edition = "2024"
"""
MEMBER_MANIFEST = """[package]
name = "cockpit-cli"
version.workspace = true

[dependencies]
cockpit-client = {{ path = "../cockpit-client", version = "{version}" }}
serde = {{ version = "{version}" }}
"""


def run(*arguments, cwd):
    """release.py's entry point: (exit code, stdout outputs as a dict, stderr)."""
    stdout, stderr = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        code = release.main(list(arguments), cwd=cwd)
    outputs = dict(line.split("=", 1) for line in stdout.getvalue().splitlines() if "=" in line)
    return code, outputs, stderr.getvalue()


class Repository:
    """A throwaway Git history whose first commit stands in for the bootstrap commit."""

    def __init__(self, directory, version="0.1.0"):
        self.path = Path(directory)
        self.git("init", "--quiet", "--initial-branch=main")
        self.git("config", "user.email", "release-test@example.invalid")
        self.git("config", "user.name", "release test")
        self.git("config", "tag.gpgSign", "false")
        self.git("config", "commit.gpgSign", "false")
        self.write_version(version)
        (self.path / "Cargo.lock").write_text(
            f'version = 4\n\n[[package]]\nname = "cockpit-cli"\nversion = "{version}"\n'
        )
        (self.path / "docs").mkdir()
        (self.path / release.KNOWN_GAPS).write_text("## Known gaps\n")
        self.commit_count = 0
        self.bootstrap = self.commit("build: bootstrap")

    def git(self, *arguments):
        return subprocess.run(
            ["git", *arguments], cwd=self.path, check=True, capture_output=True, text=True
        ).stdout.strip()

    def write_version(self, version):
        (self.path / "Cargo.toml").write_text(WORKSPACE_MANIFEST.format(version=version))

    def commit(self, subject):
        self.git("add", "--all")
        self.git("commit", "--quiet", "--allow-empty", "-m", subject)
        return self.git("rev-parse", "HEAD")

    def commits(self, count):
        # Distinct subjects: equal trees, parents and timestamps would give equal hashes.
        for _ in range(count):
            self.commit_count += 1
            self.commit(f"feat: change {self.commit_count}")
        return self.git("rev-parse", "HEAD")

    def tag(self, name, revision="HEAD"):
        self.git("tag", name, revision)


class ReleaseTestCase(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.repository = Repository(directory.name)
        bootstrap = patch.object(release, "BOOTSTRAP_COMMIT", self.repository.bootstrap)
        bootstrap.start()
        self.addCleanup(bootstrap.stop)

    def run_release(self, *arguments):
        return run(*arguments, cwd=self.repository.path)

    def next_prerelease(
        self, alpha_checklist="closed", beta_checklist="open", main="main", version=None
    ):
        import release_pr

        repository = self.repository
        source = repository.git("rev-parse", "HEAD")
        version = version or ("0.1.0-beta.1" if beta_checklist == "closed" else "0.1.0-alpha.1")
        paths = [repository.path / name for name in ("Cargo.toml", "Cargo.lock", "CHANGELOG.md")]
        before = {path: path.read_bytes() if path.exists() else None for path in paths}
        repository.write_version(version)
        paths[1].write_text(
            f'version = 4\n\n[[package]]\nname = "cockpit-cli"\nversion = "{version}"\n'
        )
        paths[2].write_text(f"# Changelog\n\n## {version}\n\n- Fixture change\n")
        repository.git("add", ".")
        tree = repository.git("write-tree")
        head = repository.git("commit-tree", tree, "-p", source, "-m", "fixture draft")
        repository.git("read-tree", "HEAD")
        for path, content in before.items():
            if content is None:
                path.unlink()
            else:
                path.write_bytes(content)
        branch = "release-plz/" + source
        repository.git("update-ref", "refs/heads/" + branch, head)
        if "origin" not in repository.git("remote").splitlines():
            repository.git("remote", "add", "origin", str(repository.path))
        repo = {"full_name": release_pr.REPOSITORY}
        pr = {
            "number": 1,
            "state": "open",
            "draft": True,
            "title": f"chore(release): prepare {version}",
            "body": release_pr.proposal_body(version, source, head),
            "user": {"login": "agent-cockpit-proposal[bot]", "type": "Bot", "id": 351595931},
            "head": {"sha": head, "ref": branch, "repo": repo},
            "base": {"ref": "main", "repo": repo},
        }

        def api(method, path, data=None, paginate=False):
            self.assertEqual(method, "GET")
            if path.endswith("git/ref/heads/main"):
                return {"object": {"sha": repository.git("rev-parse", main)}}
            if "pulls?" in path:
                return [[pr]]
            if path.endswith("pulls/1"):
                return pr
            self.fail(f"unexpected fixture API request {path}")

        with (
            patch.object(release_pr, "api", api),
            patch.object(release_pr, "ORIGIN", str(repository.path)),
        ):
            return self.run_release(
                "next-prerelease",
                "--main",
                main,
                "--alpha-checklist",
                alpha_checklist,
                "--beta-checklist",
                beta_checklist,
            )

    def check_tag(self, tag, alpha_checklist="closed", beta_checklist="open", main="main"):
        return self.run_release(
            "check-tag",
            tag,
            "--main",
            main,
            "--alpha-checklist",
            alpha_checklist,
            "--beta-checklist",
            beta_checklist,
        )


class TagGrammarTests(ReleaseTestCase):
    def test_only_the_three_policy_shapes_are_release_tags(self):
        for tag in [
            "0.1.0",
            "v0.1",
            "v0.1.0-rc.1",
            "v0.1.0-alpha",
            "v0.1.0-alpha.0",
            "v0.1.0-beta.01",
            "v01.1.0",
            "nightly",
            "v0.1.0+build",
        ]:
            with self.subTest(tag=tag):
                self.repository.tag(tag)
                code, _outputs, error = self.check_tag(tag)
                self.assertEqual(code, 1)
                self.assertIn("not a release tag", error)


class CheckTagTests(ReleaseTestCase):
    def test_the_first_alpha_is_one_regardless_of_commit_count(self):
        revision = self.repository.commits(3)
        self.repository.tag("v0.1.0-alpha.1")
        code, outputs, error = self.check_tag("v0.1.0-alpha.1")
        self.assertEqual(code, 0, error)
        self.assertEqual(
            outputs,
            {
                "tag": "v0.1.0-alpha.1",
                "channel": "alpha",
                "version": "0.1.0-alpha.1",
                "prerelease": "true",
                "revision": revision,
                "notes_range": f"{self.repository.bootstrap}..v0.1.0-alpha.1",
            },
        )

    def test_a_pre_release_that_skips_the_next_number_is_refused(self):
        self.repository.commits(3)
        self.repository.tag("v0.1.0-alpha.2")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.2")
        self.assertEqual(code, 1)
        self.assertIn("next sequential version is 0.1.0-alpha.1", error)

    def test_a_pre_release_on_a_bumped_workspace_is_refused(self):
        self.repository.write_version("0.2.0")
        self.repository.commit("chore: bump by hand")
        self.repository.tag("v0.1.0-alpha.1")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.1")
        self.assertEqual(code, 1)
        self.assertIn("must still carry the base's 0.1.0", error)

    def test_pre_release_notes_start_at_the_highest_lower_release_it_descends_from(self):
        self.repository.commits(2)
        self.repository.tag("v0.1.0-alpha.1")
        self.repository.commits(3)
        self.repository.tag("v0.1.0-alpha.2")
        code, outputs, error = self.check_tag("v0.1.0-alpha.2")
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["notes_range"], "v0.1.0-alpha.1..v0.1.0-alpha.2")

    def test_a_shallow_clone_is_refused(self):
        self.repository.commits(2)
        self.repository.tag("v0.1.0-alpha.2")
        with tempfile.TemporaryDirectory() as directory:
            clone = Path(directory) / "clone"
            subprocess.run(
                [
                    "git",
                    "clone",
                    "--quiet",
                    "--depth",
                    "1",
                    "--no-local",
                    f"file://{self.repository.path}",
                    str(clone),
                ],
                check=True,
            )
            subprocess.run(
                ["git", "fetch", "--quiet", "--depth", "1", "origin", "tag", "v0.1.0-alpha.2"],
                cwd=clone,
                check=True,
            )
            code, _outputs, error = run(
                "check-tag",
                "v0.1.0-alpha.2",
                "--main",
                "origin/main",
                "--alpha-checklist",
                "closed",
                "--beta-checklist",
                "open",
                cwd=clone,
            )
        self.assertEqual(code, 1)
        self.assertIn("shallow clone", error)

    def test_a_hand_made_alpha_without_its_known_gaps_is_refused(self):
        (self.repository.path / release.KNOWN_GAPS).unlink()
        self.repository.commit("docs: drop the known gaps")
        self.repository.tag("v0.1.0-alpha.1")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.1")
        self.assertEqual(code, 1)
        self.assertIn("an alpha needs docs/KNOWN-GAPS.md", error)

    def test_a_second_release_tag_on_one_commit_is_refused(self):
        self.repository.commits(1)
        self.repository.tag("v0.1.0-alpha.1")
        self.repository.tag("v0.1.0-beta.1")
        code, _outputs, error = self.check_tag("v0.1.0-beta.1", beta_checklist="closed")
        self.assertEqual(code, 1)
        self.assertIn("already released as v0.1.0-alpha.1", error)

    def test_a_tag_on_a_commit_outside_main_is_refused(self):
        self.repository.git("checkout", "--quiet", "-b", "feature")
        self.repository.commits(2)
        self.repository.tag("v0.1.0-alpha.2")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.2")
        self.assertEqual(code, 1)
        self.assertIn("is not on main", error)

    def test_an_alpha_tag_while_the_alpha_1_checklist_is_open_is_refused(self):
        self.repository.commits(1)
        self.repository.tag("v0.1.0-alpha.1")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.1", alpha_checklist="open")
        self.assertEqual(code, 1)
        self.assertIn("no alpha while the Alpha 1 checklist (#52) is open", error)

    def test_a_beta_tag_while_the_beta_1_checklist_is_open_is_refused(self):
        self.repository.commits(1)
        self.repository.tag("v0.1.0-beta.1")
        for alpha_checklist in ("open", "closed"):
            with self.subTest(alpha_checklist=alpha_checklist):
                code, _outputs, error = self.check_tag(
                    "v0.1.0-beta.1", alpha_checklist=alpha_checklist, beta_checklist="open"
                )
                self.assertEqual(code, 1)
                self.assertIn("no beta while the Beta 1 checklist (#57) is open", error)

    def test_an_alpha_tag_once_the_beta_1_checklist_is_closed_is_refused(self):
        self.repository.commits(1)
        self.repository.tag("v0.1.0-alpha.1")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.1", beta_checklist="closed")
        self.assertEqual(code, 1)
        self.assertIn("no alpha once the Beta 1 checklist (#57) is closed", error)

    def test_a_pre_release_tag_below_the_newest_pre_release_is_refused(self):
        self.repository.commits(1)
        self.repository.tag("v0.1.0-beta.1")
        self.repository.commits(1)
        self.repository.tag("v0.1.0-alpha.2")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.2", beta_checklist="open")
        self.assertEqual(code, 1)
        self.assertIn("not above the newest pre-release v0.1.0-beta.1", error)

    def test_every_stable_tag_is_refused_until_theo_decides(self):
        # v0.1.0 equals the workspace version, so only the no-stable rule can refuse it.
        self.repository.commits(1)
        self.repository.tag("v0.1.0")
        for beta_checklist in ("open", "closed"):
            with self.subTest(beta_checklist=beta_checklist):
                code, _outputs, error = self.check_tag("v0.1.0", beta_checklist=beta_checklist)
                self.assertEqual(code, 1)
                self.assertIn("no stable release and no 1.0 until Theo decides (#389)", error)

    def test_a_pre_release_that_does_not_descend_from_a_lower_one_is_refused(self):
        self.repository.commits(2)
        self.repository.tag("v0.1.0-beta.2")
        self.repository.commits(2)
        self.repository.tag("v0.1.0-alpha.4")
        code, _outputs, error = self.check_tag("v0.1.0-beta.2", beta_checklist="closed")
        self.assertEqual(code, 1)
        self.assertIn("does not descend from v0.1.0-alpha.4", error)

    def test_a_release_shaped_tag_outside_main_does_not_count(self):
        self.repository.git("checkout", "--quiet", "-b", "side")
        self.repository.commits(5)
        self.repository.tag("v0.1.0-alpha.5")
        self.repository.git("checkout", "--quiet", "main")
        self.repository.commits(2)
        self.repository.tag("v0.1.0-alpha.1")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.1")
        self.assertEqual(code, 0, error)


class NextPrereleaseTests(ReleaseTestCase):
    def test_after_the_alpha_1_checklist_the_first_alpha_is_one(
        self,
    ):
        revision = self.repository.commits(7)
        code, outputs, error = self.next_prerelease(alpha_checklist="closed", beta_checklist="open")
        self.assertEqual(code, 0, error)
        self.assertEqual(
            outputs,
            {
                "tag": "v0.1.0-alpha.1",
                "channel": "alpha",
                "revision": revision,
                "proposal_number": "1",
                "proposal_head": self.repository.git(
                    "rev-parse", "refs/heads/release-plz/" + revision
                ),
            },
        )

    def test_additional_commits_do_not_consume_an_alpha_number(self):
        self.repository.commits(7)
        self.assertEqual(self.next_prerelease()[1]["tag"], "v0.1.0-alpha.1")
        self.repository.commits(4)
        self.assertEqual(self.next_prerelease()[1]["tag"], "v0.1.0-alpha.1")

    def test_a_tag_consumes_its_number_even_without_publication(self):
        self.repository.commits(7)
        self.repository.tag("v0.1.0-alpha.1")
        self.repository.commits(4)
        code, outputs, error = self.next_prerelease(version="0.1.0-alpha.2")
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["tag"], "v0.1.0-alpha.2")

    def test_a_conflicting_next_tag_outside_main_is_refused(self):
        self.repository.git("checkout", "--quiet", "-b", "side")
        self.repository.commits(2)
        self.repository.tag("v0.1.0-alpha.1")
        self.repository.git("checkout", "--quiet", "main")
        self.repository.commits(3)
        code, _, error = self.next_prerelease()
        self.assertEqual(code, 1)
        self.assertIn("v0.1.0-alpha.1 already exists", error)

    def test_no_alpha_without_its_known_gaps(self):
        (self.repository.path / release.KNOWN_GAPS).unlink()
        self.repository.commit("docs: drop the known gaps")
        code, _outputs, error = self.next_prerelease(
            alpha_checklist="closed", beta_checklist="open"
        )
        self.assertEqual(code, 1)
        self.assertIn("an alpha needs docs/KNOWN-GAPS.md", error)

    def test_no_alpha_while_the_alpha_1_checklist_is_open(self):
        self.repository.commits(1)
        code, _outputs, error = self.next_prerelease(alpha_checklist="open", beta_checklist="open")
        self.assertEqual(code, 1)
        self.assertIn("no alpha while the Alpha 1 checklist (#52) is open", error)

    def test_after_the_beta_1_checklist_the_suffix_is_beta(self):
        self.repository.commits(2)
        code, outputs, error = self.next_prerelease(
            alpha_checklist="closed", beta_checklist="closed"
        )
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["tag"], "v0.1.0-beta.1")

    def test_a_commit_that_already_has_a_pre_release_is_refused(self):
        self.repository.commits(2)
        self.repository.tag("v0.1.0-alpha.2")
        code, _outputs, error = self.next_prerelease(
            alpha_checklist="closed", beta_checklist="open"
        )
        self.assertEqual(code, 1)
        self.assertIn("already released as v0.1.0-alpha.2", error)

    def test_a_release_shaped_tag_outside_main_does_not_count(self):
        self.repository.git("checkout", "--quiet", "-b", "side")
        self.repository.commits(5)
        self.repository.tag("v0.1.0-alpha.5")
        self.repository.git("checkout", "--quiet", "main")
        self.repository.commits(2)
        code, outputs, error = self.next_prerelease(alpha_checklist="closed", beta_checklist="open")
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["tag"], "v0.1.0-alpha.1")

    def test_a_commit_outside_main_is_refused(self):
        self.repository.git("checkout", "--quiet", "-b", "side")
        self.repository.commits(1)
        code, _outputs, error = self.next_prerelease(
            alpha_checklist="closed", beta_checklist="open"
        )
        self.assertEqual(code, 1)
        self.assertIn("is not on main", error)

    def test_a_re_run_on_a_commit_older_than_the_newest_pre_release_is_refused(self):
        older_commit = self.repository.commits(1)
        self.repository.commits(3)
        self.repository.tag("v0.1.0-alpha.4")
        self.repository.git("checkout", "--quiet", "--detach", older_commit)
        code, _outputs, error = self.next_prerelease(
            alpha_checklist="closed", beta_checklist="open"
        )
        self.assertEqual(code, 1)
        self.assertIn("current main", error)

    def test_a_pre_release_not_above_the_newest_one_is_refused(self):
        self.repository.commits(2)
        self.repository.tag("v0.1.0-beta.1", "HEAD~1")
        code, _outputs, error = self.next_prerelease(
            alpha_checklist="closed", beta_checklist="open"
        )
        self.assertEqual(code, 1)
        self.assertIn("not above the newest pre-release v0.1.0-beta.1", error)

    def test_a_refused_stable_tag_left_on_main_is_not_the_base(self):
        self.repository.commits(2)
        self.repository.tag("v0.1.0")
        self.repository.commits(1)
        code, outputs, error = self.next_prerelease()
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["tag"], "v0.1.0-alpha.1")


class DryRunVersionTests(ReleaseTestCase):
    def test_a_dry_run_version_has_no_release_tag_shape(self):
        self.repository.commits(5)
        code, outputs, error = self.run_release("dry-run-version")
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["version"], "0.1.0-dev.5")
        self.assertEqual(outputs["notes_range"], f"{self.repository.bootstrap}..HEAD")
        self.assertEqual(outputs["notes_tag"], "v0.1.0-dev.5")
        self.assertIsNone(release.TAG_PATTERN.fullmatch(f"v{outputs['version']}"))

    def test_a_stable_tag_is_not_the_dry_run_base(self):
        self.repository.commits(2)
        self.repository.tag("v0.3.0")
        self.repository.commits(1)
        code, outputs, error = self.run_release("dry-run-version")
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["version"], "0.1.0-dev.3")


class StampTests(unittest.TestCase):
    def test_stamping_changes_only_the_workspace_version_line(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text(WORKSPACE_MANIFEST.format(version="0.1.0"))
            member = root / "crates/cockpit-cli/Cargo.toml"
            member.parent.mkdir(parents=True)
            member.write_text(MEMBER_MANIFEST.format(version="0.1.0"))
            code, outputs, error = run("stamp", "0.1.0-alpha.7", cwd=root)
            self.assertEqual(code, 0, error)
            self.assertEqual(outputs, {"version": "0.1.0-alpha.7", "previous": "0.1.0"})
            self.assertEqual(release.workspace_version(root), "0.1.0-alpha.7")
            self.assertEqual(member.read_text(), MEMBER_MANIFEST.format(version="0.1.0"))
            self.assertEqual(
                (root / "Cargo.toml").read_text(),
                WORKSPACE_MANIFEST.format(version="0.1.0-alpha.7"),
            )

    def test_stamping_leaves_member_manifests_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copy(ROOT / "Cargo.toml", root / "Cargo.toml")
            for member in ROOT.glob("crates/*/Cargo.toml"):
                copy = root / member.relative_to(ROOT)
                copy.parent.mkdir(parents=True)
                shutil.copy(member, copy)
            before = {p: p.read_bytes() for p in root.glob("crates/*/Cargo.toml")}
            code, _outputs, error = run("stamp", "9.9.9-alpha.1", cwd=root)
            self.assertEqual(code, 0, error)
            for member, contents in before.items():
                self.assertEqual(member.read_bytes(), contents, str(member))


# Stand-ins for the built binaries: the real ones are proven by the release workflow's
# install jobs, which run this same check on every platform's archive.
# `provider list` prints what the fake daemon found; `version --all` the bundle's manifest.
FAKE_COCKPIT = """#!/bin/sh
if [ "$1" = provider ]; then cat "$HOME/providers"; exit; fi
prefix=$(cd "$(dirname "$0")/.." && pwd)
if [ "$1" = tui ]; then shift; exec "$prefix/bin/torlek-tui" "$@"; fi
echo "torlek 0.1.0-alpha.7 (revision 0123abcd)"
echo "bundle manifest $prefix/share/torlek/manifest.json:"
cat "$prefix/share/torlek/manifest.json"
"""
# As the real daemon: a provider reports events when its `--version` answers and every
# bridge program beside the daemon it needs runs.
FAKE_DAEMON = """#!/bin/sh
programs=$(dirname "$0")
report() {
  provider=$1; shift
  "$provider" --version > /dev/null 2>&1 || return 0
  for bridge in "$@"; do
    if ! "$programs/$bridge" --version > /dev/null 2>&1; then
      printf '%s\\tresume\\tno-events\\n' "$provider" >> "$HOME/providers"
      return 0
    fi
  done
  printf '%s\\tresume\\tevents\\n' "$provider" >> "$HOME/providers"
}
: > "$HOME/providers"
report claude torlek-claude-hook
report codex torlek-codex-hook torlek-codex-session
report opencode torlek-opencode-bridge
echo "ready $HOME/.local/state/torlek/run/daemon.sock"
exec sleep 60
"""
# The provider each helper bridges.
HELPER_PROVIDERS = {
    "torlek-claude-hook": "claude",
    "torlek-codex-hook": "codex",
    "torlek-codex-session": "codex",
    "torlek-opencode-bridge": "opencode",
}
FAKE_HOOK_BRIDGE = """#!/bin/sh
exit 0
"""


class PackagingTestCase(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.binaries = self.root / "target"
        self.binaries.mkdir()
        self.write_fake("torlek", FAKE_COCKPIT)
        self.write_fake("torlek-daemon", FAKE_DAEMON)
        self.write_fake(
            "torlek-tui",
            '#!/bin/sh\n[ "$1" = --version ] || exit 2\necho "torlek-tui 0.1.0-alpha.7"\n',
        )
        for helper in release.HELPERS:
            self.write_fake(helper, FAKE_HOOK_BRIDGE)
        self.out = self.root / "dist"
        self.out.mkdir()

    def write_fake(self, name, script):
        fake = self.binaries / name
        fake.write_text(script)
        fake.chmod(0o755)

    def package(self, platform="linux-x86_64"):
        return run(
            "package",
            "--version",
            "0.1.0-alpha.7",
            "--channel",
            "alpha",
            "--platform",
            platform,
            "--revision",
            "0123abcd",
            "--binaries",
            str(self.binaries),
            "--out",
            str(self.out),
            cwd=ROOT,
        )


class PackageTests(PackagingTestCase):
    def test_alpha1_refuses_a_macos_payload_before_creating_an_archive(self):
        code, _, error = self.package(platform="macos-universal")
        self.assertEqual(code, 1)
        self.assertIn("Alpha1", error)
        self.assertEqual(list(self.out.iterdir()), [])

    def test_the_archive_follows_the_naming_contract_and_holds_the_bundle(self):
        code, outputs, error = self.package()
        self.assertEqual(code, 0, error)
        archive = Path(outputs["archive"])
        self.assertEqual(archive.name, "torlek-v0.1.0-alpha.7-linux-x86_64.tar.gz")
        with tarfile.open(archive) as bundle:
            names = set(bundle.getnames())
            members = bundle.getmembers()
            self.assertTrue(all(member.mtime == 0 for member in members))
            modes = {member.name.split("/", 1)[-1]: member.mode for member in members}
        for binary in release.BINARIES:
            self.assertEqual(modes[f"bin/{binary}"], 0o755)
        self.assertEqual(modes["README.md"], 0o644)
        prefix = "torlek-v0.1.0-alpha.7-linux-x86_64"
        self.assertLessEqual(
            {
                f"{prefix}/bin/torlek",
                f"{prefix}/bin/torlek-daemon",
                f"{prefix}/bin/torlek-tui",
                f"{prefix}/share/torlek/manifest.json",
                f"{prefix}/LICENSE",
                f"{prefix}/README.md",
            },
            names,
        )

    def test_the_bundle_readme_installs_the_bundle_s_own_version(self):
        code, outputs, error = self.package()
        self.assertEqual(code, 0, error)
        readme = (Path(outputs["root"]) / "README.md").read_text()
        self.assertEqual(re.findall(r"(?m)^\s*version=(.*)$", readme), ["0.1.0-alpha.7"])

    def test_a_readme_without_one_install_version_line_is_refused(self):
        for readme in ("# Torlek\n", "version=1\nversion=2\n"):
            with self.subTest(readme=readme), tempfile.TemporaryDirectory() as directory:
                source = Path(directory)
                (source / "LICENSE").write_text("license\n")
                (source / "README.md").write_text(readme)
                code, _, error = run(
                    "package",
                    "--version=0.1.0-alpha.7",
                    "--channel=alpha",
                    "--platform=linux-x86_64",
                    "--revision=0123abcd",
                    f"--binaries={self.binaries}",
                    f"--out={source / 'dist'}",
                    cwd=source,
                )
                self.assertEqual(code, 1)
                self.assertIn("the install step needs one", error)

    def test_the_manifest_names_version_revision_components_and_contents(self):
        code, outputs, error = self.package()
        self.assertEqual(code, 0, error)
        manifest = json.loads((Path(outputs["root"]) / release.MANIFEST_PATH).read_text())
        self.assertEqual(manifest["version"], "0.1.0-alpha.7")
        self.assertEqual(manifest["channel"], "alpha")
        self.assertEqual(manifest["source_revision"], "0123abcd")
        self.assertEqual(
            manifest["helpers"],
            {
                "torlek-claude-hook": "0.1.0-alpha.7",
                "torlek-codex-hook": "0.1.0-alpha.7",
                "torlek-codex-session": "0.1.0-alpha.7",
                "torlek-opencode-bridge": "0.1.0-alpha.7",
                "torlek-sandbox": "0.1.0-alpha.7",
            },
        )
        self.assertIsNone(manifest["instruction_bundle"])
        self.assertEqual(
            manifest["contents"]["bin/torlek"],
            release.sha256_of(self.binaries / "torlek"),
        )

    def test_the_gzip_header_carries_no_build_time(self):
        code, outputs, error = self.package()
        self.assertEqual(code, 0, error)
        header = Path(outputs["archive"]).read_bytes()[:10]
        self.assertEqual(header[4:8], b"\0\0\0\0")

    def test_packaging_twice_gives_the_same_bytes(self):
        code, first, error = self.package()
        self.assertEqual(code, 0, error)
        first_bytes = Path(first["archive"]).read_bytes()
        shutil.rmtree(first["root"])
        Path(first["archive"]).unlink()
        code, second, error = self.package()
        self.assertEqual(code, 0, error)
        self.assertEqual(Path(second["archive"]).read_bytes(), first_bytes)

    def test_an_unknown_platform_is_refused(self):
        code, _outputs, error = self.package(platform="windows-x86_64")
        self.assertEqual(code, 1)
        self.assertIn("not one of", error)

    def test_a_missing_binary_is_refused(self):
        for binary in release.BINARIES:
            with self.subTest(binary=binary):
                (self.binaries / binary).rename(self.root / binary)
                try:
                    code, _outputs, error = self.package()
                finally:
                    (self.root / binary).rename(self.binaries / binary)
                    shutil.rmtree(self.out)
                    self.out.mkdir()
                self.assertEqual(code, 1)
                self.assertIn(f"{binary}: built binary missing", error)


class VerifyInstallTests(PackagingTestCase):
    def verify(self, root, revision="0123abcd"):
        return run("verify-install", str(root), "--revision", revision, cwd=ROOT)

    def edit_manifest(self, root, change):
        manifest_file = Path(root) / release.MANIFEST_PATH
        manifest = json.loads(manifest_file.read_text())
        change(manifest)
        manifest_file.write_text(json.dumps(manifest))

    def test_an_unchanged_install_passes(self):
        _code, outputs, _error = self.package()
        code, verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 0, error)
        self.assertEqual(verified["version"], "0.1.0-alpha.7")

    def test_a_readme_that_installs_another_version_fails_the_install_check(self):
        _code, outputs, _error = self.package()
        root = Path(outputs["root"])
        readme = root / "README.md"
        readme.write_text(readme.read_text().replace("0.1.0-alpha.7", "0.1.0-alpha.1"))
        self.edit_manifest(
            root, lambda m: m["contents"].update({"README.md": release.sha256_of(readme)})
        )
        code, _verified, error = self.verify(root)
        self.assertEqual(code, 1)
        self.assertIn("names ['0.1.0-alpha.1'], the bundle is 0.1.0-alpha.7", error)

    def test_the_unpacked_archive_runs_its_tui(self):
        code, outputs, error = self.package()
        self.assertEqual(code, 0, error)
        unpacked = self.root / "unpacked"
        with tarfile.open(outputs["archive"]) as bundle:
            bundle.extractall(unpacked, filter="data")
        root = unpacked / Path(outputs["root"]).name
        code, _, error = self.verify(root)
        self.assertEqual(code, 0, error)
        # Even if both packaging and its manifest omit the TUI, executing the
        # installed CLI must detect the missing sibling.
        (root / "bin/torlek-tui").unlink(missing_ok=True)
        self.edit_manifest(root, lambda m: m["contents"].pop("bin/torlek-tui", None))
        with patch.object(
            release, "BINARIES", tuple(b for b in release.BINARIES if b != "torlek-tui")
        ):
            code, _, error = self.verify(root)
        self.assertEqual(code, 1)
        self.assertIn("torlek tui --version", error)

    def test_a_tui_that_cannot_run_fails_the_install_check(self):
        self.write_fake("torlek-tui", "#!/bin/sh\nexit 126\n")
        _, outputs, _ = self.package()
        code, _, error = self.verify(outputs["root"])
        self.assertEqual(code, 1)
        self.assertIn("torlek tui --version", error)

    def test_a_missing_or_changed_file_fails_naming_it(self):
        _code, outputs, _error = self.package()
        root = Path(outputs["root"])
        (root / "README.md").unlink()
        (root / "bin/torlek").write_text("changed")
        code, _verified, error = self.verify(root)
        self.assertEqual(code, 1)
        self.assertIn("README.md: missing", error)
        self.assertIn("bin/torlek: checksum differs", error)

    def test_an_install_without_a_bridge_fails_naming_it(self):
        for helper in release.HELPERS:
            with self.subTest(helper=helper):
                _code, outputs, _error = self.package()
                (Path(outputs["root"]) / f"bin/{helper}").unlink()
                code, _verified, error = self.verify(outputs["root"])
                shutil.rmtree(self.out)
                self.out.mkdir()
                self.assertEqual(code, 1)
                self.assertIn(f"bin/{helper}: missing", error)

    def test_a_manifest_that_does_not_list_a_bridge_fails(self):
        for helper in release.HELPERS:
            with self.subTest(helper=helper):
                _code, outputs, _error = self.package()
                self.edit_manifest(
                    outputs["root"],
                    lambda manifest, helper=helper: manifest["contents"].pop(f"bin/{helper}"),
                )
                (Path(outputs["root"]) / f"bin/{helper}").unlink()
                code, _verified, error = self.verify(outputs["root"])
                shutil.rmtree(self.out)
                self.out.mkdir()
                self.assertEqual(code, 1)
                self.assertIn(f"bin/{helper}: not listed in the manifest", error)

    def test_a_manifest_whose_helpers_record_is_wrong_fails(self):
        _code, outputs, _error = self.package()
        self.edit_manifest(outputs["root"], lambda manifest: manifest.update(helpers={}))
        code, _verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 1)
        self.assertIn(
            f"helpers: {{}}, expected {release.helper_versions('0.1.0-alpha.7')!r}", error
        )

    def test_a_file_the_manifest_does_not_list_fails_naming_it(self):
        _code, outputs, _error = self.package()
        (Path(outputs["root"]) / "bin/torlek.debug").write_text("stray")
        code, _verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 1)
        self.assertIn("bin/torlek.debug: not listed in the manifest", error)

    def test_a_daemon_whose_helper_keeps_its_pipes_open_does_not_hang_the_check(self):
        self.write_fake(
            "torlek-daemon",
            "#!/bin/sh\nfor provider in claude codex opencode; do\n"
            '  printf \'%s\\tresume\\tevents\\n\' "$provider" >> "$HOME/providers"\ndone\n'
            'sleep 600 &\necho "ready $HOME/daemon.sock"\nexec sleep 600\n',
        )
        _code, outputs, _error = self.package()
        code, _verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 0, error)

    def test_a_version_command_that_hangs_fails_instead_of_hanging_the_check(self):
        self.write_fake("torlek", "#!/bin/sh\nsleep 600 &\nexec sleep 600\n")
        _code, outputs, _error = self.package()
        with patch.object(release, "COMMAND_TIMEOUT_SECONDS", 1):
            code, _verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 1)
        self.assertIn("did not finish in 1s", error)

    def test_a_binary_built_from_another_revision_fails(self):
        _code, outputs, _error = self.package()
        code, _verified, error = self.verify(outputs["root"], revision="fedcba98")
        self.assertEqual(code, 1)
        self.assertIn("expected 'torlek 0.1.0-alpha.7 (revision fedcba98)'", error)

    def test_an_install_whose_bridge_cannot_run_fails_naming_its_provider(self):
        for helper, provider in HELPER_PROVIDERS.items():
            with self.subTest(helper=helper):
                self.write_fake(helper, "#!/bin/sh\nexit 126\n")
                _code, outputs, _error = self.package()
                code, _verified, error = self.verify(outputs["root"])
                self.write_fake(helper, FAKE_HOOK_BRIDGE)
                shutil.rmtree(self.out)
                self.out.mkdir()
                self.assertEqual(code, 1)
                self.assertIn(f"did not find a runnable bridge for {provider}:", error)
                self.assertIn(f"{provider}\\tresume\\tno-events", error)

    def test_a_provider_list_that_hangs_fails_with_the_daemon_log(self):
        # A FIFO nobody writes keeps the fake `torlek provider list` reading forever.
        self.write_fake(
            "torlek-daemon",
            "#!/bin/sh\nmkfifo \"$HOME/providers\"\necho 'stuck on the hook bridge' >&2\n"
            'echo "ready $HOME/daemon.sock"\nexec sleep 600\n',
        )
        _code, outputs, _error = self.package()
        with patch.object(release, "COMMAND_TIMEOUT_SECONDS", 1):
            code, _verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 1)
        self.assertIn("provider list did not finish in 1s", error)
        self.assertIn("stuck on the hook bridge", error)

    def test_the_daemon_is_started_with_providers_that_answer_their_version(self):
        _code, outputs, _error = self.package()
        with patch.object(release, "STAND_IN_PROVIDER", "#!/bin/sh\nexit 1\n"):
            code, _verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 1)
        self.assertIn("did not find a runnable bridge for claude, codex, opencode", error)

    def verify_in_scratch_parent(self, root, refusal=None):
        scratch_parent = self.root / "homes"
        scratch_parent.mkdir()
        with patch.object(release, "VERIFY_HOME_PARENT", str(scratch_parent)):
            if refusal is None:
                release.verify_install(root, "0123abcd")
            else:
                with self.assertRaisesRegex(release.Refusal, refusal):
                    release.verify_install(root, "0123abcd")
        return scratch_parent

    def write_daemon_recording_its_pid(self, ready_script):
        pid_file = self.root / "daemon.pid"
        self.write_fake("torlek-daemon", f'#!/bin/sh\necho $$ > "{pid_file}"\n{ready_script}')
        return pid_file

    def test_a_passing_check_removes_its_temporary_home(self):
        _code, outputs, _error = self.package()
        scratch_parent = self.verify_in_scratch_parent(outputs["root"])
        self.assertEqual(list(scratch_parent.iterdir()), [])

    def test_a_failing_cli_check_removes_its_temporary_home(self):
        self.write_fake("torlek", '#!/bin/sh\ntouch "$HOME/left-behind"\nexit 1\n')
        _code, outputs, _error = self.package()
        scratch_parent = self.verify_in_scratch_parent(
            outputs["root"], "torlek tui --version|torlek version --all"
        )
        self.assertEqual(list(scratch_parent.iterdir()), [])

    def test_a_failing_daemon_check_stops_the_daemon_and_removes_the_home(self):
        pid_file = self.write_daemon_recording_its_pid(
            'echo "ready $HOME/daemon.sock"\nexec sleep 600\n'
        )
        _code, outputs, _error = self.package()
        with patch.object(release, "STAND_IN_PROVIDER", "#!/bin/sh\nexit 1\n"):
            scratch_parent = self.verify_in_scratch_parent(
                outputs["root"], "did not find a runnable bridge"
            )
        self.assertEqual(list(scratch_parent.iterdir()), [])
        daemon_pid = int(pid_file.read_text())
        with self.assertRaises(ProcessLookupError):
            os.kill(daemon_pid, 0)

    def test_a_daemon_that_does_not_become_ready_fails(self):
        self.write_fake("torlek-daemon", "#!/bin/sh\necho 'no state dir' >&2\nexit 1\n")
        _code, outputs, _error = self.package()
        code, _verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 1)
        self.assertIn("did not print `ready <socket>`", error)
        self.assertIn("no state dir", error)


class GateGapTests(unittest.TestCase):
    def test_future_native_intel_install_and_current_compatibility_keep_their_wiring(self):
        workflow = RELEASE_WORKFLOW.read_text()
        install = workflow.split("\n  install:\n", 1)[1].split("\n  install-wsl2:\n", 1)[0]
        self.assertIn("matrix: ${{ fromJSON(needs.plan.outputs.install_matrix) }}", install)
        self.assertIn(
            'if [ "$NATIVE_RUNNER" = macos-26-intel ]; then test "$(uname -m)" = x86_64; fi',
            install,
        )
        self.assertEqual(
            release.GATE_CHECKS["clean install runs the macOS universal archive's x86_64 slice"][0],
            "install",
        )
        self.assertEqual(
            release.GATE_CHECKS["version-skew and interrupted-action fixtures"][0],
            "compatibility",
        )
        compatibility = re.search(r"\n  compatibility:\n((?:    [^\n]*\n|\n)*)", workflow)[1]
        self.assertIn("needs: [plan, build]", compatibility)
        self.assertIn("make release-compatibility", workflow)

    def test_beta_keeps_only_the_macos_restoration_gap(self):
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(io.StringIO()):
            code = release.main(["gate-gaps", "beta"], cwd=ROOT)
        self.assertEqual(code, 1)
        self.assertEqual(stdout.getvalue().count("\n"), 1, stdout.getvalue())
        self.assertIn("#568", stdout.getvalue())

    def test_an_unproven_gate_check_blocks_publishing_by_name(self):
        with patch.dict(release.GATE_CHECKS, {"unproven fixture gate": (None, ())}):
            error = io.StringIO()
            stdout = io.StringIO()
            with contextlib.redirect_stderr(error), contextlib.redirect_stdout(stdout):
                code = release.main(["gate-gaps", "beta"], cwd=ROOT)
            self.assertEqual(code, 1)
            self.assertIn("unproven fixture gate", stdout.getvalue())
            self.assertIn("publishing waits", error.getvalue())

    def test_a_beta_publishes_only_after_its_running_sessions_survived_the_upgrade(self):
        self.assertEqual(
            release.GATE_CHECKS["running sessions survive a previous-channel upgrade"],
            ("upgrade-sessions", ("alpha",)),
        )
        proven = {check: gate for check, gate in release.GATE_CHECKS.items() if gate[0]}
        jobs = {job for job, exempt in proven.values() if "beta" not in exempt}
        results = {job: {"result": "success"} for job in jobs | {"plan", "draft"}}
        with patch.dict(release.GATE_CHECKS, proven, clear=True):
            release.verify_gates("beta", json.dumps(results))
            results["upgrade-sessions"]["result"] = "failure"
            with self.assertRaisesRegex(release.Refusal, "did not succeed: upgrade-sessions"):
                release.verify_gates("beta", json.dumps(results))

    def test_the_running_session_job_upgrades_from_the_earlier_channel_release(self):
        workflow = RELEASE_WORKFLOW.read_text()
        job = re.search(r"\n  upgrade-sessions:\n((?:    [^\n]*\n|\n)*)", workflow)[1]
        self.assertIn("needs: [plan, build, upgrade-baseline]", job)
        self.assertIn("runs-on: ubuntu-26.04", job)
        self.assertIn("name: archive-linux-x86_64", job)
        self.assertIn("apt-get install --no-install-recommends --yes tmux", job)
        self.assertIn("python3 tools/release_upgrade.py sessions", job)
        self.assertIn("--downloads baseline/linux-x86_64", job)

    def test_an_alpha_is_not_blocked_by_the_upgrade_check(self):
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(io.StringIO()):
            release.main(["gate-gaps", "alpha"], cwd=ROOT)
        self.assertNotIn("upgrade", stdout.getvalue())
        self.assertEqual(stdout.getvalue(), "")

    def test_publishing_waits_for_every_gate_job_of_the_release_workflow(self):
        workflow = RELEASE_WORKFLOW.read_text()
        publish_needs = re.search(
            r"\n  publish:\n(?:    [^\n]*\n)*?    needs: \[([^\]]*)\]", workflow
        )
        self.assertIsNotNone(publish_needs, "expected the publish job's needs list")
        needed = {job.strip() for job in publish_needs[1].split(",")}
        for job in {job for job, _exempt in release.GATE_CHECKS.values() if job is not None}:
            with self.subTest(job=job):
                self.assertRegex(workflow, rf"\n  {job}:\n")
                self.assertIn(job, needed)


def pull_request_paths():
    paths = re.search(
        r"\n  pull_request:\n    paths:\n((?:      - [^\n]*\n)*)", RELEASE_WORKFLOW.read_text()
    )
    assert paths, "expected the pull_request trigger's paths"
    return re.findall(r"- (\S+)", paths[1])


class KnownGapsTests(unittest.TestCase):
    GAPS = "# Known gaps\n\nThis is an alpha.\n\n- **No macOS builds**\n  (#568, #562).\n"
    REPOSITORY = "Androlax2/agent-cockpit"

    def known_gaps(self, gaps, *options, closed=()):
        """release.py known-gaps on a throwaway file, with `gh` answering each issue's state."""

        def fake_gh(command, **_):
            self.assertEqual(command[:3], ["gh", "issue", "view"])
            self.assertEqual(command[4:], ["--repo", self.REPOSITORY, "--json", "state"])
            if command[3] == "404":
                return subprocess.CompletedProcess(command, 1, "", "not found")
            state = "CLOSED" if command[3] in closed else "OPEN"
            return subprocess.CompletedProcess(command, 0, json.dumps({"state": state}), "")

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "docs").mkdir()
            if gaps is not None:
                (root / release.KNOWN_GAPS).write_text(gaps)
            stdout, stderr = io.StringIO(), io.StringIO()
            with (
                contextlib.redirect_stdout(stdout),
                contextlib.redirect_stderr(stderr),
                patch.object(release.subprocess, "run", fake_gh),
            ):
                code = release.main(["known-gaps", *options], cwd=root)
            return code, stdout.getvalue(), stderr.getvalue()

    def test_the_notes_name_the_release_and_no_other(self):
        code, notes, error = self.known_gaps(
            self.GAPS, "--version", "0.1.0-alpha.7", "--open-issues-of", self.REPOSITORY
        )
        self.assertEqual(code, 0, error)
        self.assertEqual(notes, self.GAPS.replace("# Known gaps", "# Known gaps in 0.1.0-alpha.7"))
        self.assertEqual(release.PRERELEASE_VERSION_PATTERN.findall(notes), ["0.1.0-alpha.7"])

    def test_refused_gaps_name_the_rule(self):
        cases = {
            "a gap that links a closed issue": (self.GAPS, ("562",), "#562 is closed"),
            "an issue whose state cannot be read": (self.GAPS + "- **Lost** (#404).\n", (), "404"),
            "a gap without an issue": (self.GAPS + "- **Untracked.**\n", (), "links no issue"),
            "a hard-coded release": (
                self.GAPS + "\nFixes are planned for `v0.1.0-alpha.2`.\n",
                (),
                "names 0.1.0-alpha.2",
            ),
            "another heading": ("## Gaps\n", (), "first line must be"),
            "no file": (None, (), "docs/KNOWN-GAPS.md: missing"),
        }
        for name, (gaps, closed, refusal) in cases.items():
            with self.subTest(name):
                code, notes, error = self.known_gaps(
                    gaps,
                    "--version",
                    "0.1.0-alpha.7",
                    "--open-issues-of",
                    self.REPOSITORY,
                    closed=closed,
                )
                self.assertEqual(code, 1)
                self.assertEqual(notes, "")
                self.assertIn(refusal, error)

    def test_another_program_s_version_is_not_a_release(self):
        gaps = self.GAPS + "- **Needs tmux 3.5.1 on 127.0.0.1** (#568).\n"
        code, _, error = self.known_gaps(gaps, "--version", "0.1.0-alpha.7")
        self.assertEqual(code, 0, error)

    def test_a_tag_s_own_run_renders_without_reading_issue_states(self):
        code, notes, error = self.known_gaps(
            self.GAPS, "--version", "0.1.0-alpha.7", closed=("562", "568")
        )
        self.assertEqual(code, 0, error)
        self.assertIn("# Known gaps in 0.1.0-alpha.7\n", notes)

    def test_without_a_version_the_gaps_are_checked_and_nothing_is_printed(self):
        code, notes, error = self.known_gaps(self.GAPS, "--open-issues-of", self.REPOSITORY)
        self.assertEqual((code, notes), (0, ""), error)
        code, _, error = self.known_gaps(
            self.GAPS, "--open-issues-of", self.REPOSITORY, closed=("568",)
        )
        self.assertEqual(code, 1)
        self.assertIn("#568 is closed", error)

    def test_the_repository_s_gaps_name_no_release_and_link_issues(self):
        self.assertTrue(release.known_gaps("0.1.0-alpha.7", None, ROOT))

    def test_issue_states_are_read_before_a_tag_and_in_dry_runs_but_not_for_a_tag(self):
        prerelease = (ROOT / ".github/workflows/prerelease.yml").read_text()
        notes = RELEASE_WORKFLOW.read_text().split("\n  notes:\n", 1)[1].split("\n  build:\n", 1)[0]
        self.assertIn(
            'tools/release.py known-gaps --open-issues-of "$GITHUB_REPOSITORY"', prerelease
        )
        self.assertIn('if [ "$CHANNEL" = alpha ]; then issues=(); fi', notes)
        self.assertIn('tools/release.py known-gaps --version "$VERSION" "${issues[@]}"', notes)
        self.assertNotIn("cat docs/KNOWN-GAPS.md", notes)


class ReleaseTriggerTests(unittest.TestCase):
    def test_pull_requests_start_the_release_workflow_only_for_its_own_machinery(self):
        self.assertEqual(
            set(pull_request_paths()),
            {
                ".github/workflows/release.yml",
                ".github/workflows/prerelease.yml",
                "tools/release.py",
                "tools/tests/test_release.py",
                "tools/tests/test_release_plan.py",
                "cliff.toml",
                "tools/changelog.py",
                "Makefile",
                "tools/release_upgrade.py",
                "tools/tests/test_release_upgrade.py",
                "crates/cockpit-daemon/tests/candidate/mod.rs",
                "crates/cockpit-daemon/tests/common/mod.rs",
                "crates/cockpit-daemon/tests/real_daemon/mod.rs",
                "crates/cockpit-daemon/tests/latency.rs",
                "crates/cockpit-daemon/tests/latency/termination_evidence.rs",
                "crates/cockpit-daemon/tests/protocol.rs",
                "crates/cockpit-daemon/tests/restart_reattach.rs",
                "crates/cockpit-daemon/tests/session_restore_crash.rs",
                "crates/cockpit-turso/tests/migrations.rs",
                "packaging/aur/ci-install.sh",
                "packaging/aur/render.py",
                "packaging/aur/torlek-bin/PKGBUILD.in",
                "packaging/aur/torlek-bin/PKGBUILD",
                "packaging/aur/torlek-bin/.SRCINFO",
                "tools/tests/test_aur_package.py",
            },
        )

    def test_every_pull_request_path_exists(self):
        for path in pull_request_paths():
            with self.subTest(path=path):
                self.assertTrue((ROOT / path).is_file())

    def test_tags_the_nightly_and_a_dispatch_still_start_it(self):
        workflow = RELEASE_WORKFLOW.read_text()
        triggers = re.search(r"\non:\n((?:  [^\n]*\n|\n)*)", workflow)
        self.assertIsNotNone(triggers, "expected the on: block")
        self.assertEqual(
            set(re.findall(r"^  (\w+):", triggers[1], re.MULTILINE)),
            {"push", "pull_request", "schedule", "workflow_dispatch"},
        )
        self.assertRegex(triggers[1], r"tags: \[\s*[\"']v\*[\"']\s*\]")


class WorkflowMatrixTests(unittest.TestCase):
    def test_build_and_install_cover_every_platform_in_the_reviewed_plan(self):
        workflow = RELEASE_WORKFLOW.read_text()
        for channel in ("alpha", "beta"):
            code, outputs, error = run(
                "release-plan", "--version", f"0.1.0-{channel}.1", "--channel", channel, cwd=ROOT
            )
            self.assertEqual(code, 0, error)
            for job in ("build", "install"):
                with self.subTest(job=job, channel=channel):
                    body = re.search(rf"\n  {job}:\n((?:    [^\n]*\n|\n)*)", workflow)
                    self.assertIsNotNone(body, f"expected the {job} job")
                    self.assertIn(
                        "matrix: ${{ fromJSON(needs.plan.outputs." + job + "_matrix) }}", body[1]
                    )
                    listed = [
                        entry["platform"]
                        for entry in json.loads(outputs[job + "_matrix"])["include"]
                    ]
                    expected = list(
                        release.ALPHA1_PLATFORMS if channel == "alpha" else release.PLATFORMS
                    )
                    if job == "install" and channel == "beta":
                        expected.append("macos-universal")
                    self.assertEqual(sorted(listed), sorted(expected))

    def test_the_build_gathers_every_bundled_binary(self):
        workflow = RELEASE_WORKFLOW.read_text()
        built = re.search(r"cargo build --release --locked [^\n]*", workflow)
        self.assertIsNotNone(built, "expected the cargo build line")
        built_binaries = set()
        for package in re.findall(r"-p (\S+)", built[0]):
            manifest = tomllib.loads((ROOT / "crates" / package / "Cargo.toml").read_text())
            built_binaries.update(binary["name"] for binary in manifest.get("bin", []))
        self.assertLessEqual(set(release.BINARIES), built_binaries)
        gathered = re.search(r"\n\s*for binary in ([^;\n]*); do\n", workflow)
        self.assertIsNotNone(gathered, "expected the gather loop")
        self.assertEqual(gathered[1].split(), list(release.BINARIES))

    def test_the_daemon_looks_for_every_helper_beside_itself(self):
        daemon = (ROOT / "crates/cockpit-daemon/src/serve.rs").read_text()
        for helper in release.HELPERS:
            with self.subTest(helper=helper):
                self.assertIn(f'programs_dir.join("{helper}")', daemon)

    def test_every_archive_artifact_named_by_hand_is_a_platform(self):
        workflow = RELEASE_WORKFLOW.read_text()
        for platform in re.findall(r"\n\s*name: archive-([a-z0-9_-]+)\n", workflow):
            with self.subTest(platform=platform):
                self.assertIn(platform, release.PLATFORMS)


class ChecklistIssueTests(unittest.TestCase):
    def test_both_workflows_read_the_checklist_issues_release_py_names(self):
        for workflow in ("prerelease.yml", "release.yml"):
            text = (ROOT / ".github/workflows" / workflow).read_text()
            read_issues = re.findall(r"gh issue view (\d+) ", text)
            with self.subTest(workflow=workflow):
                self.assertEqual(
                    read_issues,
                    [str(release.ALPHA_CHECKLIST_ISSUE), str(release.BETA_CHECKLIST_ISSUE)],
                )


class ChangelogConfigTests(unittest.TestCase):
    def test_git_cliff_reads_exactly_the_release_tag_shapes(self):
        tag_pattern = re.compile(
            tomllib.loads((ROOT / "cliff.toml").read_text())["git"]["tag_pattern"]
        )
        for tag in [
            "v0.1.0",
            "v0.1.0-alpha.3",
            "v1.2.3-beta.10",
            "v0.1.0-rc.1",
            "nightly",
            "v0.1.0-alpha.0",
            "v01.0.0",
            "v0.1.0-dev.4",
        ]:
            with self.subTest(tag=tag):
                self.assertEqual(
                    tag_pattern.search(tag) is not None,
                    release.TAG_PATTERN.fullmatch(tag) is not None,
                )


class RepositoryBaseTests(unittest.TestCase):
    def test_the_bootstrap_commit_carries_the_first_pre_release_version(self):
        if release.git("rev-parse", "--is-shallow-repository", cwd=ROOT) == "true":
            self.skipTest("shallow clone: the bootstrap commit is not in this history")
        self.assertEqual(release.workspace_version(ROOT, release.BOOTSTRAP_COMMIT), "0.1.0")


if __name__ == "__main__":
    unittest.main()
