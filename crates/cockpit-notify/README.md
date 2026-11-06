# cockpit-notify

Desktop notifications and sounds when a session needs the owner: a client of the daemon's
change feed, so the daemon stays headless.

Issues #62 and #682; report 19 and decision 11 of the greenfield design.

## What it tells

One event per reason a session needs the owner, each on or off apart for the desktop and for
sound:

| Event | Raised when | Sound (freedesktop name) |
| --- | --- | --- |
| `failure` | the session failed, its agent exited by itself, or its turn failed | `dialog-error` |
| `hold` | a command waits for approval, a sandboxed host was refused, or a provider's permission dialog is open | `dialog-warning` |
| `question` | the agent asks a question, a guide waits on its step, or a command waits for a line of input | `message-new-instant` |
| `quota` | the plan's usage limit stopped its turn | `alarm-clock-elapsed` |
| `finished` | the task list says it finished, or its turn completed | `complete` |

An event is told when it is raised, never while it stays. Events that land together make one
notification, the most urgent first, and one sound, the most urgent event's. Nothing is told
for the state the notifier started on, for a session the owner ended, or for a question a
coordinator holds (#710).

## Outlets

- **Desktop:** `notify-send`, over the session bus (`DBUS_SESSION_BUS_ADDRESS` or
  `$XDG_RUNTIME_DIR/bus`). A failure is critical, everything else normal.
- **Sound:** `canberra-gtk-play --id=NAME`, which plays the owner's sound theme; without it,
  `pw-play` or `paplay` plays `sounds/freedesktop/stereo/NAME.oga` from `XDG_DATA_DIRS`.

No new crate is needed: both are programs every Linux desktop has. A host without a session
bus, `notify-send` or a player runs the notifier all the same (#62's headless acceptance):
that outlet is off and its log says why. A program that fails or hangs is logged and stopped
after ten seconds; nothing waits on it. macOS stays with #568.

## How it runs

`torlek-notify --socket PATH --state-dir DIR [--desktop EVENTS] [--sound EVENTS]`, where
`EVENTS` is `none` or a comma-separated list of event names; left out, every event is on. It
follows the change feed for sessions and view stamps, asks the daemon for the agents' reports,
the approval queue and the network requests when their stamp moves and every five seconds,
and logs under `DIR/logs/notify` (report 12).

## Dependency edges

- Depends on: `cockpit-client` and `cockpit-protocol` for the daemon, `cockpit-diagnostics`
  for its own log, `tokio` (its `process` feature runs the outlets), `tracing`, `serde`,
  `serde_json`.
- Used by: nothing; it is a client (`CLIENTS` in `cockpit-workspace-rules/tests/crate_graph.rs`:
  never persistence, scheduling or tmux).
