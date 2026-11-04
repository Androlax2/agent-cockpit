# cockpit-tui

The plain reference terminal UI: renders the daemon's state and sends the owner's commands
through cockpit-client.

Deliberately basic until M6 (decision 17, report 05): the session list grouped by project,
open, next needing you, new session (`N`), restart, exit, the approval pane,
the task view (`T`, #44), the workflow view (`F`, #330), bug report drafts (one needs-you card, the `B` list and the `b`
form, #471; review and send stay in `torlek bug-report`), the log view and help. Effort, billing, compact and Remote Control are CLI verbs and provider
capabilities only; no key reaches them here.

## Dependency edges

- Depends on: `cockpit-client` and `cockpit-protocol` for the daemon, `cockpit-diagnostics`
  for its own log (#176, the `tui -> client/protocol/diagnostics only` rule of
  `cockpit-workspace-rules/tests/crate_graph.rs`), `ratatui` (with its Crossterm backend),
  `crossterm` itself for the key reader, with `use-dev-tty` (#356), `tokio`, `tracing`,
  `serde`, `serde_json`.
- Used by: nothing; it is a frontend. `torlek tui` (the CLI) opens the manager session that
  runs it.

## How it runs

`torlek tui` asks the daemon for the manager tmux session (`manager-open`) on the private
server and attaches the terminal to it. Its left pane runs `torlek-tui --column --socket
PATH`, the pane beside it `torlek-tui --viewer` until Enter shows a session there
(`session-show`, a nested attach the daemon makes, waking an asleep session). Closing or
detaching the terminal leaves everything running.

- **Never waits.** Keys are read on their own thread (presses only, report 05); each request
  runs in its own task on one shared connection; answers update the view as they arrive.
- **State.** Sessions, checkouts and bug report drafts come from the change feed (#199, #471): [`follow_feed`]
  subscribes on a connection of its own and [`Board`] applies each batch whole or refuses it
  whole; the cursor advances only once a batch is applied. A lost feed or a refused batch
  starts again from a snapshot (after 1 s, doubling to 30 s) and shows on the status line,
  beside any failed command, until a batch applies.
  What is not on the feed is refreshed every second: `project-list`, `agent-reports` and
  `approval-queue`. A command's answer never takes a session back to an older revision nor
  brings back one the feed took off.
- **Usage.** Every key counts as one named command (`open`, `restart`, `approval-approve`,
  `unbound`…) in `$XDG_STATE_HOME/torlek/logs/tui/cockpit.log`: the name only, never what
  was typed, so M6 prunes on data.

## Attention bell (#682)

The column rings the terminal bell (`BEL`, which works over SSH and inside tmux, where
`bell-action` decides what the terminal does with it) when a session newly needs the owner:

- it finished its work (the task list says so, or its turn completed);
- it asks a question or waits for an approval, a guide or a line of input;
- it failed, or its agent exited by itself.

One bell per look at the daemon's state, however many sessions changed in it. A cue rings when
it is raised, never while it stays, so the next refresh repeats nothing. What the owner does
rings nothing: ending a session, or answering a question. What was already waiting when the
TUI started rings nothing either. Every cue is on the screen too: the bell carries no
information of its own, and a terminal without one loses nothing. Network and permission holds
and usage alerts ring once the TUI sees them; the desktop client (#62) and the phone (#247)
deliver their own sounds.

The owner turns the bell off with `[tui] attention_bell = false` in `config.toml` (on by
default; owner setting, a project file cannot set it). The daemon reads it when it builds the
manager session and starts the column with `--bell on|off`, so a change counts the next time the
manager is built (close it, then `torlek tui`).

## Tasks (#44)

`T` lists open tasks across projects. `a` adds one, `e` edits the selected item;
Tab changes the title, notes, kind and project field, Left/Right chooses kind or
project, Enter saves and Esc cancels. An edit retains the revision it opened on;
a refusal keeps the draft text. `Q` sets the selected task's queue flag, and `?`
shows the last scheduling explanation, including its evaluation time and action.

`h` opens a picker of launched sessions in the task's project; Enter hands the
captured task snapshot to the selected session launch. The daemon retains its
eligibility and admission checks, including busy, viewed, idle-window and paused
queue refusals. A stale request stays stale until the owner cancels and selects
again. For CLI handoff, `torlek todo show ID` prints the task revisions and
`torlek todo targets` prints each session's ID, launch number, state and directory;
pass the selected values to `torlek todo handoff`. Items with running workflows use the workflow view. The approval pane
remains reachable with Tab outside the task form.

## Approval pane (#166, a region of the TUI by #178)

- When a request appears and nothing was shown, the pane takes focus and asks the daemon
  for tmux's keys (`manager-focus`), so the next key reaches the pane, not the agent in the
  viewer (#317). The daemon answers whether the viewer had them; when nothing waits any
  more, keys taken from the viewer go back to it, unless the owner tabbed to the list or has a
  form, confirmation, the task or workflow view, help or log open; keys the list already had stay (#345).
- `y` approves the request shown, naming its revision and digest; no navigation key does.
- Input is ignored for [`INPUT_GRACE`] after the pane shows a request (or a new revision),
  takes focus (in the TUI, or tmux's keys arriving) or ignored a key; the daemon refuses an
  approval within its own grace as the backstop.
- While the owner types into the pane (a line for a waiting command, a guide answer or
  note), the request shown stays whatever arrives.
- Warnings (each escaped kind, non-ASCII characters, the timeout hint, changed build files,
  a directory outside the project) and the folded command with an explicit "more" come from
  the wire fields.
- A session with something waiting shows `!` right after the selection marker; one whose
  agent asked its own question and still works shows `?` and "asked you, still working"
  instead (#215), and `n` still opens it. "waiting on you", the agent's label and its doing
  share an indented line under the id, wrapped at the list's width under that indent rather
  than cut (#291).
- The list scrolls to keep the selected card whole on screen (#291).
- A card names its session's coordinator grants and the latest message a coordinator queued
  to it, sender and delivery state included, on one plain line until the card has a design
  (#410).

## Tests

- `tests/feed.rs`: the list rendered from a recorded change feed, grouped by project.
- `tests/feed_follow.rs`: a session shown from the feed alone against a scripted daemon,
  with `subscribe` the only request; no list in the refresh; late answers ignored; a lost
  feed kept on the status line.
- `tests/pane.rs`: the pane's input model with an injected clock.
- `tests/keys.rs`: open, next needing you, restart and exit after `y`, help, the log view,
  and every key counted as a named command.
- `tests/form.rs`: the `N` form, from a fresh install's one project to `session-new`.
- `tests/tasks.rs`: list, form, stale replies and handoff identity, queue explanation,
  scrolling and approval-pane focus; `tests/e2e/tasks.rs` drives add/edit/queue/explanation
  through a real terminal and isolated daemon.
- `tests/workflows.rs`: the `F` view, a running workflow's steps, checkpoint, next action
  and last handoff, and the owner's complete, release, unblock and cancel keys.
- `tests/usage.rs`: each key's command reaches the TUI's log by name.
- `tests/coordination.rs`: the card's coordinator grants and inbound message line (#410).
- `tests/e2e/` (#300): the TUI end to end, the way Playwright drives a browser. A real
  daemon on a scratch home under `/tmp`, `torlek tui` attached in a pane of a test-owned
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
