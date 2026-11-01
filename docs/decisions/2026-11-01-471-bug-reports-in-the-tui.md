# Bug report drafts reach the TUI on the change feed, as one needs-you card, a list and a draft form

Issue #471. Points 1 to 3 were decided by the coordinator, to whom Theo delegated UX choices that
bring no new dependency or security trade-off. The rest was decided by the implementing session.

- **One aggregate card, not one per draft (1 B).** The session list opens with
  `! Bug report drafts (N): B lists them` whenever a draft waits. Daemon drafts can pile up
  over a day, and one card per draft would push down the sessions that need the owner.
- **Drafts are a `bug-report` entity on the change feed.** `cockpit-daemon`'s
  `bug_report_feed` publishes each draft as its `bug-report-list` view after every commit, in
  its own transaction, the way `session_feed` publishes sessions (decision #199). A sent or
  discarded draft is retracted. The TUI follows the feed and never polls `bug-report-list`.
  Only the owner's socket may `subscribe`, so agents never read a draft. The feed's change
  history keeps past states, descriptions included, for as long as it keeps any change. That
  history lives in the owner's own state database, like the drafts themselves.
- **The TUI only drafts (2 A).** `b` opens a form (what went wrong, then an optional title and
  steps) that sends `bug-report-new`. `B` lists the drafts: number, origin, title, repeats and
  session. Review, edit, send and discard stay in `cockpit bug-report`, and the list names
  those commands. Sending needs the owner's browser or `gh`, which belong to the CLI.
- **Plain text, no design session (3).** The card, list and form use the TUI's existing
  paragraphs and the `N` form's layout, the way #410 shipped the coordinator line. The
  designed version belongs to Beta 3 design #465.
