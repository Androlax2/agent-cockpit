# A launched session gets `run-commands` over itself, beside `change-state`, and loses both when it ends or fails

Issue #270. Decided by Theo (option A).

- **Granted at launch, revoked at end, exactly like `change-state`.** Every launch grants
  the session `run-commands` scoped to `Scope::Session(itself)` in the transaction that
  mints its launch token, and every move to `ended` or `failed` revokes it
  (`crates/cockpit-sessions/src/lifecycle.rs`, `SELF_PERMISSIONS`). Everything
  `docs/decisions/2026-10-31-31-self-state-grant.md` says about that grant (one grant
  whoever gave it, audited, kept while asleep) holds for this one too.
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
