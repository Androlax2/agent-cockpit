# Claude Code's `StopFailure` `rate_limit` is a failed turn, not a quota stop

Issue #212. Decided by the implementing session, applying the issue's own rule ("if it also
means a transient 429, the outcome is `failed` with reason `rate_limit`").

- **`rate_limit` is ambiguous in Claude Code 2.1.316.** Its API-error mapping sets
  `error: "rate_limit"` on every HTTP 429 and on the model "experiencing high load" errors: a
  spent plan window ("You've hit your 5-hour limit · resets …") and a server-side throttle
  ("Server is temporarily limiting requests (not your usage limit)") alike. The hook input
  carries only that `error`, an optional `error_details` and the `last_assistant_message`
  text; no structured field tells the two apart. Evidence: the 2.1.316 binary's error mapping
  and the `StopFailure` hook-input schema (`error` enum: `authentication_failed`,
  `oauth_org_not_allowed`, `account_on_hold`, `verification_required`, `billing_error`,
  `rate_limit`, `overloaded`, `invalid_request`, `model_not_found`, `server_error`, `unknown`,
  `max_output_tokens`, `cloud_credential_error`).
- **So the adapter reports `failed` with reason `rate_limit`.** `quota-stopped` makes report
  18 restart the session on another account; a transient 429 must not do that. Claude Code
  therefore never reports `quota-stopped` from its hooks. This supersedes the "completed,
  failed or quota-stopped" line of `2026-10-31-33-provider-contract.md` for Claude Code.
- **Rejected:** matching `last_assistant_message` against "You've hit your" because it is
  display text that changes between releases and with the account type. A quota stop is to be
  told apart from the plan windows `cockpit-usage` reads (#58), not from the hook.
- **Fixtures** `stop-failure-rate-limit-plan-window.json` and
  `stop-failure-rate-limit-transient.json` are written from 2.1.316's code, not captured: a
  rate limit cannot be provoked on demand (`crates/cockpit-claude/tests/rate_limit.rs`).
