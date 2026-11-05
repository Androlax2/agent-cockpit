# The TUI's phone layout: the daemon fits the manager, the viewer reads only

Issue #855. Decided by the worker within the issue's terms (one column under about 60
columns, cards first, a session read-only with one key to type, the same approval decision).

- **The daemon decides phone or desktop, from the window's width.** The TUI sends
  `manager-fit { session_open }` at start, on each resize and when a session opens or
  closes; the daemon reads the manager window's width and compares it with
  `tui.phone_below_columns` (owner only, default 60, `0` never). The column alone is zoomed
  for the list; an open session stacks a three-row bar of the column over the viewer. A fit
  reads the layout and the viewer's input from tmux rather than remembering them, so a fit
  after a failed one or a daemon restart still undoes a phone layout, and changes only what
  differs, so the resize it causes fits to the same layout and stops. On the phone,
  `session-show` stacks the viewer before it attaches, so the session takes the size the
  owner sees (#338). The TUI's own width would not do: zoomed, the column is the window; stacked, it
  is not, and only tmux knows the window.
- **Read-only is the viewer pane's input off (`select-pane -d`), not `attach -f read-only`.**
  tmux refuses every `send-keys` with "client is read-only" while a read-only client is its
  latest client, which would break message delivery and `session-type`. The pane flag drops
  the owner's keys and survives the viewer's respawn; the desktop turns it back on.
- **Typing is one line through the daemon (`session-type`), only into the session on
  screen.** The TUI keeps the keys on the phone and edits the line in its bar; the daemon
  types it and Enter into the agent pane and refuses any other session, so a stale TUI
  cannot type into a session the owner no longer sees. A minute without a key, Esc or
  leaving the session returns to read-only.
- **Approvals are the pane's keys, unchanged.** `y`/`x` on the phone go through the same
  pane, grace and command (revision, digest, `through: tui`), so the daemon's checks and the
  recorded decision are the desktop's. On a phone a card can be taller than the screen:
  its keys (`y`, `x`, and `i`/`s` that type to it) wait until its end has been on screen
  (`j` scrolls). No key in the open session decides a request.
- **Rejected:** a separate phone client (a second approval path to keep identical); the TUI
  measuring its own width (wrong once zoomed or stacked); typing raw keys into the viewer
  after a switch (a stray key reaches the agent, and tmux's read-only clients break
  `send-keys`); folding the command on the phone (the issue forbids truncation before a
  decision).
