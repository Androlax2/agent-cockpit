# The tmux observer never makes tmux wait, and panes never inherit a pointer to Cockpit's server

Issue #30. Decided by the worker, from measurements on tmux 3.8 (report 05 names control
mode with `pause-after` flow control).

- **The reader always drains tmux.** A control client that stops reading makes tmux stop
  reading the pane, so the agent's own output stalls, and `%pause` then often never comes
  because the stalled pane produces nothing more to age (6 of 12 runs, in a flood fixture).
  Past `capacity` waiting outputs the reader drops that pane's output and reports
  `OutputLost`; the capture stream answers with a fresh snapshot (at most two outstanding
  per pane). Replies and the exit are always queued, in order with the output; they answer
  the consumer's own commands, so they stay bounded. `pause-after` stays set as tmux's own
  backstop and is handled the same way.
- **`-C`, not `-CC`.** The daemon reads control mode through pipes; `-CC` needs a terminal.
- **Exact targets are `=name:`.** `=name` alone still reached `agent-12` from `agent-1` in
  `list-panes -s`, which takes a window target and falls back to window names.
- **Snapshots through the observing client.** `capture-pane` is sent on the control
  client's own stdin, so its reply is ordered with the pane's `%output` lines.
- **Std only.** tmux calls are processes and the reader is a dedicated thread (report 03), so
  the crate has no dependency; `crates/cockpit-tmux/tests/dependencies.rs` keeps it that way.
  Clients may not link it (`clients -> tmux` in the crate-graph rule): they get the capture
  stream from the daemon.
- **Panes lose `TMUX` and `TMUX_PANE`.** tmux sets both in every pane, and `new-session`
  ignores `-e TMUX=`, so a plain `tmux` an agent runs would list, read or kill Cockpit's
  sessions. Every pane command, the viewer's nested client included, runs as
  `/bin/sh -c 'unset TMUX TMUX_PANE; exec "$@"' sh <command>`; `default-command` does the
  same for a pane a human opens in the viewer (tests: `*_cannot_list_the_servers_sessions`
  in `crates/cockpit-tmux/tests/sessions.rs`). This removes the inherited pointer only: the
  same user can still find and name the socket, which is the sandbox's to forbid.
- **An exited pane reports no pid.** CI's Ubuntu tmux still prints the ended process's pid,
  which the system may reuse; tmux 3.8 prints none.
- **Rejected:** blocking on the consumer with `pause-after` alone, because it stalls the
  agent; `env -u TMUX -u TMUX_PANE --` as the pane wrapper, because `env` reads a `=` in the
  program path as an assignment; `-e TMUX=`, because `new-session` ignores it and nothing
  clears `TMUX_PANE`.
- **Not here:** daemon wiring and the session verbs (#31, #35); this issue delivers the
  library they call, proven against real tmux.
