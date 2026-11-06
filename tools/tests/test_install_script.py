"""install.sh through its real entry point: a shell runs it with only the tools it may use on
PATH, against a loopback fake release server (TORLEK_INSTALL_ORIGIN) and a throwaway HOME.
Every test runs under /bin/sh and again under dash, Debian's /bin/sh."""

import hashlib
import importlib.util
import io
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
INSTALL_SH = ROOT / "install.sh"
REVISION = "d" * 40
OLDER = "v0.1.0-alpha.2"
NEWER = "v0.1.0-alpha.3"
DRY_RUN = "v0.1.0-dev.4"
# What install.sh may call; uname, id and gh are fakes the tests describe.
TOOLS = (
    "awk",
    "cat",
    "curl",
    "cut",
    "dirname",
    "grep",
    "gzip",
    "ln",
    "mkdir",
    "mktemp",
    "mv",
    "readlink",
    "rm",
    "rmdir",
    "sed",
    "sha256sum",
    "sort",
    "tar",
    "tr",
)


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


sys.path.insert(0, str(ROOT / "tools"))
release = load("release", ROOT / "tools/release.py")
fake_server = load("fake_release_server", Path(__file__).with_name("fake_release_server.py"))


def bundle_name(tag, platform="linux-x86_64"):
    return f"torlek-{tag}-{platform}"


def build_archive(folder, tag):
    channel = "dev" if "-dev." in tag else "alpha"
    binaries = Path(folder) / f"binaries-{tag}"
    binaries.mkdir()
    for binary in release.BINARIES:
        (binaries / binary).write_text(f"#!/bin/sh\necho {binary} {tag}\n")
    built = release.package(
        tag.removeprefix("v"), channel, "linux-x86_64", REVISION, binaries, Path(folder), ROOT
    )
    return Path(built["archive"])


def crafted_archive(names):
    """A .tar.gz of empty executable files and one symbolic link per `name -> target` entry."""
    crafted = io.BytesIO()
    with tarfile.open(fileobj=crafted, mode="w:gz") as bundle:
        for name in names:
            name, _, target = name.partition(" -> ")
            member = tarfile.TarInfo(name)
            if target:
                member.type = tarfile.SYMTYPE
                member.linkname = target
            else:
                member.mode = 0o755
            bundle.addfile(member, io.BytesIO())
    return crafted.getvalue()


class InstallFixture(unittest.TestCase):
    shell = "/bin/sh"

    @classmethod
    def setUpClass(cls):
        cls.build = tempfile.TemporaryDirectory()
        cls.archives = {tag: build_archive(cls.build.name, tag) for tag in (OLDER, NEWER)}

    @classmethod
    def tearDownClass(cls):
        cls.build.cleanup()

    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.work = Path(folder.name)
        self.home = self.work / "home with space"
        self.home.mkdir()
        self.temporary = self.work / "tmp"
        self.temporary.mkdir()
        self.releases = self.work / "releases"
        self.releases.mkdir()
        self.publish()
        server, self.origin = fake_server.start(self.releases)
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.tools = self.work / "tools"
        self.tools.mkdir()
        for tool in TOOLS:
            found = shutil.which(tool)
            self.assertIsNotNone(found, f"{tool} is needed to run install.sh")
            (self.tools / tool).symlink_to(found)
        self.describe_machine()
        self.fake("id", '[ "$1" = -u ] && echo 1000')

    def publish(self, *tags, body="notes, with commas"):
        """Lists TAGS, newest first, and serves each one's archive and SHA256SUMS."""
        entries = []
        for tag in tags:
            folder = self.releases / tag
            folder.mkdir(exist_ok=True)
            archive = self.archives[tag]
            shutil.copyfile(archive, folder / archive.name)
            (folder / "SHA256SUMS").write_text(
                f"{release.sha256_of(archive)}  {archive.name}\n{'3' * 64}  install.sh\n"
            )
            entries.append({**fake_server.release_entry(tag), "body": body})
        (self.releases / "releases.json").write_text(json.dumps(entries))

    def describe_machine(self, system="Linux", machine="x86_64"):
        self.fake("uname", f'case "$1" in -s) echo {system} ;; -m) echo {machine} ;; esac')

    def fake(self, name, body):
        program = self.tools / name
        program.unlink(missing_ok=True)
        program.write_text(f"#!/bin/sh\n{body}\n")
        program.chmod(0o755)

    def install(self, *arguments, origin=None, with_bin_on_path=True):
        path = str(self.tools)
        if with_bin_on_path:
            path = f"{self.home}/.local/bin:{path}"
        environment = {"PATH": path, "HOME": str(self.home), "TMPDIR": str(self.temporary)}
        environment["TORLEK_INSTALL_ORIGIN"] = self.origin if origin is None else origin
        if not environment["TORLEK_INSTALL_ORIGIN"]:
            del environment["TORLEK_INSTALL_ORIGIN"]
        return subprocess.run(
            [self.shell, str(INSTALL_SH), *arguments],
            cwd=self.work,
            capture_output=True,
            text=True,
            check=False,
            env=environment,
            timeout=60,
        )

    @property
    def library(self):
        return self.home / ".local/lib/torlek"

    @property
    def link(self):
        return self.home / ".local/bin/torlek"

    def requests(self):
        log = self.releases / "requests.log"
        return log.read_text().splitlines() if log.exists() else []

    def assert_installed(self, tag):
        bundle = self.library / bundle_name(tag)
        self.assertEqual(os.readlink(self.link), f"{bundle}/bin/torlek")
        manifest = json.loads((bundle / release.MANIFEST_PATH).read_text())
        release.refuse_changed_contents(bundle, manifest)
        ran = subprocess.run([self.link], capture_output=True, text=True, check=True)
        self.assertEqual(ran.stdout, f"torlek {tag}\n")

    def assert_refused(self, result, message):
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(message, result.stderr)
        self.assertFalse((self.home / ".local").exists())
        self.assertEqual(list(self.temporary.iterdir()), [])


