# cockpit-processes

Spawn, execution identity, stream draining, whole-tree termination and reattach of OS executions.

An execution is one OS process group or one systemd user scope, identified by the boot id,
its process group and its leader's start token, never by PID alone (report 04). Its row in
the state database is its reservation: the launch intent is committed before anything is
spawned, and only an observation that no process of the execution remains releases it,
never a dropped task or a timed-out launcher (report 03).

## Containment

- Linux (WSL2 included): a transient systemd user scope per execution
  (`systemd-run --user --scope`) when this user's manager answers; its cgroup holds every
  descendant, including one that calls `setsid`. From systemd 258 a scope rewrites `$` in
  its command unless told `--expand-environment=no`, which exists from 254: the switch is
  passed where `systemd-run --version` says it is known.
- macOS, and Linux without a reachable user manager: a process group led by the program.
  A descendant that calls `setsid` leaves it.
- Each execution records the containment it was launched in, so a fallback is visible and a
  restart reconciles it with the right one (docs/DECISIONS.md, 2026-10-31).
- A systemd execution also receives `XDG_RUNTIME_DIR` and `DBUS_SESSION_BUS_ADDRESS`, which
  `systemd-run` needs to reach the manager; apart from those, an execution's environment is
  exactly its launch spec's.

## Life of an execution

1. `launch` commits the intent (`launching`), spawns `/bin/sh` in its own process group
   (inside the scope when there is one), records the group, and waits for the leader file:
   the shell writes its pid and start token there, then execs the program. That file, written
   by the execution itself, is the start confirmation (`running`). A launcher that does not
   confirm in time answers `LaunchTimedOut` and keeps the reservation.
2. `observe` checks the real processes and records a confirmed start or an end.
3. `terminate` records the cancel request, sends `SIGTERM` to the whole containment, then
   `SIGKILL` after the grace period, and answers `Confirmed` only once the execution was
   observed empty; otherwise `Unconfirmed`, reservation kept.
4. `reconcile` runs at daemon startup: each occupying execution is reattached when
   positively identified, ended when observed gone (`exited`, `terminated`,
   `never-started`, `host-restarted`), and kept charged when neither (`Uncertain`).

A **hosted** execution is one whose program a host starts itself, because it needs a
terminal: `cockpit-sessions` runs agents in tmux panes (report 05). `prepare_hosted` commits
the intent and answers the command the host runs: `env -i` with the spec's environment and
the host variables named (such as `TERM`), the scope wrapper when there is one, and the
launch script, so the leader file confirms the start as above (`confirm_hosted`). The pane's
process leads its own process group, so observation and termination are the same as for a
launched execution. `abandon_hosted` releases a reservation whose host never ran the command.
Its output stays in the host's terminal. The environment's values never travel in argv,
where any local process could read them: they are written to a 0600 file in the execution's
directory, which the host step reads and deletes before the program starts. Names must be
plain shell names.

## Terminal executions

A `LaunchSpec` with a `Terminal` runs in a pseudo-terminal instead (#163; approved runs of
`cockpit-approvals`). The execution is the terminal's session: the spec's session leader
(`run_session_leader`, which the daemon runs as `cockpit-daemon --run-leader`) calls
`setsid`, takes the terminal as controlling terminal and execs the launch script, so the
program keeps its pid, leads the session and its process group, and its exit status is the
one `Launched::exit` reports. Until the leader's `setsid`, its pid sits in the daemon's group
and its own group has no member yet; an observation then sees the live launcher and keeps
the execution `launching`. The terminal's other side belongs to the caller: a daemon that
stops hangs the session up.

`reserve` records a launch intent in the caller's transaction, so a capability commits it
with its own record of what the execution is for; `launch_reserved` then launches it once.

Output goes to `stdout` and `stderr` files in `<executions dir>/<database epoch>/<id>/`, not to pipes: a
pipe whose reader is the daemon would kill the program with `SIGPIPE` after a daemon
restart. `follow_output` drains a file in bounded chunks into a byte-bounded window; a
viewer that falls behind reads a gap marker naming the bytes it missed. The files
themselves are not size-limited yet.

## Dependency edges

- Depends on: `cockpit-turso` (the transactions and migrations of its table), `rustix`
  (process-group signals without `unsafe`), `tokio`.
- Depends on no product capability; `tests/crate_graph.rs` enforces it, and the workspace
  rule `verification -> processes` keeps verification from launching processes.
- Used by: the daemon, which reconciles executions before it accepts requests, and
  `cockpit-sessions`, whose agents run as hosted executions, and `cockpit-approvals`, whose
  approved runs are terminal executions.

## Tests

- `tests/lifecycle.rs`: launch, output, natural exit, and whole-group termination with
  escalation on real processes and a real state database.
- `tests/reattach.rs`: issue #29's orphan fixture (timed-out launcher, process still
  running, reservation kept and reconciled) and the restart cases of reconciliation.
- `tests/hosted.rs`: an execution a host (playing tmux) starts: confirmed as its own
  process group, exactly its environment, no value in its command, abandoned launches.
- `tests/output.rs`: bounded draining and gap markers for a slow viewer.
- `tests/terminal.rs`: terminal executions with util-linux `setsid --ctty` as the leader:
  the program leads the session and has the terminal, its exit status, whole-session
  termination, a leader that cannot take the terminal, a reserved intent launched once, and
  a launcher not yet leading its group.
- `tests/systemd_scope.rs`: the scope backend through fake `systemd-run` and `systemctl`;
  the real-manager case is `#[ignore]`d (sandboxes and CI have none), run it with
  `cargo nextest run -p cockpit-processes --run-ignored only` on a desktop session.
- `tests/crate_graph.rs`: no Cockpit dependency but `cockpit-turso`.
