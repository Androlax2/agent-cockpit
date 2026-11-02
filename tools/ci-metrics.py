#!/usr/bin/env python3
"""Summarize cached GitHub CI evidence offline; see the #397 baseline report.

Usage: python3 tools/ci-metrics.py CACHE_DIRECTORY
Weighted minutes are a comparison model (Linux x1, Windows x2, macOS x10), NOT a bill.
"""

import argparse
import json
import math
import statistics
import sys
from collections import Counter, defaultdict
from datetime import datetime
from pathlib import Path


def read_json(directory, name):
    return json.loads((directory / name).read_text())


def seconds_between(start, end):
    seconds = (datetime.fromisoformat(end) - datetime.fromisoformat(start)).total_seconds()
    if seconds < 0:
        raise ValueError(f"negative interval: {start} to {end}")
    return seconds


def job_cost(job):
    if job["conclusion"] == "skipped":
        return 0, 0
    if job["status"] != "completed" or not job["started_at"] or not job["completed_at"]:
        raise ValueError(f"job {job['id']}: unfinished or missing timestamps")
    factors = {
        factor
        for prefix, factor in (("ubuntu-", 1), ("windows-", 2), ("macos-", 10))
        if any(label.startswith(prefix) for label in job["labels"])
    }
    if len(factors) != 1:
        raise ValueError(f"job {job['id']}: unknown platform labels {job['labels']}")
    seconds = seconds_between(job["started_at"], job["completed_at"])
    return seconds, math.ceil(seconds / 60) * factors.pop()


def ready_time(directory, pull):
    name = f"timeline-{pull['number']}.json"
    events = read_json(directory, name)
    if len(events) >= 100:
        raise ValueError(f"{name}: full timeline page; resolve pagination before analysis")
    transitions = sorted(
        (event for event in events if event["event"] in ("ready_for_review", "convert_to_draft")),
        key=lambda event: event["created_at"],
    )
    if not transitions:
        return pull["createdAt"], "createdAt_proxy"
    if transitions[-1]["event"] != "ready_for_review":
        raise ValueError(f"{name}: draft without subsequent ready event")
    return transitions[-1]["created_at"], "ready_for_review"


def summarize(directory):
    census = read_json(directory, "runs.json")
    runs = read_json(directory, "sample-runs.json")
    pulls = read_json(directory, "sample-pulls.json")
    if not pulls or not runs:
        raise ValueError("empty cohort")
    if len({run["id"] for run in runs}) != len(runs):
        raise ValueError("duplicate sampled run")
    if len({pull["number"] for pull in pulls}) != len(pulls):
        raise ValueError("duplicate sampled PR")
    per_pull = {}
    for pull in pulls:
        ready_at, source = ready_time(directory, pull)
        per_pull[pull["number"]] = {
            "number": pull["number"],
            "ready_source": source,
            "ready_to_merge_seconds": seconds_between(ready_at, pull["mergedAt"]),
            "pr_runs": 0,
            "pr_reruns": 0,
            "main_runs": 0,
            "main_reruns": 0,
            "pr_runner_seconds": 0,
            "pr_weighted_minutes": 0,
            "main_runner_seconds": 0,
            "main_weighted_minutes": 0,
            "run_ids": [],
        }
    job_totals = defaultdict(lambda: {"seconds": 0, "weighted_minutes": 0})
    conclusions = defaultdict(Counter)
    successful_durations = defaultdict(list)
    for run in runs:
        if run["status"] != "completed":
            raise ValueError(f"run {run['id']}: unfinished workflow")
        if run["name"] != "CI" or run["event"] not in ("push", "pull_request"):
            raise ValueError(f"run {run['id']}: expected CI push or pull_request")
        if run["event"] == "pull_request":
            linked = {pull["number"] for pull in run["pull_requests"]}
            matches = [
                pull
                for pull in pulls
                if pull["number"] in linked
                or (
                    not linked
                    and pull["headRefName"] == run["head_branch"]
                    and pull["createdAt"] <= run["created_at"] <= pull["mergedAt"]
                )
            ]
            event = "pr"
        else:
            matches = [pull for pull in pulls if pull["mergeCommit"]["oid"] == run["head_sha"]]
            event = "main"
        if len(matches) != 1:
            raise ValueError(f"run {run['id']}: expected unique PR attribution")
        metrics = per_pull[matches[0]["number"]]
        metrics[f"{event}_runs"] += 1
        metrics[f"{event}_reruns"] += run["run_attempt"] - 1
        metrics["run_ids"].append(run["id"])
        name = f"jobs-{run['id']}.json"
        response = read_json(directory, name)
        jobs = response["jobs"]
        if response["total_count"] != len(jobs):
            raise ValueError(f"{name}: truncated jobs response")
        if len({job["id"] for job in jobs}) != len(jobs):
            raise ValueError(f"{name}: duplicate job")
        if {job["run_attempt"] for job in jobs} != set(range(1, run["run_attempt"] + 1)):
            raise ValueError(f"{name}: missing attempt; retrieve jobs with filter=all")
        for job in jobs:
            if job["run_id"] != run["id"]:
                raise ValueError(f"{name}: job belongs to another run")
            seconds, weighted = job_cost(job)
            metrics[f"{event}_runner_seconds"] += seconds
            metrics[f"{event}_weighted_minutes"] += weighted
            key = f"{event}/{job['name']}"
            job_totals[key]["seconds"] += seconds
            job_totals[key]["weighted_minutes"] += weighted
            conclusions[key][job["conclusion"]] += 1
            if job["conclusion"] == "success":
                successful_durations[key].append(seconds)
    for metrics in per_pull.values():
        if not metrics["pr_runs"] or not metrics["main_runs"]:
            raise ValueError(f"PR {metrics['number']}: missing PR or main run")
    for key, totals in job_totals.items():
        durations = successful_durations[key]
        totals["successful_median_seconds"] = statistics.median(durations) if durations else None
        totals["conclusions"] = dict(sorted(conclusions[key].items()))
    return {
        "census": dict(sorted(Counter(f"{run['name']}/{run['event']}" for run in census).items())),
        "sample_runs": len(runs),
        "sample_prs": len(pulls),
        "weighted_minutes_model": "ceil(each job seconds / 60) * Linux 1, Windows 2, macOS 10",
        "actual_billed_minutes": None,
        "cache_hit_rate": None,
        "jobs": dict(sorted(job_totals.items())),
        "pulls": sorted(per_pull.values(), key=lambda pull: pull["number"]),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    arguments = parser.parse_args()
    try:
        result = summarize(arguments.directory)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"ci-metrics: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
