# Status polls write only once a minute; the rest are read-only

Issue #164. Decided by the worker: the issue asks that `expires_at` move "only when it is
getting stale", and leaves the threshold open.

- **A poll stores a new expiry only when the stored one is at least 60 seconds older than a
  fresh one** (`REFRESH_AFTER_SECONDS`, `crates/cockpit-approvals/src/rows.rs`). Every other
  poll of an asking request or guide writes no row and no change-feed record. An asking
  request therefore expires between 14 and 15 minutes after its agent last asked about it,
  refining the 15 minutes of the #34 section of
  `2026-10-31-163-approvals-processes.md`; `cockpit ask run --wait` polls every 500 ms, so a
  waiting agent never comes near either bound.
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

## Revisited 2026-10-31 (#174)

Decided by the implementing session, within the issue's proposal. Status polls read first
and take the writer only to bind a launch or move an expiry.

- **`identify_bound` judges a pending token read-only and defers only its binding.** The
  checks `bind` makes (root recorded, sender in the tree) live in one read-only `bindable`
  (`crates/cockpit-agent-channel/src/launch.rs`) that both paths call, so a leaked or early
  token is refused without the writer, and only a token that would bind returns `None` for
  `identify` to spend. `tests/identify_bound.rs` checks every refusal matches `identify`.
- **The write fallback starts over rather than reusing what the read saw.** It identifies
  and reads again under the writer, so a launch bound or a request decided between the two
  transactions is seen as it is now; the read path costs one more read on the rare slow path.
- **`run_status_if_fresh` and `guide_status_if_fresh` say `None` when the expiry must move**
  (`crates/cockpit-approvals/src/rows.rs` `refreshed_expiry`), so the refresh rule stays in
  `cockpit-approvals` and the daemon never decides when a write is due.
- **Rejected:** passing the read path's session into the write fallback. The fallback runs
  about once a minute per waiting request; re-identifying under the writer keeps it equal to
  the pre-#174 path at the cost of one more scan of the launch roots.
- **Rejected for now:** `UPDATE … RETURNING` in the approvals mutations (the issue's
  optional item); it changes every mutation, not the poll path.
