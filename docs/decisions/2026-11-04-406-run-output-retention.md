# Check output older than the retention or over a project's budget is retired; receipts, tails and each check's newest run stay

Issue #406. Decided by Theo on 2026-11-04 (option A for PR 2 of the
[split plan](https://github.com/Androlax2/agent-cockpit/issues/406#issuecomment-6632573806)).

- **Only check output is retired, only its files.** Check runs are the only executions whose
  output is captured in files today (sessions are hosted, approved runs use a terminal).
  Retention removes a run's `stdout` and `stderr`, nothing else in its directory. The receipt
  stays with its output tail, so `torlek check` and the gate read the same evidence; a
  receipt whose files went says so (`output_retired` on the receipt view).
- **Age, then budget.** Owner settings in `[storage]`: `output_retention_days` (14),
  `output_budget_mib` per project (1024), `output_budget_grace_hours` (24). An output past the
  retention is selected. While a project's measured total is still over budget, its oldest
  outputs past the grace period are selected until the rest fit. These are the coordinator's
  retained defaults from the issue.
- **Never retired:** each check's newest run and its newest finished run, a live session's
  latest output, an output someone reads, a pending result, an output with no sealed check
  receipt, an output of an earlier database epoch, and an output whose files cannot be
  measured. Protected outputs still count toward the budget; unmeasured ones are counted
  and never treated as zero bytes.
- **Fenced and recoverable.** A retirement rechecks the output and its protections under the
  writer, refuses when anyone holds a reader guard, and commits `retiring` before any file
  goes; from then on no reader guard can be taken. The files go off the async workers, then
  `retired` is committed. A daemon start and every `storage outputs prune` retry a retirement
  interrupted or failed between the two; one that fails again is reported and does not stop
  the others.
- **The owner runs it; automation is opt-in.** `torlek storage outputs` reports per project
  what the policy selects (a dry run); `torlek storage outputs prune` retires it. Both are
  owner-only on the Bulk lane. `output_prune_automatic` (off) runs the same pass every
  `output_prune_interval_minutes` (60) over `output_prune_projects` (empty) only.
- **A finished check run is observed right away.** Retention only considers ended runs; the
  daemon now observes a check run's execution once its receipt is recorded. If its processes
  are still being torn down, the next reservation sweep (up to 30 s) ends it as before.
- **Known limits.** "Newest run" is per project and check, not per checkout: a clone's
  latest run of a check that another clone ran since can be retired (its receipt and tail
  stay). One pass considers at most 16 384 outputs and says when it left some out; a project
  cut off by that bound is judged against its budget on the outputs seen. A pass reads each
  output's protections one query at a time. Apparent bytes are not reclaimed physical
  space. Citation pins (#314) do not exist
  yet; retiring raw output keeps receipts and claims, so it removes nothing a citation names.
- **Rejected:** retiring receipts with their output (#314 owns evidence retention), a
  per-check newest-N rule (one newest run per check covers the gate's evidence), and deleting
  the execution directory (it also holds the launch confirmation).

Evidence: `crates/cockpit-processes/tests/output_retirement.rs` (listing, reader and revision
fences, file removal, recovery), `crates/cockpit-verification/tests/pending.rs` (newest runs),
`crates/cockpit-daemon/src/output_retention.rs` unit tests (age and budget selection),
`crates/cockpit-daemon/tests/verification/output_retention.rs` (dry run, prune, newest run
kept, receipt flag, owner-only), `crates/cockpit-cli/tests/storage.rs` (`storage outputs`),
`crates/cockpit-config/tests/storage_settings.rs` (settings and bounds).
