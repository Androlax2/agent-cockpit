# The daemon projects sessions and checkouts onto the change feed after each commit, and the TUI follows it instead of polling

Issue #199. Decided by Theo (the projector, over in-transaction producers); the rest by the
worker, with the reasons below. Supersedes the polling note of
`2026-10-31-35-basic-tui.md`.

- **A daemon projector publishes the `session` and `checkout` kinds.** One supervised task
  (`crates/cockpit-daemon/src/session_feed.rs`) wakes on every commit and every 5 s, reads
  the views `session-list` and `checkout-list` answer, and publishes them in one write
  transaction,
  retracting ended sessions and removed checkouts it finds with
  `cockpit_messaging::published_ids`. It touches no capability crate: `cockpit-sessions`
  (about 15 write transactions) and `cockpit-projects` (about 6) stay as they are.
- **One commit late, never backwards.** The views land in the transaction after the change,
  not inside it. One task publishes in order, so a view never returns to an older state;
  every commit runs a pass, so the feed converges. A pass that derives what the last one
  wrote takes no write transaction, so the writer is not held and the projector does not
  wake itself; a write that changes nothing rolls back.
- **In-transaction publishing stays the contract for producers that own their writes**
  (`crates/cockpit-messaging/README.md`). Derived views, like these, may be projected by the
  daemon on commit. The daemon owns the two kinds outright: anything else published under
  them is retracted by the next pass that writes, which is why
  `cockpit-daemon/tests/change_feed.rs` now uses `attempt`.
- **The 5 s period bounds what changes without a commit.** The changed start files
  `SessionView` carries are read from disk, and checkout trust from the owner's
  configuration; neither commits when it changes. Polling showed them within 1 s.
- **A broken owner configuration leaves the published checkouts unchanged** (trust needs it)
  rather than publishing every checkout untrusted; sessions are still published, and the
  warning is logged once per problem. A session whose view cannot be read is logged and kept
  as published, so it does not stop the others.
- **The TUI subscribes on its own connection and drops `session-list` and `checkout-list`
  from its refresh.** Projects, agent reports and the approval queue stay polled every
  second until they are on the feed. Resync is the feed's own snapshot: after a lost feed or
  a refused batch the TUI subscribes from a snapshot, not its cursor, because a resumed feed
  with nothing new sends no batch and only an applied batch says the board is current.
  Failures in a row wait 1 s, doubling to 30 s, so a snapshot the board keeps refusing is not
  resent every second; the problem stays on the status line until a batch applies.
- **A command's answer never takes a session back to an older revision, nor brings back one
  the feed took off** (`Board::put_session`): without a polling refresh, a late answer would
  otherwise stay on screen until the next change. Only the feed writes the lists: the
  `session-list` and `checkout-list` answers are no longer applied.
- **Rejected:** publishing inside every `cockpit-sessions` and `cockpit-projects`
  transaction, because it edits the crates other M1 issues work in, spreads the wire shape
  over two capability crates, and cannot carry the changed start files; an in-memory set of
  published ids, because a crash between a removal and its retraction would leave the
  entity on the feed for good.
