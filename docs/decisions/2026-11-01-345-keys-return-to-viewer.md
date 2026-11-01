# Keys the approval pane took from the viewer go back to it once nothing waits

Issue #345. Decided by the worker: the issue's option A, refined so it never moves keys the
owner did not have in the viewer.

- **`manager-focus` names its pane and answers where the keys were.** `pane: column | viewer`;
  left out, as a TUI before #345 sends it, it means the column, its only meaning then.
  `manager-focused` carries `took_from_viewer`; the daemon serializes focus moves so that
  answer stays true. The TUI remembers keys it took
  from the viewer and asks for `viewer` once the queue empties, including when tmux's answer
  arrives after the queue emptied (`crates/cockpit-tui/src/app.rs`, `TakenKeys`).
- **Keys the owner uses in the column stay there:** when the owner tabbed to the list, or a
  form, confirmation, the workflow view, help or log is open as the queue empties, nothing
  goes back.
- **Keys the list already had stay in the list.** An owner who clicked the list before the
  request keeps typing to the list after it (`e2e ask::the_keys_stay_in_the_list_the_owner_had_them_in_once_the_pane_empties`).
- **Rejected:** always giving the keys to the viewer on empty (moves keys the owner put in
  the list); option B, leaving them in the list (the bug: `r` then `y` meant for the agent
  restarts the selected session); the daemon remembering the previous pane (state the TUI
  restarting would leave stale, and the TUI already owns the pane's lifecycle).
