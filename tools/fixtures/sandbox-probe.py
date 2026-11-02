#!/usr/bin/env python3
"""Fake Claude agent for #433: bounded fixture reads and two unauthenticated HEAD requests."""

import errno
import json
import os
import subprocess
import sys
import time
from pathlib import Path


def network(url, environment, max_time):
    try:
        result = subprocess.run(
            [
                "curl",
                "--silent",
                "--show-error",
                "--fail",
                "--head",
                "--output",
                os.devnull,
                "--proto",
                "=https",
                "--connect-timeout",
                str(max_time),
                "--max-time",
                str(max_time),
                "--max-redirs",
                "0",
                "--write-out",
                "%{http_code} %{http_connect}\n",
                url,
            ],
            env=environment,
            capture_output=True,
            text=True,
            timeout=max_time + 5,
            check=False,
        )
        words = result.stdout.split()
        if len(words) != 2 or not all(word.isascii() and word.isdigit() for word in words):
            return {"exit": result.returncode, "http": 0, "connect": 0}
        return {"exit": result.returncode, "http": int(words[0]), "connect": int(words[1])}
    except (OSError, subprocess.TimeoutExpired):
        return {"exit": -1, "http": 0, "connect": 0}


def run_probe(request, checkout, session, environment):
    allowed = checkout / "allowed.txt"
    try:
        contents = allowed.read_text()
        allowed.write_text(contents)
        allowed_path = contents == "cockpit enforcement fixture\n"
    except OSError:
        allowed_path = False
    denied = {}
    for name, path in request["denied_paths"].items():
        try:
            with open(path, "rb") as source:
                source.read(1)
            denied[name] = "READABLE"
        except OSError as error:
            denied[name] = errno.errorcode.get(error.errno, "UNKNOWN")
    return {
        "nonce": request["nonce"],
        "session": session,
        "allowed_path": allowed_path,
        "denied_paths": denied,
        "allowed_network": network("https://api.github.com/", environment, 10),
        # The pinned proxy holds unknown destinations for a 30s approval grace period.
        "denied_network": network("https://example.com/", environment, 40),
    }


def main():
    if sys.argv[1:] == ["--version"]:
        print("2.1.316 (Claude Code)")
        return 0
    if sys.argv[1:3] == ["auth", "status"]:
        return 2
    checkout = Path.cwd()
    folder = checkout / ".enforcement"
    request = json.loads((folder / "request.json").read_text())
    result = run_probe(request, checkout, os.environ["COCKPIT_SESSION"], dict(os.environ))
    staged = folder / (request["nonce"] + ".partial")
    with staged.open("x") as output:
        json.dump(result, output)
    staged.replace(folder / (request["nonce"] + ".json"))
    # Cockpit owns termination/restart. No more requests or reads after this batch.
    while True:
        time.sleep(60)


if __name__ == "__main__":
    sys.exit(main())
