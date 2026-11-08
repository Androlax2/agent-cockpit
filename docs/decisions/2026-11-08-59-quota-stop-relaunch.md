# A quota stop relaunches the session on the next account with capacity, on the session's own report

Status: accepted

Issue #59, part 2. That a report from inside the session alone moves it was decided by Theo
(2026-11-08, option A); routing only at launch and at a quota-stop relaunch is report 18's
rule. The rest are engineering defaults listed in the pull request.

- **The session's report is the trigger.** A recorded `quota-stopped` outcome relaunches the
  session; the daemon's own quota readings are not waited for. A lying agent can only spend
  accounts the owner listed, and it holds a placeholder on each, never the credential.
- **At once, and again on the periodic pass.** The daemon starts the relaunch once the report
  is committed, in the background since it stops the agent that sent the report; the
  sessions' pass (30 s) retries a session still quota-stopped on its current launch.
- **The stop is the daemon's record.** `account_quota_stops` keeps when each provider account
  last quota-stopped, written in the report's transaction. For five hours, the shortest plan
  window, a relaunch skips it and a first launch takes it only when no other account holds a
  credential. The stop names no reset time, so the hold is an estimate.
- **Pending is derived, not stored.** A session is due a relaunch while its agent's latest
  report is a quota stop on its current launch and it waits at its prompt; the relaunch's new
  launch ends that, so no request row can go stale.
- **The switch is recorded on the launch.** `session_launches.switched_from` names the account
  the launch moved off; `torlek session show` prints it.
- **No account to move to changes nothing.** The relaunch is checked before the stop: with no
  other account holding a credential, or an unusable credential first in line, the session
  keeps running on its account.
- **No message is sent for the agent.** The conversation is resumed and waits at its prompt;
  sending "continue" would act for the owner.
- **Rejected:** waiting for a quota reading to confirm (per-account polling, about a day more,
  declined by Theo); a request table (it could outlive the launch it was for).

Evidence: `crates/cockpit-daemon/tests/sandbox.rs`
(`a_quota_stop_resumes_the_conversation_on_the_next_account_within_a_minute`,
`a_new_session_passes_over_an_account_that_quota_stopped`,
`a_quota_stop_with_no_other_account_keeps_the_session_where_it_is`),
`crates/cockpit-cli/tests/sessions.rs`
(`session_show_prints_the_account_a_quota_stop_moved_its_launch_off`).
