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

## Revisited 2026-11-01 (#337)

Decided by the worker: the issue offers two options and recommends neither. The pane's input
grace is 2 s, so it always outlasts the daemon's.

- **`INPUT_GRACE` is 2 s** (`crates/cockpit-tui/src/pane.rs`; it was 1 s, decision #35). The
  daemon refuses an approval while `now < revised_at + 2` in whole seconds
  (`APPROVAL_GRACE_SECONDS`, `crates/cockpit-approvals/src/runs.rs`), at most 2 real seconds
  after the revision appeared. The TUI shows a revision only after the daemon created it, so a
  key accepted 2 s after showing reaches the daemon past its grace. A `y` pressed once the
  hint "Read it first" goes away is never refused, and the e2e approval test no longer
  presses `y` again after a refusal.
- **It assumes the wall clock does not step back** during the grace: the TUI counts on
  `Instant`, the daemon on wall-clock seconds. After such a step the daemon refuses and the
  owner presses `y` again, as before; the e2e test checks the value against the real daemon.
- **Rejected:** keeping the hint until the request's `revised_at` plus the daemon's grace
  (option B). It compares the daemon's wall clock with the TUI's, couples the TUI to the
  daemon's constant, and saves at most one second of reading time on a pane meant to be read.

## Revisited 2026-11-01 (#345)

Decided by the worker: the issue's option A, refined so it never moves keys the owner did not
have in the viewer. Keys the approval pane took from the viewer go back to it once nothing
waits.

- **`manager-focus` names its pane and answers where the keys were.** `pane: column | viewer`;
  left out, as a TUI before #345 sends it, it means the column, its only meaning then.
  `manager-focused` carries `took_from_viewer`; the daemon serializes focus moves so that
  answer stays true. The TUI remembers keys it took from the viewer and asks for `viewer` once
  the queue empties, including when tmux's answer arrives after the queue emptied
  (`crates/cockpit-tui/src/app.rs`, `TakenKeys`).
- **Keys the owner uses in the column stay there:** when the owner tabbed to the list, or a
  form, confirmation, the workflow view, help or log is open as the queue empties, nothing
  goes back.
- **Keys the list already had stay in the list.** An owner who clicked the list before the
  request keeps typing to the list after it (`e2e ask::the_keys_stay_in_the_list_the_owner_had_them_in_once_the_pane_empties`).
- **Rejected:** always giving the keys to the viewer on empty (moves keys the owner put in
  the list); option B, leaving them in the list (the bug: `r` then `y` meant for the agent
  restarts the selected session); the daemon remembering the previous pane (state the TUI
  restarting would leave stale, and the TUI already owns the pane's lifecycle).
