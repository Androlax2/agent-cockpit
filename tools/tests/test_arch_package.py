import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

DIRECTORY = Path(__file__).resolve().parents[2] / "packaging/arch"
PACKAGER = DIRECTORY / "package.py"
SOURCE = "a" * 40
VERSION = "0.0.0.test"
BINARIES = [
    "ask-to-guide",
    "ask-to-run",
    "claude-audit",
    "cockpit-codex-hook",
    "cockpit-demo",
    "cockpit-design",
    "cockpit-fade",
    "cockpit-failsafe",
    "cockpit-guide",
    "cockpit-remote",
    "cockpit-run-approve",
    "cockpit-sidebar",
    "cockpit-subscription",
    "cockpit-team",
    "cockpit-todo",
    "cockpit-idle-prompt",
    "osd",
]
EXTRA_FILES = [
    "LICENSE",
    "THIRD_PARTY_NOTICES.txt",
    "components/licenses/greywall/LICENSE",
    "components/licenses/greyproxy/LICENSE",
    "components/licenses/tun2socks/LICENSE",
    "assets/LICENSE-LUCIDE",
    "assets/LICENSE-FEATHER",
    "assets/osd/style.css",
    "assets/idle/style.css",
    *[
        f"assets/icons/{name}.svg"
        for name in [
            "volume-mute",
            "volume-low",
            "volume-mid",
            "volume-high",
            "bell-dot",
            "bell",
            "brightness",
        ]
    ],
]


class ArchPackageTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)
        self.payload = self.root / "runtime"
        self.payload.mkdir()
        self.destination = self.root / "package"
        paths = []
        for name in ["runtime-files.txt", "integration-files.txt"]:
            paths.extend((DIRECTORY / name).read_text().splitlines())
        paths.extend(f"apps/target/release/{name}" for name in BINARIES)
        paths.extend(["components/greywall", "components/greyproxy", *EXTRA_FILES])
        files = {}
        for relative in paths:
            path = self.payload / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(f"fixture {relative}\n")
            path.chmod(
                0o755
                if relative.startswith(("apps/target/", "config/bin/", "components/grey"))
                else 0o644
            )
            files[relative] = hashlib.sha256(path.read_bytes()).hexdigest()
        (self.payload / "LICENSE").write_bytes((DIRECTORY.parents[1] / "LICENSE").read_bytes())
        files["LICENSE"] = hashlib.sha256((self.payload / "LICENSE").read_bytes()).hexdigest()
        self.manifest = {
            "version": VERSION,
            "source": SOURCE,
            "target": "x86_64-unknown-linux-gnu",
            "components": {
                "greywall": "7c06d51afd26cbc90a32c82375a393570c22ffc7",
                "greyproxy": "cdd5cde76dce1262c0fd8d6b1ab2bd9953df5e71",
            },
            "files": files,
        }

    def run_packager(self, raw=None):
        (self.payload / "release.json").write_text(
            json.dumps(self.manifest) if raw is None else raw
        )
        return subprocess.run(
            [
                sys.executable,
                str(PACKAGER),
                "--payload",
                str(self.payload),
                "--destination",
                str(self.destination),
                "--version",
                VERSION,
                "--source",
                SOURCE,
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )

    def test_it_stages_only_package_owned_files_without_activation(self):
        result = self.run_packager()
        self.assertEqual(result.returncode, 0, result.stderr)
        command = self.destination / "usr/bin/cockpit"
        self.assertTrue(command.is_symlink())
        self.assertEqual(
            command.resolve(),
            self.destination / "usr/lib/agent-cockpit/config/bin/.local/bin/cockpit",
        )
        self.assertTrue(
            (self.destination / "usr/share/licenses/agent-cockpit-bin/LICENSE").is_file()
        )
        for name in ["LICENSE-LUCIDE", "LICENSE-FEATHER"]:
            self.assertEqual(
                (
                    self.destination / "usr/share/licenses/agent-cockpit-bin/assets" / name
                ).read_bytes(),
                (self.payload / "assets" / name).read_bytes(),
            )
        self.assertFalse((self.destination / "home").exists())
        self.assertFalse((self.destination / "usr/lib/systemd").exists())

    def test_changed_payload_bytes_fail_before_creating_output(self):
        (self.payload / "config/bin/.local/bin/cockpit").write_text("changed")
        result = self.run_packager()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum mismatch", result.stderr)
        self.assertFalse(self.destination.exists())

    def test_an_unexpected_manifest_path_is_rejected(self):
        self.manifest["files"]["private/credentials.json"] = "b" * 64
        result = self.run_packager()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unexpected payload inventory", result.stderr)
        self.assertFalse(self.destination.exists())

    def test_a_missing_required_helper_is_not_silently_omitted(self):
        (self.payload / "config/waybar/.config/waybar/scripts/codex-quota.py").unlink()
        result = self.run_packager()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing required payload file", result.stderr)
        self.assertFalse(self.destination.exists())

    def test_a_symlink_to_outside_payload_is_rejected(self):
        script = self.payload / "config/bin/.local/bin/cockpit"
        outside = self.root / "outside"
        outside.write_bytes(script.read_bytes())
        script.unlink()
        script.symlink_to(outside)
        result = self.run_packager()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("payload symlink", result.stderr)
        self.assertFalse(self.destination.exists())

    def test_an_unapproved_component_revision_is_rejected(self):
        self.manifest["components"]["greywall"] = "b" * 40
        result = self.run_packager()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("component identity mismatch", result.stderr)
        self.assertFalse(self.destination.exists())

    def test_existing_output_is_preserved(self):
        self.destination.mkdir()
        existing = self.destination / "keep"
        existing.write_text("user data")
        result = self.run_packager()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("destination is not empty", result.stderr)
        self.assertEqual(existing.read_text(), "user data")

    def test_unowned_metadata_is_not_copied_to_the_package(self):
        self.manifest["private-setting"] = "user data"
        result = self.run_packager()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unexpected release metadata", result.stderr)
        self.assertFalse(self.destination.exists())

    def test_native_recipe_only_stages_the_prebuilt_payload(self):
        (self.payload / "release.json").write_text(json.dumps(self.manifest))
        for name in ["package.py", "runtime-files.txt", "integration-files.txt"]:
            (self.root / name).write_bytes((DIRECTORY / name).read_bytes())
        result = subprocess.run(
            [
                "bash",
                "-c",
                'source "$1" || exit; srcdir=$2; pkgdir=$3; '
                "if declare -F build >/dev/null; then exit 9; fi; package",
                "recipe-test",
                str(DIRECTORY / "PKGBUILD"),
                str(self.root),
                str(self.destination),
            ],
            env=os.environ
            | {
                "COCKPIT_PACKAGE_VERSION": VERSION,
                "COCKPIT_SOURCE_COMMIT": SOURCE,
                "COCKPIT_PAYLOAD_SHA256": hashlib.sha256(b"test archive").hexdigest(),
            },
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.destination / "usr/bin/cockpit").is_symlink())

    def test_conflicting_duplicate_metadata_is_rejected(self):
        encoded = json.dumps(self.manifest)
        inputs = [
            '{"source":"' + "b" * 40 + '",' + encoded[1:],
            encoded.replace('"files": {', '"files": {"LICENSE":"' + "b" * 64 + '",', 1),
        ]
        for index, raw in enumerate(inputs):
            with self.subTest(index=index):
                self.destination = self.root / f"package-{index}"
                result = self.run_packager(raw)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("duplicate key", result.stderr)
                self.assertFalse(self.destination.exists())

    def test_recipe_auxiliary_sources_match_their_pinned_hashes(self):
        result = subprocess.run(
            [
                "bash",
                "-c",
                'source "$1" || exit; for index in 1 2 3; do '
                'printf "%s %s\\n" "${source[$index]}" "${sha256sums[$index]}"; done',
                "recipe-test",
                str(DIRECTORY / "PKGBUILD"),
            ],
            env=os.environ
            | {
                "COCKPIT_PACKAGE_VERSION": VERSION,
                "COCKPIT_SOURCE_COMMIT": SOURCE,
                "COCKPIT_PAYLOAD_SHA256": hashlib.sha256(b"test archive").hexdigest(),
            },
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        sources = result.stdout.splitlines()
        self.assertEqual(len(sources), 3)
        for line in sources:
            name, expected = line.split()
            with self.subTest(source=name):
                self.assertEqual(
                    hashlib.sha256((DIRECTORY / name).read_bytes()).hexdigest(), expected
                )


if __name__ == "__main__":
    unittest.main()
