import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RENDER = ROOT / "packaging/aur/render.py"
TAG = "0.1.0-alpha.1"
REVISION = "b" * 40


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


sys.path.insert(0, str(ROOT / "tools"))
release = load("release", ROOT / "tools/release.py")
render = load("aur_render", RENDER)
HAS_MAKEPKG = shutil.which("makepkg") is not None
HAS_VERCMP = shutil.which("vercmp") is not None


def sums_for(digests):
    return "".join(f"{digest}  {name}\n" for name, digest in digests.items())


def release_sums(tag=TAG, x86_64="1" * 64, aarch64="2" * 64):
    return sums_for(
        {
            f"torlek-v{tag}-linux-aarch64.tar.gz": aarch64,
            f"torlek-v{tag}-linux-x86_64.tar.gz": x86_64,
        }
    )


def run_render(*arguments):
    return subprocess.run(
        [sys.executable, str(RENDER), *arguments], capture_output=True, text=True, check=False
    )


class PackageVersionTest(unittest.TestCase):
    def test_prerelease_tags_drop_only_their_hyphen(self):
        for tag, expected in [
            ("0.1.0-alpha.1", "0.1.0alpha.1"),
            ("0.1.0-alpha.12", "0.1.0alpha.12"),
            ("0.2.3-beta.1", "0.2.3beta.1"),
        ]:
            with self.subTest(tag=tag):
                self.assertEqual(render.package_version(tag), expected)

    def test_refuses_tags_outside_the_prerelease_grammar(self):
        for tag in ["v0.1.0-alpha.1", "0.1.0", "0.1.0-dev.3", "0.1.0-alpha.0", "01.0.0-alpha.1"]:
            with self.subTest(tag=tag), self.assertRaises(render.Refusal):
                render.package_version(tag)

    @unittest.skipUnless(HAS_VERCMP, "vercmp (pacman) is not installed")
    def test_pacman_orders_alpha_before_beta_before_the_stable_core(self):
        ordered = ["0.1.0-alpha.1", "0.1.0-alpha.2", "0.1.0-alpha.10", "0.1.0-beta.1"]
        versions = [render.package_version(tag) for tag in ordered] + ["0.1.0", "0.1.1alpha.1"]
        for older, newer in zip(versions, versions[1:], strict=False):
            with self.subTest(older=older, newer=newer):
                result = subprocess.run(
                    ["vercmp", older, newer], capture_output=True, text=True, check=True
                )
                self.assertEqual(result.stdout.strip(), "-1")


class ChecksumSelectionTest(unittest.TestCase):
    def test_selects_each_linux_archive_by_its_architecture(self):
        sums = release_sums() + f"{'3' * 64}  torlek-v{TAG}-macos-universal.tar.gz\n"
        self.assertEqual(
            render.archive_checksums(TAG, sums), {"x86_64": "1" * 64, "aarch64": "2" * 64}
        )

    def test_refuses_sums_without_both_linux_archives(self):
        only_x86_64 = sums_for({f"torlek-v{TAG}-linux-x86_64.tar.gz": "1" * 64})
        with self.assertRaisesRegex(render.Refusal, "linux-aarch64"):
            render.archive_checksums(TAG, only_x86_64)

    def test_refuses_sums_of_another_release(self):
        with self.assertRaisesRegex(render.Refusal, "v0.1.0-alpha.1 release"):
            render.archive_checksums(TAG, release_sums(tag="0.1.0-alpha.2"))

    def test_refuses_an_archive_listed_twice(self):
        sums = release_sums() + f"{'4' * 64}  torlek-v{TAG}-linux-x86_64.tar.gz\n"
        with self.assertRaisesRegex(render.Refusal, "listed twice"):
            render.archive_checksums(TAG, sums)

    def test_refuses_a_malformed_line(self):
        for line in ["SKIP  torlek.tar.gz", f"{'A' * 64}  torlek.tar.gz", f"{'1' * 64} one-space"]:
            with self.subTest(line=line), self.assertRaisesRegex(render.Refusal, "line 1"):
                render.archive_checksums(TAG, line + "\n" + release_sums())


