# cockpit-notify

Desktop notifications and sounds when a session needs the owner: a client of the daemon's
change feed, so the daemon stays headless.

Issues #62 and #682; report 19 and decision 11 of the greenfield design.

## What it tells

One event per reason a session, or the host, needs the owner, each on or off apart for the desktop and for
sound:

| Event | Raised when | Sound (bundled; theme name if no file) |
| --- | --- | --- |
| `failure` | the session failed, its agent exited by itself, or its turn failed | marimba, E5 then C5 |
| `hold` | a command waits for approval, a sandboxed host was refused, or a provider's permission dialog is open | glass, D6 twice |
| `question` | the agent asks a question, a guide waits on its step, or a command waits for a line of input | marimba, D5 then A5 |
| `quota` | the plan's usage limit stopped its turn | soft sine, G5 E5 C5 falling |
| `disk` | free disk space fell below `storage.low_disk_floor_mib` (#1143); the host's, not a session's | `dialog-warning` (theme) |
| `finished` | the task item is marked done (the task list says it finished) | kalimba, E5 then C6 |
| `turn` | its agent completed a turn, done or not; off by default for both outlets (#1127) | `message` (theme) |

An event is told when it is raised, never while it stays. `disk` follows the low-disk guard's
`disk` entity on the change feed: one notice per entry into low disk, none while it stays low,
another when it rises above the floor and falls below it again. It names the host (`session=host`
in the log) and is no session in the status file. A session that exits by itself
raises `failure`, not `finished`. Events that land together make one
notification, the most urgent first, and one sound, the most urgent event's. Nothing is told
for the state the notifier started on, for a session the owner ended, or for a question a
coordinator holds (#710).

## Outlets

- **Desktop:** `notify-send`, over the session bus (`DBUS_SESSION_BUS_ADDRESS` or
  `$XDG_RUNTIME_DIR/bus`). A failure is critical, everything else normal.
- **Sound:** each event but `disk` and `turn` has Torlek's own Ogg Vorbis sound (#1130), rendered by the
  owner from synthesized partials, no sample, and embedded in the binary; the notifier writes
  them under `DIR/sounds/` at start. `--sound-file EVENT=PATH` (the owner's
  `[notify.sound.files]`) replaces one. `canberra-gtk-play --file=PATH` plays the file when
  there is a display (`DISPLAY` or `WAYLAND_DISPLAY`); otherwise `pw-play` or `paplay` does.
  `disk` and `turn` have no file: they play the owner's sound theme (`canberra-gtk-play --id=message`, or
  `message.oga` from `XDG_DATA_DIRS`). `--test EVENT` plays one event's sound and exits.
- **Text:** the agent writes the names and details, so each shows on one line, cut short, its
  markup escaped (notification servers read the body as markup).

No new crate is needed: both are programs every Linux desktop has. A host without a session
bus, `notify-send` or a player runs the notifier all the same (#62's headless acceptance):
that outlet is off and its log says why. A program that fails or hangs is logged and stopped
after ten seconds; nothing waits on it. macOS stays with #568.

## Holds

While a hold stands the notifier keeps quiet: no notification, no sound (report 19, the
owner's choice on #62). What it would have told is kept, once per session and event; when the
last hold ends, one low-urgency notification without a sound lists what of it still stands.

- `away` stands while the `--away-file` exists, checked at every notice; `torlek notify away
  on|off` writes and removes it beside the status file, so it ends at logout when that is in
  `$XDG_RUNTIME_DIR`.
- `--hold NAME=PROGRAM` stands while `PROGRAM` exits 0 within three seconds. Every program
  runs at once, every five seconds, in a task of its own, so a slow one holds up nothing else.
  One that fails, cannot start or runs longer does not hold, and is logged once; only the
  program itself is stopped, so a script should end its children or `exec` its last command.
  `torlek notify` passes the owner's `[notify.hold]` programs.

A hold is not the `hold` event: that one tells a session is held on the owner's permission.

## Status file

With `--status-file FILE` the notifier keeps `FILE` up to date for a bar (report 19): at once
when what it shows changes and at least every five seconds while it runs, whatever the daemon
does; whole (written beside and renamed); its directory made with mode 700 when missing. It is
removed when the notifier stops on an error; a notifier stopped by a signal leaves it behind,
and its `written_at` ages. `torlek notify` puts it beside the daemon's socket, in the
profile's runtime directory (`paths.runtime_dir` in `torlek config show`): usually
`$XDG_RUNTIME_DIR/torlek/status.json`, or `<state dir>/run/status.json` where
`XDG_RUNTIME_DIR` is unset. Schema 1:

```json
{
  "schema": 1,
  "written_at": 1762470000,
  "reachable": true,
  "holds": [],
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

- `written_at`, `changed_at`: seconds since 1970. A file whose `written_at` is more than about
  fifteen seconds old was left by a notifier that is gone: show nothing from it.
- `reachable`: the daemon answered the last request, the change feed is current and the
  notifier has loaded its state; while false, `sessions` is what was last known, or empty.
- `holds`: the holds that stand, by name (`away`, or a `[notify.hold]` name); while any
  does, the notifier keeps quiet.
- `sessions`: every session that has not ended, by id. `name` is the label the agent
  reported, else the id; `project` is `null` until the feed names its checkout.
- `state`: `needs-you` when the session needs the owner for anything but having finished or completed a turn (a
  failed session among them), else `launching`, `working`, `waiting`, `running` (no report
  yet), `stopping`, `asleep` or `unknown`. `needs_you` counts the `needs-you` sessions.
- `needs`: the events above that stand for it now, most urgent first.

A new field keeps the schema number; a field that changes meaning or goes bumps it. Readers
ignore fields they do not know. Plan usage is not in schema 1: the protocol does not carry it
yet.

## How it runs

`torlek-notify --socket PATH --state-dir DIR [--desktop EVENTS] [--sound EVENTS] [--sound-file EVENT=PATH]... [--test EVENT]
[--status-file FILE] [--away-file FILE] [--hold NAME=PROGRAM]...`, where
`EVENTS` is `none` or a comma-separated list of event names; left out, every event is on. It
follows the change feed for sessions and view stamps, asks the daemon for the agents' reports,
the approval queue and the network requests when their stamp moves and every five seconds,
and logs under `DIR/logs/notify` (report 12). A daemon out of reach is asked again after a
pause that doubles up to thirty seconds; nothing is news until it has answered all three.
`torlek notify` starts it with the daemon's socket, the profile's state directory, the
owner's `[notify.desktop]`, `[notify.sound]` and `[notify.hold]` settings, the status file
and the away file.

## Dependency edges

- Depends on: `cockpit-client` and `cockpit-protocol` for the daemon, `cockpit-diagnostics`
  for its own log, `tokio` (its `process` feature runs the outlets), `tracing`, `serde`,
  `serde_json`.
- Used by: nothing; it is a client (`CLIENTS` in `cockpit-workspace-rules/tests/crate_graph.rs`:
  never persistence, scheduling or tmux).

## The log

Besides the log files, every notice the notifier handles is one line on standard error, which
journald keeps under the unit (`journalctl --user -u torlek-notify`). A line starts with
`torlek-notify: told` or `skipped`, then the event, `session=ID`, `name="..."` and one status per
outlet: `desktop=` and `sound=` are `told`, `off(setting)`, `off(no program)` or, for a sound, `off(covered by a more urgent event)`. A notice
kept back by a hold reads `skipped EVENT session=ID name="..." reason=hold(away)`. When the last
hold ends, the quiet summary is `told summary of N held notices, sessions=...`.
