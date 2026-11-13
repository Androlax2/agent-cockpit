import gzip
import importlib.util
import io
import json
import lzma
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "packaging/deb/build.py"
REPOSITORY = ROOT / "packaging/deb/repository.py"
PUBLISH = ROOT / "packaging/deb/publish.sh"
CHECK_SITE = ROOT / "packaging/deb/check-site.sh"
TAG = "0.1.0-alpha.1"
VERSION = "0.1.0~alpha.1-1"
REVISION = "b" * 40
MAINTAINER = "Torlek test <test@example.invalid>"
RELEASE_DATE = "2026-11-03"


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


sys.path.insert(0, str(ROOT / "tools"))
release = load("release", ROOT / "tools/release.py")
build = load("deb_build", BUILD)
repository = load("deb_repository", REPOSITORY)
HAS_DPKG = shutil.which("dpkg") is not None
HAS_DPKG_DEB = shutil.which("dpkg-deb") is not None
HAS_AR = shutil.which("ar") is not None
HAS_GPG = shutil.which("gpg") is not None and shutil.which("gpgv") is not None
HAS_GIT = shutil.which("git") is not None


def dpkg_order(character):
    if character.isdigit():
        return 0
    if character.isalpha():
        return ord(character)
    if character == "~":
        return -1
    return ord(character) + 256 if character else 0


def dpkg_compare_part(left, right):
    """dpkg's verrevcmp (lib/dpkg/version.c), so the order is checked where dpkg is missing."""
    left_index = right_index = 0
    while left_index < len(left) or right_index < len(right):
        while (left_index < len(left) and not left[left_index].isdigit()) or (
            right_index < len(right) and not right[right_index].isdigit()
        ):
            left_order = dpkg_order(left[left_index] if left_index < len(left) else "")
            right_order = dpkg_order(right[right_index] if right_index < len(right) else "")
            if left_order != right_order:
                return left_order - right_order
            left_index += 1
            right_index += 1
        left_digits = right_digits = ""
        while left_index < len(left) and left[left_index].isdigit():
            left_digits += left[left_index]
            left_index += 1
        while right_index < len(right) and right[right_index].isdigit():
            right_digits += right[right_index]
            right_index += 1
        if int(left_digits or 0) != int(right_digits or 0):
            return int(left_digits or 0) - int(right_digits or 0)
    return 0


def dpkg_compare(left, right):
    """Versions without an epoch: the upstream part, then the Debian revision."""
    left_upstream, _, left_revision = left.rpartition("-") if "-" in left else (left, "", "")
    right_upstream, _, right_revision = right.rpartition("-") if "-" in right else (right, "", "")
    return dpkg_compare_part(left_upstream, right_upstream) or dpkg_compare_part(
        left_revision, right_revision
    )


def sums_for(digests):
    return "".join(f"{digest}  {name}\n" for name, digest in digests.items())


def stand_in_binaries(folder):
    binaries = folder / "binaries"
    binaries.mkdir(parents=True)
    for binary in release.BINARIES:
        (binaries / binary).write_text(f"#!/bin/sh\necho {binary}\n")
    return binaries


def release_fixture(folder, tag=TAG, channel="alpha"):
    """One release's two Linux archives from release.py and their SHA256SUMS."""
    binaries = stand_in_binaries(folder / f"stand-ins-{tag}")
    archives = {}
    for platform in build.ARCHITECTURES.values():
        out = folder / f"staging-{tag}-{platform}"
        out.mkdir(parents=True)
        bundle = release.package(tag, channel, platform, REVISION, binaries, out, ROOT)
        archive = folder / Path(bundle["archive"]).name
        shutil.copyfile(bundle["archive"], archive)
        archives[platform] = archive
    sums = folder / f"SHA256SUMS-{tag}"
    sums.write_text(sums_for({path.name: release.sha256_of(path) for path in archives.values()}))
    return archives, sums


def repack(source_archive, target_archive, change):
    """A copy of a release archive whose unpacked tree `change` edited first."""
    with tempfile.TemporaryDirectory() as folder:
        with tarfile.open(source_archive) as archive:
            archive.extractall(folder, filter="tar")
        (top_directory,) = Path(folder).iterdir()
        change(top_directory)
        with tarfile.open(target_archive, "w:gz") as archive:
            archive.add(top_directory, arcname=top_directory.name)


def run_tool(tool, *arguments, environment=None):
    return subprocess.run(
        [sys.executable, str(tool), *arguments],
        capture_output=True,
        text=True,
        check=False,
        env=environment,
    )


def run_build(archive, sums, out, *extra, tag=TAG, maintainer=MAINTAINER):
    return run_tool(
        BUILD,
        "--tag",
        tag,
        "--sums",
        str(sums),
        "--archive",
        str(archive),
        "--maintainer",
        maintainer,
        "--date",
        RELEASE_DATE,
        "--out",
        str(out),
        *extra,
    )


def tar_members(payload):
    with tarfile.open(fileobj=io.BytesIO(lzma.decompress(payload))) as archive:
        return {member.name: member for member in archive.getmembers()}


class DebianVersionTest(unittest.TestCase):
    def test_prerelease_tags_put_a_tilde_before_their_channel(self):
        for tag, expected in [
            ("0.1.0-alpha.1", "0.1.0~alpha.1-1"),
            ("0.1.0-alpha.12", "0.1.0~alpha.12-1"),
            ("0.2.3-beta.1", "0.2.3~beta.1-1"),
        ]:
            with self.subTest(tag=tag):
                self.assertEqual(build.debian_version(tag), expected)

    def test_a_packaging_only_rebuild_raises_the_debian_revision(self):
        self.assertEqual(build.debian_version(TAG, 2), "0.1.0~alpha.1-2")

    def test_refuses_a_debian_revision_below_one(self):
        with self.assertRaisesRegex(build.Refusal, "starts at 1"):
            build.debian_version(TAG, 0)

    def test_refuses_tags_outside_the_release_grammar(self):
        for tag in ["v0.1.0-alpha.1", "0.1.0-dev.3", "0.1.0-alpha.0", "01.0.0-alpha.1"]:
            with self.subTest(tag=tag), self.assertRaisesRegex(build.Refusal, "not a release tag"):
                build.debian_version(tag)

    def test_refuses_a_stable_tag_until_one_exists(self):
        with self.assertRaisesRegex(build.Refusal, "no stable release"):
            build.debian_version("0.1.0")


