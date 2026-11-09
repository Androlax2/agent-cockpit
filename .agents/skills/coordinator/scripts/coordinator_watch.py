"""Coordinator event filter: wakes the coordinator only for events that need action.

Immediate: a worker question or an open dialog appears or changes (a dialog is the owner's to
act on in the worker's pane, so the worker is blocked, not idle), a worker still idle without a
merged PR after a grace period (once per idle spell; a short working blip does not end it), a CI failure on a
PR's current head commit (once per commit and check), a PR merged or closed. Everything else
that changed is dropped, so every printed line is one a coordinator acts on. `--summary-seconds N`
opts in to one `SUMMARY` line of those changes every N seconds.
Every few minutes the full `torlek agent sessions` list is read too, and a question the watch
stream never delivered is printed then (#1298).
State persists in the state file so a re-armed monitor does not repeat what it already reported.

When `torlek agent sessions --watch` ends, or a helper thread dies, it prints `WATCH ENDED`, kills
the watch it started and exits non-zero (torlek's own status when that was non-zero, else 1), so
the monitor running this script wakes the coordinator instead of going silent.

Usage: coordinator_watch.py STATE_FILE [--repo OWNER/NAME] [--worker-root DIR]
"""

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import threading
import time
import traceback
from datetime import date, timedelta
from pathlib import Path

# The macOS job cannot pass until macOS is supported, so its failures are not events.
KNOWN_FAILING_ON_MAIN = {"greenfield-macos"}
FAILED_CONCLUSIONS = ("FAILURE", "TIMED_OUT", "CANCELLED", "STARTUP_FAILURE")
FIELD = re.compile(r'(\w[\w-]*)="((?:[^"\\]|\\.)*)"')
FAILURE_REPORT_SECONDS = 1800
# A worker waiting on CI wakes on every poll; a working spell shorter than this is a blip.
DEFAULT_WORKING_SECONDS = 120
# A worker that started a background review or command is woken by it; an idle that ends
# within this period is harmless, so only a longer one is reported.
DEFAULT_IDLE_GRACE_SECONDS = 180
# How often the full session list is compared with the questions already reported.
DEFAULT_FULL_LIST_SECONDS = 180
# The watch polls every 2 seconds: a question it has not delivered this long after the full
# list showed it is missed, not merely on its way.
DEFAULT_STREAM_LAG_SECONDS = 5
TORLEK_TIMEOUT_SECONDS = 30

lock = threading.Lock()
batched = []
gh_timeout_seconds = 60


