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
- **Sound:** `canberra-gtk-play --id=NAME`, which plays the owner's sound theme, when there
  is a display (`DISPLAY` or `WAYLAND_DISPLAY`); otherwise `pw-play` or `paplay` plays
  `sounds/freedesktop/stereo/NAME.oga` from `XDG_DATA_DIRS`.
- **Text:** the agent writes the names and details, so each shows on one line, cut short, its
  markup escaped (notification servers read the body as markup).

No new crate is needed: both are programs every Linux desktop has. A host without a session
bus, `notify-send` or a player runs the notifier all the same (#62's headless acceptance):
that outlet is off and its log says why. A program that fails or hangs is logged and stopped
after ten seconds; nothing waits on it. macOS stays with #568.

## Status file

With `--status-file FILE` the notifier keeps `FILE` up to date for a bar (report 19): at once
when what it shows changes, at least every five seconds while it runs, whole (written beside
and renamed), and removed when it stops on an error. `torlek notify` puts it in the profile's
runtime directory, beside the daemon's socket: `$XDG_RUNTIME_DIR/torlek/status.json` for the
default profile. Schema 1:

```json
{
  "schema": 1,
  "written_at": 1762470000,
  "reachable": true,
  "needs_you": 1,
  "sessions": [
    {
      "id": "s00000000000a",
      "name": "api",
      "project": "agent-cockpit",
      "state": "needs-you",
      "needs": ["question"],
      "doing": "writing tests",
      "changed_at": 1762469990
    }
  ]
}
```

- `written_at`, `changed_at`: seconds since 1970. A file older than about thirty seconds was
  left by a notifier that is gone.
- `reachable`: the daemon answered the last request and the notifier has loaded its state;
  while false, `sessions` is what was last known, or empty.
- `sessions`: every session but those that ended and need nothing, by id. `name` is the
  label the agent reported, else the id; `project` is `null` until the feed names its
  checkout.
- `state`: `needs-you` when the session needs the owner for anything but having finished,
  else `launching`, `working`, `waiting`, `running` (no report yet), `stopping`, `asleep`,
  `ended` or `unknown`. `needs_you` counts the `needs-you` sessions.
- `needs`: the events above that stand for it now, most urgent first.

A new field keeps the schema number; a field that changes meaning or goes bumps it. Readers
ignore fields they do not know. Plan usage is not in schema 1: the protocol does not carry it
yet.

## How it runs

`torlek-notify --socket PATH --state-dir DIR [--desktop EVENTS] [--sound EVENTS]
[--status-file FILE]`, where
`EVENTS` is `none` or a comma-separated list of event names; left out, every event is on. It
follows the change feed for sessions and view stamps, asks the daemon for the agents' reports,
the approval queue and the network requests when their stamp moves and every five seconds,
and logs under `DIR/logs/notify` (report 12). A daemon out of reach is asked again after a
pause that doubles up to thirty seconds; nothing is news until it has answered all three.
`torlek notify` starts it with the daemon's socket, the profile's state directory, the
owner's `[notify.desktop]` and `[notify.sound]` settings and the status file.

## Dependency edges

- Depends on: `cockpit-client` and `cockpit-protocol` for the daemon, `cockpit-diagnostics`
  for its own log, `tokio` (its `process` feature runs the outlets), `tracing`, `serde`,
  `serde_json`.
- Used by: nothing; it is a client (`CLIENTS` in `cockpit-workspace-rules/tests/crate_graph.rs`:
  never persistence, scheduling or tmux).