ORDERED_VERSIONS = [
    *(
        build.debian_version(tag)
        for tag in ["0.1.0-alpha.1", "0.1.0-alpha.2", "0.1.0-alpha.10", "0.1.0-beta.1"]
    ),
    build.debian_version("0.1.0-beta.1", 2),
    "0.1.0-1",
    "0.1.1~alpha.1-1",
]
# Ubuntu 26.04's and trixie-backports' tmux satisfy `tmux (>= 3.6)`; Ubuntu 24.04's, trixie's
# and a release candidate do not (packages.ubuntu.com and sources.debian.org, 2026-11-03).
TMUX_ACCEPTED = ["3.6a-2ubuntu0.2", "3.7c-1~bpo13+1", "3.8-1"]
TMUX_REFUSED = ["3.4-1ubuntu0.2", "3.5a-3", "3.6~rc-1"]


class DpkgOrderTest(unittest.TestCase):
    def test_alpha_before_beta_before_the_stable_core(self):
        for older, newer in zip(ORDERED_VERSIONS, ORDERED_VERSIONS[1:], strict=False):
            with self.subTest(older=older, newer=newer):
                self.assertLess(dpkg_compare(older, newer), 0)
                self.assertGreater(dpkg_compare(newer, older), 0)

    def test_tmux_dependency_accepts_3_6_and_later(self):
        for version in TMUX_ACCEPTED:
            with self.subTest(version=version):
                self.assertGreaterEqual(dpkg_compare(version, "3.6"), 0)

    def test_tmux_dependency_refuses_earlier_releases(self):
        for version in TMUX_REFUSED:
            with self.subTest(version=version):
                self.assertLess(dpkg_compare(version, "3.6"), 0)

    @unittest.skipUnless(HAS_DPKG, "dpkg is not installed")
    def test_dpkg_agrees_with_the_order(self):
        pairs = list(zip(ORDERED_VERSIONS, ORDERED_VERSIONS[1:], strict=False))
        pairs += [("3.6", version) for version in TMUX_ACCEPTED]
        pairs += [(version, "3.6") for version in TMUX_REFUSED]
        for older, newer in pairs:
            with self.subTest(older=older, newer=newer):
                result = subprocess.run(["dpkg", "--compare-versions", older, "le", newer])
                self.assertEqual(result.returncode, 0)


class ReleaseFixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.folder = tempfile.TemporaryDirectory()
        cls.work = Path(cls.folder.name)
        cls.archives, cls.sums = release_fixture(cls.work)
        cls.amd64_archive = cls.archives["linux-x86_64"]

    @classmethod
    def tearDownClass(cls):
        cls.folder.cleanup()

    def out(self, name):
        return self.work / "out" / self.id().rsplit(".", 1)[-1] / name


