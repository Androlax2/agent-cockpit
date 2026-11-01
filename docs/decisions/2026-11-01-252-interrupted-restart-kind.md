# A restart a crash interrupted after its stop committed relaunches as `restart`

Issue #252 (with #254 and #256 of the same pull request). Decided by the worker: #205 made a
restart interrupted during its stop relaunch as `restart`, so one step later must not read
as `restore`.

- **The launch kind records the owner's intent.** A `launching` session whose latest
  transition is `stopping (restart)` → `launching` relaunches with `LaunchKind::Restart`
  (`Sessions::launching_for_restart`). The intent is read from the transition history,
  written in the same transaction as the `launching` state, so no column is added.
- **Only the restart.** A wake, resume or start a crash interrupted while `launching` still
  relaunches as `restore`: those launches have no committed step before `launching` that a
  crash can split, so the kind stays one fact per operation.
- **Rejected:** a `launching_for` column on `sessions` (a migration in a crate where
  parallel work adds migrations, for a fact the history already holds).
- **Stalled stops on restore (#254): only the terminations overlap.** Measured through the
  daemon binary with three agents that ignore `SIGTERM`: 15.2 s before the first answered
  request when stopped one after another, under 6 s with their terminations at once
  (`cockpit-daemon/tests/restore_stalled_stops.rs`). The sessions are then restored one by
  one as before, so terminals never race on the tmux server (#197) and relaunches do not
  burst. Rejected: restoring whole sessions concurrently (that tmux race, unbounded
  relaunches) and finishing stops after the daemon accepts requests (restore holds no
  session lock across that boundary, and one grace is within the restart contract).
- **A database error stops the start only where nothing recorded the failure (#251).** A
  stop that cannot be finished for a database error, direct or wrapped, stops the start; a
  failed reopen stops it only when the session could not be left `failed`, as
  `fail_restore` already did, so one session's bad row cannot block every start.
- **A launch reserves its execution with its record (#256).** `cockpit-processes`
  reconciliation keeps a reserved hosted execution whose command never ran charged as
  `uncertain` under process-group containment, so it cannot resolve the orphan; the
  reservation now commits with `insert_launch` (`Supervisor::prepare_reserved_hosted`).
