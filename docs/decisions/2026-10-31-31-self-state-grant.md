# A launched session gets `change-state` over itself, and loses it when it ends or fails

Issue #31, question #143. Decided by Theo (option A).

- **Granted at launch.** Every launch (start, restart, wake, resume, restore) grants the
  session `change-state` scoped to `Scope::Session(itself)`, in the same transaction that
  mints its launch token (`crates/cockpit-sessions/src/lifecycle.rs`, `try_launch`). A grant
  still active from an earlier launch is returned unchanged. The agent channel then accepts
  the agent's own reports, so idle sleep works without an owner step.
- **Revoked when the session ends or fails.** Every move to `ended` (the owner's `end`, an
  agent observed gone, an asleep session past its expiry) revokes that grant in the
  transaction that records the move and releases the clone. A move to `failed` revokes it
  too: a failed launch may have left an agent running, and it must not report for a session
  shown as failed. `resume` grants it again. An asleep session keeps it; its agent is
  stopped and its next launch is the same session.
- **One grant, whoever gave it.** An owner grant of the same permission and scope is the
  same grant (`grant` returns an active one unchanged), so ending the session revokes it
  too.
- **Audited like any grant.** It is an ordinary `cockpit-authorization` grant: the grant
  and its revocation each advance the authorization revision and stay in
  `authorization_grants`. Nothing else is granted on the owner's behalf except
  `run-commands` over itself (the #270 section below); every other permission stays the
  owner's to give.
- **Proof:** `crates/cockpit-daemon/tests/sessions.rs`
  `a_launched_session_holds_change_state_over_itself`,
  `ending_a_session_revokes_its_change_state_over_itself`,
  `an_agent_that_exits_loses_change_state_over_its_session`,
  `a_session_that_fails_to_restore_loses_change_state_over_itself`; the idle-sleep tests report
  with no manual grant.
- **Rejected:**
  - B, owner-only grants (a verb per session, or a standing owner setting): every session
    would stay unable to report, and never sleep, until the owner acts.
  - C, reporting own state without an authorization check: it would put an exception in the
    agent channel's rule instead of an auditable grant.

## Revisited 2026-10-31 (#270)

Decided by Theo (option A). A launched session gets `run-commands` over itself, beside
`change-state`, and loses both when it ends or fails.

- **Granted at launch, revoked at end, exactly like `change-state`.** Every launch grants
  the session `run-commands` scoped to `Scope::Session(itself)` in the transaction that
  mints its launch token, and every move to `ended` or `failed` revokes it
  (`crates/cockpit-sessions/src/lifecycle.rs`, `SELF_PERMISSIONS`). Everything said above
  about the `change-state` grant (one grant whoever gave it, audited, kept while asleep)
  holds for this one too.
- **Asking is not running.** The grant only lets the agent file an `ask run` or `ask guide`;
  every command still needs the owner's `y` on the bound revision and digest, and a guide's
  `run:` steps go through the same approval. Without the grant, `cockpit ask run` from any
  real session was refused with an unblock action the owner had no verb for.
- **Proof:** `crates/cockpit-daemon/tests/sessions.rs`
  `a_launched_sessions_ask_run_reaches_the_owners_queue_without_a_manual_grant`,
  `ending_a_session_revokes_its_run_commands_over_itself`,
  `a_session_that_fails_to_restore_loses_run_commands_over_itself`.
- **Rejected:**
  - B, an owner verb (`cockpit session grant SESSION run-commands`): one more owner step
    before every new session can ask, for no safety the per-command approval lacks.
  - C, per-project configuration, default off: nothing works out of the box.

## Revisited 2026-11-01 (#284)

Decided by the worker: the issue's first option, the smaller one that covers every later
addition to `SELF_PERMISSIONS` without a new migration each time. A restore that keeps a
still-running agent grants its session the self permissions it lacks.

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
