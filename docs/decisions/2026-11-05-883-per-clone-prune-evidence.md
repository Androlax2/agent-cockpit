# A clone's target prune waits only for work in that clone, or work no clone can be named for

Issue #883. Decided by the worker under the issue's acceptance.

- **Per clone, not global.** The managed-idle evidence (`storage::evidence`) used to defer
  every clone's `target/` prune while any session anywhere was launching, running, stopping or
  restart-pending, and named that session as each clone's reason. With an always-on
  coordinator, no ended worker's `target/` was ever pruned. A session's agent works in its own
  checkout, so now only this clone's sessions, their agents' executions and their scheduling
  placements hold it back, named as such.
- **Work no clone can be named for still holds every clone, and says so.** An execution that
  is no session's agent (a check, an older launch), an approved or running host command, an
  execution whose end was not observed, and a placement whose session is unknown keep every
  clone, with a reason that says it may work in any clone.
- **Unchanged:** the queue pause and foreground profile defer all pruning; the reservation's
  global launch guard still refuses new sessions, checks and approvals while a prune runs; the
  clone sweep itself never consulted this evidence, so a clean ended clone was already swept.
- **An occupied clone's verdict names its holder** (`checkout is occupied by <holder>`).

Evidence: `crates/cockpit-daemon/tests/sessions/end_sweep.rs` (with a session running in
another clone: an ended session's clean clone is swept; an uncommitted one's `target/` outcome
does not name the running session; the storage report names it only on its own clone).
