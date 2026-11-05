"""Coordinator event filter: wakes the coordinator only for events that need action.

Immediate: a worker question appears or changes, a CI failure on a PR's current head commit
(once per commit and check), a PR merged or closed, a worker going idle without a merged PR.
Everything else that changed is batched into one summary every 10 minutes.
State persists in the state file so a re-armed monitor does not repeat what it already reported.

Exits 1 when `torlek agent sessions --watch` ends, so the monitor running this script wakes
the coordinator instead of going silent.

Usage: coordinator_watch.py STATE_FILE [--repo OWNER/NAME] [--worker-root DIR]
"""

import argparse
import json
import re
import subprocess
import sys
import threading
import time
from datetime import date, timedelta
from pathlib import Path

# The macOS job cannot pass until macOS is supported, so its failures are not events.
KNOWN_FAILING_ON_MAIN = {"greenfield-macos"}
FAILED_CONCLUSIONS = ("FAILURE", "TIMED_OUT", "CANCELLED", "STARTUP_FAILURE")
FIELD = re.compile(r'(\w[\w-]*)="((?:[^"\\]|\\.)*)"')

lock = threading.Lock()
batched = []


def run_text(command):
    return subprocess.run(command, capture_output=True, text=True, check=True).stdout.strip()


def default_repo():
    return run_text(["gh", "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"])


def default_worker_root():
    """Where `torlek agent start --clone` puts clones: <projects>/.parallel/<project>/."""
    checkout = Path(run_text(["git", "rev-parse", "--show-toplevel"]))
    return checkout.parent / ".parallel" / checkout.name