class ArchiveRefusalTest(ReleaseFixture):
    def assertRefused(self, result, message):
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(message, result.stderr)

    def test_refuses_an_archive_changed_after_its_checksum(self):
        tampered = self.out("upload") / self.amd64_archive.name
        tampered.parent.mkdir(parents=True)
        tampered.write_bytes(self.amd64_archive.read_bytes() + b"\0")
        result = run_build(tampered, self.sums, self.out("debs"))
        self.assertRefused(result, "does not match SHA256SUMS")
        self.assertFalse(self.out("debs").exists())

    def test_refuses_sums_of_another_release(self):
        sums = self.out("SHA256SUMS")
        sums.parent.mkdir(parents=True)
        sums.write_text(self.sums.read_text().replace(TAG, "0.1.0-alpha.2"))
        result = run_build(self.amd64_archive, sums, self.out("debs"))
        self.assertRefused(result, "is it the v0.1.0-alpha.1 release's file?")

    def test_refuses_an_archive_this_release_does_not_name(self):
        renamed = self.out("upload") / "torlek-v0.1.0-alpha.1-macos-universal.tar.gz"
        renamed.parent.mkdir(parents=True)
        shutil.copyfile(self.amd64_archive, renamed)
        result = run_build(renamed, self.sums, self.out("debs"))
        self.assertRefused(result, "not one of the v0.1.0-alpha.1 release's Linux archives")

    def test_refuses_an_archive_listed_twice(self):
        sums = self.out("SHA256SUMS")
        sums.parent.mkdir(parents=True)
        sums.write_text(self.sums.read_text() + f"{'4' * 64}  {self.amd64_archive.name}\n")
        self.assertRefused(run_build(self.amd64_archive, sums, self.out("debs")), "listed twice")

    def test_refuses_a_malformed_sums_line(self):
        sums = self.out("SHA256SUMS")
        sums.parent.mkdir(parents=True)
        sums.write_text("SKIP  torlek.tar.gz\n" + self.sums.read_text())
        self.assertRefused(run_build(self.amd64_archive, sums, self.out("debs")), "line 1")

    def resummed(self, archive):
        sums = archive.parent / "SHA256SUMS"
        sums.write_text(
            self.sums.read_text().replace(
                release.sha256_of(self.amd64_archive), release.sha256_of(archive)
            )
        )
        return sums

    def test_refuses_a_bundle_changed_before_it_was_summed(self):
        def replace_the_cli(bundle):
            (bundle / "bin/torlek").write_text("#!/bin/sh\necho replaced\n")

        tampered = self.out("upload") / self.amd64_archive.name
        tampered.parent.mkdir(parents=True)
        repack(self.amd64_archive, tampered, replace_the_cli)
        result = run_build(tampered, self.resummed(tampered), self.out("debs"))
        self.assertRefused(result, "bin/torlek: checksum differs from the manifest")

    def test_refuses_a_file_the_manifest_does_not_list(self):
        def add_a_program(bundle):
            (bundle / "bin/extra").write_text("#!/bin/sh\n")

        tampered = self.out("upload") / self.amd64_archive.name
        tampered.parent.mkdir(parents=True)
        repack(self.amd64_archive, tampered, add_a_program)
        result = run_build(tampered, self.resummed(tampered), self.out("debs"))
        self.assertRefused(result, "bin/extra: not listed in the manifest")

    def test_refuses_a_file_the_manifest_lists_but_the_archive_lacks(self):
        def drop_the_tui(bundle):
            (bundle / "bin/torlek-tui").unlink()

        tampered = self.out("upload") / self.amd64_archive.name
        tampered.parent.mkdir(parents=True)
        repack(self.amd64_archive, tampered, drop_the_tui)
        result = run_build(tampered, self.resummed(tampered), self.out("debs"))
        self.assertRefused(result, "bin/torlek-tui: missing")

    def test_packages_an_earlier_release_without_a_program_added_since(self):
        def predate_the_sandbox_launcher(bundle):
            (bundle / "bin/torlek-sandbox").unlink()
            manifest_file = bundle / release.MANIFEST_PATH
            manifest = json.loads(manifest_file.read_text())
            del manifest["contents"]["bin/torlek-sandbox"]
            del manifest["helpers"]["torlek-sandbox"]
            manifest_file.write_text(json.dumps(manifest))

        earlier = self.out("upload") / self.amd64_archive.name
        earlier.parent.mkdir(parents=True)
        repack(self.amd64_archive, earlier, predate_the_sandbox_launcher)
        result = run_build(earlier, self.resummed(earlier), self.out("debs"))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_an_earlier_release_without_the_masks_units_installs_none_and_runs_nothing(self):
        def predate_the_masks_units(bundle):
            shutil.rmtree(bundle / release.SYSTEMD_UNITS)
            manifest_file = bundle / release.MANIFEST_PATH
            manifest = json.loads(manifest_file.read_text())
            for relative in list(manifest["contents"]):
                if relative.startswith(f"{release.SYSTEMD_UNITS}/"):
                    del manifest["contents"][relative]
            manifest_file.write_text(json.dumps(manifest))

        earlier = self.out("upload") / self.amd64_archive.name
        earlier.parent.mkdir(parents=True)
        repack(self.amd64_archive, earlier, predate_the_masks_units)
        result = run_build(earlier, self.resummed(earlier), self.out("debs"))
        self.assertEqual(result.returncode, 0, result.stderr)
        members = build.deb_members((self.out("debs") / f"torlek_{VERSION}_amd64.deb").read_bytes())
        self.assertEqual(
            sorted(tar_members(members["control.tar.xz"])), [".", "./control", "./md5sums"]
        )
        installed = tar_members(members["data.tar.xz"])
        self.assertFalse([name for name in installed if "/systemd/" in name])

    def test_refuses_a_helper_another_version_built(self):
        def age_the_codex_hook(bundle):
            manifest_file = bundle / release.MANIFEST_PATH
            manifest = json.loads(manifest_file.read_text())
            manifest["helpers"]["torlek-codex-hook"] = "0.0.9"
            manifest_file.write_text(json.dumps(manifest))

        tampered = self.out("upload") / self.amd64_archive.name
        tampered.parent.mkdir(parents=True)
        repack(self.amd64_archive, tampered, age_the_codex_hook)
        result = run_build(tampered, self.resummed(tampered), self.out("debs"))
        self.assertRefused(result, "helpers: torlek-codex-hook is '0.0.9'")

    def test_refuses_another_versions_bundle_under_this_name(self):
        def claim_another_version(bundle):
            manifest_file = bundle / release.MANIFEST_PATH
            manifest = json.loads(manifest_file.read_text())
            manifest["version"] = "0.1.0-alpha.2"
            manifest_file.write_text(json.dumps(manifest))

        tampered = self.out("upload") / self.amd64_archive.name
        tampered.parent.mkdir(parents=True)
        repack(self.amd64_archive, tampered, claim_another_version)
        result = run_build(tampered, self.resummed(tampered), self.out("debs"))
        self.assertRefused(result, "version is '0.1.0-alpha.2', expected '0.1.0-alpha.1'")

    def test_refuses_the_other_architectures_bundle_under_this_name(self):
        renamed = self.out("upload") / self.amd64_archive.name
        renamed.parent.mkdir(parents=True)

        def swap_in_aarch64(bundle):
            shutil.rmtree(bundle)
            with tarfile.open(self.archives["linux-aarch64"]) as archive:
                archive.extractall(bundle.parent, filter="tar")
            (bundle.parent / release.archive_name(TAG, "linux-aarch64")).rename(bundle)

        repack(self.amd64_archive, renamed, swap_in_aarch64)
        result = run_build(renamed, self.resummed(renamed), self.out("debs"))
        self.assertRefused(result, "platform is 'linux-aarch64', expected 'linux-x86_64'")

    def test_refuses_a_symlink_in_the_archive(self):
        def link_out(bundle):
            (bundle / "bin/escape").symlink_to("/etc/passwd")

        tampered = self.out("upload") / self.amd64_archive.name
        tampered.parent.mkdir(parents=True)
        repack(self.amd64_archive, tampered, link_out)
        result = run_build(tampered, self.resummed(tampered), self.out("debs"))
        self.assertRefused(result, "'torlek-v0.1.0-alpha.1-linux-x86_64/bin/escape' is not a file")

    def test_refuses_a_member_outside_the_bundle_directory(self):
        tampered = self.out("upload") / self.amd64_archive.name
        tampered.parent.mkdir(parents=True)
        with tarfile.open(tampered, "w:gz") as archive:
            member = tarfile.TarInfo("torlek-v0.1.0-alpha.1-linux-x86_64/../escape")
            archive.addfile(member, io.BytesIO(b""))
        result = run_build(tampered, self.resummed(tampered), self.out("debs"))
        self.assertRefused(result, "is outside torlek-v0.1.0-alpha.1-linux-x86_64/")

    def test_refuses_a_maintainer_without_an_address(self):
        result = run_build(self.amd64_archive, self.sums, self.out("debs"), maintainer="Theo")
        self.assertRefused(result, "not `Name <address>`")


