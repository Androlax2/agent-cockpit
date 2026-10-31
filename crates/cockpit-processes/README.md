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
  descendant, including one that calls `setsid`.
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
- Used by: the daemon, which reconciles executions before it accepts requests.

## Tests

- `tests/lifecycle.rs`: launch, output, natural exit, and whole-group termination with
  escalation on real processes and a real state database.
- `tests/reattach.rs`: issue #29's orphan fixture (timed-out launcher, process still
  running, reservation kept and reconciled) and the restart cases of reconciliation.
- `tests/output.rs`: bounded draining and gap markers for a slow viewer.
- `tests/systemd_scope.rs`: the scope backend through fake `systemd-run` and `systemctl`;
  the real-manager case is `#[ignore]`d (sandboxes and CI have none), run it with
  `cargo nextest run -p cockpit-processes --run-ignored only` on a desktop session.
- `tests/crate_graph.rs`: no Cockpit dependency but `cockpit-turso`.
