# 19. Desktop integration and remote clients

Status: proposed

## Decision

The daemon has no desktop dependencies. Everything that touches a window manager, a bar,
a notification system, a lock screen or a calendar is an **optional client** of the
change feed and the protocol. Today's OSD daemon (GTK4 layer-shell hosting the registry,
sockets and queues) is split: registry and queues go to `cockpit-daemon`; the visuals go here.

## `cockpit-notify` (shipped)

- Subscribes to the change feed; emits desktop notifications for needs-you, approval requests,
  turn end on watched sessions, quota alerts, incidents.
- Backends: Linux D-Bus notifications (`notify-send` compatible), macOS `osascript` /
  `terminal-notifier`, WSL none by default (documented).
- Writes a **status file** (sessions, needs-you count, plan usage) that any bar can read;
  the waybar module becomes a reader of that file.
- Hold rules (calendar meeting, game running, away mode) are notifier settings with pluggable
  detectors; the daemon does not know about `khal` or Hyprland.

## Desktop-specific extras (Theo's dotfiles or plugins)

Workspace toggling, kitty floating approval/guide windows, Hyprland `eval` hooks, fade of the
unfocused pane, idle lock prompt, volume/brightness island. The TUI renders approvals and
guides in tmux panes so none of these are required for a working Cockpit.

## Integration contract (decided 2026-10-31, from the dotfiles inventory)

Cockpit is standalone; Theo's dotfiles add Hyprland extras on top through this public
surface only. Every row names what the product ships and what stays in the dotfiles.

| Dotfiles today | Product surface | Dotfiles keep |
| --- | --- | --- |
| `SUPER+C` → `cockpit show` (workspace toggle) | `cockpit tui` attaches in the current terminal | the bind, the workspace, the kitty window |
| `SUPER+SHIFT+C` → `cockpit handoff` | `cockpit session handoff --all` (Remote Control capability, CLI only) | the bind |
| `cockpit prs` scratchpad, web scratchpads (`P`/`W`/`G`) | none before M6 (Pull requests tab dropped; web windows are an M6 issue) | nothing until then |
| `lock.sh` → `cockpit away on\|off` | `cockpit notify away on\|off` (a hold rule of `cockpit-notify`) | the hyprlock hook |
| `claude-idle.sh` → `cockpit setting idle-lock`, `cockpit-idle-prompt` | none: the idle lock is a desktop behaviour; its setting lives in the dotfiles | the script, its setting, its prompt |
| `cockpit log lock\|idle …` | none: Cockpit logs cover its own operation only (report 12); the dotfiles log to the journal | `logger -t` |
| waybar `custom/cockpit` reads `$XDG_RUNTIME_DIR/cockpit.json` | the status file above, with a documented, versioned schema | the module and its styling |
| waybar `osd-show focus-next` | `cockpit session next` prints the session needing you | focusing its window |
| `osd-show volume\|brightness\|mute` | none: not a Cockpit feature; the GTK OSD leaves with `apps/` | a desktop OSD of their choice |
| `autostart.lua` → `systemctl --user start cockpit.service` | `cockpit daemon install` writes the user unit (launchd agent on macOS) | enabling it at login |
| `tmux.conf`: `cockpit-left`, `Alt+Q` queue-prompt popup, focus hooks | `cockpit-tmux` configures its own server: queue-prompt key, Left-key and focus fade as M6 polish rows | nothing: the user's `tmux.conf` is never read |
| `CLAUDE.md` rules for `ask-to-run`, `cockpit-todo`, session label/doing | the agent instruction bundle (report 08) installed per provider | personal rules on top of the bundle |
| `config/hyprland`, `config/kitty`, `config/waybar`, `config/fish` in this repository | removed at the M5 cutover (#71) | moved into the dotfiles |

Nothing in the dotfiles may depend on a path under `apps/` or on `cockpit-osd`; a new Cockpit
install works with the table's left column absent.

## `cockpit-web` (phone/remote client)

Kept as an optional client, same protocol as the TUI:

- Pairing (QR link + short code), per-phone secrets, unpair; serves on loopback for a tailnet
  or SSH forward; never exposed to the internet by default.
- Live session screen from `cockpit-tmux` capture at a bounded rate; interrupt; send message;
  wake; Remote Control toggle; answer permission dialogs and approvals; `[y/N]` follow-ups.
- Tabs: sessions (+ start in clone), todo, pull requests, usage; reply images bounded in size.
- Web Push (allowed vendor endpoints only) and lock-time notification policy via
  `cockpit-notify` settings.
- Chief escalations and answers, if Chief is enabled.

## Rules

- A remote client holds the owner's authority only after pairing; every action is logged in
  the audit log with the device identity.
- The screen stream is display-only; state comes from the daemon.
- Clients may lag or disconnect; the daemon never waits on them (report 03).

## Validate

- Cockpit runs with no desktop session at all (SSH to a headless Linux box) and all features
  except notifications and the phone page work.

## Sources

- Current: `apps/osd/src/bin/osd.rs`, `apps/cockpit/src/bin/cockpit-remote.rs`,
  `config/waybar/**`, `config/hyprland/**`
