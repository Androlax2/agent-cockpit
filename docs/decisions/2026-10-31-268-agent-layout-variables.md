# Launched agents inherit the daemon's XDG layout variables

Issue #268. Decided by the coordinator of the M1 acceptance fixes (2026-10-31), choosing
between the two directions the issue left open.

- **The daemon passes `XDG_RUNTIME_DIR`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME` and
  `XDG_CACHE_HOME` to every agent it launches, when they are set.** The daemon forwards the values it resolved its own layout
  from (`cockpit_config::Environment::layout_variables`), so the list has one owner. An agent's `cockpit` (its reports, `ask`, `todo`, the
  Claude hook bridge) then resolves the same directories as the daemon, so an agent of a
  daemon on a scratch layout (#135) or a second install (#196) reaches that daemon, not the
  default one. Proof: `crates/cockpit-daemon/tests/agent_environment.rs`.
- **Every tool in the agent sees the same layout, not only `cockpit`.** On a normal install
  that is the environment the agent would have had anyway: the daemon runs with the owner's
  variables. A daemon on scratch directories gives its agents' git and gh that scratch
  configuration too; that isolation is what the scratch run asks for, and #196's profiles
  replace scratch variables for side-by-side installs.
- **Rejected for now: the daemon sets `COCKPIT_AGENT_SOCKET` instead.** The CLI already
  prefers that variable, and a sandbox will need it to hand the agent exactly one socket
  path. It fixes only the socket, though: a `cockpit` that reads the layout for anything
  else would still read the default one. It belongs with the sandbox profiles (report 11).
