# Target pruning keeps a clone with unpushed commits, and the storage report shows each clone's prune verdict

Issue #406. Decided by Theo on 2026-11-04 (option A of the
[split plan](https://github.com/Androlax2/agent-cockpit/issues/406#issuecomment-6632573806)).

- **No prune of a clone whose commits are on no remote.** Eligibility now refuses a clone
  whose HEAD or a local branch has a commit no remote-tracking branch has, with the kept
  reason `the clone has unpushed commits (on no remote-tracking branch; fetch if they were
  pushed)`, before the ended-session or LOCAL ancestry proof.
  It is the clone sweep's rule (`sweep::has_unpushed_commits`), so a clone the sweep keeps for
  its commits is not pruned either. Git failing to answer keeps the clone. Pruning `target/`
  never touched `.git`; the rule is the owner's: nothing of a clone holding unpushed work is
  removed. The worker rechecks it before detaching, so a commit made after the plan stops the
  operation as `needs-owner` with the files kept.
- **Known limit.** A squash-merged branch whose remote branch was deleted and fetched with
  `--prune` counts as unpushed again, and stays kept until its commits are deleted or pushed.
  So does a clone whose branch was merged into the main checkout's LOCAL base but whose
  remote-tracking refs were never fetched; the reason says a fetch clears it. The sweep had the
  same limit until `2026-11-04-767-sweep-on-session-end.md` and
  `2026-11-05-882-sweep-asks-origin.md` changed its rule; read them for what the shared
  `has_unpushed_commits` now accepts. Rejected: asking GitHub whether a pull request merged,
  which the pruning decision ruled out as proof.
- **`torlek storage` shows the verdict prune would reach now.** Each row gains a `prune` column
  (`eligible`, `eligible-auto` when automatic policy would take it, `kept` with the reason in
  the `reason` column) and the report ends with `eligible for prune: N of M clones, B apparent
  bytes`, counting only complete measurements. The daemon builds the verdict from the same
  evidence and policy as `storage prune` (`prune_verdict`, sharing `eligibility` with the
  preview) and the report's own measurement, without walking `target/` again or persisting a
  plan. The verdict is an observation: prune measures again with its own limits (mount
  boundaries, manifest size) and can still refuse. A clone whose evidence cannot be read is
  kept with `could not decide: <error>` instead of failing the whole report. The wire field is
  optional, as additive fields are, so a client and daemon of different builds still read the
  report; the eligibility proof stays in `storage prune --dry-run`.
  While a session runs in a clone, or work no clone can be named for runs (the #883 section below), the
  managed-idle evidence keeps that clone, and the report says so.
- **Rejected:** running a full prune preview per clone in the report (a second walk of every
  `target/`), and a separate eligibility command (one more step before the owner sees what
  prune would free).

Evidence: `crates/cockpit-projects/tests/target_prune.rs` (unpushed guard at preview and at the
worker step, verdict without a plan), `crates/cockpit-cli/tests/storage.rs` (verdict column,
total, and an unpushed clone kept through the real daemon).

## Revisited 2026-11-05 (#883)

Decided by the worker under the issue's acceptance. A clone's target prune waits only for
work in that clone, or work no clone can be named for.

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
