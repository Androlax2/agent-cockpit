# The approval pane's input grace is 2 s, so it always outlasts the daemon's

Issue #337. Decided by the worker: the issue offers two options and recommends neither.

- **`INPUT_GRACE` is 2 s** (`crates/cockpit-tui/src/pane.rs`; it was 1 s, decision #35). The
  daemon refuses an approval while `now < revised_at + 2` in whole seconds
  (`APPROVAL_GRACE_SECONDS`, `crates/cockpit-approvals/src/runs.rs`), at most 2 real seconds
  after the revision appeared. The TUI shows a revision only after the daemon created it, so a
  key accepted 2 s after showing reaches the daemon past its grace. A `y` pressed once the
  hint "Read it first" goes away is never refused, and the e2e approval test no longer
  presses `y` again after a refusal.
- **Rejected:** keeping the hint until the request's `revised_at` plus the daemon's grace
  (option B). It compares the daemon's wall clock with the TUI's, couples the TUI to the
  daemon's constant, and saves at most one second of reading time on a pane meant to be read.
