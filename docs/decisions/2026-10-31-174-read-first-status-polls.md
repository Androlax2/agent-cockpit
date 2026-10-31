# Status polls read first and take the writer only to bind a launch or move an expiry

Issue #174. Decided by the implementing session, within the issue's proposal.

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
