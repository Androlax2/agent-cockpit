import contextlib
import importlib.util
import io
import json
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
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

    def check_tag(self, tag, m3_gate="closed", m5_gate="open", main="main"):
        return self.run_release(
            "check-tag", tag, "--main", main, "--m3-gate", m3_gate, "--m5-gate", m5_gate
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
    def test_an_alpha_before_the_first_stable_counts_from_the_bootstrap_commit(self):
        revision = self.repository.commits(3)
        self.repository.tag("v0.1.0-alpha.3")
        code, outputs, error = self.check_tag("v0.1.0-alpha.3")
        self.assertEqual(code, 0, error)
        self.assertEqual(
            outputs,
            {
                "tag": "v0.1.0-alpha.3",
                "channel": "alpha",
                "version": "0.1.0-alpha.3",
                "prerelease": "true",
                "revision": revision,
                "notes_range": f"{self.repository.bootstrap}..v0.1.0-alpha.3",
            },
        )

    def test_a_pre_release_whose_number_is_not_the_commit_count_is_refused(self):
        self.repository.commits(3)
        self.repository.tag("v0.1.0-alpha.2")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.2")
        self.assertEqual(code, 1)
        self.assertIn("which is 3", error)

    def test_a_pre_release_on_a_bumped_workspace_is_refused(self):
        self.repository.write_version("0.2.0")
        self.repository.commit("chore: bump by hand")
        self.repository.tag("v0.1.0-alpha.1")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.1")
        self.assertEqual(code, 1)
        self.assertIn("must still carry the base's 0.1.0", error)

    def test_a_stable_tag_must_equal_the_workspace_version(self):
        self.repository.commits(1)
        self.repository.tag("v0.2.0")
        code, _outputs, error = self.check_tag("v0.2.0", m5_gate="closed")
        self.assertEqual(code, 1)
        self.assertIn("a stable tag must equal it", error)

    def test_a_stable_tag_reports_its_notes_from_the_previous_stable(self):
        self.repository.commits(1)
        self.repository.tag("v0.1.0")
        self.repository.commits(2)
        self.repository.write_version("0.2.0")
        revision = self.repository.commit("chore: release v0.2.0")
        self.repository.tag("v0.2.0")
        code, outputs, error = self.check_tag("v0.2.0", m5_gate="closed")
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["channel"], "stable")
        self.assertEqual(outputs["prerelease"], "false")
        self.assertEqual(outputs["revision"], revision)
        self.assertEqual(outputs["notes_range"], "v0.1.0..v0.2.0")
        self.assertEqual(outputs["notes_tag_pattern"], release.STABLE_TAG_REGEX)

    def test_pre_release_notes_start_at_the_highest_lower_release_it_descends_from(self):
        self.repository.commits(2)
        self.repository.tag("v0.1.0-alpha.2")
        self.repository.commits(3)
        self.repository.tag("v0.1.0-alpha.5")
        code, outputs, error = self.check_tag("v0.1.0-alpha.5")
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["notes_range"], "v0.1.0-alpha.2..v0.1.0-alpha.5")

    def test_a_beta_after_a_stable_counts_from_that_stable(self):
        self.repository.tag("v0.1.0")
        self.repository.commits(4)
        self.repository.tag("v0.2.0-beta.4")
        code, outputs, error = self.check_tag("v0.2.0-beta.4", m5_gate="closed")
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["version"], "0.2.0-beta.4")
        self.assertEqual(outputs["notes_range"], "v0.1.0..v0.2.0-beta.4")

    def test_a_pre_release_not_above_the_previous_stable_is_refused(self):
        self.repository.tag("v0.1.0")
        self.repository.commits(1)
        self.repository.tag("v0.1.0-beta.1")
        code, _outputs, error = self.check_tag("v0.1.0-beta.1", m5_gate="closed")
        self.assertEqual(code, 1)
        self.assertIn("not above the previous stable 0.1.0", error)

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
                "--m3-gate",
                "closed",
                "--m5-gate",
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
        code, _outputs, error = self.check_tag("v0.1.0-beta.1", m5_gate="closed")
        self.assertEqual(code, 1)
        self.assertIn("already released as v0.1.0-alpha.1", error)

    def test_a_tag_on_a_commit_outside_main_is_refused(self):
        self.repository.git("checkout", "--quiet", "-b", "feature")
        self.repository.commits(2)
        self.repository.tag("v0.1.0-alpha.2")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.2")
        self.assertEqual(code, 1)
        self.assertIn("is not on main", error)

    def test_an_alpha_tag_while_the_m3_gate_is_open_is_refused(self):
        self.repository.commits(1)
        self.repository.tag("v0.1.0-alpha.1")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.1", m3_gate="open")
        self.assertEqual(code, 1)
        self.assertIn("M3 gate issue (#52) is open", error)

    def test_a_tag_of_the_wrong_channel_for_the_gates_is_refused(self):
        self.repository.commits(1)
        self.repository.tag("v0.1.0-beta.1")
        code, _outputs, error = self.check_tag("v0.1.0-beta.1")
        self.assertEqual(code, 1)
        self.assertIn("the gates make this an alpha, not a beta", error)

    def test_a_pre_release_tag_below_the_newest_pre_release_is_refused(self):
        self.repository.commits(1)
        self.repository.tag("v0.1.0-beta.1")
        self.repository.commits(1)
        self.repository.tag("v0.1.0-alpha.2")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.2", m5_gate="open")
        self.assertEqual(code, 1)
        self.assertIn("not above the newest pre-release v0.1.0-beta.1", error)

    def test_a_stable_tag_while_the_m5_gate_is_open_is_refused(self):
        self.repository.tag("v0.1.0")
        code, _outputs, error = self.check_tag("v0.1.0")
        self.assertEqual(code, 1)
        self.assertIn("M5 gate issue (#66) is open", error)

    def test_a_stable_tag_not_above_the_previous_stable_is_refused(self):
        self.repository.tag("v0.2.0", self.repository.bootstrap)
        self.repository.commits(1)
        self.repository.tag("v0.1.0")
        code, _outputs, error = self.check_tag("v0.1.0", m5_gate="closed")
        self.assertEqual(code, 1)
        self.assertIn("not above the previous stable v0.2.0", error)

    def test_a_pre_release_that_does_not_descend_from_a_lower_one_is_refused(self):
        self.repository.commits(2)
        self.repository.tag("v0.1.0-beta.2")
        self.repository.commits(2)
        self.repository.tag("v0.1.0-alpha.4")
        code, _outputs, error = self.check_tag("v0.1.0-beta.2", m5_gate="closed")
        self.assertEqual(code, 1)
        self.assertIn("does not descend from v0.1.0-alpha.4", error)

    def test_a_stable_tag_that_does_not_descend_from_the_newest_pre_release_is_refused(self):
        stable_commit = self.repository.commits(1)
        self.repository.commits(1)
        self.repository.tag("v0.1.0-beta.2")
        self.repository.tag("v0.1.0", stable_commit)
        code, _outputs, error = self.check_tag("v0.1.0", m5_gate="closed")
        self.assertEqual(code, 1)
        self.assertIn("does not descend from v0.1.0-beta.2", error)

    def test_a_stable_tag_on_an_already_released_commit_is_refused(self):
        self.repository.commits(1)
        self.repository.tag("v0.1.0-beta.1")
        self.repository.tag("v0.1.0")
        code, _outputs, error = self.check_tag("v0.1.0", m5_gate="closed")
        self.assertEqual(code, 1)
        self.assertIn("already released as v0.1.0-beta.1", error)

    def test_a_release_shaped_tag_outside_main_does_not_count(self):
        self.repository.git("checkout", "--quiet", "-b", "side")
        self.repository.commits(5)
        self.repository.tag("v0.1.0-alpha.5")
        self.repository.git("checkout", "--quiet", "main")
        self.repository.commits(2)
        self.repository.tag("v0.1.0-alpha.2")
        code, _outputs, error = self.check_tag("v0.1.0-alpha.2")
        self.assertEqual(code, 0, error)


