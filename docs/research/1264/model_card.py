#!/usr/bin/env python3
"""Print the worker model card from model-trials.csv, one row per model, effort and kind.

Usage: python3 -I docs/research/1264/model_card.py [CSV]

The README next to this file says what each column means and when the card is read.
"""

import csv
import math
import re
import statistics
import sys
from collections import defaultdict
from pathlib import Path

DEFAULT_CSV = Path(__file__).resolve().parent / "model-trials.csv"
COLUMNS = [
    "merged", "issue", "pr", "harness", "model", "effort", "kind", "trial", "lines_added",
    "ci_fix_rounds", "ci_cause", "nudges", "idles", "reviewer_model", "reviewer_blocking",
    "reviewer_minor", "open_to_merge_h", "regressions", "notes",
]
KINDS = {"security", "risky", "contained", "docs", "research"}
HARNESSES = {"claude", "codex"}
EFFORTS = {"low", "medium", "high", "xhigh", "max", "ultra", "?"}
ARMS = {"default", "trial", "control", "replay"}
# Arms not picked by the coordinator's judgement; only these decide a cell.
DECISION_ARMS = {"trial", "control", "replay"}
WHOLE_NUMBER = re.compile(r"0|[1-9][0-9]*")
HOURS = re.compile(r"[0-9]+(\.[0-9]+)?")
DATE = re.compile(r"20[0-9]{2}-[01][0-9]-[0-3][0-9]")
CAUSES = {"own", "check", "flake", "drift", "platform", "unknown"}
# Failures the worker could have prevented; flakes, main drift and CI-only platforms are not.
WORKER_CAUSES = {"own", "check"}


def load(path):
    with open(path, newline="") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames != COLUMNS:
            sys.exit(f"{path}: columns are {reader.fieldnames}, expected {COLUMNS}")
        rows = list(reader)
    for line, row in enumerate(rows, start=2):
        check(path, line, row)
    return rows


def check(path, line, row):
    where = f"{path}:{line} (PR {row['pr'] or '-'})"
    if not DATE.fullmatch(row["merged"]):
        sys.exit(f"{where}: merged {row['merged']!r} is not a YYYY-MM-DD date")
    if not WHOLE_NUMBER.fullmatch(row["pr"]):
        sys.exit(f"{where}: pr {row['pr']!r} is not a pull request number")
    if row["harness"] not in HARNESSES:
        sys.exit(f"{where}: harness {row['harness']!r} is not one of {sorted(HARNESSES)}")
    if not row["model"]:
        sys.exit(f"{where}: model is empty; write ? when unknown")
    if row["effort"] not in EFFORTS:
        sys.exit(f"{where}: effort {row['effort']!r} is not one of {sorted(EFFORTS)}")
    if row["open_to_merge_h"] and not HOURS.fullmatch(row["open_to_merge_h"]):
        sys.exit(f"{where}: open_to_merge_h {row['open_to_merge_h']!r} is not a number of hours")
    if row["kind"] not in KINDS:
        sys.exit(f"{where}: kind {row['kind']!r} is not one of {sorted(KINDS)}")
    if row["trial"] not in ARMS:
        sys.exit(f"{where}: trial {row['trial']!r} is not one of {sorted(ARMS)}")
    for column in ("lines_added", "ci_fix_rounds", "nudges", "idles", "reviewer_blocking",
                   "reviewer_minor", "regressions"):
        if row[column] and not WHOLE_NUMBER.fullmatch(row[column]):
            sys.exit(f"{where}: {column} {row[column]!r} is not a whole number")
    failed = row["ci_fix_rounds"] not in ("", "0")
    causes = set(row["ci_cause"].split("+")) if row["ci_cause"] else set()
    if failed != bool(causes) or not causes <= CAUSES:
        sys.exit(f"{where}: ci_cause {row['ci_cause']!r} does not match "
                 f"ci_fix_rounds {row['ci_fix_rounds']!r} (causes: {sorted(CAUSES)})")
    if bool(row["reviewer_model"]) != bool(row["reviewer_blocking"]):
        sys.exit(f"{where}: reviewer_model and reviewer_blocking must be filled together")


def wilson(hits, total):
    """95% Wilson interval, as whole percents."""
    z = 1.96
    share = hits / total
    centre = (share + z * z / (2 * total)) / (1 + z * z / total)
    half = z * math.sqrt(share * (1 - share) / total + z * z / (4 * total * total)) / (1 + z * z / total)
    return round(100 * max(0.0, centre - half)), round(100 * min(1.0, centre + half))


def share(rows, test):
    if not rows:
        return "-"
    hits = sum(1 for row in rows if test(row))
    low, high = wilson(hits, len(rows))
    return f"{hits}/{len(rows)} ({low}-{high}%)"


def worker_ci_failure(row):
    return bool(set(row["ci_cause"].split("+")) & WORKER_CAUSES)


def decision_issues(rows):
    """Issues with a trial, control or replay row: the stopping rule counts each issue once."""
    return {row["issue"] for row in rows if row["trial"] in DECISION_ARMS}


def evidence(count):
    """Thresholds of the stopping rule in the research doc: a cell is decided at 20 issues."""
    return "none" if count < 5 else "screening" if count < 20 else "decidable"


def print_card(rows):
    cells = defaultdict(list)
    for row in rows:
        cells[(row["model"], row["effort"], row["kind"])].append(row)
    print("| model | effort | kind | PRs | decision issues | evidence | median +lines | worker CI failure "
          "| idled | reviewed | blocking per reviewed PR | regressions |")
    print("| --- | --- | --- | ---: | ---: | --- | ---: | --- | --- | ---: | --- | ---: |")
    for (model, effort, kind), cell in sorted(cells.items()):
        lines = [int(row["lines_added"]) for row in cell if row["lines_added"]]
        known_ci = [row for row in cell if row["ci_fix_rounds"]]
        reviewed = [row for row in cell if row["reviewer_blocking"]]
        blocking = (f"{statistics.mean(int(row['reviewer_blocking']) for row in reviewed):.1f}"
                    if reviewed else "-")
        regressions = sum(int(row["regressions"]) for row in cell if row["regressions"])
        decided_by = len(decision_issues(cell))
        print(f"| {model} | {effort} | {kind} | {len(cell)} | {decided_by} | {evidence(decided_by)} "
              f"| {f'{statistics.median(lines):g}' if lines else '-'} "
              f"| {share(known_ci, worker_ci_failure)} "
              f"| {share(cell, lambda row: row['idles'] not in ('', '0'))} "
              f"| {len(reviewed)} | {blocking} | {regressions} |")


def main():
    if len(sys.argv) > 2:
        sys.exit(__doc__)
    print_card(load(Path(sys.argv[1]) if len(sys.argv) == 2 else DEFAULT_CSV))


if __name__ == "__main__":
    main()
