# One owner setting keeps launched agents off the owner's desktop session

Issue #305. Decided by Theo in #341 (option A), 2026-11-01.

- **One named opt-out: `DesktopSession::Shared | Withheld`.** Withheld, a launched agent
  gets none of `DISPLAY`, `XAUTHORITY`, `WAYLAND_DISPLAY`, `DBUS_SESSION_BUS_ADDRESS`; the
  session bus reaches `org.freedesktop.systemd1` (`StartTransientUnit`) and the Secret
  Service, outside the agent's containment. The four names are one list,
  `cockpit_protocol::DESKTOP_SESSION_VARIABLES`. Proof:
  `crates/cockpit-daemon/tests/agent_environment.rs`
  `an_owner_who_withholds_the_desktop_session_launches_agents_without_any_of_it`.
- **Selected today by the owner setting `agents.desktop_session = "shared" | "withheld"`,**
  default `shared` as #303 decided; later by a sandbox profile (#61) through the same value.
  Owner-only: a project file that sets `[agents]` fails, like `[sessions]`.
- **Spelled `desktop_session`, not `desktop-session` as #341 wrote it:** every other key
  (`sessions.idle_sleep_after_minutes`, `tasks.blocker_project`) uses underscores.
- **The value lives in `cockpit-config`, re-exported as `cockpit_sessions::DesktopSession`.**
  The configuration parses it and `cockpit-sessions` applies it, as with `StartFile`; the
  config crate cannot name a type of the crate that depends on it.
- **Read at daemon start,** like the other owner settings; a changed value counts from the
  next start.
- **Rejected:** waiting for #61 (no test until M5) and a per-project setting (one global
  value is what the owner asked for; a profile will give per-agent control).
