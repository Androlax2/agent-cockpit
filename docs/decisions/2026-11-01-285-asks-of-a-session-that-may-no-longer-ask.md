# The owner's queue re-evaluates the asking session; requests of a session that may no longer ask are hidden and cannot be approved

Issue #285. Decided by the worker: the issue offered two options; the crate graph and report 17's
"`cockpit-authorization` decides" settle it without a product choice.

- **Re-evaluate `run-commands` when the owner reads or decides, not when the session ends.**
  `open_runs` and `open_guides` leave out asking requests whose session `cockpit-authorization`
  now refuses; `approve_run`, `edit_run` and `answer_step` refuse them with
  `Refusal::AskerMayNotAsk` (wire code `conflict`). Ending, failing or an owner revoking the
  grant all reach the same rule, through the one owner of the grant.
- **Declining a run and closing or reporting stuck on a guide stay allowed:** they run
  nothing, and an owner who already holds the request's id can still clear it. Approved and
  running runs stay in the queue: the decision was made while the session could ask.
- **A dedicated refusal, not `NotAuthorized`:** that one tells the owner to grant
  `run-commands` to the session, which is wrong for a session that ended.
- **Rejected:** declining or expiring the requests in the transaction that ends the session.
  `cockpit-sessions` would have to call `cockpit-approvals` (a new crate edge), and every
  path that removes the grant (end, fail, owner revoke) would need its own call.

Evidence: `crates/cockpit-approvals/tests/runs.rs`, `tests/guides.rs`
(`*_whose_session_may_no_longer_ask_*`) and `crates/cockpit-daemon/tests/agent_asks.rs`
(`*_of_a_session_that_ended_*`).