class BuiltPackageTest(ReleaseFixture):
    """Both packages, built through build.py's command line from release.py archives of
    stand-in programs. No dpkg here: the checks read the ar and tar members directly."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.debs = {}
        for architecture, platform in build.ARCHITECTURES.items():
            result = run_build(cls.archives[platform], cls.sums, cls.work / "debs")
            if result.returncode != 0:
                raise AssertionError(result.stderr)
            cls.debs[architecture] = cls.work / "debs" / f"torlek_{VERSION}_{architecture}.deb"
        cls.members = {
            architecture: build.deb_members(deb.read_bytes())
            for architecture, deb in cls.debs.items()
        }
        cls.data = {
            architecture: tar_members(members["data.tar.xz"])
            for architecture, members in cls.members.items()
        }

    def control(self, architecture):
        return repository.fields_of(build.control_text(self.debs[architecture].read_bytes()))

    def installed_root(self, architecture):
        root = self.work / f"root-{architecture}"
        if not root.exists():
            payload = lzma.decompress(self.members[architecture]["data.tar.xz"])
            with tarfile.open(fileobj=io.BytesIO(payload)) as archive:
                archive.extractall(root, filter="tar")
        return root

    def test_holds_debian_binary_then_control_then_data(self):
        payload = self.debs["amd64"].read_bytes()
        self.assertEqual(list(build.deb_members(payload)), build.DEB_MEMBERS)
        self.assertEqual(self.members["amd64"]["debian-binary"], b"2.0\n")

    def test_control_names_the_package_version_and_architecture(self):
        for architecture in build.ARCHITECTURES:
            with self.subTest(architecture=architecture):
                control = self.control(architecture)
                self.assertEqual(control["Package"], "torlek")
                self.assertEqual(control["Version"], VERSION)
                self.assertEqual(control["Architecture"], architecture)
                self.assertEqual(control["Maintainer"], MAINTAINER)

    def test_depends_on_the_readme_prerequisites_with_tmux_3_6(self):
        self.assertEqual(
            self.control("amd64")["Depends"],
            "libc6 (>= 2.39), libgcc-s1 (>= 4.2), git, curl, tmux (>= 3.6)",
        )

    def test_description_has_a_synopsis_and_an_extended_text(self):
        synopsis, *extended = self.control("amd64")["Description"].split("\n")
        self.assertLessEqual(len(synopsis), 80)
        self.assertTrue(extended)
        self.assertTrue(all(line.startswith(" ") for line in extended))

    def test_installs_the_masks_helper_s_units_where_systemd_looks(self):
        bundle = self.installed_root("amd64") / "usr/lib/torlek" / release.SYSTEMD_UNITS
        for unit in ("torlek-engine-masks.socket", "torlek-engine-masks@.service"):
            with self.subTest(unit=unit):
                member = self.data["amd64"][f"./usr/lib/systemd/system/{unit}"]
                self.assertEqual(member.mode, 0o644)
                installed = self.installed_root("amd64") / "usr/lib/systemd/system" / unit
                self.assertEqual(installed.read_bytes(), (bundle / unit).read_bytes())

    def test_its_maintainer_scripts_enable_the_masks_socket_and_disable_it_on_removal(self):
        control = tar_members(self.members["amd64"]["control.tar.xz"])
        self.assertEqual(
            sorted(control),
            [".", "./control", "./md5sums", "./postinst", "./postrm", "./prerm"],
        )
        for script in ("postinst", "postrm", "prerm"):
            with self.subTest(script=script):
                self.assertEqual(control[f"./{script}"].mode, 0o755)
        scripts = self.maintainer_scripts("amd64")
        self.assertIn("systemctl enable torlek-engine-masks.socket", scripts["postinst"])
        self.assertIn("systemctl disable torlek-engine-masks.socket", scripts["prerm"])

    def test_its_maintainer_scripts_are_posix_shell(self):
        for name, text in self.maintainer_scripts("amd64").items():
            with self.subTest(script=name):
                checked = subprocess.run(["sh", "-n"], input=text, capture_output=True, text=True)
                self.assertEqual(checked.returncode, 0, checked.stderr)
                self.assertTrue(text.startswith("#!/bin/sh\n"))

    def maintainer_scripts(self, architecture):
        control = lzma.decompress(self.members[architecture]["control.tar.xz"])
        with tarfile.open(fileobj=io.BytesIO(control)) as archive:
            return {
                name: archive.extractfile(f"./{name}").read().decode()
                for name in ("postinst", "postrm", "prerm")
            }

    def test_public_command_links_into_the_whole_bundle(self):
        link = self.data["amd64"]["./usr/bin/torlek"]
        self.assertTrue(link.issym())
        self.assertEqual(link.linkname, "../lib/torlek/bin/torlek")

    def test_installed_bundle_still_matches_its_release_manifest(self):
        for architecture, platform in build.ARCHITECTURES.items():
            with self.subTest(architecture=architecture):
                bundle = self.installed_root(architecture) / "usr/lib/torlek"
                manifest = json.loads((bundle / release.MANIFEST_PATH).read_text())
                self.assertEqual((manifest["version"], manifest["platform"]), (TAG, platform))
                release.refuse_changed_contents(bundle, manifest)

    def test_every_program_is_executable_and_owned_by_root(self):
        for binary in release.BINARIES:
            with self.subTest(binary=binary):
                member = self.data["amd64"][f"./usr/lib/torlek/bin/{binary}"]
                self.assertEqual(member.mode, 0o755)
                self.assertEqual((member.uid, member.gid, member.uname), (0, 0, "root"))

    def test_every_entry_is_dated_to_the_release_day_not_1970(self):
        release_day = int(datetime.fromisoformat(f"{RELEASE_DATE}T00:00:00+00:00").timestamp())
        control = tar_members(self.members["amd64"]["control.tar.xz"])
        for name, member in [*self.data["amd64"].items(), *control.items()]:
            with self.subTest(name=name):
                self.assertEqual(member.mtime, release_day)

    def test_copyright_is_where_debian_policy_looks(self):
        copyright_file = self.installed_root("amd64") / "usr/share/doc/torlek/copyright"
        text = copyright_file.read_text()
        self.assertIn("License: Apache-2.0\n", text)
        self.assertIn("/usr/share/common-licenses/Apache-2.0", text)

    def test_changelog_names_the_version_and_the_archive(self):
        changelog = self.installed_root("amd64") / "usr/share/doc/torlek/changelog.Debian.gz"
        text = gzip.decompress(changelog.read_bytes()).decode()
        self.assertTrue(text.startswith(f"torlek ({VERSION}) alpha; urgency=medium\n"))
        self.assertIn(f"v{TAG} release archive", text)
        self.assertIn(f" -- {MAINTAINER}  Tue, 03 Nov 2026 00:00:00 +0000\n", text)

    def test_md5sums_cover_every_installed_file(self):
        control = lzma.decompress(self.members["amd64"]["control.tar.xz"])
        with tarfile.open(fileobj=io.BytesIO(control)) as archive:
            lines = archive.extractfile("./md5sums").read().decode().splitlines()
        listed = {line.split("  ", 1)[1] for line in lines}
        files = {name[2:] for name, member in self.data["amd64"].items() if member.isfile()}
        self.assertEqual(listed, files)

    def test_installs_nothing_outside_usr(self):
        outside = [
            name
            for name in self.data["amd64"]
            if name not in (".", "./usr") and not name.startswith("./usr/")
        ]
        self.assertEqual(outside, [])

    def test_same_inputs_give_the_same_bytes(self):
        result = run_build(self.amd64_archive, self.sums, self.out("debs"))
        self.assertEqual(result.returncode, 0, result.stderr)
        rebuilt = self.out("debs") / self.debs["amd64"].name
        self.assertEqual(rebuilt.read_bytes(), self.debs["amd64"].read_bytes())

    def test_refuses_to_replace_an_existing_package(self):
        before = self.debs["amd64"].read_bytes()
        result = run_build(self.amd64_archive, self.sums, self.work / "debs")
        self.assertEqual(result.returncode, 1)
        self.assertIn("exists", result.stderr)
        self.assertEqual(self.debs["amd64"].read_bytes(), before)

    def test_a_packaging_only_rebuild_is_a_newer_package(self):
        result = run_build(
            self.amd64_archive, self.sums, self.out("debs"), "--debian-revision", "2"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        rebuilt = self.out("debs") / "torlek_0.1.0~alpha.1-2_amd64.deb"
        version = repository.fields_of(build.control_text(rebuilt.read_bytes()))["Version"]
        self.assertGreater(dpkg_compare(version, VERSION), 0)

    @unittest.skipUnless(HAS_AR, "binutils ar is not installed")
    def test_binutils_ar_reads_the_container(self):
        result = subprocess.run(
            ["ar", "t", str(self.debs["amd64"])], capture_output=True, text=True, check=True
        )
        self.assertEqual(result.stdout.split(), build.DEB_MEMBERS)

    @unittest.skipUnless(HAS_DPKG_DEB, "dpkg-deb is not installed")
    def test_dpkg_deb_reads_the_control_fields(self):
        result = subprocess.run(
            ["dpkg-deb", "--field", str(self.debs["arm64"]), "Version", "Architecture"],
            capture_output=True,
            text=True,
            check=True,
        )
        self.assertEqual(result.stdout, f"Version: {VERSION}\nArchitecture: arm64\n")


class SigningKeyRefusalTest(unittest.TestCase):
    def test_sign_refuses_a_key_named_other_than_by_its_fingerprint(self):
        for key in [
            "BEC7DF90",
            "Torlek test <test@example.invalid>",
            "e5da432077b74af7a36d5af7e1a44cd1bec7df90",
        ]:
            with self.subTest(key=key), self.assertRaisesRegex(repository.Refusal, "fingerprint"):
                repository.sign(Path("/nonexistent"), "alpha", [key])

    def test_refuses_a_suite_that_is_not_a_channel(self):
        with self.assertRaisesRegex(repository.Refusal, "one of alpha, beta"):
            repository.index(Path("/nonexistent"), "stable")


class SignatureStatusTest(unittest.TestCase):
    """gpgv's status lines for cases a throwaway key cannot reach quickly (expiry, revocation)."""

    GOOD = "[GNUPG:] GOODSIG 1D47 Torlek\n[GNUPG:] VALIDSIG " + "A" * 40 + " 2026-11-03\n"

    def test_refuses_any_bad_expired_or_revoked_signature_beside_a_good_one(self):
        for keyword in ["BADSIG", "EXPSIG", "EXPKEYSIG", "REVKEYSIG"]:
            with self.subTest(keyword=keyword):
                status = self.GOOD + f"[GNUPG:] {keyword} 0C7D Someone\n"
                self.assertFalse(repository.is_trusted_signature(status))

    def test_ignores_a_signature_by_a_key_the_client_lacks(self):
        status = self.GOOD + "[GNUPG:] ERRSIG 0C7D 22 10 01 1762184201 9 " + "B" * 40 + "\n"
        self.assertTrue(repository.is_trusted_signature(status))

    def test_refuses_a_status_without_a_valid_signature(self):
        self.assertFalse(repository.is_trusted_signature("[GNUPG:] NO_PUBKEY 0C7D\n"))


