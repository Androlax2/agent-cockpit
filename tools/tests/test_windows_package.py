import hashlib
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
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WINDOWS = ROOT / "packaging/windows"
RENDER = WINDOWS / "render.py"
INSTALL_IN_WSL = WINDOWS / "install-in-wsl.sh"
INSTALL_SH = ROOT / "install.sh"
TAG = "0.1.0-alpha.1"
DEV_TAG = "0.1.0-dev.4"
LOOPBACK = "http://127.0.0.1:8765"
BUNDLE = f"torlek-v{TAG}-linux-x86_64"
REVISION = "c" * 40
WSL2_KERNEL = "6.6.87.2-microsoft-standard-WSL2"
# Scoop unpacks a download whose local name matches this (lib/depends.ps1, Test-7zipRequirement).
SCOOP_7ZIP_NAME = re.compile(
    r"\.(001|7z|bz(ip)?2?|gz|img|iso|lzma|lzh|nupkg|rar|tar|t[abgpx]z2?|t?zst|xz)(\.[^\d.]+)?$"
)


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


sys.path.insert(0, str(ROOT / "tools"))
release = load("release", ROOT / "tools/release.py")
render = load("windows_render", RENDER)


def release_sums(tag=TAG, x86_64="1" * 64, aarch64="2" * 64):
    return f"{aarch64}  torlek-v{tag}-linux-aarch64.tar.gz\n{x86_64}  torlek-v{tag}-linux-x86_64.tar.gz\n"


def git(repo, *arguments):
    subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            *arguments,
        ],
        capture_output=True,
        check=True,
    )


def tagged_repository(folder, tag=TAG):
    """A repository whose v<tag> holds today's launcher files, standing in for the release tag."""
    repo = Path(folder) / "repo"
    for path in render.LAUNCHER_FILES.values():
        (repo / path).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / path, repo / path)
    git(repo, "init", "--quiet")
    git(repo, "add", ".")
    git(repo, "commit", "--quiet", "-m", "fixture")
    git(repo, "tag", f"v{tag}")
    return repo


class TagTest(unittest.TestCase):
    def test_scoop_version_is_the_prerelease_tag(self):
        for tag in ["0.1.0-alpha.1", "0.1.0-alpha.12", "0.2.3-beta.1"]:
            with self.subTest(tag=tag):
                self.assertEqual(render.checked_tag(tag), tag)

    def test_refuses_tags_outside_the_release_grammar(self):
        for tag in ["v0.1.0-alpha.1", "0.1.0-dev.3", "0.1.0-alpha.0", "latest"]:
            with self.subTest(tag=tag), self.assertRaisesRegex(render.Refusal, "not a release tag"):
                render.checked_tag(tag)

    def test_refuses_a_stable_tag_until_one_exists(self):
        with self.assertRaisesRegex(render.Refusal, "no stable release"):
            render.checked_tag("0.1.0")


class ChecksumSelectionTest(unittest.TestCase):
    def test_selects_the_linux_x86_64_archive(self):
        self.assertEqual(render.archive_checksum(TAG, release_sums()), "1" * 64)

    def test_refuses_sums_without_the_x86_64_archive(self):
        only_aarch64 = f"{'2' * 64}  torlek-v{TAG}-linux-aarch64.tar.gz\n"
        with self.assertRaisesRegex(render.Refusal, "linux-x86_64"):
            render.archive_checksum(TAG, only_aarch64)

    def test_refuses_sums_of_another_release(self):
        with self.assertRaisesRegex(render.Refusal, "v0.1.0-alpha.1 release"):
            render.archive_checksum(TAG, release_sums(tag="0.1.0-alpha.2"))

    def test_refuses_an_archive_listed_twice(self):
        sums = release_sums() + f"{'4' * 64}  torlek-v{TAG}-linux-x86_64.tar.gz\n"
        with self.assertRaisesRegex(render.Refusal, "listed twice"):
            render.archive_checksum(TAG, sums)

    def test_refuses_a_malformed_line(self):
        for line in ["SKIP  torlek.tar.gz", f"{'A' * 64}  torlek.tar.gz", f"{'1' * 64} one-space"]:
            with self.subTest(line=line), self.assertRaisesRegex(render.Refusal, "line 1"):
                render.archive_checksum(TAG, line + "\n" + release_sums())


class ManifestTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.folder = tempfile.TemporaryDirectory()
        cls.repo = tagged_repository(cls.folder.name)
        cls.sums = release_sums().encode()
        cls.manifest = json.loads(render.manifest(TAG, cls.sums, cls.repo))
        cls.download = cls.manifest["architecture"]["64bit"]

    @classmethod
    def tearDownClass(cls):
        cls.folder.cleanup()

    def hash_of(self, url_suffix):
        (index,) = [i for i, url in enumerate(self.download["url"]) if url.endswith(url_suffix)]
        return self.download["hash"][index]

    def test_version_is_the_tag(self):
        self.assertEqual(self.manifest["version"], TAG)

    def test_every_download_names_the_tag_and_has_its_own_hash(self):
        self.assertEqual(len(self.download["url"]), len(self.download["hash"]))
        for url in self.download["url"]:
            with self.subTest(url=url):
                self.assertIn(f"/v{TAG}/", url)
                self.assertTrue(url.startswith("https://"))
                self.assertNotIn("latest", url)

    def test_binds_the_archive_to_its_release_checksum(self):
        self.assertEqual(self.hash_of(f"{BUNDLE}.archive"), "1" * 64)

    def test_binds_the_checksum_file_itself(self):
        self.assertEqual(self.hash_of("/SHA256SUMS"), hashlib.sha256(self.sums).hexdigest())

    def test_binds_each_launcher_file_to_its_tagged_content(self):
        for path in render.LAUNCHER_FILES.values():
            with self.subTest(path=path):
                expected = hashlib.sha256((self.repo / path).read_bytes()).hexdigest()
                self.assertEqual(self.hash_of(path), expected)

    def test_hashes_the_tagged_launcher_not_a_later_edit(self):
        path = render.LAUNCHER_FILES["@SHA256_TORLEK_PS1@"]
        tagged = hashlib.sha256((self.repo / path).read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as folder:
            repo = tagged_repository(folder)
            (repo / path).write_text("# edited after the tag\n")
            git(repo, "commit", "--quiet", "-am", "edit")
            download = json.loads(render.manifest(TAG, self.sums, repo))["architecture"]["64bit"]
        (index,) = [i for i, url in enumerate(download["url"]) if url.endswith(path)]
        self.assertEqual(download["hash"][index], tagged)

    def test_scoop_keeps_the_archive_packed(self):
        """Unpacking on NTFS would drop the bundle's Unix modes, so the local name avoids 7-Zip."""
        (archive_url,) = [url for url in self.download["url"] if ".tar.gz" in url]
        local_name = archive_url.split("#/", 1)[1]
        self.assertEqual(local_name, f"{BUNDLE}.archive")
        self.assertIsNone(SCOOP_7ZIP_NAME.search(local_name))

    def test_no_arm64_entry_so_scoop_uses_the_x64_build_and_the_check_refuses_aarch64(self):
        # Scoop falls back to 64bit on arm64 only when no "arm64" string appears (lib/manifest.ps1).
        self.assertEqual(list(self.manifest["architecture"]), ["64bit"])
        self.assertNotRegex(json.dumps(self.manifest), r"['\"]arm64['\"]")

    def test_shims_only_the_two_launchers(self):
        self.assertEqual(self.manifest["bin"], ["torlek.ps1", "torlek-wsl.ps1"])

    def test_has_no_install_or_uninstall_script(self):
        for field in [
            "installer",
            "pre_install",
            "post_install",
            "uninstaller",
            "pre_uninstall",
            "post_uninstall",
        ]:
            with self.subTest(field=field):
                self.assertNotIn(field, self.manifest)
                self.assertNotIn(field, self.download)

    def test_notes_disclose_wsl2_and_that_uninstall_keeps_wsl_data(self):
        notes = " ".join(self.manifest["notes"])
        self.assertIn("WSL 2", notes)
        self.assertIn("x86_64", notes)
        self.assertIn("never touches WSL distributions", notes)

    def test_refuses_a_tag_the_repository_does_not_have(self):
        with self.assertRaisesRegex(render.Refusal, "fetch the tag"):
            render.manifest("0.1.0-alpha.2", release_sums(tag="0.1.0-alpha.2").encode(), self.repo)


def head_revision(repo):
    return subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "HEAD"], capture_output=True, text=True, check=True
    ).stdout.strip()


