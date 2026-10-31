# Status polls read first and take the writer only to bind a launch or move an expiry

Issue #174. Decided by the implementing session, within the issue's proposal.

- **`identify_bound` defers a pending token instead of judging it.** When a well-formed
  token names a pending launch it returns `None`, and the daemon redoes the whole poll with
  `identify` in a write transaction. Binding, `LaunchNotStarted` and `OutsideLaunchTree`
  therefore keep one owner (`bind` in `crates/cockpit-agent-channel/src/launch.rs`); every
  other outcome is shared code, and `tests/identify_bound.rs` checks the refusals match.
- **The write fallback starts over rather than reusing what the read saw.** It identifies
  and reads again under the writer, so a launch bound or a request decided between the two
  transactions is seen as it is now; the read path costs one more read on the rare slow path.
- **`run_status_if_fresh` and `guide_status_if_fresh` say `None` when the expiry must move**
  (`crates/cockpit-approvals/src/rows.rs` `refreshed_expiry`), so the refresh rule stays in
  `cockpit-approvals` and the daemon never decides when a write is due.
- **Rejected:** checking the token's launch tree read-only and refusing early. It would
  duplicate `bind`'s refusals for a case (a leaked token) that is already rare and refused.
- **Rejected for now:** `UPDATE … RETURNING` in the approvals mutations (the issue's
  optional item); it changes every mutation, not the poll path.
