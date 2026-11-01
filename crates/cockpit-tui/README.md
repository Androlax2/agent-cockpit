# cockpit-tui

The plain reference terminal UI: renders the daemon's state and sends the owner's commands
through cockpit-client.

Deliberately basic until M6 (decision 17, report 05): the session list grouped by project,
open, next needing you, new session (`N`, the one form), restart, exit, the approval pane,
the log view and help. Effort, billing, compact and Remote Control are CLI verbs and provider
capabilities only; no key reaches them here.

## Dependency edges

- Depends on: `cockpit-client` and `cockpit-protocol` for the daemon, `cockpit-diagnostics`
  for its own log (#176, the `tui -> client/protocol/diagnostics only` rule of
  `cockpit-workspace-rules/tests/crate_graph.rs`), `ratatui` (with its Crossterm backend),
  `tokio`, `tracing`, `serde`, `serde_json`.
- Used by: nothing; it is a frontend. `cockpit tui` (the CLI) opens the manager session that
  runs it.

## How it runs

`cockpit tui` asks the daemon for the manager tmux session (`manager-open`) on the private
server and attaches the terminal to it. Its left pane runs `cockpit-tui --column --socket
PATH`, the pane beside it `cockpit-tui --viewer` until Enter shows a session there
(`session-show`, a nested attach the daemon makes, waking an asleep session). Closing or
detaching the terminal leaves everything running.

- **Never waits.** Keys are read on their own thread (presses only, report 05); each request
  runs in its own task on one shared connection; answers update the view as they arrive.
- **State.** Sessions and checkouts come from the change feed (#199): [`follow_feed`]
  subscribes on a connection of its own and [`Board`] applies each batch whole or refuses it
  whole; the cursor advances only once a batch is applied. A lost feed or a refused batch
  starts again from a snapshot (after 1 s, doubling to 30 s) and shows on the status line,
  beside any failed command, until a batch applies.
  What is not on the feed is refreshed every second: `project-list`, `agent-reports` and
  `approval-queue`. A command's answer never takes a session back to an older revision nor
  brings back one the feed took off.
- **Usage.** Every key counts as one named command (`open`, `restart`, `approval-approve`,
  `unbound`…) in `$XDG_STATE_HOME/cockpit/logs/tui/cockpit.log`: the name only, never what
  was typed, so M6 prunes on data.

## Approval pane (#166, a region of the TUI by #178)

- `y` approves the request shown, naming its revision and digest; no navigation key does.
- Input is ignored for [`INPUT_GRACE`] after the pane shows a request (or a new revision) or
  takes focus; the daemon refuses an approval within its own grace as the backstop.
- While the owner types into the pane (a line for a waiting command, a guide answer or
  note), the request shown stays whatever arrives.
- Warnings (each escaped kind, non-ASCII characters, the timeout hint, changed build files,
  a directory outside the project) and the folded command with an explicit "more" come from
  the wire fields.
- A session with something waiting shows `!` right after the selection marker; "waiting on
  you", the agent's label and its doing share an indented line under the id, wrapped at the
  list's width rather than cut.

## Tests

- `tests/feed.rs`: the list rendered from a recorded change feed, grouped by project.
- `tests/feed_follow.rs`: a session shown from the feed alone against a scripted daemon,
  with `subscribe` the only request; no list in the refresh; late answers ignored; a lost
  feed kept on the status line.
- `tests/pane.rs`: the pane's input model with an injected clock.
- `tests/keys.rs`: open, next needing you, restart and exit after `y`, help, the log view,
  and every key counted as a named command.
- `tests/form.rs`: the `N` form, from a fresh install's one project to `session-new`.
- `tests/usage.rs`: each key's command reaches the TUI's log by name.
- `tests/e2e/` (#300): the TUI end to end, the way Playwright drives a browser. A real
  daemon on a scratch home under `/tmp`, `cockpit tui` attached in a pane of a test-owned
  tmux server (the owner's terminal), a fake `claude` that reports through the agent channel
  and runs `!`-prefixed lines as commands. `press`, `type_text`, `screen`, `wait_for`,
  `click` (the owner's click on the list or the viewer) and `snapshot` (files under
  `tests/e2e/snapshots/`, recorded with `COCKPIT_E2E_UPDATE=1`). A screen wait reads the
  screen again after each write a control-mode client reports; a state wait follows the
  change feed; neither sleeps. Each module is a section of a live acceptance guide
  (`docs/acceptance/m1.md`, `m1-idle.md`); idle sleep runs on the daemon crate's
  `cockpit-e2e-daemon` example with a 3-second window. Every later guide adds its sections
  here, and a finding from a live walk gets a failing test here before its fix. Needs tmux;
  CI runs it on Linux and macOS, without the modules that need the agent channel
  (`ask`, `idle`, `needs_you`) until it reads process trees there (#336).
