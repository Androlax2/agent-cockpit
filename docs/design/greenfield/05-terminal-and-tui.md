# 05. Terminal hosting and TUI

Status: decided

## Decision

- **tmux is a required dependency.** It hosts agent terminals: persistence, detach/reattach,
  resize, scrollback, interactive agent CLIs. Works on Linux, macOS and WSL2.
- `cockpit-tmux` owns the whole integration: a **private tmux server** with Cockpit's own
  config, pane creation/attachment, control-mode observation (`-CC`), flow control, and
  version checks at install/readiness time.
- The TUI is **Ratatui + Crossterm**; it renders local view state and talks to the daemon
  through `cockpit-client`. Input handling never waits on provider, Git, database or tmux calls.
- **Plain reference TUI first** (keys decided 2026-10-31): session list, open, next needing you, new session, restart, exit, approval pane, log view, help; basic todo list and form; basic team requests and Chief needs-you lists. Nothing else until M6, where usage data from diagnostics decides what returns. It proves
  every capability is reachable and exposes responsiveness problems. Designed UI/UX is the
  **last milestone**, done in FigCraft with the `design-first` workflow.
- Approvals and guided steps are **terminal-native**; desktop notifications are optional.

## Rules

- A closed pane proves nothing: completion comes from verification, termination from
  `cockpit-processes` observing the process group/container.
- Terminal bytes are untrusted display data. Provider adapters detect session state from
  structured events first (hooks, plugin, Codex bridge); screen classification is allowed only
  as an adapter-internal fallback that records its confidence (`observed-by: event|screen`),
  because hooks are known to go missing. Task completion never comes from the screen.
- One **private tmux server** (`-L cockpit`): the TUI session, one session per agent, approval
  panes split under the viewer; nested attach from the viewer pane as today.
- The daemon never depends on a TUI being attached; closing the TUI leaves work running.
- All view logic stays in `cockpit-tui`; the daemon exposes data contracts the final UI
  needs from day one: "why waiting", "what changed since revision R", revisions and cursors.
- Crossterm key events: filter to `KeyEventKind::Press` so WSL/Windows terminals do not
  double-fire.

## Rejected

- Cockpit-owned terminal runtime (PTY, emulation, reattach): large effort, no product gain.
- Tauri/web desktop or Iced native UI: not a terminal product; see report 01.
- Skipping the plain TUI and building only CLI: interactive latency problems would surface
  only at the designed-UI stage.

## Sources

- tmux control mode and flow control: <https://github.com/tmux/tmux/wiki/Control-Mode>
- tmux concepts: <https://github.com/tmux/tmux/wiki/Getting-Started>
- Ratatui async/event guidance: <https://ratatui.rs/faq/>
- Crossterm event rules: <https://docs.rs/crossterm/latest/crossterm/event/index.html>
- xterm.js on terminals as untrusted data (principle): <http://xtermjs.org/docs/guides/security/>