class RehearsalTest(unittest.TestCase):
    """The release workflow's dry run: a -dev.N version, launchers from a revision, loopback URLs."""

    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.work = Path(folder.name)
        self.repo = tagged_repository(self.work)
        self.revision = head_revision(self.repo)
        self.archive = self.work / f"torlek-v{DEV_TAG}-linux-x86_64.tar.gz"
        self.archive.write_bytes(b"archive bytes")
        self.sums = self.work / "SHA256SUMS"
        self.sums.write_text(release_sums(tag=DEV_TAG, x86_64=release.sha256_of(self.archive)))
        self.out = self.work / "served"

    def rehearse(self, tag=DEV_TAG, url=LOOPBACK, archive=None):
        rehearsal = render.Rehearsal(self.revision, url)
        return render.rehearse(
            tag, self.sums, archive or self.archive, rehearsal, self.out, self.repo
        )

    def test_renders_the_dev_version_with_every_download_on_the_loopback_server(self):
        manifest = json.loads(self.rehearse().read_text())
        self.assertEqual(manifest["version"], DEV_TAG)
        for url in manifest["architecture"]["64bit"]["url"]:
            with self.subTest(url=url):
                self.assertTrue(
                    url.startswith(f"{LOOPBACK}/release/") or url.startswith(f"{LOOPBACK}/source/")
                )

    def test_serves_exactly_the_files_the_manifest_names(self):
        manifest = json.loads(self.rehearse().read_text())
        for url, digest in zip(
            manifest["architecture"]["64bit"]["url"],
            manifest["architecture"]["64bit"]["hash"],
            strict=True,
        ):
            served = self.out / url.removeprefix(f"{LOOPBACK}/").split("#", 1)[0]
            with self.subTest(url=url):
                self.assertEqual(hashlib.sha256(served.read_bytes()).hexdigest(), digest)

    def test_binds_the_launchers_to_the_revision_not_a_later_edit(self):
        path = render.LAUNCHER_FILES["@SHA256_TORLEK_PS1@"]
        committed = (self.repo / path).read_bytes()
        (self.repo / path).write_text("# edited after the revision\n")
        git(self.repo, "commit", "--quiet", "-am", "edit")
        self.rehearse()
        self.assertEqual((self.out / "source" / path).read_bytes(), committed)

    def test_the_rehearsed_manifest_has_the_published_shape(self):
        rehearsed = json.loads(self.rehearse().read_text())
        published = json.loads(render.manifest(TAG, release_sums().encode(), self.repo))
        for manifest in (rehearsed, published):
            manifest["version"] = ""
            manifest["architecture"]["64bit"].pop("url")
            manifest["architecture"]["64bit"].pop("hash")
        self.assertEqual(rehearsed, published)

    def test_refuses_a_release_tag(self):
        with self.assertRaisesRegex(render.Refusal, "dry run's X.Y.Z-dev.N"):
            self.rehearse(tag=TAG)

    def test_a_release_render_refuses_a_dev_version(self):
        with self.assertRaisesRegex(render.Refusal, "not a release tag"):
            render.manifest(DEV_TAG, release_sums(tag=DEV_TAG).encode(), self.repo)

    def test_refuses_a_url_that_is_not_loopback(self):
        for url in (
            "https://github.com/Androlax2/agent-cockpit",
            "http://192.0.2.1:8765",
            "http://localhost:8765",
            "http://127.0.0.1",
        ):
            with (
                self.subTest(url=url),
                self.assertRaisesRegex(render.Refusal, "127.0.0.1:PORT only"),
            ):
                self.rehearse(url=url)

    def test_refuses_an_archive_of_another_version(self):
        other = self.work / f"torlek-v{TAG}-linux-x86_64.tar.gz"
        other.write_bytes(b"x")
        with self.assertRaisesRegex(render.Refusal, "expected the file"):
            self.rehearse(archive=other)

    def test_refuses_to_replace_an_existing_manifest(self):
        self.rehearse()
        with self.assertRaisesRegex(render.Refusal, "exists"):
            self.rehearse()


