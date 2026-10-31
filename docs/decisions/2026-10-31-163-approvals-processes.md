# Approved runs are `cockpit-processes` terminal executions

Issue #163. Decided by the worker, within report 17's design and the follow-up
`docs/decisions/2026-10-31-34-approvals-runs.md` left open.

- **`cockpit-approvals` depends on `cockpit-processes`.** The runner reserves the run's
  execution and marks the run running with it in one transaction (`Supervisor::reserve`,
  `start_run`), the cross-capability atomicity report 02 asks for, then launches it. A lower
  capability with no product dependency, so the edge points down; the approvals crate-graph
  allow-list names it.
- **The session leader moved to `cockpit-processes`** (`run_session_leader`, still started
  as `cockpit-daemon --run-leader`). A terminal launch runs the leader first and the launch
  script inside it, so the leader file is written after `setsid`: the recorded process group
  is the session's, and the program keeps the pid whose exit status the daemon reaps.
  Terminal launches skip `process_group(0)` because `setsid` refuses a group leader.
- **A live launcher pid keeps an unconfirmed execution `launching`.** Before its `setsid`
  the launcher sits in the daemon's group and its own group is empty, which used to read as
  never started. Generalised to every launch: an unconfirmed execution whose launcher pid
  still exists is not observed gone (`tests/terminal.rs`
  `a_launcher_not_yet_leading_its_group_is_not_observed_gone`).
- **A restart decides from the execution.** Never started: `not-run`. Ended while the daemon
  was down, or the host restarted: `outcome-unknown`, the note says which. Still running:
  the run stays `running`, the daemon watches the execution and records `outcome-unknown`
  once it ended. A running row without an execution (from before this change) stays
  `outcome-unknown` as before.
- **The terminal stays with the daemon.** Its master side is the daemon's, so a daemon that
  stops hangs the command up; a command that ignores the hang-up is reattached, but its
  later output, its input and its exit code are gone. Keeping the terminal across restarts
  needs a holder process outside the daemon and is not part of this issue.
- **Processes a command leaves behind keep running**, as before #163; they keep the
  execution charged and the daemon watches them until observed gone.
- **Rejected:** terminating those leftovers when the command exits, because a command the
  owner approved may start a background service on purpose; the daemon composing
  `Supervisor` and `Runner` through a callback, because it adds indirection for an edge
  report 02 allows.
