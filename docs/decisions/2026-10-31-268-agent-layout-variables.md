# Launched agents inherit the daemon's XDG layout variables

Issue #268. Decided by the coordinator of the M1 acceptance fixes (2026-10-31), choosing
between the two directions the issue left open.

- **The daemon passes `XDG_RUNTIME_DIR`, `XDG_STATE_HOME` and `XDG_CONFIG_HOME` to every
  agent it launches, when they are set.** They join the base variables in
  `crates/cockpit-daemon/src/main.rs`. An agent's `cockpit` (its reports, `ask`, `todo`, the
  Claude hook bridge) then resolves the same directories as the daemon, so an agent of a
  daemon on a scratch layout (#135) or a second install (#196) reaches that daemon, not the
  default one. Proof: `crates/cockpit-daemon/tests/agent_environment.rs`.
- **Rejected for now: the daemon sets `COCKPIT_AGENT_SOCKET` instead.** The CLI already
  prefers that variable, and a sandbox will need it to hand the agent exactly one socket
  path. It fixes only the socket, though: a `cockpit` that reads the layout for anything
  else would still read the default one. It belongs with the sandbox profiles (report 11).