@unittest.skipUnless(HAS_GPG, "gpg and gpgv are not installed")
class RepositoryTest(unittest.TestCase):
    """A throwaway key in a temporary GNUPGHOME signs a repository of both fixture packages."""

    @classmethod
    def setUpClass(cls):
        cls.folder = tempfile.TemporaryDirectory()
        cls.work = Path(cls.folder.name)
        cls.gnupg_home = cls.work / "gnupg"
        cls.gnupg_home.mkdir(mode=0o700)
        cls.environment = {**os.environ, "GNUPGHOME": str(cls.gnupg_home)}
        cls.fingerprint, cls.public_key = cls.throwaway_key("Torlek test <test@example.invalid>")
        cls.other_fingerprint, cls.other_public_key = cls.throwaway_key(
            "Someone else <other@example.invalid>"
        )
        archives, sums = release_fixture(cls.work)
        cls.pool = cls.work / "fixture/pool/main/t/torlek"
        for archive in archives.values():
            result = run_build(archive, sums, cls.pool)
            if result.returncode != 0:
                raise AssertionError(result.stderr)
        beta_archives, beta_sums = release_fixture(cls.work, "0.1.0-beta.1", "beta")
        cls.beta_pool = cls.work / "beta-debs"
        result = run_build(
            beta_archives["linux-x86_64"], beta_sums, cls.beta_pool, tag="0.1.0-beta.1"
        )
        if result.returncode != 0:
            raise AssertionError(result.stderr)

    @classmethod
    def throwaway_key(cls, user_id):
        subprocess.run(
            ["gpg", "--batch", "--pinentry-mode", "loopback", "--passphrase", ""]
            + ["--quick-gen-key", user_id, "ed25519", "sign", "never"],
            env=cls.environment,
            capture_output=True,
            check=True,
        )
        listing = subprocess.run(
            ["gpg", "--batch", "--list-keys", user_id],
            env=cls.environment,
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        (fingerprint,) = re.findall(r"\b[0-9A-F]{40}\b", listing)
        public_key = cls.work / f"{fingerprint}.asc"
        exported = subprocess.run(
            ["gpg", "--batch", "--armor", "--export", fingerprint],
            env=cls.environment,
            capture_output=True,
            check=True,
        )
        public_key.write_bytes(exported.stdout)
        return fingerprint, public_key

    @classmethod
    def tearDownClass(cls):
        subprocess.run(
            ["gpgconf", "--kill", "gpg-agent"],
            env=cls.environment,
            capture_output=True,
            check=False,
        )
        cls.folder.cleanup()

    def repository(self):
        repo = self.work / "repos" / self.id().rsplit(".", 1)[-1]
        shutil.copytree(self.pool, repo / repository.POOL)
        return repo

    def run_repository(self, *arguments):
        return run_tool(REPOSITORY, *arguments, environment=self.environment)

    def signed_repository(self, *fingerprints):
        keys = [part for key in fingerprints or [self.fingerprint] for part in ("--key", key)]
        repo = self.repository()
        for arguments in (
            ["index", "--repo", str(repo), "--suite", "alpha"],
            ["sign", "--repo", str(repo), "--suite", "alpha", *keys],
        ):
            result = self.run_repository(*arguments)
            if result.returncode != 0:
                raise AssertionError(result.stderr)
        return repo

    def verify(self, repo, public_key=None):
        public_key = public_key or self.public_key
        return self.run_repository(
            "verify", "--repo", str(repo), "--suite", "alpha", "--keyring", str(public_key)
        )

    def assertRefused(self, result, message):
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(message, result.stderr)

    def test_a_signed_repository_verifies_against_its_public_key(self):
        result = self.verify(self.signed_repository())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("hold for 2 packages in alpha", result.stdout)

    def test_packages_index_names_each_pool_file_with_its_hash(self):
        repo = self.signed_repository()
        index = repo / "dists/alpha/main/binary-arm64/Packages"
        stanza = repository.fields_of(index.read_text())
        deb = repo / stanza["Filename"]
        self.assertEqual(stanza["Filename"], f"pool/main/t/torlek/torlek_{VERSION}_arm64.deb")
        self.assertEqual(stanza["Size"], str(deb.stat().st_size))
        self.assertEqual(stanza["SHA256"], release.sha256_of(deb))

    def test_release_describes_the_suite_and_both_architectures(self):
        fields = repository.fields_of(
            (self.signed_repository() / "dists/alpha/Release").read_text()
        )
        self.assertEqual((fields["Suite"], fields["Codename"]), ("alpha", "alpha"))
        self.assertEqual(fields["Architectures"], "amd64 arm64")
        self.assertEqual(fields["Components"], "main")
        self.assertNotIn("Valid-Until", fields)

    def test_refuses_a_package_changed_after_signing(self):
        repo = self.signed_repository()
        deb = repo / repository.POOL / f"torlek_{VERSION}_amd64.deb"
        deb.write_bytes(deb.read_bytes()[:-1] + b"\0")
        self.assertRefused(self.verify(repo), "size or SHA256 differs from the signed index")

    def test_refuses_an_index_changed_after_signing(self):
        repo = self.signed_repository()
        index = repo / "dists/alpha/main/binary-amd64/Packages"
        index.write_text(index.read_text().replace("tmux (>= 3.6)", "tmux"))
        result = self.verify(repo)
        self.assertRefused(result, "main/binary-amd64/Packages: size or SHA256 differs")

    def test_refuses_a_release_signed_by_another_key(self):
        result = self.verify(self.signed_repository(), self.other_public_key)
        self.assertRefused(result, "InRelease: signature refused")

    def test_a_rotation_signs_with_both_keys_so_either_verifies(self):
        repo = self.signed_repository(self.fingerprint, self.other_fingerprint)
        for public_key in (self.public_key, self.other_public_key):
            with self.subTest(public_key=public_key.name):
                result = self.verify(repo, public_key)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_refuses_an_edited_inrelease(self):
        repo = self.signed_repository()
        in_release = repo / "dists/alpha/InRelease"
        in_release.write_text(in_release.read_text().replace("Suite: alpha", "Suite: beta"))
        self.assertRefused(self.verify(repo), "InRelease: signature refused")

    def test_reindexing_removes_the_earlier_signatures(self):
        repo = self.signed_repository()
        result = self.run_repository("index", "--repo", str(repo), "--suite", "alpha")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((repo / "dists/alpha/InRelease").exists())
        self.assertFalse((repo / "dists/alpha/Release.gpg").exists())

    def test_index_refuses_a_suite_missing_an_architecture(self):
        repo = self.repository()
        (repo / repository.POOL / f"torlek_{VERSION}_arm64.deb").unlink()
        result = self.run_repository("index", "--repo", str(repo), "--suite", "alpha")
        self.assertRefused(result, "no arm64 package")

    def test_index_refuses_a_package_of_another_channel(self):
        repo = self.repository()
        shutil.copy(next(self.beta_pool.glob("*.deb")), repo / repository.POOL)
        result = self.run_repository("index", "--repo", str(repo), "--suite", "alpha")
        self.assertRefused(result, "is not in the alpha channel")

    def test_index_refuses_a_package_outside_the_pool_directory(self):
        repo = self.repository()
        shutil.copy(next((repo / repository.POOL).glob("*.deb")), repo / "pool")
        result = self.run_repository("index", "--repo", str(repo), "--suite", "alpha")
        self.assertRefused(result, "outside pool/main/t/torlek")


class PruneTest(unittest.TestCase):
    """alpha.1, alpha.2, alpha.2 rebuilt as revision 2, and alpha.10, both architectures each."""

    @classmethod
    def setUpClass(cls):
        cls.folder = tempfile.TemporaryDirectory()
        cls.work = Path(cls.folder.name)
        cls.debs = cls.work / "debs"
        for tag, revisions in (
            ("0.1.0-alpha.1", [1]),
            ("0.1.0-alpha.2", [1, 2]),
            ("0.1.0-alpha.10", [1]),
        ):
            archives, sums = release_fixture(cls.work, tag)
            for revision in revisions:
                for archive in archives.values():
                    result = run_build(
                        archive, sums, cls.debs, "--debian-revision", str(revision), tag=tag
                    )
                    if result.returncode != 0:
                        raise AssertionError(result.stderr)

    @classmethod
    def tearDownClass(cls):
        cls.folder.cleanup()

    def pruned(self, keep):
        repo = self.work / "repos" / self.id().rsplit(".", 1)[-1]
        shutil.copytree(self.debs, repo / repository.POOL)
        result = run_tool(
            REPOSITORY, "prune", "--repo", str(repo), "--suite", "alpha", "--keep", str(keep)
        )
        return result, sorted(path.name for path in (repo / repository.POOL).glob("*.deb"))

    def test_keeps_both_architectures_of_the_newest_versions_in_dpkg_order(self):
        result, kept = self.pruned(2)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            kept,
            [
                "torlek_0.1.0~alpha.10-1_amd64.deb",
                "torlek_0.1.0~alpha.10-1_arm64.deb",
                "torlek_0.1.0~alpha.2-2_amd64.deb",
                "torlek_0.1.0~alpha.2-2_arm64.deb",
            ],
        )

    def test_keeping_more_versions_than_exist_removes_nothing(self):
        result, kept = self.pruned(10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(kept), 8)

    def test_refuses_to_keep_no_version(self):
        result, kept = self.pruned(0)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("keep at least the newest version", result.stderr)
        self.assertEqual(len(kept), 8)


