#!/usr/bin/env python3
"""Apply one missing repository setting, or probe one protection, with the owner's gh login.

Each subcommand is idempotent and reads the result back. Agents never run this: the owner
runs it through `torlek ask run`. docs/security/repository.md lists when to run each one.
"""

import argparse
import json
import subprocess
import sys

REPOSITORY = "Androlax2/agent-cockpit"
PREFIX = f"repos/{REPOSITORY}"
PROBE_TAG = "refs/tags/v0.0.0-ruleset-probe"


def call_github(method, endpoint, body=None):
    """Return (HTTP status, decoded JSON body or None); a failed request is a status, not an error."""
    command = ["gh", "api", f"{PREFIX}/{endpoint}", "--hostname", "github.com"]
    command += ["--method", method, "--include"]
    if body is not None:
        command += ["--input", "-"]
    result = subprocess.run(
        command,
        input=json.dumps(body) if body is not None else None,
        capture_output=True,
        text=True,
        check=False,
        timeout=60,
    )
    headers, _, payload = result.stdout.replace("\r\n", "\n").partition("\n\n")
    status = headers.split(" ", 2)[1] if headers.startswith("HTTP/") else ""
    if not status.isdigit():
        raise SystemExit(
            f"gh gave no HTTP response for {method} {endpoint}: {result.stderr.strip()}"
        )
    return int(status), json.loads(payload) if payload.strip() else None


def read_actions_permissions():
    status, permissions = call_github("GET", "actions/permissions")
    if status != 200:
        raise SystemExit(f"cannot read actions/permissions: HTTP {status}")
    return permissions


def require_action_sha_pinning():
    permissions = read_actions_permissions()
    if permissions.get("sha_pinning_required") is True:
        print("Actions SHA pinning is already required; nothing changed.")
        return 0
    if permissions.get("enabled") is not True:
        print("Actions is disabled; refusing to change its policy.", file=sys.stderr)
        return 1
    desired = {
        "enabled": True,
        "allowed_actions": permissions["allowed_actions"],
        "sha_pinning_required": True,
    }
    status, _ = call_github("PUT", "actions/permissions", desired)
    if status != 204:
        print(f"PUT actions/permissions answered HTTP {status}.", file=sys.stderr)
        return 1
    readback = read_actions_permissions()
    if readback.get("sha_pinning_required") is not True:
        print(f"readback still shows {readback}; the setting did not apply.", file=sys.stderr)
        return 1
    print(f"Actions SHA pinning is now required: {readback}")
    return 0


def probe_tag_refusal():
    status, commit = call_github("GET", "commits/main")
    if status != 200:
        raise SystemExit(f"cannot read main: HTTP {status}")
    status, answer = call_github("POST", "git/refs", {"ref": PROBE_TAG, "sha": commit["sha"]})
    message = (answer or {}).get("message", "")
    if status == 422 and "rule violation" in message.lower():
        print(f"{PROBE_TAG} was refused by a ruleset, as required: {message.strip()}")
        return 0
    if status == 201:
        print(
            f"{PROBE_TAG} was CREATED: v* tags are NOT protected. The immutability ruleset "
            "now keeps it; record this and fix the creation ruleset before removing it.",
            file=sys.stderr,
        )
        return 1
    if status == 422 and "already exists" in message.lower():
        print(f"{PROBE_TAG} already exists: an earlier probe was not refused.", file=sys.stderr)
        return 1
    print(f"inconclusive: HTTP {status} {message.strip()}", file=sys.stderr)
    return 2


SUBCOMMANDS = {
    "require-action-sha-pinning": require_action_sha_pinning,
    "probe-tag-refusal": probe_tag_refusal,
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("subcommand", choices=SUBCOMMANDS)
    return SUBCOMMANDS[parser.parse_args().subcommand]()


if __name__ == "__main__":
    raise SystemExit(main())
