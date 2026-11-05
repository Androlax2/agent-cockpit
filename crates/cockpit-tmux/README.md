# cockpit-tmux

All tmux integration on Cockpit's private server: version check, sessions and panes, control-mode observation and the capture stream.

tmux hosts every agent terminal (report 05, decided): persistence, detach and reattach,
resize, scrollback. This crate is the only code that runs tmux. It owns no session
lifecycle, no process termination and no task state: `cockpit-sessions` decides which
sessions exist, `cockpit-processes` ends process trees, and verification decides completion.

## Dependency edges

- Depends on: the standard library only (`tests/dependencies.rs`). tmux commands are
  processes and the control-mode reader is a dedicated thread (report 03), so it needs no
  async runtime.
- Used by: nothing yet. `cockpit-sessions` (#31), `cockpit-approvals` (#34) and the daemon's
  wiring will; clients never link it and read the capture stream through the daemon.

## Contract

- **Readiness.** [`Server::open`] runs `tmux -V` and refuses anything older than tmux 3.6
  ([`MINIMUM_VERSION`]), `next-3.6`/`3.6-rc…`, or a version it cannot read (`tmux master`).
  It queries the private running server's `#{version}` before writing configuration:
  upgrading the client alone does not replace the server that reaps agents. An old or
  unknown server is refused without ending sessions; an absent/exiting server is allowed. It
  writes [`config`] (mode 0600), applies it to a server already running (`source-file`, so
  an upgrade's config reaches the sessions it holds; #308) and starts nothing. The config's
  `history-limit` is [`ServerSpec::history_limit`], the owner's `memory.tmux_history_lines`
  (#406). Panes opened afterwards are bound by it; tmux 3.8 trims the open ones too.
- **Private server.** Every command is `tmux -f <Torlek's config> -S <socket>`: the caller
  names the socket by path, never by `-L` name, which falls back to `/tmp` when
  `$TMUX_TMPDIR` is missing and reaches another installation's server (#845). The user's
  `~/.tmux.conf` never applies, and the caller's `TMUX`/`TMUX_PANE` are removed so a daemon started inside tmux cannot reach it.
- **Sessions.** One session per agent. [`SessionName`] admits only `A-Z a-z 0-9 _ -`, and
  every target is `=name:` so `agent-1` never reaches `agent-12`. A missing working directory
  is refused (tmux would start the pane in `$HOME`). A command's words are never parsed by
  a shell, and it runs with `TMUX` and `TMUX_PANE` unset; a pane opened by hand gets the
  same (`default-command`). That removes the inherited pointer to this server, not the
  socket: keeping an agent from naming it is the sandbox's job. Sessions with names Cockpit
  would not choose are not listed. A `new-session` that reaches a server another caller's
  close just emptied waits for that server's exit and tries again (#202). An exited
  pane stays, with its exit status and its last screen in its history, until its owner
  closes it (`remain-on-exit`); this observation proves nothing about the work.
- **Viewer and approvals.** [`Server::show_in_viewer`] gives the agent session's window the
  viewer pane's size, then respawns the viewer pane as a nested client attached to it
  (#338); [`Server::attach_command`] is the same command, for a frontend that attaches a
  terminal to the manager session.
  [`Server::split_approval_pane`] splits a focused pane under the viewer.
  A [`Viewing::ReadOnly`] viewer has its pane input off (`select-pane -d`): keys typed into
  it reach no session, while `send-keys` to the session still works, which a client attached
  with `-f read-only` would refuse while it is tmux's latest client (#855).
  [`Server::set_viewing`] turns that input alone on or off; [`Server::pane_geometry`],
  [`Server::toggle_zoom`], [`Server::stack_panes`] and [`Server::place_side_by_side`]
  arrange the manager for its window's width.
- **Observation.** [`Server::observe`] attaches a `read-only,ignore-size` control-mode client
  (`-C`; `-CC` needs a terminal) and returns once tmux has attached it. Its reader thread
  always drains tmux, because a client that stops reading makes tmux stop reading the pane
  and would slow the agent. Past [`ObserveOptions::capacity`] it drops output and reports
  [`ControlEvent::OutputLost`]; tmux's `pause-after` remains as a backstop.
- **Capture stream.** [`Server::capture_stream`] gives clients a [`CaptureFrame::Snapshot`] of
  each pane, then [`CaptureFrame::Output`], and a fresh snapshot after any loss (at most two
  outstanding per pane). Snapshots are taken by the observing client itself, so they are
  ordered with the output. A pane that closed before its snapshot is reported
  [`CaptureFrame::PaneUnavailable`]; the stream goes on.
- **Restart.** The tmux server is not a child of the process that started it, and a [`Server`]
  holds no process: a restarted daemon opens a new handle, lists the sessions and observes
  them again, with the same panes and processes.

## Rules

- Terminal bytes are untrusted display data (report 05). Nothing here reads task or session
  state from a screen; provider adapters own any screen fallback.

## Tests

- `tests/fake_tmux.rs`: the version check and the command lines, through a fake `tmux`.
- `tests/sessions.rs`, `tests/control_mode.rs`, `tests/capture_stream.rs`,
  `tests/reattach.rs`: a real tmux on a private socket in a temporary directory, never the
  user's server.
- `tests/phone_layout.rs` (#855): through an owner client in control mode, keys reach the
  session through a read-write viewer and none through a read-only one, `send-keys` still
  works, and stacking, zooming and side by side give the panes their sizes.