class RenderCommandTest(unittest.TestCase):
    def run_render(self, folder, *extra):
        sums = Path(folder) / "SHA256SUMS"
        sums.write_text(release_sums())
        repo = tagged_repository(folder)
        return subprocess.run(
            [
                sys.executable,
                str(RENDER),
                "render",
                "--tag",
                TAG,
                "--sums",
                str(sums),
                "--repo",
                str(repo),
                *extra,
            ],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_writes_the_manifest(self):
        with tempfile.TemporaryDirectory() as folder:
            out = Path(folder) / "bucket"
            result = self.run_render(folder, "--out", str(out))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads((out / "torlek.json").read_text())["version"], TAG)

    def test_refuses_to_replace_an_existing_manifest(self):
        with tempfile.TemporaryDirectory() as folder:
            (Path(folder) / "torlek.json").write_text("kept\n")
            result = self.run_render(folder, "--out", folder)
            self.assertEqual(result.returncode, 1)
            self.assertIn("exists", result.stderr)
            self.assertEqual((Path(folder) / "torlek.json").read_text(), "kept\n")


def build_archive(folder, tag=TAG, channel="alpha"):
    binaries = Path(folder) / f"binaries-{tag}"
    binaries.mkdir()
    for binary in release.BINARIES:
        (binaries / binary).write_text(f"#!/bin/sh\necho {binary}\n")
    built = release.package(
        tag, channel, "linux-x86_64", REVISION, binaries, Path(folder) / "out", ROOT
    )
    return Path(built["archive"])


