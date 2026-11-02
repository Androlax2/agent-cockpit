#!/usr/bin/env python3
"""Clean-home smoke of the pinned release installer and real daemon/doctor entry points.

Network use is bounded to two pinned public release downloads. Engines run only --version
and --help. Provider commands and service managers are fakes; no provider login or service
is used. This is installation evidence, not sandbox enforcement evidence (#433).
"""

import argparse
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def run(binary, *arguments, env):
    result = subprocess.run(
        [str(binary), *arguments],
        env=env,
        capture_output=True,
        text=True,
        timeout=240,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(
            f"{binary.name} {' '.join(arguments)}: {result.returncode}\n{result.stdout}\n{result.stderr}"
        )
    return result.stdout


def smoke(programs, home):
    fake_bin = home / "fake-bin"
    fake_bin.mkdir()
    for name, script in {
        "claude": 'if [ "$1" = --version ]; then echo "2.1.316 (Claude Code)"; else exit 1; fi',
        "codex": 'if [ "$1" = --version ]; then echo "codex-cli 0.161.0"; else exit 1; fi',
        "opencode": 'if [ "$1" = --version ]; then echo "1.19.5"; else exit 1; fi',
        "systemctl": "exit 1",
        "launchctl": "exit 1",
        "greywall": "exit 99",
        "greyproxy": "exit 99",
    }.items():
        path = fake_bin / name
        path.write_text(f"#!/bin/sh\n{script}\n")
        path.chmod(0o755)
    runtime = home / "run"
    runtime.mkdir(mode=0o700)
    # Keep the caller's TLS trust and routing; never add a token or change proxy policy.
    env = {
        key: value
        for key, value in os.environ.items()
        if key.lower()
        in {
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
            "ssl_cert_file",
            "ssl_cert_dir",
            "curl_ca_bundle",
        }
    }
    env.update(
        HOME=str(home),
        XDG_CONFIG_HOME=str(home / "config"),
        XDG_STATE_HOME=str(home / "state"),
        XDG_CACHE_HOME=str(home / "cache"),
        XDG_RUNTIME_DIR=str(runtime),
        TMUX_TMPDIR=str(home / "tmux"),
        PATH=f"{fake_bin}:/usr/bin:/bin:/usr/sbin:/sbin",
        LANG="C.UTF-8",
    )
    cockpit = programs / "cockpit"
    installed = run(cockpit, "--profile", "smoke", "sandbox", "install", env=env)
    print(installed, end="")
    if "cockpit --profile smoke doctor" not in installed:
        raise RuntimeError("installer advice lost the selected profile")
    paths = dict(
        line.split(": ", 1)
        for line in installed.splitlines()
        if line.startswith(("greywall: ", "greyproxy: "))
    )
    greywall = Path(paths["greywall"])
    greyproxy = Path(paths["greyproxy"])
    for path in [
        greywall.parent / "LICENSE",
        greywall.parent / "NOTICE",
        greywall.parent / "LICENSES/tun2socks-GPL-3.0.txt",
        greyproxy.parent / "LICENSE",
    ]:
        if not path.is_file() or not path.stat().st_size:
            raise RuntimeError(f"missing licence material: {path}")
    if (greywall.parent / "greywatch").readlink() != Path("greywall"):
        raise RuntimeError("greywatch alias was not preserved")
    pins = json.loads((ROOT / "crates/cockpit-sandbox/engine-pins.json").read_text())
    for name, path in [("greywall", greywall), ("greyproxy", greyproxy)]:
        # Greywall reports missing kernel capabilities with a nonzero exit; commit validation
        # and doctor must still distinguish an installed engine from enforcement readiness.
        version = subprocess.run(
            [str(path), "--version"],
            env=env,
            capture_output=True,
            text=True,
            timeout=15,
            check=False,
        )
        if pins[name]["commit"] not in version.stdout + version.stderr:
            raise RuntimeError(f"{name} did not report the exact pinned commit")
    with (home / "daemon.log").open("w+") as log:
        daemon = subprocess.Popen(
            [str(programs / "cockpit-daemon"), "--profile", "smoke", "--foreground"],
            env=env,
            stdout=log,
            stderr=log,
        )
        try:
            deadline = time.monotonic() + 30
            while not (runtime / "cockpit-smoke/daemon.sock").exists():
                if daemon.poll() is not None or time.monotonic() >= deadline:
                    log.seek(0)
                    raise RuntimeError(f"fixture daemon did not become ready:\n{log.read()}")
                time.sleep(0.05)
            report = run(cockpit, "--profile", "smoke", "doctor", env=env)
            sandbox = next(line for line in report.splitlines() if "  sandbox: " in line)
            if not sandbox.strip().startswith("available"):
                raise RuntimeError(sandbox)
            for pin in pins.values():
                if pin["commit"][:7] not in sandbox:
                    raise RuntimeError(f"doctor omitted a pinned engine: {sandbox}")
            print(sandbox.strip())
        finally:
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait(timeout=5)
    print(
        "PASS: fresh-home pinned install, archive layout, licences, exact commits, owned-path daemon and doctor"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--programs", type=Path, default=ROOT / "target/debug")
    options = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="cockpit-install-", dir="/tmp") as directory:
        smoke(options.programs.resolve(), Path(directory).resolve())


if __name__ == "__main__":
    main()
