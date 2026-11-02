"""Upgrade entry points against fake GitHub and isolated executable bundle fixtures."""

import importlib.util
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/release_upgrade.py"
SPEC = importlib.util.spec_from_file_location("release", ROOT / "tools/release.py")
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


class UpgradeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="upgrade-fixture-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.metadata = self.root / "releases.json"
        self.metadata.write_text("[]")
        self.assets = self.root / "assets"
        self.assets.mkdir()
        self.events = self.root / "events.jsonl"
        self.gh_log = self.root / "gh.log"
        gh = self.bin / "gh"
        gh.write_text(
            f"#!{sys.executable}\nimport json, pathlib, shutil, sys\n"
            f"with open({str(self.gh_log)!r}, 'a') as log: log.write(repr(sys.argv[1:]) + '\\n')\n"
            "if sys.argv[1:4] == ['api', '--paginate', '--slurp']:\n"
            f"    assert sys.argv[4] == 'repos/Androlax2/agent-cockpit/releases'\n"
            f"    print(pathlib.Path({str(self.metadata)!r}).read_text())\n"
            "elif sys.argv[1] == 'api':\n"
            "    assert sys.argv[2] == 'repos/Androlax2/agent-cockpit/commits/v0.1.0-beta.1'\n"
            "    print(json.dumps({'sha': '1' * 40}))\n"
            "else:\n"
            "    assert sys.argv[1:3] == ['release', 'download']\n"
            "    assert sys.argv[sys.argv.index('--repo') + 1] == 'Androlax2/agent-cockpit'\n"
            f"    source = pathlib.Path({str(self.assets)!r}) / sys.argv[3]\n"
            "    destination = pathlib.Path(sys.argv[sys.argv.index('--dir') + 1])\n"
            "    for path in source.iterdir():\n        if path.is_file(): shutil.copyfile(path, destination / path.name)\n"
        )
        gh.chmod(0o755)
        self.environment = {
            "HOME": str(self.root),
            "PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}",
            "GH_TOKEN": "fixture-token-not-a-credential",
        }

    def run_tool(self, *arguments):
        return subprocess.run(
            [sys.executable, str(SCRIPT), *arguments],
            text=True,
            capture_output=True,
            env=self.environment,
            timeout=20,
            check=False,
        )

    def test_selects_the_latest_published_earlier_release_of_the_same_channel(self):
        self.metadata.write_text(
            json.dumps(
                [
                    [
                        {"tag_name": "v0.1.0-beta.3", "draft": False},
                        {"tag_name": "v0.1.0-alpha.99", "draft": False},
                    ],
                    [
                        {"tag_name": "v0.1.0-beta.8", "draft": True},
                        {"tag_name": "v0.1.0-beta.11", "draft": False},
                        {"tag_name": "v0.1.0-beta.2", "draft": False},
                    ],
                ]
            )
        )
        result = self.run_tool("select", "--version", "0.1.0-beta.10")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "v0.1.0-beta.3")

    def test_missing_previous_beta_is_a_named_failure(self):
        result = self.run_tool("select", "--version", "0.1.0-beta.1")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no published earlier beta release", result.stderr)

    def bundle(self, version, revision, *, fail_start=False, lose_data=False):
        binaries = self.root / (version + "-bin")
        binaries.mkdir()
        common = f"""#!{sys.executable}
import json, os, pathlib, sys, time
root = pathlib.Path(__file__).resolve().parents[1]
manifest = json.loads((root / 'share/cockpit/manifest.json').read_text())
home = pathlib.Path(os.environ['HOME'])
assert 'GH_TOKEN' not in os.environ
"""
        cli = (
            common
            + f"""
args = sys.argv[1:]
if args == ['version', '--all']:
    print('cockpit ' + manifest['version'] + ' (revision ' + manifest['source_revision'] + ')')
    print((root / 'share/cockpit/manifest.json').read_text(), end='')
    sys.exit(0)
assert args[:2] == ['--profile', 'release-gate'], args
args = args[2:]
os.kill(int((home / 'daemon-live').read_text()), 0)
tasks = home / 'tasks.json'
items = json.loads(tasks.read_text()) if tasks.exists() else []
if args[:2] == ['project', 'add']:
    assert pathlib.Path(args[2]).is_dir()
elif args[:2] == ['todo', 'add']:
    items.append([len(items) + 1, 'upgrade', 'open', 0, 'task', args[2]])
    tasks.write_text(json.dumps(items))
elif args[:2] == ['todo', 'list']:
    for item in ([] if {lose_data!r} else items): print('\\t'.join(map(str, item)))
else:
    raise AssertionError(args)
"""
        )
        daemon = (
            common
            + f"""
assert sys.argv[1:] == ['--profile', 'release-gate'], sys.argv
pidfile = home / 'daemon-live'
if pidfile.exists():
    try: os.kill(int(pidfile.read_text()), 0)
    except ProcessLookupError: pass
    else: raise AssertionError('old and new daemons overlap')
with open({str(self.events)!r}, 'a') as events:
    events.write(json.dumps({{'version': manifest['version'], 'home': str(home), 'pid': os.getpid()}}) + '\\n')
if {fail_start!r}:
    print('fixture startup failure', file=sys.stderr)
    sys.exit(1)
pidfile.write_text(str(os.getpid()))
print('ready ' + str(home / 'run/cockpit-release-gate/daemon.sock'), flush=True)
time.sleep(60)
"""
        )
        for name in release.BINARIES:
            path = binaries / name
            path.write_text(
                cli
                if name == "cockpit"
                else daemon
                if name == "cockpit-daemon"
                else "#!/bin/sh\nexit 0\n"
            )
            path.chmod(0o755)
        output = self.assets / ("v" + version)
        output.mkdir()
        packaged = release.package(
            version,
            release.parse_tag("v" + version)["channel"],
            "linux-x86_64",
            revision,
            binaries,
            output,
            ROOT,
        )
        archive = Path(packaged["archive"])
        (output / "SHA256SUMS").write_text(f"{release.sha256_of(archive)}  {archive.name}\n")
        return Path(packaged["root"]), archive

    def prepare_upgrade(self, **old_options):
        old, archive = self.bundle("0.1.0-beta.1", "1" * 40, **old_options)
        self.metadata.write_text(json.dumps([[{"tag_name": "v0.1.0-beta.1", "draft": False}]]))
        return old, archive

    def verify(self, candidate):
        return self.run_tool("verify", "--candidate", str(candidate), "--revision", "2" * 40)

    def test_upgrade_retains_state_stops_before_start_and_cleans_its_home(self):
        self.prepare_upgrade()
        candidate, _ = self.bundle("0.1.0-beta.2", "2" * 40)
        result = self.verify(candidate)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("upgrade=passed", result.stdout)
        events = [json.loads(line) for line in self.events.read_text().splitlines()]
        self.assertEqual([event["version"] for event in events], ["0.1.0-beta.1", "0.1.0-beta.2"])
        self.assertEqual(events[0]["home"], events[1]["home"])
        for event in events:
            self.assertFalse(Path(event["home"]).exists())
            with self.assertRaises(ProcessLookupError):
                os.kill(event["pid"], 0)

    def test_lost_state_is_not_a_successful_upgrade(self):
        self.prepare_upgrade()
        candidate, _ = self.bundle("0.1.0-beta.2", "2" * 40, lose_data=True)
        result = self.verify(candidate)
        self.assertEqual(result.returncode, 1)
        self.assertIn("retained task", result.stderr)

    def test_startup_failure_is_reported_and_does_not_start_the_candidate(self):
        self.prepare_upgrade(fail_start=True)
        candidate, _ = self.bundle("0.1.0-beta.2", "2" * 40)
        result = self.verify(candidate)
        self.assertEqual(result.returncode, 1)
        self.assertIn("fixture startup failure", result.stderr)
        events = [json.loads(line) for line in self.events.read_text().splitlines()]
        self.assertEqual(len(events), 1)
        self.assertFalse(Path(events[0]["home"]).exists())

    def test_bad_archive_checksum_prevents_executing_the_baseline(self):
        _, archive = self.prepare_upgrade()
        archive.write_bytes(archive.read_bytes() + b"tampered")
        candidate, _ = self.bundle("0.1.0-beta.2", "2" * 40)
        result = self.verify(candidate)
        self.assertEqual(result.returncode, 1)
        self.assertIn("checksum", result.stderr)
        self.assertFalse(self.events.exists())

    def test_alpha_explicitly_reports_its_exemption_without_github_access(self):
        candidate, _ = self.bundle("0.1.0-alpha.2", "2" * 40)
        result = self.verify(candidate)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("upgrade=not-run", result.stdout)
        self.assertIn("alpha", result.stdout)
        self.assertFalse(self.gh_log.exists())

    def test_offline_verification_uses_the_downloaded_baseline_without_github_access(self):
        self.prepare_upgrade()
        downloads = self.root / "downloaded"
        fetched = self.run_tool(
            "fetch",
            "--version",
            "0.1.0-beta.2",
            "--channel",
            "beta",
            "--platform",
            "linux-x86_64",
            "--out",
            str(downloads),
        )
        self.assertEqual(fetched.returncode, 0, fetched.stderr)
        self.gh_log.unlink()
        candidate, _ = self.bundle("0.1.0-beta.2", "2" * 40)
        result = self.run_tool(
            "verify",
            "--candidate",
            str(candidate),
            "--revision",
            "2" * 40,
            "--downloads",
            str(downloads),
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.gh_log.exists())

    def test_archive_links_and_parent_traversal_are_refused_before_execution(self):
        _, archive = self.prepare_upgrade()
        candidate, _ = self.bundle("0.1.0-beta.2", "2" * 40)
        for name, kind in (
            ("../escaped", tarfile.REGTYPE),
            ("cockpit-v0.1.0-beta.1-linux-x86_64/link", tarfile.SYMTYPE),
        ):
            with self.subTest(name=name):
                with tarfile.open(archive, "w:gz") as bundle:
                    member = tarfile.TarInfo(name)
                    member.type = kind
                    member.linkname = "../../../escaped"
                    bundle.addfile(member, io.BytesIO())
                (archive.parent / "SHA256SUMS").write_text(
                    f"{release.sha256_of(archive)}  {archive.name}\n"
                )
                result = self.verify(candidate)
                self.assertEqual(result.returncode, 1)
                self.assertIn("unsafe", result.stderr)
                self.assertFalse(self.events.exists())

    def test_malformed_metadata_is_a_named_refusal(self):
        for value in ({}, [None], [[None]]):
            self.metadata.write_text(json.dumps(value))
            result = self.run_tool("select", "--version", "0.1.0-beta.2")
            self.assertEqual(result.returncode, 1)
            self.assertIn("invalid release", result.stderr)
            self.assertNotIn("Traceback", result.stderr)

    def test_a_dry_run_records_an_exemption_artifact_without_fetching_a_release(self):
        output = self.root / "dry-run"
        result = self.run_tool(
            "fetch",
            "--version",
            "0.1.0-dev.132",
            "--channel",
            "dev",
            "--platform",
            "linux-x86_64",
            "--out",
            str(output),
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("upgrade=not-run", result.stdout)
        self.assertTrue((output / "not-run.txt").is_file())
        self.assertFalse(self.gh_log.exists())

    def test_an_offline_baseline_cannot_switch_channel_or_upgrade_backwards(self):
        self.prepare_upgrade()
        candidate, _ = self.bundle("0.1.0-beta.2", "2" * 40)
        downloads = self.root / "wrong-baseline"
        downloads.mkdir()
        for tag in ("v0.1.0-alpha.1", "v0.1.0-beta.2", "v0.1.0-beta.3"):
            with self.subTest(tag=tag):
                (downloads / "baseline.json").write_text(
                    json.dumps({"tag": tag, "revision": "1" * 40})
                )
                result = self.run_tool(
                    "verify",
                    "--candidate",
                    str(candidate),
                    "--revision",
                    "2" * 40,
                    "--downloads",
                    str(downloads),
                )
                self.assertEqual(result.returncode, 1)
                self.assertIn("earlier release of the same channel", result.stderr)
                self.assertFalse(self.events.exists())

    def test_candidate_revision_and_channel_are_checked_before_any_download(self):
        candidate, _ = self.bundle("0.1.0-beta.2", "2" * 40)
        result = self.run_tool("verify", "--candidate", str(candidate), "--revision", "3" * 40)
        self.assertEqual(result.returncode, 1)
        self.assertIn("source revision", result.stderr)
        manifest = candidate / release.MANIFEST_PATH
        data = json.loads(manifest.read_text())
        data["channel"] = "alpha"
        manifest.write_text(json.dumps(data))
        result = self.verify(candidate)
        self.assertEqual(result.returncode, 1)
        self.assertIn("channel disagree", result.stderr)
        self.assertFalse(self.gh_log.exists())


if __name__ == "__main__":
    unittest.main()