class InstallInWslTest(unittest.TestCase):
    """install-in-wsl.sh as torlek-wsl.ps1 starts it inside WSL: its own directory holds the
    Scoop downloads, install.sh among them; uname and id are faked to describe the distribution.
    tools/tests/test_install_script.py covers install.sh's own refusals."""

    @classmethod
    def setUpClass(cls):
        cls.build = tempfile.TemporaryDirectory()
        cls.archive = build_archive(cls.build.name)

    @classmethod
    def tearDownClass(cls):
        cls.build.cleanup()

    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.work = Path(folder.name)
        self.app = self.work / "scoop app"
        self.app.mkdir()
        self.home = self.work / "home"
        self.home.mkdir()
        self.fakes = self.work / "fakes"
        self.fakes.mkdir()
        shutil.copyfile(INSTALL_IN_WSL, self.app / "install-in-wsl.sh")
        shutil.copyfile(INSTALL_SH, self.app / "install.sh")
        shutil.copyfile(self.archive, self.app / f"{BUNDLE}.archive")
        self.write_sums(release.sha256_of(self.archive))
        self.describe_distribution()
        # A logged-out gh, whatever the host has: provenance is install.sh's test's concern.
        self.fake("gh", "exit 1")

    def write_sums(self, digest):
        (self.app / "SHA256SUMS").write_text(release_sums(x86_64=digest))

    def describe_distribution(self, kernel=WSL2_KERNEL, machine="x86_64", uid=1000):
        self.fake(
            "uname",
            f'case "$1" in -r) echo {kernel} ;; -s) echo Linux ;; -m) echo {machine} ;; esac',
        )
        self.fake("id", f'[ "$1" = -u ] && echo {uid}')

    def fake(self, name, body):
        program = self.fakes / name
        program.write_text(f"#!/bin/sh\n{body}\n")
        program.chmod(0o755)

    def run_step(self, action, tag=TAG):
        return subprocess.run(
            ["/bin/sh", "./install-in-wsl.sh", action, tag],
            cwd=self.app,
            capture_output=True,
            text=True,
            check=False,
            env={
                "PATH": f"{self.fakes}:/usr/bin:/bin",
                "HOME": str(self.home),
                "TMPDIR": str(self.work),
            },
        )

    @property
    def install_root(self):
        return self.home / ".local/lib/torlek"

    @property
    def link(self):
        return self.home / ".local/bin/torlek"

    def assert_refused(self, result, message):
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(message, result.stderr)
        self.assertFalse((self.install_root / BUNDLE).exists())
        self.assertFalse(self.link.exists())

    def test_check_reports_a_ready_distribution_without_changing_it(self):
        result = self.run_step("check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("verified; installing would unpack", result.stdout)
        self.assertFalse((self.home / ".local").exists())

    def test_install_unpacks_the_whole_bundle_where_install_sh_does_and_links_it(self):
        result = self.run_step("install")
        self.assertEqual(result.returncode, 0, result.stderr)
        bundle = self.install_root / BUNDLE
        self.assertEqual(os.readlink(self.link), f"{bundle}/bin/torlek")
        manifest = json.loads((bundle / release.MANIFEST_PATH).read_text())
        release.refuse_changed_contents(bundle, manifest)
        self.assertTrue(os.access(self.link, os.X_OK))
        self.assertIn("torlek doctor", result.stdout)

    def test_install_again_changes_nothing(self):
        self.assertEqual(self.run_step("install").returncode, 0)
        marker = self.install_root / f"{BUNDLE}/user-file"
        marker.write_text("kept\n")
        result = self.run_step("install")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("already installed", result.stdout)
        self.assertEqual(marker.read_text(), "kept\n")

    def test_runs_without_the_network(self):
        self.assertNotIn("curl", os.listdir(self.fakes))
        self.fake("curl", "echo curl must not run >&2; exit 7")
        result = self.run_step("install")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("not looked up on GitHub", result.stdout)

    def test_refuses_a_wsl1_or_non_wsl_kernel(self):
        self.describe_distribution(kernel="4.4.0-19041-Microsoft")
        self.assert_refused(self.run_step("install"), "not a WSL 2 kernel")

    def test_refuses_an_aarch64_distribution(self):
        self.describe_distribution(machine="aarch64")
        self.assert_refused(self.run_step("install"), "runs on aarch64")

    def test_refuses_root(self):
        self.describe_distribution(uid=0)
        self.assert_refused(self.run_step("install"), "running as root")

    def test_refuses_a_tampered_archive(self):
        with open(self.app / f"{BUNDLE}.archive", "ab") as archive:
            archive.write(b"\0")
        self.assert_refused(self.run_step("install"), "integrity check failed")

    def test_refuses_sums_that_do_not_list_the_archive_once(self):
        sums = self.app / "SHA256SUMS"
        sums.write_text(sums.read_text() * 2)
        self.assert_refused(self.run_step("check"), "2 times")

    def test_refuses_an_archive_that_writes_outside_its_bundle(self):
        crafted = io.BytesIO()
        with tarfile.open(fileobj=crafted, mode="w:gz") as bundle:
            for name in [f"{BUNDLE}/bin/torlek", f"{BUNDLE}/../escaped"]:
                member = tarfile.TarInfo(name)
                member.size = 0
                member.mode = 0o755
                bundle.addfile(member, io.BytesIO())
        (self.app / f"{BUNDLE}.archive").write_bytes(crafted.getvalue())
        self.write_sums(hashlib.sha256(crafted.getvalue()).hexdigest())
        self.assert_refused(self.run_step("install"), "climbs out of its directory")
        self.assertFalse((self.home / ".local/lib/escaped").exists())

    def test_refuses_an_archive_with_a_link(self):
        crafted = io.BytesIO()
        with tarfile.open(fileobj=crafted, mode="w:gz") as bundle:
            link = tarfile.TarInfo(f"{BUNDLE}/bin")
            link.type = tarfile.SYMTYPE
            link.linkname = "/tmp"
            bundle.addfile(link)
        (self.app / f"{BUNDLE}.archive").write_bytes(crafted.getvalue())
        self.write_sums(hashlib.sha256(crafted.getvalue()).hexdigest())
        self.assert_refused(self.run_step("install"), "holds links or special files")

    def test_refuses_a_tag_outside_the_release_grammar(self):
        self.assert_refused(self.run_step("install", tag="latest"), "is not a Torlek release tag")

    def test_installs_a_dry_runs_dev_version(self):
        dev_archive = build_archive(self.work, DEV_TAG, "dev")
        dev_bundle = f"torlek-v{DEV_TAG}-linux-x86_64"
        shutil.copyfile(dev_archive, self.app / f"{dev_bundle}.archive")
        (self.app / "SHA256SUMS").write_text(
            f"{release.sha256_of(dev_archive)}  {dev_bundle}.tar.gz\n"
        )
        result = self.run_step("install", tag=DEV_TAG)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(os.readlink(self.link), f"{self.install_root / dev_bundle}/bin/torlek")


class LauncherConsentTest(unittest.TestCase):
    """No pwsh here: these read the PowerShell source. Nothing in it ran on Windows."""

    STRING_OR_COMMENT = re.compile(r"'(?:[^']|'')*'|\"(?:[^\"`]|`.)*\"|#[^\n]*")

    def code_of(self, name):
        return self.STRING_OR_COMMENT.sub("''", (WINDOWS / name).read_text())

    def test_wsl_is_only_ever_asked_to_list_or_run(self):
        for name in ["torlek.ps1", "torlek-wsl.ps1"]:
            for line in (WINDOWS / name).read_text().splitlines():
                if "& wsl.exe" not in line:
                    continue
                with self.subTest(script=name, line=line.strip()):
                    options = set(re.findall(r"(?<!\w)--[a-z-]+", line))
                    self.assertLessEqual(
                        options, {"--distribution", "--exec", "--cd", "--list", "--quiet"}
                    )

    def test_no_feature_elevation_service_or_environment_change(self):
        forbidden = re.compile(
            r"Enable-WindowsOptionalFeature|dism|RunAs|Start-Process|New-Service|Set-Service|"
            r"SetEnvironmentVariable|Set-ItemProperty|New-ItemProperty|Remove-Item|--unregister",
            re.IGNORECASE,
        )
        for name in ["torlek.ps1", "torlek-wsl.ps1"]:
            with self.subTest(script=name):
                self.assertIsNone(forbidden.search(self.code_of(name)))

    def test_install_runs_only_after_the_consent_prompt(self):
        source = (WINDOWS / "torlek-wsl.ps1").read_text()
        self.assertLess(source.index("Read-Host"), source.index("Invoke-InDistribution 'install'"))
        self.assertIn("Nothing was installed.", source)


@unittest.skipUnless(
    os.name == "posix" and shutil.which("pwsh"),
    "needs PowerShell 7 on a POSIX host: the fake wsl.exe is a shell script",
)
class LauncherPromptTest(unittest.TestCase):
    """Runs torlek-wsl.ps1 under pwsh against a fake wsl.exe that logs what it is asked to run."""

    FAKE_WSL = """#!/bin/sh
case "$1" in --list) echo Ubuntu-24.04; exit 0;; esac
echo "$@" >> "$FAKE_WSL_LOG"
"""
    INSTALL_CALL = re.compile(r"install-in-wsl\.sh install ")

    def setUp(self):
        directory = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, directory)
        self.bin = directory / "bin"
        self.bin.mkdir()
        (self.bin / "wsl.exe").write_text(self.FAKE_WSL)
        (self.bin / "wsl.exe").chmod(0o755)
        self.script = directory / "torlek-wsl.ps1"
        shutil.copy(WINDOWS / "torlek-wsl.ps1", self.script)
        (directory / "torlek-v1.0.0-alpha.1-linux-x86_64.archive").touch()
        self.log = directory / "wsl.log"

    def install(self, stdin, *extra):
        environment = dict(
            os.environ,
            PATH=f"{self.bin}{os.pathsep}{os.environ['PATH']}",
            FAKE_WSL_LOG=str(self.log),
        )
        environment.pop("TORLEK_WSL_DISTRIBUTION", None)
        return subprocess.run(
            [
                "pwsh",
                "-NoProfile",
                "-File",
                str(self.script),
                "install",
                "-Distribution",
                "Ubuntu-24.04",
                *extra,
            ],
            input=stdin,
            env=environment,
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )

    def installed(self):
        return bool(self.log.exists() and self.INSTALL_CALL.search(self.log.read_text()))

    def test_anything_but_an_explicit_yes_installs_nothing(self):
        for answer in ["", "\n", "n\n", "no\n", "maybe\n"]:
            with self.subTest(answer=answer):
                self.log.unlink(missing_ok=True)
                result = self.install(answer)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("Nothing was installed.", result.stderr)
                self.assertFalse(self.installed())

    def test_yes_installs(self):
        for answer in ["y\n", "yes\n"]:
            with self.subTest(answer=answer):
                self.log.unlink(missing_ok=True)
                result = self.install(answer)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertTrue(self.installed())

    def test_the_yes_switch_skips_the_prompt(self):
        result = self.install("", "-Yes")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(self.installed())


if __name__ == "__main__":
    unittest.main()