class RecipeTest(unittest.TestCase):
    def test_binds_the_tag_and_each_checksum(self):
        text = render.recipe(TAG, release_sums())
        self.assertIn("_tag=0.1.0-alpha.1\n", text)
        self.assertIn("pkgver=0.1.0alpha.1\n", text)
        self.assertIn(f"sha256sums_x86_64=('{'1' * 64}')", text)
        self.assertIn(f"sha256sums_aarch64=('{'2' * 64}')", text)
        self.assertNotRegex(text, r"@[A-Z0-9_]+@")

    def test_never_downloads_a_moving_url(self):
        text = render.recipe(TAG, release_sums())
        self.assertNotIn("latest", text)
        self.assertNotIn("SKIP", text)

    @unittest.skipUnless(HAS_MAKEPKG, "makepkg is not installed")
    def test_render_writes_the_srcinfo_makepkg_prints(self):
        with tempfile.TemporaryDirectory() as folder:
            sums = Path(folder) / "SHA256SUMS"
            sums.write_text(release_sums())
            out = Path(folder) / "recipe"
            result = run_render("render", "--tag", TAG, "--sums", str(sums), "--out", str(out))
            self.assertEqual(result.returncode, 0, result.stderr)
            srcinfo = (out / ".SRCINFO").read_text()
            self.assertIn("\tpkgver = 0.1.0alpha.1\n", srcinfo)
            self.assertIn(f"\tsha256sums_aarch64 = {'2' * 64}\n", srcinfo)
            self.assertEqual(run_render("check-srcinfo", str(out)).returncode, 0)

    @unittest.skipUnless(HAS_MAKEPKG, "makepkg is not installed")
    def test_check_srcinfo_refuses_a_recipe_edited_after_rendering(self):
        with tempfile.TemporaryDirectory() as folder:
            sums = Path(folder) / "SHA256SUMS"
            sums.write_text(release_sums())
            out = Path(folder) / "recipe"
            run_render("render", "--tag", TAG, "--sums", str(sums), "--out", str(out))
            pkgbuild = out / "PKGBUILD"
            pkgbuild.write_text(pkgbuild.read_text().replace("pkgrel=1", "pkgrel=2"))
            result = run_render("check-srcinfo", str(out))
            self.assertEqual(result.returncode, 1)
            self.assertIn("does not match its PKGBUILD", result.stderr)

    def test_render_refuses_to_replace_an_existing_recipe(self):
        with tempfile.TemporaryDirectory() as folder:
            sums = Path(folder) / "SHA256SUMS"
            sums.write_text(release_sums())
            (Path(folder) / "PKGBUILD").write_text("kept\n")
            result = run_render("render", "--tag", TAG, "--sums", str(sums), "--out", folder)
            self.assertEqual(result.returncode, 1)
            self.assertEqual((Path(folder) / "PKGBUILD").read_text(), "kept\n")


@unittest.skipUnless(HAS_MAKEPKG, "makepkg is not installed")
class FixturePackageTest(unittest.TestCase):
    """makepkg builds the recipe from a release.py bundle of stand-in programs; no install."""

    @classmethod
    def setUpClass(cls):
        cls.folder = tempfile.TemporaryDirectory()
        work = Path(cls.folder.name)
        architecture = os.uname().machine
        if architecture not in render.ARCHITECTURES:
            raise unittest.SkipTest(f"no Torlek archive for {architecture}")
        binaries = work / "binaries"
        binaries.mkdir()
        for binary in release.BINARIES:
            (binaries / binary).write_text(f"#!/bin/sh\necho {binary}\n")
        archives = work / "archives"
        bundle = release.package(
            TAG,
            "alpha",
            render.ARCHITECTURES[architecture],
            REVISION,
            binaries,
            archives,
            ROOT,
        )
        archive = Path(bundle["archive"])
        with archive.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        checksums = {"x86_64": "1" * 64, "aarch64": "2" * 64, architecture: digest}
        sums = work / "SHA256SUMS"
        sums.write_text(release_sums(**checksums))
        recipe = work / "recipe"
        result = run_render("render", "--tag", TAG, "--sums", str(sums), "--out", str(recipe))
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        shutil.copy(archive, recipe / archive.name)
        environment = {
            **os.environ,
            "PKGDEST": str(work / "packages"),
            "BUILDDIR": str(work / "build"),
            "SRCDEST": str(recipe),
            "PKGEXT": ".pkg.tar",
        }
        build = subprocess.run(
            ["makepkg", "--nodeps", "--noconfirm", "--noprogressbar"],
            cwd=recipe,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        if build.returncode != 0:
            raise AssertionError(build.stdout + build.stderr)
        (cls.package,) = (work / "packages").glob("torlek-bin-*.pkg.tar")
        cls.root = work / "root"
        with tarfile.open(cls.package) as built:
            cls.members = {member.name: member for member in built.getmembers()}
            built.extractall(cls.root, filter="tar")

    @classmethod
    def tearDownClass(cls):
        cls.folder.cleanup()

    def test_package_carries_the_pacman_version_and_dependencies(self):
        info = (self.root / ".PKGINFO").read_text()
        self.assertIn("pkgver = 0.1.0alpha.1-1\n", info)
        self.assertIn("depend = tmux>=3.6\n", info)

    def test_public_command_links_into_the_whole_bundle(self):
        link = self.members["usr/bin/torlek"]
        self.assertTrue(link.issym())
        self.assertEqual(link.linkname, "../lib/torlek/bin/torlek")

    def test_installed_bundle_still_matches_its_release_manifest(self):
        bundle = self.root / "usr/lib/torlek"
        manifest = json.loads((bundle / release.MANIFEST_PATH).read_text())
        self.assertEqual(manifest["version"], TAG)
        release.refuse_changed_contents(bundle, manifest)

    def test_every_program_is_executable(self):
        for binary in release.BINARIES:
            with self.subTest(binary=binary):
                self.assertEqual(self.members[f"usr/lib/torlek/bin/{binary}"].mode & 0o777, 0o755)

    def test_license_is_installed_where_pacman_looks(self):
        link = self.root / "usr/share/licenses/torlek-bin/LICENSE"
        self.assertEqual(link.resolve(), (self.root / "usr/lib/torlek/LICENSE").resolve())
        self.assertTrue(link.is_file())

    def test_package_writes_nothing_under_home_or_etc(self):
        owned = [name for name in self.members if not name.startswith(".")]
        self.assertEqual([name for name in owned if not name.startswith("usr")], [])


if __name__ == "__main__":
    unittest.main()
