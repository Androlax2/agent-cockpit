"""A host for a package's root scripts (#390): the script runs as root of a user namespace whose
/etc/systemd/system, /usr/lib/systemd/system and /run/systemd are empty, with a fake systemctl
first on PATH that logs each call and fails the calls it is told to. Needs `unshare`
(util-linux) and unprivileged user namespaces; `is_available()` says whether this host has them.
"""

import shutil
import subprocess
import tempfile
from pathlib import Path

FOLDERS = ("/etc/systemd/system", "/usr/lib/systemd/system", "/run/systemd")

STEP = r"""set -eu
out=$1
shift
for folder in /etc/systemd/system /usr/lib/systemd/system /run/systemd; do
    mount -t tmpfs none "$folder"
    cp -a "$out/root$folder/." "$folder/"
done
status=0
PATH="$out/fakes:$PATH" "$@" || status=$?
for folder in /etc/systemd/system /usr/lib/systemd/system /run/systemd; do
    rm -rf "$out/root$folder"
    mkdir -p "$out/root$folder"
    cp -a "$folder/." "$out/root$folder/"
done
exit "$status"
"""

FAKE_SYSTEMCTL = """#!/bin/sh
echo "$*" >>"{log}"
if [ -n "$FAILING" ] && [ "$1" = "$FAILING" ]; then
    echo "systemctl: $1 failed" >&2
    exit 1
fi
"""


def is_available():
    if shutil.which("unshare") is None:
        return False
    probe = subprocess.run(
        ["unshare", "-Urm", "mount", "-t", "tmpfs", "none", "/run"],
        capture_output=True,
        check=False,
    )
    return probe.returncode == 0


class Host:
    """One host, its folders kept between runs."""

    def __init__(self):
        self._folder = tempfile.TemporaryDirectory()
        self.path = Path(self._folder.name)
        fakes = self.path / "fakes"
        fakes.mkdir()
        systemctl = fakes / "systemctl"
        systemctl.write_text(FAKE_SYSTEMCTL.format(log=self.path / "systemctl.log"))
        systemctl.chmod(0o755)
        for folder in FOLDERS:
            (self.path / "root" / folder.lstrip("/")).mkdir(parents=True)

    def cleanup(self):
        self._folder.cleanup()

    def at(self, path):
        return self.path / "root" / path.lstrip("/")

    def write(self, path, text=""):
        self.at(path).parent.mkdir(parents=True, exist_ok=True)
        self.at(path).write_text(text)

    def boot(self):
        """systemd runs on this host from now on."""
        self.at("/run/systemd/system").mkdir(parents=True, exist_ok=True)

    def run(self, *command, failing=""):
        """Runs `command` on the host, the fake systemctl failing its `failing` verb."""
        log = self.path / "systemctl.log"
        log.unlink(missing_ok=True)
        result = subprocess.run(
            ["unshare", "-Urm", "sh", "-c", STEP, "step", str(self.path), *command],
            capture_output=True,
            text=True,
            check=False,
            env={"PATH": "/usr/bin:/bin", "FAILING": failing},
        )
        calls = log.read_text() if log.exists() else ""
        return result, calls
