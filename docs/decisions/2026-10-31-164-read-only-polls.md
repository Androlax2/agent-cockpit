# Status polls write only once a minute; the rest are read-only

Issue #164. Decided by the worker: the issue asks that `expires_at` move "only when it is
getting stale", and leaves the threshold open.

- **A poll stores a new expiry only when the stored one is at least 60 seconds older than a
  fresh one** (`REFRESH_AFTER_SECONDS`, `crates/cockpit-approvals/src/rows.rs`). Every other
  poll of an asking request or guide writes no row and no change-feed record. An asking
  request therefore expires between 14 and 15 minutes after its agent last asked about it,
  refining the 15 minutes of `2026-10-31-34-approvals-runs.md`; `cockpit ask run --wait`
  polls every 500 ms, so a waiting agent never comes near either bound.
- **A guide's `run:` requests move with the guide**, in the same write, so they still never
  expire before the guide they belong to.
- **The poll still takes the write transaction.** `cockpit_agent_channel::identify` needs
  one, because a launch token binds on first use; the writer lock is held for reads only.
  A read-only identify path belongs to `cockpit-agent-channel`, outside this issue.
- **Rejected:** bumping on every poll (one write and two change rows per poll, 2000 per 1000
  polls in the measurement) and a threshold of several minutes (the expiry promise would
  drift further from report 17's "nobody waits for it" rule for no measurable gain).

Evidence: `crates/cockpit-approvals/tests/runs.rs`
`a_poll_while_the_expiry_is_fresh_writes_nothing`,
`a_poll_a_minute_after_the_last_refresh_moves_the_expiry`; the measurement in
`crates/cockpit-daemon/tests/approvals_poll_cost.rs`.
