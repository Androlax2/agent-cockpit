# The owner's `cockpit desktop-session import` refreshes the display variables of later launches

Issue #307. Decided by Theo in #342 (option A), 2026-11-01.

- **`cockpit desktop-session import` sends the caller's desktop session variables** (the
  four of `cockpit_protocol::DESKTOP_SESSION_VARIABLES`), only the set ones; one the caller
  lacks is unset in later agents. The daemon keeps the latest set in memory and every later
  launch uses it; running agents keep theirs. Until the first import, the daemon start's
  values, as #303 decided. Proof: `crates/cockpit-daemon/tests/agent_environment.rs`
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
- **A withholding owner still records it,** and the CLI says on stderr that agents are kept
  off it, exiting 0, so a login script does not fail on the owner's own opt-out (#305).
- **The dotfiles run it at each login and compositor start** (report 19's integration
  contract); the daemon never reads the user manager's environment or guesses a value.
- **Rejected:** the requesting client's environment per launch (daemon-made launches have no
  client) and `systemctl --user show-environment` at each launch (Linux-systemd only).
