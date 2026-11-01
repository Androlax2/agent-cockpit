# A restore that keeps a still-running agent grants its session the self permissions it lacks

Issue #284. Decided by the worker: the issue's first option, the smaller one that covers
every later addition to `SELF_PERMISSIONS` without a new migration each time.

- **Granted on the `StillRunning` restore path.** `restore_one`
  (`crates/cockpit-sessions/src/lifecycle.rs`) grants `SELF_PERMISSIONS` through the same
  `grant_self_permissions` a launch uses. `cockpit_authorization::grant` keeps a grant the
  session already holds unchanged, so a session launched by the current daemon sees no new
  grant and no revision advance; one launched before #270 gains `run-commands` without a
  relaunch.
- **In the transaction that records the move to running**, so a restore never commits one
  without the other.
- **A revocation lasts until the next launch or restore, as before.** Every relaunch
  (restart, wake, resume) already grants `SELF_PERMISSIONS` again, so a restore granting
  them treats a kept agent like a relaunched one; no owner verb revokes them today.
- **Only sessions whose agent still runs.** Every other restore path launches, and the launch
  grants; a session asleep gains them when it wakes; one `ended` or `failed` holds none.
- **Proof:** `crates/cockpit-daemon/tests/sessions.rs`
  `on_daemon_start_a_session_still_running_without_run_commands_is_granted_it`; a grant
  already held kept without a new revision by `crates/cockpit-authorization/tests/evaluation.rs`
  `granting_an_active_grant_again_returns_it_without_a_new_revision`.
- **Rejected:** a one-time migration granting them to every running or asleep session: it
  covers only the permissions known when it is written, and an asleep session needs none.
