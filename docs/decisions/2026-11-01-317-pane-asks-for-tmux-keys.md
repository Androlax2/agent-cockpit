# The TUI asks the daemon for tmux's keys when its approval pane takes focus

Issue #317. Decided by the worker: the issue offers three ways and leaves the pick to the fix.

- **The TUI sends `manager-focus` when its pane goes from showing nothing to showing a
  request** (`crates/cockpit-tui/src/app.rs`, `follow_queue`), the moment it moves its own
  focus to the pane. The daemon selects the manager's column pane on its private tmux server
  (`crates/cockpit-daemon/src/manager.rs`, `focus_column`); the TUI still runs no tmux
  (`tui -> client/protocol/diagnostics only`). The first key after the jump lands in a pane
  that already shows the request and its input grace, so it approves nothing.
- **Only on that transition**, as the TUI's own focus: a later head of the queue, or a request
  shown while the owner deliberately clicked the viewer, leaves tmux's focus where the owner
  put it. A failed `manager-focus` shows on the status line like any failed command.
- **The grace starts again on each key it ignored and when `manager-focused` arrives**
  (`crates/cockpit-tui/src/pane.rs`). An owner still typing to the agent when the keys move
  would otherwise reach a `y` in their sentence once the grace ended; and the daemon may move
  the keys well after the pane showed the request. The keys typed into the pane meanwhile are
  lost to the agent: the price of taking the keys at all.
- **Rejected:** the daemon selecting the column when its approval queue becomes non-empty.
  It would move the keys up to a refresh (1 s) before the TUI shows the request, so a key
  typed for the agent could reach the session list (`r`, then `y`, restarts a session).
  Changing the title and guide to "click the list": the leak stays.