class Watcher:
    def __init__(self, arguments):
        self.repo = arguments.repo
        self.worker_root = str(arguments.worker_root).rstrip("/") + "/"
        self.branch_prefix = arguments.branch_prefix
        self.clone_prefix = arguments.branch_prefix.rstrip("/") + "-"
        self.since = arguments.since
        self.seed_seconds = arguments.seed_seconds
        self.poll_seconds = arguments.poll_seconds
        self.summary_seconds = arguments.summary_seconds
        self.state_path = arguments.state_file
        self.is_first_run = not self.state_path.exists()
        self.state = {} if self.is_first_run else json.loads(self.state_path.read_text())
        self.started_at = time.monotonic()
        self.sessions = self.state.setdefault("sessions", {})
        self.reported_failures = set(self.state.setdefault("failures", []))
        self.pr_states = self.state.setdefault("prs", {})

    def save(self):
        self.state["failures"] = sorted(self.reported_failures)
        self.state_path.write_text(json.dumps(self.state))

    def branch_of(self, path):
        name = Path(path).name
        return re.sub(f"^{re.escape(self.clone_prefix)}", self.branch_prefix, name, count=1)

    def parse(self, line):
        columns = line.split("\t")
        if len(columns) < 5:
            return None
        # Sessions outside the worker root belong to the owner, not to this coordinator.
        if not columns[4].startswith(self.worker_root):
            return None
        fields = dict(FIELD.findall("\t".join(columns[5:])))
        return {
            "id": columns[0],
            "lifecycle": columns[1],
            "state": columns[3],
            "branch": self.branch_of(columns[4]),
            "doing": fields.get("doing", ""),
            "question": fields.get("needs-input", ""),
        }

    def is_merged(self, branch):
        if self.pr_states.get(branch, {}).get("state") == "MERGED":
            return True
        count = subprocess.run(
            ["gh", "pr", "list", "-R", self.repo, "--head", branch, "--state", "merged",
             "--json", "number", "--jq", "length"],
            capture_output=True, text=True,
        ).stdout.strip()  # fmt: skip
        return count not in ("", "0")

    def on_session(self, current):
        session_id = current["id"]
        previous = self.sessions.get(session_id)
        self.sessions[session_id] = current
        if previous == current:
            return
        if self.is_first_run and time.monotonic() - self.started_at < self.seed_seconds:
            return
        tag = f"{session_id} ({current['branch']})"
        if current["question"] and current["question"] != (previous or {}).get("question"):
            print(f"QUESTION {tag}: {current['question']}", flush=True)
            return
        went_idle = current["state"] == "waiting" and (previous or {}).get("state") != "waiting"
        if went_idle and not current["question"]:
            idle_key = f"{session_id}:{current['doing']}"
            reported_idle = self.state.setdefault("idle", [])
            if idle_key not in reported_idle and not self.is_merged(current["branch"]):
                reported_idle.append(idle_key)
                print(f"IDLE without merged PR {tag}: doing={current['doing']!r}", flush=True)
                return
        if (
            previous is None
            or previous["doing"] != current["doing"]
            or previous["state"] != current["state"]
        ):
            batched.append(f"{tag} {current['state']}: {current['doing']}")

    def follow_sessions(self):
        process = subprocess.Popen(
            ["torlek", "agent", "sessions", "--watch"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1,
        )  # fmt: skip
        for raw in process.stdout:
            line = raw.rstrip("\n")
            columns = line.split("\t")
            if len(columns) == 2 and columns[1] == "ended":
                with lock:
                    self.sessions.pop(columns[0], None)
                    self.save()
                continue
            current = self.parse(line)
            if current is None:
                continue
            with lock:
                self.on_session(current)
                self.save()
        exit_status = process.wait()
        reason = process.stderr.read().strip()
        print(
            f"WATCH ENDED: torlek agent sessions --watch exited {exit_status}: {reason}", flush=True
        )
        return exit_status or 1

    def poll_prs(self):
        query = [
            "gh", "pr", "list", "-R", self.repo, "--state", "all", "--search",
            f"head:{self.branch_prefix} updated:>={self.since}", "--limit", "60",
            "--json", "number,state,headRefName,headRefOid,statusCheckRollup",
        ]  # fmt: skip
        while True:
            try:
                pulls = json.loads(run_text(query))
            except (subprocess.CalledProcessError, json.JSONDecodeError) as error:
                print(f"PR POLL FAILED: {error}", flush=True)
                time.sleep(self.poll_seconds)
                continue
            with lock:
                self.on_pulls(pulls)
                self.save()
            time.sleep(self.poll_seconds)

    def on_pulls(self, pulls):
        first_poll = not self.pr_states
        for pull in pulls:
            branch = pull["headRefName"]
            known = self.pr_states.get(branch, {}).get("state")
            self.pr_states[branch] = {"number": pull["number"], "state": pull["state"]}
            if not first_poll and known != pull["state"] and pull["state"] in ("MERGED", "CLOSED"):
                print(f"PR #{pull['number']} {pull['state']} ({branch})", flush=True)
            elif not first_poll and known is None and pull["state"] == "OPEN":
                batched.append(f"PR #{pull['number']} opened ({branch})")
            if pull["state"] != "OPEN":
                continue
            for check in pull.get("statusCheckRollup") or []:
                if check.get("conclusion") not in FAILED_CONCLUSIONS:
                    continue
                if check.get("name") in KNOWN_FAILING_ON_MAIN:
                    continue
                key = f"{pull['number']}@{pull['headRefOid']}:{check.get('name')}"
                if key in self.reported_failures:
                    continue
                self.reported_failures.add(key)
                if self.is_first_run and first_poll:
                    continue
                print(
                    f"CI {check['conclusion']} PR #{pull['number']} ({branch}) "
                    f"head {pull['headRefOid'][:8]}: {check.get('name')} {check.get('detailsUrl', '')}",
                    flush=True,
                )

    def summarize(self):
        while True:
            time.sleep(self.summary_seconds)
            with lock:
                if batched:
                    print("SUMMARY (no action needed): " + " | ".join(batched), flush=True)
                    batched.clear()


def parse_arguments(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("state_file", type=Path)
    parser.add_argument(
        "--repo", help="OWNER/NAME; default: the repository of the current directory"
    )
    parser.add_argument(
        "--worker-root",
        type=Path,
        help="folder holding the workers' clones; default: <projects>/.parallel/<project>/",
    )
    parser.add_argument("--branch-prefix", default="greenfield/")
    parser.add_argument(
        "--since", help="only PRs updated on or after YYYY-MM-DD; default: 2 days ago"
    )
    parser.add_argument("--seed-seconds", type=float, default=5)
    parser.add_argument("--poll-seconds", type=float, default=120)
    parser.add_argument("--summary-seconds", type=float, default=600)
    arguments = parser.parse_args(argv)
    arguments.repo = arguments.repo or default_repo()
    arguments.worker_root = arguments.worker_root or default_worker_root()
    arguments.since = arguments.since or (date.today() - timedelta(days=2)).isoformat()
    return arguments


def main(argv):
    watcher = Watcher(parse_arguments(argv))
    print(f"WATCHING {watcher.repo}, workers under {watcher.worker_root}", flush=True)
    threading.Thread(target=watcher.poll_prs, daemon=True).start()
    threading.Thread(target=watcher.summarize, daemon=True).start()
    return watcher.follow_sessions()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
