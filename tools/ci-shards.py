#!/usr/bin/env python3
"""Split the greenfield workspace's test binaries into CI shards by their measured run time.

Usage: cargo nextest list --workspace --message-format json | ci-shards.py plan --shards N --shard K
       ci-shards.py weights JUNIT.xml [JUNIT.xml ...]

`plan` reads the workspace's test list and prints `filter=<nextest filterset>` for $GITHUB_OUTPUT:
the test binaries of shard K of N. Every shard cuts the same list the same way, so each binary
lands in exactly one shard: a new test binary needs no edit here, and a stale entry in the weights
file is ignored. A binary the weights file does not know weighs `UNKNOWN_SECONDS`.

`weights` sums each test binary's seconds from the JUnit files nextest writes (profile `ci`,
.config/nextest.toml) and prints tools/ci-test-weights.json. Refresh it from the shards' JUnit
artifacts when the shards drift apart; a stale file only unbalances the shards, never
drops a test. A binary's weight is the sum of its tests' own seconds, not its wall time (nextest
runs tests in parallel), so it ranks binaries rather than predicting a shard's duration.
"""

import argparse
import json
import re
import sys
import xml.etree.ElementTree as ElementTree
from pathlib import Path

WEIGHTS = Path(__file__).resolve().parent / "ci-test-weights.json"
UNKNOWN_SECONDS = 1.0
# Binaries below this are not worth a line in the weights file: UNKNOWN_SECONDS stands in.
MINIMUM_RECORDED_SECONDS = 1.0


def binary_ids(nextest_list):
    return sorted(nextest_list["rust-suites"])


def split_into_shards(ids, weights, shard_count):
    """Longest binaries first, each into the shard with the least time so far."""
    shards = [{"ids": [], "seconds": 0.0} for _ in range(shard_count)]
    for binary_id in sorted(ids, key=lambda i: (-weights.get(i, UNKNOWN_SECONDS), i)):
        lightest = min(shards, key=lambda shard: shard["seconds"])
        lightest["ids"].append(binary_id)
        lightest["seconds"] += weights.get(binary_id, UNKNOWN_SECONDS)
    return shards


SAFE_BINARY_ID = re.compile(r"[A-Za-z0-9_:/.-]+")


def filterset(ids):
    if not ids:
        return "none()"
    for binary_id in ids:
        # An id with a character the filterset language reads (space, comma, parenthesis, quote)
        # would cut a shard short or break its parse: refuse it here, in the build job.
        if not SAFE_BINARY_ID.fullmatch(binary_id):
            raise SystemExit(f"ci-shards: unsupported character in test binary id {binary_id!r}")
    return " | ".join(f"binary_id(={binary_id})" for binary_id in sorted(ids))


def plan(nextest_list, weights, shard_count):
    ids = binary_ids(nextest_list)
    if not ids:
        raise SystemExit("ci-shards: the workspace lists no test binary")
    return [
        {"shard": number, "filter": filterset(shard["ids"]), "seconds": round(shard["seconds"])}
        for number, shard in enumerate(split_into_shards(ids, weights, shard_count), start=1)
    ]


def weights_from_junit(paths):
    seconds = {}
    for path in paths:
        for suite in ElementTree.parse(path).getroot().iter("testsuite"):
            total = sum(float(case.get("time", 0)) for case in suite.iter("testcase"))
            seconds[suite.get("name")] = seconds.get(suite.get("name"), 0.0) + total
    return {
        name: round(total, 1)
        for name, total in sorted(seconds.items())
        if total >= MINIMUM_RECORDED_SECONDS
    }


def parse_arguments(arguments):
    parser = argparse.ArgumentParser(prog="ci-shards.py", description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    plan_command = commands.add_parser("plan")
    plan_command.add_argument("--shards", type=int, required=True)
    plan_command.add_argument("--shard", type=int, required=True)
    weights_command = commands.add_parser("weights")
    weights_command.add_argument("junit", nargs="+")
    return parser.parse_args(arguments)


def main(arguments):
    parsed = parse_arguments(arguments)
    if parsed.command == "weights":
        print(json.dumps(weights_from_junit(parsed.junit), indent=2))
        return 0
    if not 1 <= parsed.shard <= parsed.shards:
        raise SystemExit(f"ci-shards: shard {parsed.shard} is not in 1..{parsed.shards}")
    shards = plan(json.load(sys.stdin), json.loads(WEIGHTS.read_text()), parsed.shards)
    for shard in shards:
        print(f"shard {shard['shard']}: about {shard['seconds']} s", file=sys.stderr)
    print(f"filter={shards[parsed.shard - 1]['filter']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