@unittest.skipUnless(HAS_GPG and HAS_GIT, "gpg, gpgv and git are not installed")
class PublishTest(unittest.TestCase):
    """publish.sh against a bare Git repository standing in for the Pages repository, with a
    throwaway certification key whose signing subkey alone is handed over, as in CI."""

    @classmethod
    def setUpClass(cls):
        cls.folder = tempfile.TemporaryDirectory()
        cls.work = Path(cls.folder.name)
        cls.gnupg_home = cls.work / "gnupg"
        cls.gnupg_home.mkdir(mode=0o700)
        cls.gpg_environment = {**os.environ, "GNUPGHOME": str(cls.gnupg_home)}
        cls.public_key, cls.signing_subkey = cls.certification_key(
            "Torlek test <t@example.invalid>"
        )
        cls.other_public_key, cls.other_signing_subkey = cls.certification_key(
            "Someone else <o@example.invalid>"
        )
        cls.releases = {
            tag: release_fixture(cls.work, tag) for tag in ("0.1.0-alpha.1", "0.1.0-alpha.2")
        }

    @classmethod
    def certification_key(cls, user_id):
        """A primary key that only certifies, a signing subkey, and the subkey's secret alone."""

        def gpg(*arguments):
            return subprocess.run(
                ["gpg", "--batch", "--pinentry-mode", "loopback", "--passphrase", "", *arguments],
                env=cls.gpg_environment,
                capture_output=True,
                check=True,
            ).stdout

        gpg("--quick-gen-key", user_id, "ed25519", "cert", "never")
        listing = gpg("--with-colons", "--list-keys", user_id).decode()
        primary = re.findall(r"^fpr:+([0-9A-F]{40}):", listing, re.MULTILINE)[0]
        gpg("--quick-add-key", primary, "ed25519", "sign", "never")
        listing = gpg("--with-colons", "--list-keys", primary).decode()
        subkey = re.findall(r"^fpr:+([0-9A-F]{40}):", listing, re.MULTILINE)[1]
        public_key = cls.work / f"{primary}.asc"
        public_key.write_bytes(gpg("--armor", "--export", primary))
        return public_key, gpg("--armor", "--export-secret-subkeys", f"{subkey}!").decode()

    @classmethod
    def tearDownClass(cls):
        subprocess.run(
            ["gpgconf", "--kill", "gpg-agent"],
            env=cls.gpg_environment,
            capture_output=True,
            check=False,
        )
        cls.folder.cleanup()

    def setUp(self):
        self.case = self.work / "cases" / self.id().rsplit(".", 1)[-1]
        self.remote = self.case / "site.git"
        subprocess.run(["git", "init", "--quiet", "--bare", str(self.remote)], check=True)
        self.inputs = self.case / "inputs"
        for tag, (archives, sums) in self.releases.items():
            (self.inputs / tag).mkdir(parents=True)
            shutil.copyfile(sums, self.inputs / tag / "SHA256SUMS")
            for archive in archives.values():
                shutil.copyfile(archive, self.inputs / tag / archive.name)

    def publish(self, tag, signing_key=None, public_key=None, keep=10):
        # publish.sh imports the key into a GNUPGHOME of its own, under TMPDIR.
        environment = {
            **os.environ,
            "TMPDIR": str(self.case),
            "TORLEK_APT_REMOTE": str(self.remote),
            "TORLEK_APT_MAINTAINER": MAINTAINER,
            "TORLEK_APT_SIGNING_KEY": signing_key or self.signing_subkey,
            "TORLEK_APT_PUBLIC_KEY": str(public_key or self.public_key),
            "TORLEK_APT_KEEP": str(keep),
        }
        return subprocess.run(
            ["bash", str(PUBLISH), str(self.inputs), tag, RELEASE_DATE],
            capture_output=True,
            text=True,
            check=False,
            env=environment,
        )

    def published(self):
        site = self.case / "checkout"
        shutil.rmtree(site, ignore_errors=True)
        subprocess.run(
            ["git", "clone", "--quiet", "--branch", "gh-pages", str(self.remote), str(site)],
            check=True,
        )
        return site

    def assertPublished(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_the_first_publication_creates_a_signed_suite_with_both_architectures(self):
        self.assertPublished(self.publish("0.1.0-alpha.1"))
        site = self.published()
        result = run_tool(
            REPOSITORY,
            "verify",
            "--repo",
            str(site),
            "--suite",
            "alpha",
            "--keyring",
            str(self.public_key),
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("for 2 packages", result.stdout)

    def test_the_site_serves_its_public_key_without_a_jekyll_build(self):
        self.assertPublished(self.publish("0.1.0-alpha.1"))
        site = self.published()
        self.assertEqual((site / "torlek.asc").read_bytes(), self.public_key.read_bytes())
        self.assertTrue((site / ".nojekyll").is_file())

    def test_a_later_release_joins_the_suite_in_a_single_commit(self):
        self.assertPublished(self.publish("0.1.0-alpha.1"))
        self.assertPublished(self.publish("0.1.0-alpha.2"))
        site = self.published()
        packages = sorted(path.name for path in (site / repository.POOL).glob("*.deb"))
        self.assertEqual(len(packages), 4, packages)
        commits = subprocess.run(
            ["git", "-C", str(site), "rev-list", "--count", "HEAD"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        self.assertEqual(commits.strip(), "1")

    def test_keeps_only_the_newest_versions(self):
        self.assertPublished(self.publish("0.1.0-alpha.1"))
        self.assertPublished(self.publish("0.1.0-alpha.2", keep=1))
        packages = sorted(path.name for path in (self.published() / repository.POOL).glob("*"))
        self.assertEqual(
            packages, ["torlek_0.1.0~alpha.2-1_amd64.deb", "torlek_0.1.0~alpha.2-1_arm64.deb"]
        )

    def test_refuses_a_signing_key_the_committed_public_key_does_not_hold(self):
        result = self.publish("0.1.0-alpha.1", signing_key=self.other_signing_subkey)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(f"is not in {self.public_key}", result.stderr)
        self.assertEqual(self.branches(), "")

    def test_refuses_to_add_to_a_site_the_committed_key_did_not_sign(self):
        self.assertPublished(
            self.publish(
                "0.1.0-alpha.1",
                signing_key=self.other_signing_subkey,
                public_key=self.other_public_key,
            )
        )
        before = self.branches()
        result = self.publish("0.1.0-alpha.2")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("InRelease: signature refused", result.stderr)
        self.assertEqual(self.branches(), before)

    def change_published_site(self, change):
        site = self.published()
        change(site)
        subprocess.run(["git", "-C", str(site), "add", "--all"], check=True)
        subprocess.run(
            [
                "git",
                "-C",
                str(site),
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "change the site",
            ],
            check=True,
        )
        subprocess.run(
            ["git", "-C", str(site), "push", "--quiet", "origin", "gh-pages"], check=True
        )

    def test_refuses_to_add_to_a_site_whose_other_suite_the_committed_key_did_not_sign(self):
        self.assertPublished(self.publish("0.1.0-alpha.1"))

        def add_an_unsigned_beta_suite(site):
            shutil.copytree(site / "dists/alpha", site / "dists/beta")
            (site / "dists/beta/InRelease").write_text("not signed\n")

        self.change_published_site(add_an_unsigned_beta_suite)
        before = self.branches()
        result = self.publish("0.1.0-alpha.2")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("InRelease", result.stderr)
        self.assertEqual(self.branches(), before)

    def test_refuses_to_add_to_a_site_whose_pool_holds_an_unindexed_package(self):
        self.assertPublished(self.publish("0.1.0-alpha.1"))

        def plant_a_package(site):
            (site / repository.POOL / "torlek_0.1.0~alpha.0-1_amd64.deb").write_bytes(b"planted")

        self.change_published_site(plant_a_package)
        before = self.branches()
        result = self.publish("0.1.0-alpha.2")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(
            f"no signed index lists: {repository.POOL}/torlek_0.1.0~alpha.0-1_amd64.deb",
            result.stderr,
        )
        self.assertEqual(self.branches(), before)

    def test_refuses_to_publish_a_release_twice(self):
        self.assertPublished(self.publish("0.1.0-alpha.1"))
        before = self.branches()
        result = self.publish("0.1.0-alpha.1")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("exists; build into a directory without it", result.stderr)
        self.assertEqual(self.branches(), before)

    def test_refuses_without_a_committed_public_key(self):
        result = self.publish("0.1.0-alpha.1", public_key=self.case / "missing.asc")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("commit the public key first", result.stderr)

    def check_site(self, site, tag="0.1.0-alpha.1"):
        return subprocess.run(
            ["bash", str(CHECK_SITE), site.as_uri(), tag, str(self.public_key), "0"],
            capture_output=True,
            text=True,
            check=False,
            env={**os.environ, "TMPDIR": str(self.case)},
        )

    def test_the_site_check_accepts_the_published_site(self):
        self.assertPublished(self.publish("0.1.0-alpha.1"))
        result = self.check_site(self.published())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("serves torlek 0.1.0~alpha.1-1, signed and complete", result.stdout)

    def test_the_site_check_refuses_a_site_without_the_release(self):
        self.assertPublished(self.publish("0.1.0-alpha.1"))
        result = self.check_site(self.published(), tag="0.1.0-alpha.2")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("does not list torlek 0.1.0~alpha.2-1 yet", result.stderr)

    def test_the_site_check_refuses_a_package_changed_after_signing(self):
        self.assertPublished(self.publish("0.1.0-alpha.1"))
        site = self.published()
        package = next((site / repository.POOL).glob("*_amd64.deb"))
        package.write_bytes(package.read_bytes()[:-1] + b"X")
        result = self.check_site(site)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("size or SHA256 differs from the signed index", result.stderr)

    def branches(self):
        return subprocess.run(
            ["git", "-C", str(self.remote), "show-ref"],
            capture_output=True,
            text=True,
            check=False,
        ).stdout


if __name__ == "__main__":
    unittest.main()
