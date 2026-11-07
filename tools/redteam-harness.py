#!/usr/bin/env python3
"""Runs the #685 red-team catalog against the disposable VM (docs/redteam-vm.md) and appends
every attempt to a results log. It only runs commands inside the guest through
`tools/redteam-vm.sh ssh`; it never touches the owner's live setup.

A success is a finding to review, never an action to take: this script only classifies and
logs an attempt, it never files an issue or changes anything outside the results log.
"""

import argparse
import json
import re
import subprocess
import sys
from datetime import UTC, datetime
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
REDTEAM_VM = REPO_ROOT / "tools" / "redteam-vm.sh"
DEFAULT_CATALOG = REPO_ROOT / "tools" / "redteam-harness" / "catalog.json"

# What a refusal or a block already looks like in this project's own output (torlek docs
# sandbox, torlek/src/cli routes.rs): matched before guessing from the exit code alone.
REFUSAL_PATTERNS = (
    re.compile(r"torlek: refused:"),
    re.compile(r"\bNotAuthorized\b"),
)
BLOCKED_PATTERNS = (
    re.compile(r"CONNECT tunnel failed, response 403"),
    re.compile(r"Permission denied"),
    re.compile(r"Operation not permitted"),
)


def classify_outcome(exit_code: int, combined_output: str) -> str:
    """Blocked: the sandbox itself stopped the attempt. Refused: Torlek's own code declined
    it with a reason. Succeeded: nothing stopped it, a finding to review. Unclear: neither
    pattern matched and the command still failed; a human reads the raw output."""
    if any(pattern.search(combined_output) for pattern in REFUSAL_PATTERNS):
        return "refused"
    if any(pattern.search(combined_output) for pattern in BLOCKED_PATTERNS):
        return "blocked"
    if exit_code == 0:
        return "succeeded"
    return "unclear"


def load_catalog(catalog_path: Path) -> list[dict]:
    return json.loads(catalog_path.read_text())


def guest_torlek_version(run_guest_command) -> str:
    result = run_guest_command("~/.local/bin/torlek version")
    return result.stdout.strip() or f"unknown (exit {result.returncode}): {result.stderr.strip()}"


def run_check(entry: dict, run_guest_command) -> dict:
    result = run_guest_command(entry["command"])
    combined_output = result.stdout + result.stderr
    return {
        "id": entry["id"],
        "category": entry["category"],
        "title": entry["title"],
        "command": entry["command"],
        "exit_code": result.returncode,
        "stdout": result.stdout,
        "stderr": result.stderr,
        "outcome": classify_outcome(result.returncode, combined_output),
    }


def default_guest_runner(command: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [str(REDTEAM_VM), "ssh", "bash", "-lc", command],
        capture_output=True,
        text=True,
        timeout=60,
    )


def append_result_line(results_log: Path, record: dict) -> None:
    results_log.parent.mkdir(parents=True, exist_ok=True)
    with results_log.open("a") as log_file:
        log_file.write(json.dumps(record, sort_keys=True) + "\n")


def run_catalog(catalog, results_log: Path, run_guest_command, only_ids=None) -> list[dict]:
    torlek_version = guest_torlek_version(run_guest_command)
    timestamp = datetime.now(UTC).isoformat()
    records = []
    for entry in catalog:
        if only_ids is not None and entry["id"] not in only_ids:
            continue
        if entry["status"] != "automated":
            continue
        record = run_check(entry, run_guest_command)
        record["torlek_version"] = torlek_version
        record["recorded_at"] = timestamp
        append_result_line(results_log, record)
        records.append(record)
    return records


def print_summary(records: list[dict]) -> None:
    for record in records:
        print(f"{record['id']}: {record['outcome']} ({record['command']})")
    by_outcome: dict[str, int] = {}
    for record in records:
        by_outcome[record["outcome"]] = by_outcome.get(record["outcome"], 0) + 1
    print("---")
    for outcome, count in sorted(by_outcome.items()):
        print(f"{outcome}: {count}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catalog", type=Path, default=DEFAULT_CATALOG)
    parser.add_argument("--results-log", type=Path, help="JSONL file to append to")
    parser.add_argument(
        "--id", action="append", dest="only_ids", help="run only this check id (repeatable)"
    )
    parser.add_argument(
        "--list", action="store_true", help="list the catalog and exit; runs nothing"
    )
    arguments = parser.parse_args()

    catalog = load_catalog(arguments.catalog)

    if arguments.list:
        for entry in catalog:
            print(
                f"{entry['id']} [{entry['status']}] (category {entry['category']}): {entry['title']}"
            )
        return 0

    if arguments.results_log is None:
        parser.error("--results-log is required unless --list is given")

    records = run_catalog(catalog, arguments.results_log, default_guest_runner, arguments.only_ids)
    print_summary(records)
    manual_entries = [entry for entry in catalog if entry["status"] == "manual"]
    if manual_entries:
        print(
            f"---\n{len(manual_entries)} catalog entries need a human-run session (--list to see them)"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
