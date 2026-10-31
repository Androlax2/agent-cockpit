# The daemon projects sessions and checkouts onto the change feed after each commit, and the TUI follows it instead of polling

Issue #199. Decided by Theo (the projector, over in-transaction producers); the rest by the
worker, with the reasons below. Supersedes the polling note of
`2026-10-31-35-basic-tui.md`.

- **A daemon projector publishes the `session` and `checkout` kinds.** One supervised task
  (`crates/cockpit-daemon/src/session_feed.rs`) wakes on every commit, reads the views
  `session-list` and `checkout-list` answer, and publishes them in one write transaction,
  retracting ended sessions and removed checkouts it finds with
  `cockpit_messaging::published_ids`. It touches no capability crate: `cockpit-sessions`
  (about 15 write transactions) and `cockpit-projects` (about 6) stay as they are.
- **One commit late, never backwards.** The views land in the transaction after the change,
  not inside it. One task publishes in order, so a view never returns to an older state;
  every commit runs a pass, so the feed converges. A pass that changes nothing rolls back
  instead of committing, so it does not wake itself.
- **In-transaction publishing stays the contract for producers that own their writes**
  (`crates/cockpit-messaging/README.md`). Derived views, like these, may be projected by the
  daemon on commit. The daemon owns the two kinds outright: anything else published under
  them is retracted, which is why `cockpit-daemon/tests/change_feed.rs` now uses `attempt`.
- **The changed start files are compared on each pass.** `SessionView` carries them; they
  can only be computed by reading files, never inside a transaction. A start file edited
  with no commit afterwards shows on the next commit.
- **A broken owner configuration leaves the published checkouts unchanged** (trust needs it)
  rather than publishing every checkout untrusted; sessions are still published.
- **The TUI subscribes on its own connection and drops `session-list` and `checkout-list`
  from its refresh.** Projects, agent reports and the approval queue stay polled every
  second until they are on the feed. Resync is the feed's own snapshot: a lost feed resumes
  from the applied cursor, a batch the board refuses starts again from a snapshot.
- **A command's answer never takes a session back to an older revision**
  (`Board::put_session`): without a polling refresh, a late answer would otherwise stay on
  screen until the next change.
- **Rejected:** publishing inside every `cockpit-sessions` and `cockpit-projects`
  transaction, because it edits the crates other M1 issues work in, spreads the wire shape
  over two capability crates, and cannot carry the changed start files; an in-memory set of
  published ids, because a crash between a removal and its retraction would leave the
  entity on the feed for good.
