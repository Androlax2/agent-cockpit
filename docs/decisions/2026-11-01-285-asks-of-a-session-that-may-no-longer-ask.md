# An ask is decidable only while the grant it was asked under still covers its session

Issue #285. Decided by the worker: the issue offered two options; the crate graph and report 17's
"`cockpit-authorization` decides" settle it without a product choice.

- **Bind each request to its grant and re-evaluate when the owner reads or decides.** A run
  request or guide records the `run-commands` grant its session asked under
  (`asking_grant`, migration 4). `open_runs` and `open_guides` leave out asking requests
  whose session that grant no longer covers; `approve_run`, `edit_run` and `answer_step`
  refuse them with `Refusal::AskerMayNotAsk` (wire code `conflict`), before any stale
  revision or step. Ending, failing or an owner revoking the grant all reach this rule
  through the grant's one owner.
- **A new grant does not revive old asks.** A restart grants the session `run-commands`
  again; the agent that asked is gone, so its requests stay out. Rows from before migration
  4 hold no grant and read as dead; they would expire within 15 minutes anyway.
- **Declining a run and closing or reporting stuck on a guide stay allowed:** they run
  nothing. Approved and running runs stay in the queue: the decision was made while the
  session could ask.
- **A dedicated refusal, not `NotAuthorized`:** that one tells the owner to grant
  `run-commands` to the session, which is wrong for a session that ended.
- **Rejected:** declining or expiring the requests in the transaction that ends the session.
  `cockpit-sessions` would have to call `cockpit-approvals` (a new crate edge), and every
  path that removes the grant (end, fail, owner revoke) would need its own call. The cost:
  rows stay `asking` until they expire, so readers of asking rows go through
  `open_runs`/`open_guides`.

Evidence: `crates/cockpit-approvals/tests/runs.rs`, `tests/guides.rs`
(`*_may_no_longer_ask_*`, `*_granted_again`) and `crates/cockpit-daemon/tests/agent_asks.rs`
(`*_of_a_session_that_ended_*`).
