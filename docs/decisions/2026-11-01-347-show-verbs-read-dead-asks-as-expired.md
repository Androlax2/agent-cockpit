# The owner's show verbs read an ask its session may no longer make as expired

Issue #347. Decided by the worker: #285's decision record already gives the asking agent this
state, and the wire's `expired` means "nobody waits for the answer; it can no longer be
decided", which is exactly the case.

- **`ApprovalShow`, `GuideShow` and `ApprovalInput` read through `shown_run` / `shown_guide`**
  (`cockpit-approvals`), which apply #285's rule: an asking request whose asking grant no
  longer covers its session reads `expired`. The agent's status polls read the same function,
  so owner and agent see one state, and the queue, the show verbs and approve/edit/answer agree.
- **No changed-build-files hint for it:** the hint is read only for asking requests, so the
  show verb no longer runs Git for a request nobody can approve.
- **Decline and close still work on it,** as #285 decided; they run nothing. A view showing
  `expired` simply offers no approval.
- **Rejected:** a new wire state such as "its session no longer waits". It would add a
  variant every client must handle for a case `expired` already describes.
- **Rejected:** refusing the show verbs with `AskerMayNotAsk`. The owner could no longer read
  a request they may still decline.

Evidence: `crates/cockpit-daemon/tests/agent_asks.rs`
(`an_ask_run_of_a_session_that_ended_shows_as_expired_without_build_files`,
`an_ask_guide_of_a_session_that_ended_shows_as_expired`), `crates/cockpit-approvals/tests/runs.rs`
and `tests/guides.rs` (`*_may_no_longer_ask_shows_as_expired`, `*_may_still_ask_shows_as_asking`).
