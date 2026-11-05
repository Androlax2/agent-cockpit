#!/usr/bin/env python3
"""Apply one missing repository setting, or probe one protection, with the owner's gh login.

Each subcommand is idempotent and reads the result back. Agents never run this: the owner
runs it through `torlek ask run`. docs/security/repository.md lists when to run each one.
Exit 0: done or already so. Exit 1: the setting or protection is wrong. Exit 2: inconclusive.
"""

import argparse
import json
import re
import subprocess
import sys

REPOSITORY = "Androlax2/agent-cockpit"
PREFIX = f"repos/{REPOSITORY}"
PROBE_TAG = "refs/tags/v0.0.0-ruleset-probe"
# The repository's first commit holds no workflow, so a probe tag that is wrongly created
# there starts no Release run (release.yml runs on every pushed v* tag).
PROBE_COMMIT = "3abf9222c4e3b4bc819959c9d2f380875f4c2c50"
TAG_CREATION_RULESET = 26382178
RELEASE_APP_ID = 5573576


class Inconclusive(Exception):
    pass


def call_github(method, endpoint, body=None):
    """Return (HTTP status, decoded JSON body or None) for one request on this repository."""
    command = ["gh", "api", f"{PREFIX}/{endpoint}", "--hostname", "github.com"]
    command += ["--method", method, "--include"]
    if body is not None:
        command += ["--input", "-"]
    try:
        result = subprocess.run(
            command,
            input=json.dumps(body) if body is not None else None,
            capture_output=True,
            text=True,
            check=False,
            timeout=60,
        )
    except FileNotFoundError:
        raise Inconclusive("gh is not installed or not on PATH") from None
    except subprocess.TimeoutExpired:
        raise Inconclusive(f"{method} {endpoint} got no answer within 60 seconds") from None
    headers, _, payload = result.stdout.replace("\r\n", "\n").partition("\n\n")
    status = re.match(r"HTTP/\S+ (\d{3})\b", headers)
    if not status:
        raise Inconclusive(f"gh gave no HTTP response for {method} {endpoint}")
    try:
        decoded = json.loads(payload) if payload.strip() else None
    except json.JSONDecodeError:
        raise Inconclusive(f"{method} {endpoint}: HTTP {status[1]} without a JSON body") from None
    return int(status[1]), decoded


def read_object(endpoint):
    status, decoded = call_github("GET", endpoint)
    if status != 200 or not isinstance(decoded, dict):
        raise Inconclusive(f"cannot read {endpoint}: HTTP {status}")
    return decoded


def require_action_sha_pinning():
    permissions = read_object("actions/permissions")
    if permissions.get("sha_pinning_required") is True:
        print("Actions SHA pinning is already required; nothing changed.")
        return 0
    if permissions.get("enabled") is not True:
        print("Actions is disabled; refusing to change its policy.", file=sys.stderr)
        return 1
    if permissions.get("allowed_actions") not in ("all", "local_only", "selected"):
        raise Inconclusive(f"unexpected allowed_actions in {permissions}")
    desired = {
        "enabled": True,
        "allowed_actions": permissions["allowed_actions"],
        "sha_pinning_required": True,
    }
    status, _ = call_github("PUT", "actions/permissions", desired)
    if status != 204:
        print(f"PUT actions/permissions answered HTTP {status}.", file=sys.stderr)
        return 1
    readback = read_object("actions/permissions")
    if readback.get("sha_pinning_required") is not True:
        print(f"readback still shows {readback}; the setting did not apply.", file=sys.stderr)
        return 1
    print(f"Actions SHA pinning is now required: {readback}")
    return 0


def creation_ruleset_problems():
    """List what makes ruleset 26382178 differ from 'only the Release App creates v* tags'."""
    ruleset = read_object(f"rulesets/{TAG_CREATION_RULESET}")
    expected = {
        "enforcement": "active",
        "target": "tag",
        "include": ["refs/tags/v*"],
        "rules": ["creation"],
        "bypass": [("Integration", RELEASE_APP_ID)],
    }
    actual = {
        "enforcement": ruleset.get("enforcement"),
        "target": ruleset.get("target"),
        "include": ruleset.get("conditions", {}).get("ref_name", {}).get("include"),
        "rules": [rule.get("type") for rule in ruleset.get("rules", [])],
        "bypass": [
            (actor.get("actor_type"), actor.get("actor_id"))
            for actor in ruleset.get("bypass_actors", [])
        ],
    }
    return [
        f"{key}: {actual[key]!r}, expected {expected[key]!r}"
        for key in expected
        if actual[key] != expected[key]
    ]


def probe_tag_refusal():
    status, _ = call_github("GET", f"contents/.github/workflows?ref={PROBE_COMMIT}")
    if status != 404:
        raise Inconclusive(f"probe commit {PROBE_COMMIT} is not workflow-free (HTTP {status})")
    try:
        status, answer = call_github("POST", "git/refs", {"ref": PROBE_TAG, "sha": PROBE_COMMIT})
    except Inconclusive as error:
        raise Inconclusive(f"{error}; check whether {PROBE_TAG} now exists") from None
    message = ((answer or {}).get("message") or "").strip()
    if status == 201:
        print(
            f"{PROBE_TAG} was CREATED: v* tags are NOT protected. It points at a commit "
            "without workflows, so no run started; the immutability ruleset keeps it until "
            "the creation ruleset is fixed.",
            file=sys.stderr,
        )
        return 1
    if status == 422 and "already exists" in message.lower():
        print(f"{PROBE_TAG} already exists: an earlier probe was not refused.", file=sys.stderr)
        return 1
    if status != 422 or "rule violation" not in message.lower():
        raise Inconclusive(f"HTTP {status} {message}")
    problems = creation_ruleset_problems()
    if problems:
        print(
            f"{PROBE_TAG} was refused, but ruleset {TAG_CREATION_RULESET} is not the expected "
            f"creation rule: {'; '.join(problems)}",
            file=sys.stderr,
        )
        return 1
    print(
        f"{PROBE_TAG} was refused ({message.splitlines()[0]}) and ruleset "
        f"{TAG_CREATION_RULESET} lets only the Release App create v* tags."
    )
    return 0


SUBCOMMANDS = {
    "require-action-sha-pinning": require_action_sha_pinning,
    "probe-tag-refusal": probe_tag_refusal,
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("subcommand", choices=SUBCOMMANDS)
    try:
        return SUBCOMMANDS[parser.parse_args().subcommand]()
    except Inconclusive as error:
        print(f"inconclusive: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