def run_text(command, timeout=None):
    return subprocess.run(
        command, capture_output=True, text=True, check=True, timeout=timeout or gh_timeout_seconds
    ).stdout.strip()


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
        self.working_seconds = arguments.working_seconds
        self.idle_grace_seconds = arguments.idle_grace_seconds
        self.summary_seconds = arguments.summary_seconds
        self.full_list_seconds = arguments.full_list_seconds
        self.stream_lag_seconds = arguments.stream_lag_seconds
        self.state_path = arguments.state_file
        self.is_first_run = not self.state_path.exists()
        self.state = {} if self.is_first_run else json.loads(self.state_path.read_text())
        self.started_at = time.monotonic()
        self.sessions = self.state.setdefault("sessions", {})
        self.reported_failures = set(self.state.setdefault("failures", []))
        self.pr_states = self.state.setdefault("prs", {})
        # session id -> the question last printed for it. A state file from before #1298 has
        # none: the questions its sessions hold were printed then.
        if "questions" not in self.state:
            self.state["questions"] = {
                session_id: session["question"]
                for session_id, session in self.sessions.items()
                if session.get("question")
            }
        self.reported_questions = self.state["questions"]
        # session id -> when the watch stream last delivered a line for it.
        self.stream_seen_at = {}
        # session id -> the doing-text of the idle spell already reported; cleared when the
        # session works for at least `working_seconds`.
        if not isinstance(self.state.get("idle"), dict):
            self.state["idle"] = {}
        self.reported_idle = self.state["idle"]
        # session id -> when a working spell that began after this watcher saw the session idle
        # started. Not persisted: a spell of unknown start counts as long, so an idle is
        # reported again rather than silently dropped.
        self.working_since = {}
        # session id -> the timer that reports its idle spell once the grace period ends. A timer
        # that is no longer the session's entry, or whose session works again, reports nothing.
        self.idle_timers = {}
        self.saved_text = None
        self.process = None
        # failure kind -> when it was last printed; cleared once that kind succeeds again.
        self.last_failure_reports = {}

    def save(self):
        """Writes the state only when it changed, atomically (a kill mid-write keeps the old file)."""
        self.state["failures"] = sorted(self.reported_failures)
        text = json.dumps(self.state, sort_keys=True)
        if text == self.saved_text:
            return
        partial = self.state_path.with_name(self.state_path.name + ".partial")
        partial.write_text(text)
        os.replace(partial, self.state_path)
        self.saved_text = text

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
            "dialog": fields.get("dialog", ""),
        }

    @staticmethod
    def is_blocked_on_owner(session):
        # .get: a state file written before dialogs were parsed has sessions without the key.
        return bool(session["question"] or session.get("dialog"))

    def is_merged(self, branch):
        """True or False; None when GitHub could not say. Never call this holding the lock."""
        if self.pr_states.get(branch, {}).get("state") == "MERGED":
            return True
        try:
            count = run_text(
                ["gh", "pr", "list", "-R", self.repo, "--head", branch, "--state", "merged",
                 "--json", "number", "--jq", "length"],
            )  # fmt: skip
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
            return None
        return count != "0"

    def batch(self, previous, current, tag):
        changed = previous is None or previous["doing"] != current["doing"]
        if changed or previous["state"] != current["state"]:
            self.add_to_summary(f"{tag} {current['state']}: {current['doing']}")

    def add_to_summary(self, text):
        if self.summary_seconds > 0:
            batched.append(text)

    def track_working_spell(self, session_id, previous, state):
        if state != "waiting":
            if (previous or {}).get("state") == "waiting":
                self.working_since[session_id] = time.monotonic()
            return
        if (previous or {}).get("state") == "waiting":
            return
        started = self.working_since.pop(session_id, None)
        if started is None or time.monotonic() - started >= self.working_seconds:
            self.reported_idle.pop(session_id, None)

    def on_session(self, current):
        """Handles one session line. The merge check runs gh, so the lock is not held for it."""
        session_id = current["id"]
        tag = f"{session_id} ({current['branch']})"
        with lock:
            previous = self.sessions.get(session_id)
            self.sessions[session_id] = current
            self.stream_seen_at[session_id] = time.monotonic()
            if previous == current:
                return
            self.track_working_spell(session_id, previous, current["state"])
            # The first look at a fresh start fills the summary with history; a question or an
            # idle worker found then is still news.
            seeding = self.is_first_run and time.monotonic() - self.started_at < self.seed_seconds
            if not current["question"]:
                self.reported_questions.pop(session_id, None)
            reported_question = bool(current["question"]) and (
                current["question"] != self.reported_questions.get(session_id)
            )
            if reported_question:
                self.report_question(session_id, tag, current["question"])
            reported_dialog = bool(current["dialog"]) and (
                current["dialog"] != (previous or {}).get("dialog")
            )
            if reported_dialog:
                print(f"DIALOG {tag}: {current['dialog']}", flush=True)
                self.save()
            if reported_question or reported_dialog:
                return
            was_idle = (previous or {}).get("state") == "waiting" and not self.is_blocked_on_owner(
                previous
            )
            went_idle = current["state"] == "waiting" and not was_idle
            needs_merge_check = (
                went_idle
                and not self.is_blocked_on_owner(current)
                and self.reported_idle.get(session_id) != current["doing"]
            )
            if not needs_merge_check:
                if not seeding:
                    self.batch(previous, current, tag)
                self.save()
                return
            if self.idle_grace_seconds > 0:
                self.start_idle_timer(session_id, previous, tag, seeding)
                self.save()
                return
        self.report_idle(session_id, previous, current, tag, seeding)

    def report_question(self, session_id, tag, question, how=""):
        """Prints the question once per text. Call holding the lock."""
        print(f"QUESTION {tag}{how}: {question}", flush=True)
        self.reported_questions[session_id] = question
        self.save()

    def start_idle_timer(self, session_id, previous, tag, seeding):
        """Reports the idle spell after the grace period unless the session works again first."""

        def report_idle_after_grace():
            with lock:
                if self.idle_timers.get(session_id) is not timer:
                    return
                del self.idle_timers[session_id]
                current = self.sessions.get(session_id)
            if current is None or current["state"] != "waiting":
                return
            if self.is_blocked_on_owner(current):
                return
            self.report_idle(session_id, previous, current, tag, seeding)

        timer = threading.Timer(
            self.idle_grace_seconds,
            run_or_exit,
            args=(self, report_idle_after_grace),
        )
        timer.daemon = True
        self.idle_timers[session_id] = timer
        timer.start()

    def report_idle(self, session_id, previous, current, tag, seeding):
        merged = self.is_merged(current["branch"])
        with lock:
            latest = self.sessions.get(session_id)
            # gh ran without the lock; a worker that resumed or asked meanwhile is not idle. A
            # question only the full-list check found is not in `latest`.
            if (
                latest is None
                or latest["state"] != "waiting"
                or self.is_blocked_on_owner(latest)
                or session_id in self.reported_questions
            ):
                return
            if merged is None:
                reason = f"IDLE, merge state unknown (gh failed) {tag}: doing={current['doing']!r}"
                print(reason, flush=True)
            elif not merged:
                self.reported_idle[session_id] = current["doing"]
                print(f"IDLE without merged PR {tag}: doing={current['doing']!r}", flush=True)
            elif not seeding:
                self.batch(previous, current, tag)
            self.save()

    def on_session_ended(self, session_id):
        with lock:
            self.sessions.pop(session_id, None)
            self.idle_timers.pop(session_id, None)
            self.reported_idle.pop(session_id, None)
            self.reported_questions.pop(session_id, None)
            self.stream_seen_at.pop(session_id, None)
            self.working_since.pop(session_id, None)
            self.save()

    def handle_line(self, line):
        columns = line.split("\t")
        if len(columns) == 2 and columns[1] == "ended":
            self.on_session_ended(columns[0])
            return
        current = self.parse(line)
        if current is not None:
            self.on_session(current)

    def stop_child(self):
        if self.process is not None and self.process.poll() is None:
            self.process.kill()
            self.process.wait()

    def follow_sessions(self):
        # A file, not a pipe: a chatty stderr must never block the watch while stdout is read.
        errors = tempfile.TemporaryFile(mode="w+")
        try:
            self.process = subprocess.Popen(
                ["torlek", "agent", "sessions", "--watch"],
                stdout=subprocess.PIPE, stderr=errors, text=True, bufsize=1,
            )  # fmt: skip
            for raw in self.process.stdout:
                self.handle_line(raw.rstrip("\n"))
            exit_status = self.process.wait()
            errors.seek(0)
            reason = errors.read().strip()
            print(
                f"WATCH ENDED: torlek agent sessions --watch exited {exit_status}: {reason}",
                flush=True,
            )
            return exit_status or 1
        except Exception as error:
            traceback.print_exc()
            print(f"WATCH ENDED: {type(error).__name__}: {error}", flush=True)
            return 1
        finally:
            self.stop_child()

    def report_failure(self, kind, error):
        now = time.monotonic()
        with lock:
            last = self.last_failure_reports.get(kind)
            if last is None or now - last >= FAILURE_REPORT_SECONDS:
                print(f"{kind} (repeats are not reported for 30 minutes): {error}", flush=True)
                self.last_failure_reports[kind] = now

    def check_full_list(self):
        """Prints each open question the watch stream never delivered (#1298)."""
        while True:
            time.sleep(self.full_list_seconds)
            listed_at = time.monotonic()
            try:
                listing = run_text(["torlek", "agent", "sessions"], TORLEK_TIMEOUT_SECONDS)
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
                self.report_failure("SESSIONS CHECK FAILED", error)
                continue
            self.last_failure_reports.pop("SESSIONS CHECK FAILED", None)
            time.sleep(self.stream_lag_seconds)
            for line in listing.splitlines():
                current = self.parse(line)
                if current is None or not current["question"]:
                    continue
                with lock:
                    # A line the watch delivered after the listing is newer than the listing.
                    if self.stream_seen_at.get(current["id"], 0) > listed_at:
                        continue
                    if current["question"] == self.reported_questions.get(current["id"]):
                        continue
                    tag = f"{current['id']} ({current['branch']})"
                    self.report_question(
                        current["id"], tag, current["question"], ", missed by the session watch"
                    )

    def poll_prs(self):
        query = [
            "gh", "pr", "list", "-R", self.repo, "--state", "all", "--search",
            f"updated:>={self.since}", "--limit", "200",
            "--json", "number,state,headRefName,headRefOid,statusCheckRollup",
        ]  # fmt: skip
        while True:
            try:
                pulls = json.loads(run_text(query))
            except (
                subprocess.CalledProcessError,
                subprocess.TimeoutExpired,
                json.JSONDecodeError,
            ) as error:
                self.report_failure("PR POLL FAILED", error)
                time.sleep(self.poll_seconds)
                continue
            self.last_failure_reports.pop("PR POLL FAILED", None)
            with lock:
                self.on_pulls(pulls)
                self.save()
            time.sleep(self.poll_seconds)

    def on_pulls(self, pulls):
        first_poll = not self.state.get("polled")
        self.state["polled"] = True
        for pull in pulls:
            branch = pull["headRefName"]
            if not branch.startswith(self.branch_prefix):
                continue
            known = self.pr_states.get(branch, {}).get("state")
            self.pr_states[branch] = {"number": pull["number"], "state": pull["state"]}
            if not first_poll and known != pull["state"] and pull["state"] in ("MERGED", "CLOSED"):
                print(f"PR #{pull['number']} {pull['state']} ({branch})", flush=True)
            elif not first_poll and known is None and pull["state"] == "OPEN":
                self.add_to_summary(f"PR #{pull['number']} opened ({branch})")
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
    global gh_timeout_seconds
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
    parser.add_argument(
        "--working-seconds",
        type=float,
        default=DEFAULT_WORKING_SECONDS,
        help="a working spell shorter than this does not start a new idle spell",
    )
    parser.add_argument(
        "--idle-grace-seconds",
        type=float,
        default=DEFAULT_IDLE_GRACE_SECONDS,
        help="report an idle worker only if it is still idle after this long; 0 reports at once",
    )
    parser.add_argument(
        "--summary-seconds",
        type=float,
        default=0,
        help="print one SUMMARY line of non-actionable changes every N seconds; 0 (default) never",
    )
    parser.add_argument(
        "--full-list-seconds",
        type=float,
        default=DEFAULT_FULL_LIST_SECONDS,
        help="how often the full session list is checked for a question the watch missed",
    )
    parser.add_argument(
        "--stream-lag-seconds",
        type=float,
        default=DEFAULT_STREAM_LAG_SECONDS,
        help="how long the watch may take to deliver a question the full list shows",
    )
    parser.add_argument("--gh-timeout-seconds", type=float, default=60)
    arguments = parser.parse_args(argv)
    gh_timeout_seconds = arguments.gh_timeout_seconds
    arguments.repo = arguments.repo or default_repo()
    arguments.worker_root = arguments.worker_root or default_worker_root()
    arguments.since = arguments.since or (date.today() - timedelta(days=2)).isoformat()
    return arguments


def run_or_exit(watcher, target):
    """Runs a helper thread; if it dies, take the whole watcher down so the monitor wakes."""
    try:
        target()
    except BaseException:
        traceback.print_exc()
        print(f"WATCH ENDED: {target.__name__} crashed", flush=True)
        watcher.stop_child()
        os._exit(1)


def main(argv):
    # Monitors stop a script with SIGTERM; raise so the `finally` kills the torlek watch too.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    try:
        watcher = Watcher(parse_arguments(argv))
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError, ValueError) as error:
        print(f"WATCH ENDED: could not start: {error}", flush=True)
        return 1
    print(f"WATCHING {watcher.repo}, workers under {watcher.worker_root}", flush=True)
    helpers = [watcher.poll_prs, watcher.check_full_list]
    if watcher.summary_seconds > 0:
        helpers.append(watcher.summarize)
    for helper in helpers:
        threading.Thread(target=run_or_exit, args=(watcher, helper), daemon=True).start()
    return watcher.follow_sessions()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