class NextPrereleaseTests(ReleaseTestCase):
    def test_after_the_m3_gate_the_next_alpha_numbers_the_commits_since_the_bootstrap(self):
        revision = self.repository.commits(7)
        code, outputs, error = self.run_release(
            "next-prerelease", "--main", "main", "--m3-gate", "closed", "--m5-gate", "open"
        )
        self.assertEqual(code, 0, error)
        self.assertEqual(
            outputs, {"tag": "v0.1.0-alpha.7", "channel": "alpha", "revision": revision}
        )

    def test_no_alpha_without_its_known_gaps(self):
        (self.repository.path / release.KNOWN_GAPS).unlink()
        self.repository.commit("docs: drop the known gaps")
        code, _outputs, error = self.run_release(
            "next-prerelease", "--main", "main", "--m3-gate", "closed", "--m5-gate", "open"
        )
        self.assertEqual(code, 1)
        self.assertIn("an alpha needs docs/KNOWN-GAPS.md", error)

    def test_no_alpha_while_the_m3_gate_is_open(self):
        self.repository.commits(1)
        code, _outputs, error = self.run_release(
            "next-prerelease", "--main", "main", "--m3-gate", "open", "--m5-gate", "open"
        )
        self.assertEqual(code, 1)
        self.assertIn("M3 gate issue (#52) is open", error)

    def test_after_the_m5_gate_the_suffix_is_beta(self):
        self.repository.commits(2)
        code, outputs, error = self.run_release(
            "next-prerelease", "--main", "main", "--m3-gate", "closed", "--m5-gate", "closed"
        )
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["tag"], "v0.1.0-beta.2")

    def test_a_commit_that_already_has_a_pre_release_is_refused(self):
        self.repository.commits(2)
        self.repository.tag("v0.1.0-alpha.2")
        code, _outputs, error = self.run_release(
            "next-prerelease", "--main", "main", "--m3-gate", "closed", "--m5-gate", "open"
        )
        self.assertEqual(code, 1)
        self.assertIn("already released as v0.1.0-alpha.2", error)

    def test_a_release_shaped_tag_outside_main_does_not_count(self):
        self.repository.git("checkout", "--quiet", "-b", "side")
        self.repository.commits(5)
        self.repository.tag("v0.1.0-alpha.5")
        self.repository.git("checkout", "--quiet", "main")
        self.repository.commits(2)
        code, outputs, error = self.run_release(
            "next-prerelease", "--main", "main", "--m3-gate", "closed", "--m5-gate", "open"
        )
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["tag"], "v0.1.0-alpha.2")

    def test_a_commit_outside_main_is_refused(self):
        self.repository.git("checkout", "--quiet", "-b", "side")
        self.repository.commits(1)
        code, _outputs, error = self.run_release(
            "next-prerelease", "--main", "main", "--m3-gate", "closed", "--m5-gate", "open"
        )
        self.assertEqual(code, 1)
        self.assertIn("is not on main", error)

    def test_a_re_run_on_a_commit_older_than_the_newest_pre_release_is_refused(self):
        older_commit = self.repository.commits(1)
        self.repository.commits(3)
        self.repository.tag("v0.1.0-alpha.4")
        self.repository.git("checkout", "--quiet", "--detach", older_commit)
        code, _outputs, error = self.run_release(
            "next-prerelease", "--main", "main", "--m3-gate", "closed", "--m5-gate", "open"
        )
        self.assertEqual(code, 1)
        self.assertIn("not above the newest pre-release v0.1.0-alpha.4", error)

    def test_a_pre_release_not_above_the_newest_one_is_refused(self):
        self.repository.commits(2)
        self.repository.tag("v0.1.0-beta.1", "HEAD~1")
        code, _outputs, error = self.run_release(
            "next-prerelease", "--main", "main", "--m3-gate", "closed", "--m5-gate", "open"
        )
        self.assertEqual(code, 1)
        self.assertIn("v0.1.0-alpha.2 is not above the newest pre-release v0.1.0-beta.1", error)

    def test_after_a_stable_tag_the_release_pr_version_is_required(self):
        self.repository.tag("v0.1.0")
        self.repository.commits(1)
        code, _outputs, error = self.run_release(
            "next-prerelease", "--main", "main", "--m3-gate", "closed", "--m5-gate", "closed"
        )
        self.assertEqual(code, 1)
        self.assertIn("release PR's version (#101)", error)


