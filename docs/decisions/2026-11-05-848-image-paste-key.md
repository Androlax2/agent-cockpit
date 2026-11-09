# CMD+V in the viewer has the daemon paste the clipboard's PNG into the session on screen

Issue #848 (paste an image in). Decided by Theo, option A of the issue's pull request #916: a
viewer key makes the daemon read one PNG from the desktop clipboard and type its path into the
agent; agents never read the clipboard themselves.

- **The key.** `C-v`, what kitty sends for CMD+V and what Claude Code reads as its image
  paste, is bound in the root table of Cockpit's tmux server. Pressed while the viewer pane has
  the keys (`@torlek-viewer` is set on that pane only), it runs `torlek session paste-image`
  with `run-shell -b`; anywhere else it reaches the pane's program as before. The viewer's
  nested client is a client of the same server, so the key it forwards to the agent's pane
  meets the binding again, finds no `@torlek-viewer` there and is sent on: a `C-v` handed back
  to the viewer reaches the agent. Proven by `crates/cockpit-tmux/tests/clipboard.rs`.
- **The command.** `torlek tui` sends it with `manager-open` (`image_paste`): this `torlek`,
  its profile, `session paste-image`. The daemon writes it as a script beside the tmux config,
  mode 0700, each word single-quoted, so no word reaches tmux's or a shell's parser. Its output
  is dropped; its errors show in the viewer. Each `manager-open` writes it again.
- **`session-paste-image`, owner only.** The session is the one the viewer shows, and it must
  be running; in the phone layout, where the viewer is read-only (#855), it is refused. The
  daemon runs `wl-paste --list-types`, then `wl-paste --no-newline --type image/png`, each for
  at most 5 s, with the desktop variables
  the owner last imported (`torlek desktop-session import`) in place of its own. Only
  `image/png` is read: clipboard text never leaves the desktop. The bytes must start with the
  PNG signature and stay under 32 MiB. The image is saved as a new `clipboard-<ms>.png`, mode
  0600, in the session's temporary folder (its `TMPDIR`, removed when it ends), and the path is
  pasted into the agent's pane as one bracketed paste, so Claude Code takes it as an
  attachment. The paste buffer it goes through is deleted, so prefix `]` never types it again.
  Proven by `crates/cockpit-daemon/tests/manager.rs`.
- **No PNG read** (an empty clipboard, which `wl-paste` reports as `Nothing is copied`, text
  only, no Wayland display imported or no `wl-paste`): the daemon sends `C-v` to the viewer, so
  the session gets the key as before #848, which an agent given the display
  (`agents.desktop_session`) reads itself; `torlek doctor` says what is missing. A reader that
  fails or hangs, or bytes that are not a PNG: refused, naming why, and nothing is pasted.
- **Optional:** a script path tmux cannot take as it is (outside letters, digits and
  `/ . _ - +`) leaves the key unbound, logged; the manager opens all the same.
- **`torlek doctor`** lists `clipboard out` (whether tmux takes OSC 52 copies, decision
  `2026-11-05-848-tmux-clipboard-copy-out`) and `clipboard in` (`wl-paste` on the daemon's
  PATH and a Wayland display imported). Both are optional: neither blocks.
- **Accepted:** the daemon, already the owner's process, reads the owner's display; nothing
  of it is given to a session. The image goes where the session can read it, and so can every
  other process of the owner's user. Since #828 agents run unsandboxed by default, so an agent
  could reach the owner's Wayland socket by its well-known path itself: this key adds no
  access, it gives the owner a way that needs none. Keeping agents away from the display is the
  sandbox's job (#829). X11 is not read.
- **Rejected:** an image-only clipboard bridge any session can query (agents read the
  clipboard without the owner's action), and passing the display into sessions
  (`agents.desktop_session`, which gives the agent all clipboard text at any time).

## Revisited 2026-11-09 (#1236)

Decided by Theo, option B of the issue: on a default install the daemon starts at login,
before the compositor, so no display was ever imported and CMD+V silently passed `C-v` on to
a session that has no display either.

- **No imported Wayland display fails the key,** in every case: the viewer shows `no Wayland
  display imported: run torlek desktop-session import ...`, nothing is sent to the session.
  This replaces "No PNG read ... no Wayland display imported: the daemon sends `C-v`" above;
  an empty clipboard, text only and a missing `wl-paste` still pass the key on. Proof:
  `crates/cockpit-daemon/tests/manager.rs`
  `without_the_owners_wayland_display_the_paste_fails_naming_the_import`.
- **`torlek daemon install` also writes `torlek[-NAME]-desktop-import.service`** (systemd
  only), bound to `graphical-session.target` and part of the daemon's unit, running `torlek
  desktop-session import`. A daemon restart restarts it, so the import the daemon forgets
  (#307) is handed over again. The daemon still never reads the user manager's environment or
  invents a value (decision 2026-11-01-303). A profile named `desktop-import` or ending in
  `-desktop-import` is refused, as for `-notify`. Proof: `crates/cockpit-cli/tests/profiles.rs`.
- **Rejected:** the daemon reading `systemctl --user show-environment` (option A) and keeping
  the import manual (option C).
