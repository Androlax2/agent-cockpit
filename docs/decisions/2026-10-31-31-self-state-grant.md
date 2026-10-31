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
  `authorization_grants`. Nothing else is granted on the owner's behalf; every other
  permission stays the owner's to give.
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
