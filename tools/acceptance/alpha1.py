#!/usr/bin/env python3
"""Rehearse Alpha 1's offline first run; never start a daemon, provider or service."""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from datetime import UTC, datetime
from pathlib import Path


def rehearse(binary, parent):
    with binary.open("rb") as program:
        digest = hashlib.file_digest(program, "sha256").hexdigest()
    os.umask(0o077)
    scratch = Path(tempfile.mkdtemp(prefix="alpha1-", dir=parent))
    environment = {"PATH": f"{binary.parent}{os.pathsep}{os.defpath}", "LANG": "C"}
    for variable, directory in (
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "run"),
        ("XDG_DATA_HOME", "data"),
        ("TMUX_TMPDIR", "tmux"),
        ("TMPDIR", "tmp"),
    ):
        location = scratch / directory
        location.mkdir(mode=0o700)
        environment[variable] = str(location)
    environment["COCKPIT_PROFILE"] = "next"
    project = scratch / "project"
    project.mkdir(mode=0o700)
    record = {
        "scope": "offline-rehearsal",
        "live_acceptance": False,
        "started_at": datetime.now(UTC).isoformat(),
        "candidate": str(binary),
        "sha256": digest,
        "scratch": str(scratch),
        "commands": [],
        "offline_checks_passed": True,
    }
    commands = (
        ("version", "--all"),
        ("config", "show"),
        ("init",),
        ("doctor", "--project", str(project)),
    )
    for index, arguments in enumerate(commands, 1):
        command = [str(binary), "--profile", "next", *arguments]
        stdout = scratch / f"{index}-{arguments[0]}.stdout"
        stderr = scratch / f"{index}-{arguments[0]}.stderr"
        problem = None
        with stdout.open("w") as output, stderr.open("w") as errors:
            try:
                result = subprocess.run(
                    command,
                    cwd=project,
                    env=environment,
                    stdout=output,
                    stderr=errors,
                    timeout=20,
                    check=False,
                )
                exit_code = result.returncode
            except subprocess.TimeoutExpired:
                exit_code = 124
                problem = "offline command exceeded 20 seconds"
            except OSError as error:
                exit_code = 126
                problem = str(error)
        expected = 1 if arguments[0] == "doctor" else 0
        passed = exit_code == expected
        if arguments[0] == "doctor":
            report = stdout.read_text(errors="replace")
            passed = passed and all(
                re.search(pattern, report, re.MULTILINE)
                for pattern in (
                    r"^available\s+config:",
                    r"^missing-prerequisite\s+daemon:",
                    r"^unknown\s+host:",
                    r"next: start `cockpit-daemon`",
                )
            )
        record["commands"].append(
            {
                "argv": command,
                "exit_code": exit_code,
                "expected_exit_code": expected,
                "stdout": str(stdout),
                "stderr": str(stderr),
                "problem": problem,
                "matched_offline_expectation": passed,
            }
        )
        record["offline_checks_passed"] &= passed
        if not passed:
            break
    evidence = scratch / "evidence.json"
    evidence.write_text(json.dumps(record, indent=2) + "\n")
    print(evidence)
    return 0 if record["offline_checks_passed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True, help="reviewed candidate directory")
    parser.add_argument(
        "--parent", type=Path, required=True, help="existing scratch parent directory"
    )
    arguments = parser.parse_args()
    binary = (arguments.bin_dir / "cockpit").resolve()
    parent = arguments.parent.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(f"candidate executable is missing or not executable: {binary}")
    if not parent.is_dir():
        parser.error(f"scratch parent must already be a directory: {parent}")
    # Unix socket paths have a small platform limit, including the random scratch suffix.
    if len(os.fsencode(parent / "alpha1-12345678/run/cockpit-next/daemon.sock")) >= 104:
        parser.error("scratch parent is too long for a daemon socket; choose a shorter path")
    try:
        return rehearse(binary, parent)
    except OSError as error:
        print(f"alpha1: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
