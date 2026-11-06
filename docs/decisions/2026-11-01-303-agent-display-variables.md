# Launched agents inherit the daemon's display variables

Issue #303. Decided by the coordinator of the M1 acceptance fixes (2026-11-01): pass them
through, for every agent.

- **The daemon passes `DISPLAY`, `WAYLAND_DISPLAY` and `DBUS_SESSION_BUS_ADDRESS` to every
  agent it launches, when the daemon has them.** They join the base variables in
  `crates/cockpit-daemon/src/main.rs`, beside the XDG layout of the #268 section below. A session's tools then
  reach the owner's desktop session (Claude Code's notifications first) as they do when the
  agent runs outside Cockpit. Proof: `crates/cockpit-daemon/tests/agent_environment.rs`.
- **Never invented.** A variable the daemon lacks stays unset in the agent; the daemon does
  not guess `:0`, `wayland-0` or `$XDG_RUNTIME_DIR/bus`. A daemon started as a user unit
  without the graphical session's environment gives its agents no display, and the fix is
  the unit's environment (`systemctl --user import-environment`) and a daemon restart, not a
  default here. Values are read at daemon start until the owner imports fresher ones (the #307
  section below).
- **`XAUTHORITY` joins them (#306).** Under GDM or SDDM on X11 the cookie is not
  `~/.Xauthority`, so `DISPLAY` alone gives an agent's X11 client "Authorization required".
  Same rule: passed when the daemon has it, never invented.
- **Every agent, unless the owner withholds them.** The desktop session is the owner's own;
  `agents.desktop_session = "withheld"` keeps every agent off it, a sandbox profile later
  (the #305 section below).
- **Rejected: sending notifications through the daemon instead.** The daemon has no desktop
  dependencies (decision register 11), and the failing calls are the agent's own tools,
  which read these variables directly.

## Revisited 2026-10-31 (#268)

Decided by the coordinator of the M1 acceptance fixes (2026-10-31), choosing between the two
directions the issue left open. Launched agents inherit the daemon's XDG layout variables
too.

- **The daemon passes `XDG_RUNTIME_DIR`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME` and
  `XDG_CACHE_HOME` to every agent it launches, when they are set.** The daemon forwards the
  values it resolved its own layout from (`cockpit_config::Environment::layout_variables`),
  so the list has one owner. An agent's `cockpit` (its reports, `ask`, `todo`, the Claude hook
  bridge) then resolves the same directories as the daemon, so an agent of a daemon on a
  scratch layout (#135) or a second install (#196) reaches that daemon, not the default one.
  Proof: `crates/cockpit-daemon/tests/agent_environment.rs`.
- **Every tool in the agent sees the same layout, not only `cockpit`.** On a normal install
  that is the environment the agent would have had anyway: the daemon runs with the owner's
  variables. A daemon on scratch directories gives its agents' git and gh that scratch
  configuration too; that isolation is what the scratch run asks for, and #196's profiles
  replace scratch variables for side-by-side installs.
- **Rejected for now: the daemon sets `COCKPIT_AGENT_SOCKET` instead.** The CLI already
  prefers that variable, and a sandbox will need it to hand the agent exactly one socket
  path. It fixes only the socket, though: a `cockpit` that reads the layout for anything
  else would still read the default one. It belongs with the sandbox profiles (report 11).

## Revisited 2026-11-01 (#305)

Decided by Theo in #341 (option A), 2026-11-01. One owner setting keeps launched agents off
the owner's desktop session.

- **One named opt-out: `DesktopSession::Shared | Withheld`.** Withheld, a launched agent
  gets none of `DISPLAY`, `XAUTHORITY`, `WAYLAND_DISPLAY`, `DBUS_SESSION_BUS_ADDRESS`; the
  session bus reaches `org.freedesktop.systemd1` (`StartTransientUnit`) and the Secret
  Service, outside the agent's containment. The four names are one list,
  `cockpit_protocol::DESKTOP_SESSION_VARIABLES`. Proof:
  `crates/cockpit-daemon/tests/agent_environment.rs`
  `an_owner_who_withholds_the_desktop_session_launches_agents_without_any_of_it`.
- **Under a systemd scope too.** `systemd-run --user` reaches the manager through the
  daemon's `DBUS_SESSION_BUS_ADDRESS` and `XDG_RUNTIME_DIR`, and with `--scope` its program
  inherited them; `cockpit-processes` now drops, after `systemd-run`, those the spec does
  not set, so a program gets exactly its spec's environment. Proof:
  `crates/cockpit-processes/tests/scope_environment.rs`.
- **Selected today by the owner setting `agents.desktop_session = "shared" | "withheld"`,**
  default `shared` as decided above; later by a sandbox profile (#61) through the same value.
  Owner-only: a project file that sets `[agents]` fails, like `[sessions]`. Spelled
  `desktop_session`, not `desktop-session` as #341 wrote it: every other key uses underscores.
- **The value lives in `cockpit-config`, re-exported as `cockpit_sessions::DesktopSession`.**
  The configuration parses it and `cockpit-sessions` applies it, as with `StartFile`; the
  config crate cannot name a type of the crate that depends on it. It is read at daemon
  start, so a changed value counts from the next start.
- **Rejected:** waiting for #61 (no test until M5) and a per-project setting (one global
  value is what the owner asked for; a profile will give per-agent control).

## Revisited 2026-11-01 (#307)

Decided by Theo in #342 (option A), 2026-11-01. The owner's `cockpit desktop-session import`
refreshes the display variables of later launches.

- **`cockpit desktop-session import` sends the caller's desktop session variables** (the
  four of `cockpit_protocol::DESKTOP_SESSION_VARIABLES`), only the set ones; one the caller
  lacks is unset in later agents. The daemon keeps the latest set in memory and every later
  launch uses it; running agents keep theirs. Until the first import, the daemon start's
  values, as decided above. Proof: `crates/cockpit-daemon/tests/agent_environment.rs`
  `an_agent_launched_after_an_import_sees_the_imported_desktop_session`.
- **Owner only, four names only.** The agent socket refuses it (`not-authorized`), and a name
  outside the list is `invalid-argument`, so the import cannot add another variable to
  every agent (`an_agent_may_not_import_a_desktop_session`,
  `an_import_naming_another_variable_is_refused`).
- **In memory, not in the database:** a restarted daemon starts again from its own
  environment, and the next login's import refreshes it; a stored value would outlive the
  session it named.
- **An import with none of the four set warns** on stderr and is recorded: from a TTY or
  SSH login it is more likely a mistake, but an agent with no display is a legitimate state.
  A withholding owner still records it, and the CLI says on stderr that agents are kept off
  it, exiting 0, so a login script does not fail on the owner's own opt-out.
- **The dotfiles run it at each login and compositor start** (report 19's integration
  contract); the daemon never reads the user manager's environment or guesses a value.
- **Rejected:** the requesting client's environment per launch (daemon-made launches have no
  client) and `systemctl --user show-environment` at each launch (Linux-systemd only).
