# The hook bridge tells a Claude Code quota stop from a transient rate limit by the session's own plan windows

Status: accepted

Issue #249. Decided by the worker, applying the issue's rule (a `rate_limit` failure while the
account's window is at its limit is a quota stop) and Theo's #59 decision that the session's
report alone moves it.

- **The bridge judges, in the sandbox, before reporting.** At a `StopFailure` of `rate_limit`
  (#212), `torlek-claude-hook` asks the usage endpoint for the plan windows of the sign-in the
  session runs on, the same request `Provider::quota` makes (#58): with the
  `CLAUDE_CODE_OAUTH_TOKEN` it holds, a placeholder the session proxy swaps for the account's
  credential (#968), else the credentials file a login account binds in its home (#59). That
  sign-in is the one the limit hit; the daemon's `cockpit-usage` readings are the owner's
  `default` account's, read at an interval, and may name another account or be minutes old.
- **Spent means used at or past the whole limit, or blocked.** Any window, the scoped ones
  included: a spent Opus window stops a turn on Opus as the five-hour one does.
- **Headroom, or windows that cannot be read, stay `failed` with reason `rate_limit`.** A
  quota stop moves the session to its next account (#59); a transient 429 must not. The
  reason is written on the hook's standard error, which Claude Code keeps in its debug log;
  the report still goes out and the hook exits 0.
- **Rejected:** deciding in the daemon from its recorded readings (the wrong account under
  #59, and stale by up to a refresh), or from a fresh reading at the report (the daemon
  would read each account's credential to call the endpoint, and #59 declined waiting on a
  reading); matching `last_assistant_message` (#212's reason stands).

Evidence: `crates/cockpit-claude/tests/hooks.rs` (`a_rate_limit_failure_with_a_spent_window_is_a_quota_stop`,
`a_rate_limit_failure_with_headroom_stays_failed_with_its_reason`), `crates/cockpit-claude/tests/bridge.rs`
(`a_rate_limit_stop_with_a_spent_window_is_reported_as_a_quota_stop`,
`a_rate_limit_stop_with_headroom_is_reported_as_a_failure`,
`a_bound_credential_asks_for_the_windows_with_the_token_the_session_holds`), fixture
`tests/fixtures/oauth-usage/limits-spent.json`.
