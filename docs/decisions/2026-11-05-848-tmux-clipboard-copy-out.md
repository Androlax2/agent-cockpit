# Cockpit's tmux server takes OSC 52 from its panes and answers no clipboard query

Issue #848 (copy out of a session). Decided by Theo: copy out needs no new access; paste in
(a desktop image into the agent) is a separate decision on the same issue.

- **`set-clipboard on`.** The viewer is a client of Cockpit's server nested in a pane, so a
  copy goes agent pane → nested client → viewer pane → owner's client → terminal. tmux's
  default `external` sends tmux's own copies to the terminal but refuses OSC 52 written by a
  pane's program, and the nested client is one: copies stopped at the viewer. Proven by
  `crates/cockpit-tmux/tests/clipboard.rs` (an agent's OSC 52, and a copy in the agent's copy
  mode opened with the owner's keys, prefix twice then `[`, reach a stand-in terminal; both fail
  with `external`).
- **`terminal-features` for `tmux*` and `screen*`.** tmux sends OSC 52 only to a client whose
  terminal has the clipboard feature. The nested client's terminal is a pane of this server;
  naming the feature keeps the copy from depending on the host's terminfo or on tmux's
  detection. Indexed entries, so applying the file again replaces them.
- **`get-clipboard off`.** With `on`, tmux also answers a pane's OSC 52 query with the newest
  paste buffer, which holds what the owner last copied, from any session. No pane reads it
  (`a_pane_asking_for_the_clipboard_gets_no_answer`). `request` would forward the query to
  the desktop clipboard; it is never set.
- **Accepted:** an agent can write the desktop clipboard while the owner views it (an OSC 52 in
  its output, or in a file it prints), and a paste buffer at any time, which tmux's own paste
  (prefix `]`) types into whichever pane has focus. `set-clipboard` is server-wide in tmux, so
  it cannot be on for the viewer only. A copy is parsed twice (agent pane, then viewer pane),
  so it makes two paste buffers. Reading the clipboard stays impossible from a session.
- **Audit L4** (`docs/security-audit-2026-10-19.md`) asked for `set-clipboard` off in the
  legacy `claude-run-*` sessions, where an approved command's raw output reached a pane. In
  `crates/`, an approved command runs in `cockpit-approvals`' own pseudo-terminal and the TUI
  shows its output without terminal controls, so no tmux pane parses it.
- **Rejected:** `set-clipboard external` with a copy binding that pipes to the host's
  `wl-copy`, because the tmux server has no display either, and an agent's own copy (Claude
  Code's `/copy`) would still not reach the desktop.
- **Live (decision 308):** `set-clipboard` and `get-clipboard` are read at each OSC 52, so on a
  running server every running session's next OSC 52 follows them at once, with no restart or
  opt-in. The terminal features apply at the latest when the owner's viewer attaches next
  (every session switch respawns it).
