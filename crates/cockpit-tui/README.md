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
  whole; the cursor advances only once a batch is applied. A lost feed resumes from that
  cursor, a refused batch starts again from a snapshot (1 s, doubling to 30 s), and either
  shows on the status line until a batch applies.
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
- A session with something waiting shows "waiting on you" on its card.

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