class DryRunVersionTests(ReleaseTestCase):
    def test_a_dry_run_version_has_no_release_tag_shape(self):
        self.repository.commits(5)
        code, outputs, error = self.run_release("dry-run-version", "--main", "main")
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["version"], "0.1.0-dev.5")
        self.assertEqual(outputs["notes_range"], f"{self.repository.bootstrap}..HEAD")
        self.assertEqual(outputs["notes_tag"], "v0.1.0-dev.5")
        self.assertIsNone(release.TAG_PATTERN.fullmatch(f"v{outputs['version']}"))

    def test_a_release_shaped_tag_outside_main_is_not_the_dry_run_base(self):
        self.repository.git("checkout", "--quiet", "-b", "side")
        self.repository.commits(2)
        self.repository.tag("v0.3.0")
        self.repository.commits(1)
        code, outputs, error = self.run_release("dry-run-version", "--main", "main")
        self.assertEqual(code, 0, error)
        self.assertEqual(outputs["version"], "0.1.0-dev.3")


class StampTests(unittest.TestCase):
    def test_stamping_moves_the_workspace_version_and_internal_pins_only(self):
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
            stamped = member.read_text()
            self.assertIn('path = "../cockpit-client", version = "0.1.0-alpha.7"', stamped)
            self.assertIn('serde = { version = "0.1.0" }', stamped)

    def test_every_internal_pin_in_this_workspace_is_stamped(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copy(ROOT / "Cargo.toml", root / "Cargo.toml")
            for member in ROOT.glob("crates/*/Cargo.toml"):
                copy = root / member.relative_to(ROOT)
                copy.parent.mkdir(parents=True)
                shutil.copy(member, copy)
            code, _outputs, error = run("stamp", "9.9.9-alpha.1", cwd=root)
            self.assertEqual(code, 0, error)
            for member in root.glob("crates/*/Cargo.toml"):
                for table in re.findall(r"\{[^}\n]*\bpath = [^}\n]*\}", member.read_text()):
                    with self.subTest(member=member.parent.name, dependency=table):
                        pin = re.search(r'\bversion = "([^"]*)"', table)
                        self.assertTrue(pin is None or pin[1] == "9.9.9-alpha.1")


# Stand-ins for the built binaries: the real ones are proven by the release workflow's
# install jobs, which run this same check on every platform's archive.
FAKE_COCKPIT = """#!/bin/sh
prefix=$(cd "$(dirname "$0")/.." && pwd)
echo "cockpit 0.1.0-alpha.7 (revision 0123abcd)"
echo "bundle manifest $prefix/share/cockpit/manifest.json:"
cat "$prefix/share/cockpit/manifest.json"
"""
FAKE_DAEMON = """#!/bin/sh
echo "ready $HOME/.local/state/cockpit/run/daemon.sock"
exec sleep 60
"""


class PackagingTestCase(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.binaries = self.root / "target"
        self.binaries.mkdir()
        self.write_fake("cockpit", FAKE_COCKPIT)
        self.write_fake("cockpit-daemon", FAKE_DAEMON)
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
    def test_the_archive_follows_the_naming_contract_and_holds_the_bundle(self):
        code, outputs, error = self.package()
        self.assertEqual(code, 0, error)
        archive = Path(outputs["archive"])
        self.assertEqual(archive.name, "cockpit-v0.1.0-alpha.7-linux-x86_64.tar.gz")
        with tarfile.open(archive) as bundle:
            names = set(bundle.getnames())
            members = bundle.getmembers()
            self.assertTrue(all(member.mtime == 0 for member in members))
            modes = {member.name.split("/", 1)[-1]: member.mode for member in members}
        self.assertEqual(modes["bin/cockpit"], 0o755)
        self.assertEqual(modes["README.md"], 0o644)
        prefix = "cockpit-v0.1.0-alpha.7-linux-x86_64"
        self.assertLessEqual(
            {
                f"{prefix}/bin/cockpit",
                f"{prefix}/bin/cockpit-daemon",
                f"{prefix}/share/cockpit/manifest.json",
                f"{prefix}/LICENSE",
                f"{prefix}/README.md",
            },
            names,
        )

    def test_the_manifest_names_version_revision_components_and_contents(self):
        code, outputs, error = self.package()
        self.assertEqual(code, 0, error)
        manifest = json.loads((Path(outputs["root"]) / release.MANIFEST_PATH).read_text())
        self.assertEqual(manifest["version"], "0.1.0-alpha.7")
        self.assertEqual(manifest["channel"], "alpha")
        self.assertEqual(manifest["source_revision"], "0123abcd")
        self.assertEqual(manifest["helpers"], {})
        self.assertIsNone(manifest["instruction_bundle"])
        self.assertEqual(
            manifest["contents"]["bin/cockpit"],
            release.sha256_of(self.binaries / "cockpit"),
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
        (self.binaries / "cockpit-daemon").unlink()
        code, _outputs, error = self.package()
        self.assertEqual(code, 1)
        self.assertIn("built binary missing", error)


class VerifyInstallTests(PackagingTestCase):
    def verify(self, root, revision="0123abcd"):
        return run("verify-install", str(root), "--revision", revision, cwd=ROOT)

    def test_an_unchanged_install_passes(self):
        _code, outputs, _error = self.package()
        code, verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 0, error)
        self.assertEqual(verified["version"], "0.1.0-alpha.7")

    def test_a_missing_or_changed_file_fails_naming_it(self):
        _code, outputs, _error = self.package()
        root = Path(outputs["root"])
        (root / "README.md").unlink()
        (root / "bin/cockpit").write_text("changed")
        code, _verified, error = self.verify(root)
        self.assertEqual(code, 1)
        self.assertIn("README.md: missing", error)
        self.assertIn("bin/cockpit: checksum differs", error)

    def test_a_file_the_manifest_does_not_list_fails_naming_it(self):
        _code, outputs, _error = self.package()
        (Path(outputs["root"]) / "bin/cockpit.debug").write_text("stray")
        code, _verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 1)
        self.assertIn("bin/cockpit.debug: not listed in the manifest", error)

    def test_a_daemon_whose_helper_keeps_its_pipes_open_does_not_hang_the_check(self):
        self.write_fake(
            "cockpit-daemon",
            '#!/bin/sh\nsleep 600 &\necho "ready $HOME/daemon.sock"\nexec sleep 600\n',
        )
        _code, outputs, _error = self.package()
        code, _verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 0, error)

    def test_a_version_command_that_hangs_fails_instead_of_hanging_the_check(self):
        self.write_fake("cockpit", "#!/bin/sh\nsleep 600 &\nexec sleep 600\n")
        _code, outputs, _error = self.package()
        with patch.object(release, "COMMAND_TIMEOUT_SECONDS", 1):
            code, _verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 1)
        self.assertIn("did not finish in 1s", error)

    def test_a_binary_built_from_another_revision_fails(self):
        _code, outputs, _error = self.package()
        code, _verified, error = self.verify(outputs["root"], revision="fedcba98")
        self.assertEqual(code, 1)
        self.assertIn("expected 'cockpit 0.1.0-alpha.7 (revision fedcba98)'", error)

    def test_a_daemon_that_does_not_become_ready_fails(self):
        self.write_fake("cockpit-daemon", "#!/bin/sh\necho 'no state dir' >&2\nexit 1\n")
        _code, outputs, _error = self.package()
        code, _verified, error = self.verify(outputs["root"])
        self.assertEqual(code, 1)
        self.assertIn("did not print `ready <socket>`", error)
        self.assertIn("no state dir", error)


