# Launched agents inherit the daemon's display variables

Issue #303. Decided by the coordinator of the M1 acceptance fixes (2026-11-01): pass them
through, for every agent.

- **The daemon passes `DISPLAY`, `WAYLAND_DISPLAY` and `DBUS_SESSION_BUS_ADDRESS` to every
  agent it launches, when the daemon has them.** They join the base variables in
  `crates/cockpit-daemon/src/main.rs`, beside the XDG layout of #268. A session's tools then
  reach the owner's desktop session (Claude Code's notifications first) as they do when the
  agent runs outside Cockpit. Proof: `crates/cockpit-daemon/tests/agent_environment.rs`.
- **Never invented.** A variable the daemon lacks stays unset in the agent; the daemon does
  not guess `:0`, `wayland-0` or `$XDG_RUNTIME_DIR/bus`. A daemon started as a user unit
  without the graphical session's environment gives its agents no display, and the fix is
  the unit's environment (`systemctl --user import-environment`) and a daemon restart, not a
  default here. Values are read once at daemon start; a fresher source is #307.
- **`XAUTHORITY` joins them (#306).** Under GDM or SDDM on X11 the cookie is not
  `~/.Xauthority`, so `DISPLAY` alone gives an agent's X11 client "Authorization required".
  Same rule: passed when the daemon has it, never invented.
- **Every agent, no opt-out yet.** The desktop session is the owner's own; a sandbox profile
  that must keep an agent off it (the session bus can start units outside containment) needs
  a named opt-out first: #305 (report 11).
- **Rejected: sending notifications through the daemon instead.** The daemon has no desktop
  dependencies (decision register 11), and the failing calls are the agent's own tools,
  which read these variables directly.
