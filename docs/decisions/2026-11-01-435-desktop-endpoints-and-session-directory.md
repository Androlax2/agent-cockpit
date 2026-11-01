# The sandbox mounts exactly what a launch needs beyond the checkout: the session's directory and the desktop endpoints its variables name

Issues #437 and #435. Decided by the worker, implementing the issues as written; the choices
below are the ones the issues left open.

- **The session's directory is read-write in every profile.** A provider keeps what reaches
  the running agent there (Codex's app-server socket). It is refused if it falls inside a
  denied folder, like the checkout.
- **Codex's socket folder is the session's own, never the host's.** Decided in #446
  (docs/decisions/2026-11-01-446-codex-socket-folder-mounted-from-the-session.md). The
  owner's uid comes from the daemon (`HostPaths::owner_uid`): it names Codex's folder and
  the session bus.
- **A shared desktop reaches each socket and file its variables name, nothing else.**
  `DISPLAY` (`:N` → `/tmp/.X11-unix/XN`, or XQuartz's absolute socket) and `WAYLAND_DISPLAY`
  (relative to `XDG_RUNTIME_DIR`) are read-write mounts, since connecting writes to a socket;
  `XAUTHORITY` is read-only. A display on another host, a relative path, `..`, or a Wayland
  name without an absolute `XDG_RUNTIME_DIR` is refused naming the variable and its value.
- **The session bus only through greywall's filtered proxy.** The owner's own bus starts
  programs outside the sandbox (systemd's `StartTransientUnit`), so it is never mounted.
  Greywall's `xdg-dbus-proxy` (notifications only) takes the place of `/run/user/<uid>/bus`;
  a launch is refused when `greywall --version` reports no `D-Bus isolated`, on macOS, and
  when the owner's address is another socket (abstract, TCP, a `/tmp` path), which the agent
  would then not reach.
- **Rejected:** mounting all of `$XDG_RUNTIME_DIR` or `/tmp/.X11-unix` (exposes every other
  socket there); rewriting `DBUS_SESSION_BUS_ADDRESS` for the agent (greywall's proxy reads
  the same variable to find the owner's bus, so the value cannot differ between the two
  without wrapping the agent's command); dropping the bus variable silently.
- **Not measured here:** a real greywall launch reaching the display through these mounts;
  the live enforcement jobs are #433. The stand-in greywall in `cockpit-daemon`'s tests
  checks the arguments and the evidence only.