class GateGapTests(unittest.TestCase):
    def test_an_unproven_gate_check_blocks_publishing_by_name(self):
        code, _outputs, error = run("gate-gaps", "beta", cwd=ROOT)
        self.assertEqual(code, 1)
        self.assertIn("publishing waits", error)

    def test_an_alpha_is_not_blocked_by_the_upgrade_check(self):
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(io.StringIO()):
            release.main(["gate-gaps", "alpha"], cwd=ROOT)
        self.assertNotIn("upgrade", stdout.getvalue())
        self.assertIn("gap: sustained responsiveness run", stdout.getvalue())

    def test_publishing_waits_for_every_gate_job_of_the_release_workflow(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        publish_needs = re.search(
            r"\n  publish:\n(?:    [^\n]*\n)*?    needs: \[([^\]]*)\]", workflow
        )
        self.assertIsNotNone(publish_needs, "expected the publish job's needs list")
        needed = {job.strip() for job in publish_needs[1].split(",")}
        for job in {job for job, _exempt in release.GATE_CHECKS.values() if job is not None}:
            with self.subTest(job=job):
                self.assertRegex(workflow, rf"\n  {job}:\n")
                self.assertIn(job, needed)


class WorkflowMatrixTests(unittest.TestCase):
    def test_build_and_install_cover_every_platform(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        for job in ("build", "install"):
            with self.subTest(job=job):
                body = re.search(rf"\n  {job}:\n((?:    [^\n]*\n|\n)*)", workflow)
                self.assertIsNotNone(body, f"expected the {job} job")
                listed = re.findall(r"- platform: (\S+)", body[1])
                self.assertEqual(sorted(listed), sorted(release.PLATFORMS))

    def test_every_archive_artifact_named_by_hand_is_a_platform(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        for platform in re.findall(r"\n\s*name: archive-([a-z0-9_-]+)\n", workflow):
            with self.subTest(platform=platform):
                self.assertIn(platform, release.PLATFORMS)


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

    def test_stable_notes_take_only_stable_tags_as_boundaries(self):
        stable_pattern = re.compile(release.STABLE_TAG_REGEX)
        for tag in ["v0.1.0", "v1.20.3", "v0.1.0-alpha.3", "v0.1.0-beta.1", "v01.0.0", "nightly"]:
            with self.subTest(tag=tag):
                parsed = release.TAG_PATTERN.fullmatch(tag)
                is_stable = parsed is not None and parsed["channel"] is None
                self.assertEqual(stable_pattern.search(tag) is not None, is_stable)


class RepositoryBaseTests(unittest.TestCase):
    def test_the_bootstrap_commit_carries_the_first_pre_release_version(self):
        if release.git("rev-parse", "--is-shallow-repository", cwd=ROOT) == "true":
            self.skipTest("shallow clone: the bootstrap commit is not in this history")
        self.assertEqual(release.workspace_version(ROOT, release.BOOTSTRAP_COMMIT), "0.1.0")


if __name__ == "__main__":
    unittest.main()
