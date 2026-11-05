# Cockpit's tmux server takes OSC 52 from its panes and answers no clipboard query

Issue #848 (copy out of a session). Decided by Theo: copy out needs no new access; paste in
(a desktop image into the agent) is a separate decision on the same issue.

- **`set-clipboard on`.** The viewer is a client of Cockpit's server nested in a pane, so a
  copy goes agent pane → nested client → viewer pane → owner's client → terminal. tmux's
  default `external` sends tmux's own copies to the terminal but refuses OSC 52 written by a
  pane's program, and the nested client is one: copies stopped at the viewer. Proven by
  `crates/cockpit-tmux/tests/clipboard.rs` (an agent's OSC 52, and a copy-mode selection, reach
  a stand-in terminal).
- **`get-clipboard off`.** With `on`, tmux also answers a pane's OSC 52 query with the newest
  paste buffer, which holds what the owner last copied, from any session. No pane reads it
  (`a_pane_asking_for_the_clipboard_gets_no_answer`). `request` would forward the query to
  the desktop clipboard; it is never set.
- **Accepted:** an agent can write the desktop clipboard while the owner views it, and a paste
  buffer at any time, which tmux's own paste (prefix `]`) then types into whichever pane has
  focus. Reading the clipboard stays impossible from a session.
- **Rejected:** `set-clipboard external` with a copy binding that pipes to the host's
  `wl-copy`, because the tmux server has no display either, and it would leave an agent's own
  copy (Claude Code's `/copy`) unable to reach the desktop.
- Both options are server-wide and safe live (decision 308): they change only what the next
  OSC 52 does.