class InstallScriptTest(InstallFixture):
    def test_a_fresh_install_takes_the_newest_alpha_and_links_it(self):
        self.publish(NEWER, OLDER)
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_installed(NEWER)
        self.assertIn("checksum matches SHA256SUMS", result.stdout)
        self.assertEqual(list(self.temporary.iterdir()), [])

    def test_a_fresh_install_ends_with_doctor_and_daemon_install(self):
        self.publish(NEWER)
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('next: run "torlek doctor", then "torlek daemon install"', result.stdout)

    def test_install_touches_nothing_in_home_but_the_library_and_the_link(self):
        self.publish(NEWER)
        self.assertEqual(self.install().returncode, 0)
        touched = sorted(str(path.relative_to(self.home)) for path in self.home.rglob("*"))
        outside = [path for path in touched if not path.startswith(".local/lib/torlek")]
        self.assertEqual(outside, [".local", ".local/bin", ".local/bin/torlek", ".local/lib"])

    def test_the_default_is_the_newest_alpha_even_when_a_beta_is_newer(self):
        self.publish(NEWER)
        listed = json.loads((self.releases / "releases.json").read_text())
        beta = fake_server.release_entry("v0.2.0-beta.1")
        (self.releases / "releases.json").write_text(json.dumps([beta, *listed]))
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_installed(NEWER)

    def test_release_notes_cannot_forge_the_newest_tag(self):
        self.publish(
            NEWER, body='see ,"tag_name":"v9.9.9-alpha.9", and {"tag_name":"v9.9.9-alpha.9"}'
        )
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_installed(NEWER)

    def test_a_rerun_after_a_new_release_upgrades_and_keeps_the_previous_one(self):
        self.publish(OLDER)
        self.assertEqual(self.install().returncode, 0)
        self.publish(NEWER, OLDER)
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_installed(NEWER)
        self.assertTrue((self.library / bundle_name(OLDER) / "bin/torlek").is_file())
        self.assertIn(
            f"the previous release stays in {self.library / bundle_name(OLDER)}", result.stdout
        )

    def test_a_rerun_without_a_new_release_changes_and_downloads_nothing(self):
        self.publish(NEWER)
        self.assertEqual(self.install().returncode, 0)
        before = self.requests()
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("already installed", result.stdout)
        self.assertEqual(self.requests()[len(before) :], ["/api/releases?per_page=100"])
        self.assert_installed(NEWER)

    def test_a_pinned_version_installs_that_release(self):
        self.publish(NEWER, OLDER)
        result = self.install("--version", OLDER)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_installed(OLDER)

    def test_a_pinned_older_version_already_unpacked_is_linked_without_a_download(self):
        self.publish(NEWER, OLDER)
        self.assertEqual(self.install("--version", OLDER).returncode, 0)
        self.assertEqual(self.install().returncode, 0)
        before = self.requests()
        result = self.install("--version", OLDER)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_installed(OLDER)
        self.assertEqual(self.requests()[len(before) :], [f"/api/releases/tags/{OLDER}"])

    def test_an_unpublished_tag_is_refused(self):
        self.publish(NEWER)
        result = self.install("--version", "v0.1.0-alpha.9")
        self.assert_refused(result, "v0.1.0-alpha.9 is not a published Torlek release")
        self.assertFalse(any(path.startswith("/download/") for path in self.requests()))

    def test_a_version_that_is_not_a_release_tag_is_refused_before_any_request(self):
        for version in ("latest", "0.1.0-alpha.3", "v0.1.0-alpha.3; rm -rf ~", "v0.1.0"):
            with self.subTest(version=version):
                result = self.install("--version", version)
                self.assert_refused(result, "is not a Torlek release tag")
                self.assertEqual(self.requests(), [])

    def test_a_dry_run_tag_is_refused_for_a_download_from_github(self):
        self.assert_refused(
            self.install("--version", "v0.1.0-dev.4", origin=""), "is not a Torlek release tag"
        )

    def test_a_checksum_mismatch_is_refused_before_unpacking(self):
        self.publish(NEWER)
        sums = self.releases / NEWER / "SHA256SUMS"
        sums.write_text(sums.read_text().replace(release.sha256_of(self.archives[NEWER]), "0" * 64))
        self.assert_refused(self.install(), "integrity check failed")

    def test_sums_that_do_not_list_the_archive_once_are_refused(self):
        self.publish(NEWER)
        sums = self.releases / NEWER / "SHA256SUMS"
        sums.write_text(sums.read_text() * 2)
        self.assert_refused(self.install(), "2 times")

    def test_root_is_refused_before_any_request(self):
        self.publish(NEWER)
        self.fake("id", '[ "$1" = -u ] && echo 0')
        self.assert_refused(self.install(), "running as root")
        self.assertEqual(self.requests(), [])

    def test_an_unsupported_architecture_is_refused(self):
        self.publish(NEWER)
        for machine in ("riscv64", "armv7l", "i686"):
            with self.subTest(machine=machine):
                self.describe_machine(machine=machine)
                self.assert_refused(self.install(), f"this machine is {machine}")
                self.assertEqual(self.requests(), [])

    def test_an_unsupported_system_is_refused(self):
        self.publish(NEWER)
        self.describe_machine(system="Darwin", machine="arm64")
        self.assert_refused(self.install(), "this is Darwin")

    def test_a_missing_tool_is_named(self):
        self.publish(NEWER)
        for tool in ("curl", "tar", "sha256sum"):
            with self.subTest(tool=tool):
                (self.tools / tool).unlink()
                self.assert_refused(self.install(), f"{tool} is missing")
                (self.tools / tool).symlink_to(shutil.which(tool))

    def test_a_bin_directory_missing_from_path_is_a_warning(self):
        self.publish(NEWER)
        result = self.install(with_bin_on_path=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"warning: {self.home}/.local/bin is not in your PATH", result.stderr)
        self.assertIn(f'next: run "{self.link} doctor"', result.stdout)

    def test_a_bin_directory_on_path_gives_no_warning(self):
        self.publish(NEWER)
        result = self.install()
        self.assertNotIn("not in your PATH", result.stderr)

    def test_a_torlek_that_is_not_a_release_link_is_kept(self):
        self.publish(NEWER)
        self.link.parent.mkdir(parents=True)
        self.link.write_text("#!/bin/sh\necho my own build\n")
        result = self.install()
        self.assertEqual(result.returncode, 1)
        self.assertIn("is not a link to a Torlek release", result.stderr)
        self.assertEqual(self.link.read_text(), "#!/bin/sh\necho my own build\n")
        self.assertFalse(self.library.exists())

    def test_a_link_to_something_else_is_kept(self):
        self.publish(NEWER)
        self.link.parent.mkdir(parents=True)
        self.link.symlink_to("/usr/lib/torlek/bin/torlek")
        result = self.install()
        self.assertEqual(result.returncode, 1)
        self.assertIn("points at /usr/lib/torlek/bin/torlek", result.stderr)
        self.assertEqual(os.readlink(self.link), "/usr/lib/torlek/bin/torlek")

    def test_the_readme_s_manual_install_is_upgraded_in_place(self):
        self.publish(OLDER)
        bundle = self.library / bundle_name(OLDER)
        with tarfile.open(self.archives[OLDER]) as archive:
            archive.extractall(self.library, filter="data")
        self.link.parent.mkdir(parents=True)
        self.link.symlink_to(bundle / "bin/torlek")
        self.publish(NEWER, OLDER)
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_installed(NEWER)

    def test_check_verifies_and_changes_nothing(self):
        self.publish(NEWER)
        result = self.install("--check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("verified; installing would unpack", result.stdout)
        self.assertFalse((self.home / ".local").exists())

    def test_an_archive_member_outside_its_bundle_is_refused(self):
        self.publish(NEWER)
        bundle = bundle_name(NEWER)
        self.serve_crafted([f"{bundle}/bin/torlek", f"{bundle}/../escaped"])
        self.assert_refused(self.install(), "climbs out of its directory")

    def test_an_archive_with_a_link_is_refused(self):
        self.publish(NEWER)
        self.serve_crafted([f"{bundle_name(NEWER)}/bin -> /tmp"])
        self.assert_refused(self.install(), "holds links or special files")

    def serve_crafted(self, names):
        archive = crafted_archive(names)
        folder = self.releases / NEWER
        (folder / f"{bundle_name(NEWER)}.tar.gz").write_bytes(archive)
        digest = hashlib.sha256(archive).hexdigest()
        (folder / "SHA256SUMS").write_text(f"{digest}  {bundle_name(NEWER)}.tar.gz\n")

    def test_an_origin_other_than_loopback_is_refused(self):
        for origin in (
            "https://example.com",
            "http://127.0.0.1.example.com:80",
            "http://localhost:1",
        ):
            with self.subTest(origin=origin):
                self.assert_refused(self.install(origin=origin), "must be http://127.0.0.1:PORT")


class ProvenanceTest(InstallFixture):
    """Without the test server: GitHub's routes are not reachable here, so these give the
    release's files with --archive and --sums, the route install-in-wsl.sh takes."""

    def local_install(self):
        archive = self.archives[NEWER]
        sums = self.work / "SHA256SUMS"
        sums.write_text(f"{release.sha256_of(archive)}  {archive.name}\n")
        return self.install(
            "--version", NEWER, "--archive", str(archive), "--sums", str(sums), origin=""
        )

    def fake_gh(self, logged_in=True, verified=True):
        calls = self.work / "gh-calls"
        self.fake(
            "gh",
            f'printf "%s\\n" "$*" >> "{calls}"\n'
            f'[ "$1 $2" = "auth status" ] && exit {0 if logged_in else 1}\n'
            f'[ "$1 $2" = "attestation verify" ] && exit {0 if verified else 1}\n'
            "exit 2",
        )
        return calls

    def test_without_gh_provenance_is_said_to_be_skipped(self):
        result = self.local_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("provenance not checked: gh is not installed", result.stdout)
        self.assert_installed(NEWER)

    def test_a_logged_out_gh_is_said_to_be_skipped(self):
        calls = self.fake_gh(logged_in=False)
        result = self.local_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("provenance not checked: gh is not logged in", result.stdout)
        self.assertEqual(calls.read_text(), "auth status\n")

    def test_gh_verifies_the_archive_against_the_release_workflow_and_tag(self):
        calls = self.fake_gh()
        result = self.local_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("provenance verified by gh", result.stdout)
        verify = calls.read_text().splitlines()[1]
        self.assertEqual(
            verify,
            f"attestation verify {self.archives[NEWER]} --repo Androlax2/agent-cockpit "
            "--signer-workflow Androlax2/agent-cockpit/.github/workflows/release.yml "
            f"--source-ref refs/tags/{NEWER} --deny-self-hosted-runners",
        )

    def test_a_failed_attestation_is_refused_before_unpacking(self):
        self.fake_gh(verified=False)
        self.assert_refused(self.local_install(), "gh attestation verify refused")

    def test_a_dry_run_s_files_install_and_gh_is_not_asked_for_a_tag_they_lack(self):
        """The release workflow's install-scoop job hands install-in-wsl.sh a -dev.N archive."""
        calls = self.fake_gh()
        archive = build_archive(self.work, DRY_RUN)
        sums = self.work / "SHA256SUMS"
        sums.write_text(f"{release.sha256_of(archive)}  {archive.name}\n")
        result = self.install(
            "--version", DRY_RUN, "--archive", str(archive), "--sums", str(sums), origin=""
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"provenance not checked: a dry run's {DRY_RUN}", result.stdout)
        self.assertFalse(calls.exists())
        self.assert_installed(DRY_RUN)


def dash():
    found = shutil.which("dash")
    if found:
        return found
    if os.environ.get("CI") == "true":
        raise AssertionError("dash is required in CI: install.sh must run under Debian's /bin/sh")
    raise unittest.SkipTest("dash is not installed here; CI's Ubuntu runner has it")


class InstallScriptUnderDashTest(InstallScriptTest):
    @classmethod
    def setUpClass(cls):
        cls.shell = dash()
        super().setUpClass()


class ProvenanceUnderDashTest(ProvenanceTest):
    @classmethod
    def setUpClass(cls):
        cls.shell = dash()
        super().setUpClass()


if __name__ == "__main__":
    unittest.main()
