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
