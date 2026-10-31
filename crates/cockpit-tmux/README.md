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

- **Readiness.** [`Server::open`] runs `tmux -V` and refuses anything older than tmux 3.2
  ([`MINIMUM_VERSION`]) or a version it cannot read (`tmux master`), naming the binary. It
  writes [`CONFIG`] (mode 0600) and starts nothing.
- **Private server.** Every command is `tmux -f <Cockpit's config> -L cockpit` ([`Socket`];
  tests use `-S` in a temporary directory). The user's `~/.tmux.conf` never applies, and the
  caller's `TMUX`/`TMUX_PANE` are removed so a daemon started inside tmux cannot reach it.
- **Sessions.** One session per agent. [`SessionName`] admits only `A-Z a-z 0-9 _ -`, and
  every target is `=name:` so `agent-1` never reaches `agent-12`. A command runs directly,
  never through a shell. An exited pane stays, with its exit status and its last screen in
  its history, until its owner closes it (`remain-on-exit`); this observation proves nothing
  about the work.
- **Viewer and approvals.** [`Server::show_in_viewer`] respawns the viewer pane as a nested
  client attached to an agent session (`TMUX` emptied for it only).
  [`Server::split_approval_pane`] splits a focused pane under the viewer.
- **Observation.** [`Server::observe`] attaches a `read-only,ignore-size` control-mode client
  (`-C`; `-CC` needs a terminal) and returns once tmux has attached it. Its reader thread
  always drains tmux, because a client that stops reading makes tmux stop reading the pane
  and would slow the agent. Past [`ObserveOptions::capacity`] it drops output and reports
  [`ControlEvent::OutputLost`]; tmux's `pause-after` remains as a backstop.
- **Capture stream.** [`Server::capture_stream`] gives clients a [`CaptureFrame::Snapshot`] of
  each pane, then [`CaptureFrame::Output`], and a fresh snapshot after any loss. Snapshots are
  taken by the observing client itself, so they are ordered with the output.
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
