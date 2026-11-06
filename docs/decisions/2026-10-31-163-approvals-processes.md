# Approved runs are `cockpit-processes` terminal executions

Issue #163. Decided by the worker, within report 17's design and the follow-up the #34
section below left open.

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

## Revisited 2026-10-31 (#34)

Decided by the worker, before the decisions above, within report 17's settled design; none
changes a `decided` item. How approved commands run. Kept here so one record tells how an
approved run executes; the points #163 replaced (runs outside the lanes and not through
`cockpit-processes`, with a restart marking them `outcome-unknown`) and the issues left for
later are dropped.

- **The controlling terminal comes from a leader re-exec.** Making a pseudo-terminal the
  controlling terminal needs `setsid` and `TIOCSCTTY` in the child before `exec`, which a
  `pre_exec` hook could only do with `unsafe` (the workspace denies it). The daemon starts
  itself as `cockpit-daemon --run-leader <shell> -c <command>` on the terminal; that mode
  calls `setsid`, takes the terminal and `exec`s the shell, which keeps the pid, so the exit
  code is the command's own.
- **The shell is `$SHELL` when absolute, else `/bin/sh`**, the daemon's environment being the
  owner's. The legacy fish parse check becomes `<shell> -n -c <command>`, run before the
  owner is asked, outside any transaction; `run:` steps of a guide and an owner's edit are
  checked the same way.
- **Safe Git settings are `cockpit-projects`' whole `SAFE_CONFIG`**, passed as
  `GIT_CONFIG_COUNT/KEY/VALUE` with every inherited `GIT_*` variable and the launch token
  removed. Content filters and diff or merge drivers chosen by name are read from the
  configuration seen from the run's directory just before it starts (`Git::safe_config`, the
  rule every daemon Git call follows) and reset too, user-wide ones such as Git LFS included.
  Limits: a command that `cd`s into another repository meets that repository's drivers; and
  `credential.helper` is reset, so an approved HTTPS push gets no stored credential.
- **A run that started is never reported as not run.** An exit that could not be observed,
  or an end that could not be recorded, marks the run `outcome-unknown`; a run that could not
  be marked running is recorded `not-run`, never left approved.
- **The owner's input to a prompt comes from standard input** (`cockpit approval input ID`),
  never from an argument, which shell history and process listings keep.
- **The asking agent's project is the registered checkout holding its working directory**
  (`/proc/<pid>/cwd`, read after the process tree was verified), deepest checkout first,
  until sessions record their checkout (#31). It only sets the "outside the project" flag.
- **Expiry**: an asking request or guide expires 15 minutes after its agent last asked about
  it (refined by `2026-10-31-164-read-only-polls.md`); `cockpit ask run` polls every 500 ms
  and gives up with 124 after `COCKPIT_ASK_WAIT_SECONDS` (default 540, under a ten-minute
  tool call). An expired request answers 1, not 124: `--wait` cannot revive it.
- **Error codes**: a stale revision, digest or guide step is `stale-revision` (the owner
  reads again); an expired or decided request and input to a command that is not running are
  `conflict`.
