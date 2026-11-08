import io
import os
import subprocess
import tarfile
import tempfile
import unittest
from hashlib import sha256
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "tools/redteam-vm/guest-install-claude.sh"
PINNED_VERSION = "2.1.319"

FAKE_CURL = """#!/usr/bin/env bash
while [[ $# -gt 0 ]]; do
    [[ $1 == --output ]] && output=$2
    shift
done
cp "$FAKE_ARCHIVE" "$output"
"""


def fake_claude(version):
    return f"#!/usr/bin/env bash\necho '{version} (Claude Code)'\n".encode()


def native_package_archive(program_bytes):
    """The npm tarball of the native build: the program is package/claude."""
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w:gz") as tar:
        info = tarfile.TarInfo("package/claude")
        info.size = len(program_bytes)
        info.mode = 0o755
        tar.addfile(info, io.BytesIO(program_bytes))
    return archive.getvalue()


class GuestInstallClaudeTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="guest-install-claude-test-"))
        self.addCleanup(subprocess.run, ["rm", "-rf", str(self.root)])
        self.install_dir = self.root / "bin"
        self.install_dir.mkdir()
        fakes = self.root / "fakes"
        fakes.mkdir()
        (fakes / "curl").write_text(FAKE_CURL)
        (fakes / "curl").chmod(0o755)
        self.fakes = fakes

    def run_install(self, archive_bytes, checksum=None, version=PINNED_VERSION):
        archive = self.root / "served.tgz"
        archive.write_bytes(archive_bytes)
        return subprocess.run(
            ["bash", str(SCRIPT)],
            capture_output=True,
            text=True,
            env={
                "PATH": f"{self.fakes}:{os.environ['PATH']}",
                "FAKE_ARCHIVE": str(archive),
                "CLAUDE_CODE_VERSION": version,
                "CLAUDE_CODE_URL": "https://example.invalid/claude.tgz",
                "CLAUDE_CODE_SHA256": checksum or sha256(archive_bytes).hexdigest(),
                "CLAUDE_CODE_INSTALL_DIR": str(self.install_dir),
            },
        )

    def test_installs_the_program_when_the_checksum_and_version_match(self):
        result = self.run_install(native_package_archive(fake_claude(PINNED_VERSION)))
        self.assertEqual(result.returncode, 0, result.stderr)
        installed = self.install_dir / "claude"
        self.assertEqual(installed.stat().st_mode & 0o777, 0o755)
        self.assertEqual(installed.read_bytes(), fake_claude(PINNED_VERSION))

    def test_installs_nothing_when_the_checksum_differs(self):
        result = self.run_install(
            native_package_archive(fake_claude(PINNED_VERSION)), checksum="0" * 64
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("checksum mismatch", result.stderr)
        self.assertEqual(list(self.install_dir.iterdir()), [])

    def test_refuses_a_build_that_is_not_the_pinned_version(self):
        result = self.run_install(native_package_archive(fake_claude("2.1.300")))
        self.assertEqual(result.returncode, 1)
        self.assertIn("reports 2.1.300, not the pinned 2.1.319", result.stderr)
        self.assertEqual(list(self.install_dir.iterdir()), [])

    def test_refuses_without_the_pin_from_the_host(self):
        result = subprocess.run(
            ["bash", str(SCRIPT)], capture_output=True, text=True, env={"PATH": os.environ["PATH"]}
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("CLAUDE_CODE_VERSION", result.stderr)


if __name__ == "__main__":
    unittest.main()
