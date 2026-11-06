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
- **The asking session reads such a request as `expired`.** `run_status` / `guide_status`
  (and their read-only `_if_fresh` forms) report it expired and no longer move its expiry, so
  an agent still polling after an owner revoke hears an answer instead of waiting forever.
- **An ask belongs to its own grant.** Revoking the grant it was asked under ends it even if
  another grant still covers the session; the agent can ask again.
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

## Revisited 2026-11-01 (#347)

Decided by the worker: the rule above already gives the asking agent this state, and the
wire's `expired` means "nobody waits for the answer; it can no longer be decided", which is
exactly the case. The owner's show verbs read an ask its session may no longer make as
expired.

- **`ApprovalShow`, `GuideShow` and `ApprovalInput` read through `shown_run` / `shown_guide`**
  (`cockpit-approvals`), which apply the rule above: an asking request whose asking grant no
  longer covers its session reads `expired`. The agent's status polls read the same function,
  so owner and agent see one state, and the queue, the show verbs and approve/edit/answer agree.
- **No changed-build-files hint for it:** the hint is read only for asking requests, so the
  show verb no longer runs Git for a request nobody can approve.
- **Decline and close still work on it,** as decided above; they run nothing. A view showing
  `expired` simply offers no approval.
- **Rejected:** a new wire state such as "its session no longer waits". It would add a
  variant every client must handle for a case `expired` already describes.
- **Rejected:** refusing the show verbs with `AskerMayNotAsk`. The owner could no longer read
  a request they may still decline.

Evidence: `crates/cockpit-daemon/tests/agent_asks.rs`
(`an_ask_run_of_a_session_that_ended_shows_as_expired`, `an_ask_run_of_a_session_that_ended_shows_no_changed_build_files`,
`an_ask_guide_of_a_session_that_ended_shows_as_expired`), `crates/cockpit-approvals/tests/runs.rs`
and `tests/guides.rs` (`*_may_no_longer_ask_shows_as_expired`, `*_may_still_ask_shows_as_asking`).
