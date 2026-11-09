#!/usr/bin/env python3
"""Split the greenfield workspace's test binaries into CI shards by their measured run time.

Usage: cargo nextest list --archive-file A --message-format json | ci-shards.py plan [--shards N]
       ci-shards.py weights JUNIT.xml [JUNIT.xml ...]

`plan` reads the test list of the archive the build job made and prints `matrix=<json>` for
$GITHUB_OUTPUT: one `{"shard": n, "filter": "<nextest filterset>"}` entry per shard. Every
binary in the archive lands in exactly one shard, because the shards are cut from that very
list: a new test binary needs no edit here, and a stale entry in the weights file is ignored.
A binary the weights file does not know weighs `UNKNOWN_SECONDS`.

`weights` sums each test binary's seconds from the JUnit files nextest writes (profile `ci`,
.config/nextest.toml) and prints tools/ci-test-weights.json. Refresh it from the shards' JUnit
artifacts when the shards drift apart; a stale file only unbalances the shards, never
drops a test.
"""

import json
import sys
import xml.etree.ElementTree as ElementTree
from pathlib import Path

WEIGHTS = Path(__file__).resolve().parent / "ci-test-weights.json"
DEFAULT_SHARDS = 5
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


def filterset(ids):
    return " | ".join(f"binary_id(={binary_id})" for binary_id in sorted(ids))


def plan(nextest_list, weights, shard_count):
    ids = binary_ids(nextest_list)
    if not ids:
        raise SystemExit("ci-shards: the archive lists no test binary")
    shards = split_into_shards(ids, weights, shard_count)
    return [
        {"shard": number, "filter": filterset(shard["ids"]), "seconds": round(shard["seconds"])}
        for number, shard in enumerate(shards, start=1)
        if shard["ids"]
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


def main(arguments):
    if arguments[:1] == ["plan"]:
        shard_count = DEFAULT_SHARDS
        if arguments[1:2] == ["--shards"]:
            shard_count = int(arguments[2])
        elif len(arguments) > 1:
            print(__doc__, file=sys.stderr)
            return 2
        shards = plan(json.load(sys.stdin), json.loads(WEIGHTS.read_text()), shard_count)
        for shard in shards:
            print(f"shard {shard['shard']}: about {shard['seconds']} s", file=sys.stderr)
        print(f"matrix={json.dumps({'include': shards}, separators=(',', ':'))}")
        return 0
    if arguments[:1] == ["weights"] and len(arguments) > 1:
        print(json.dumps(weights_from_junit(arguments[1:]), indent=2))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
