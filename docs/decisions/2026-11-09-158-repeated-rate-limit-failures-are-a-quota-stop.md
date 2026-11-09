# Three `rate_limit` failed turns within 30 minutes are recorded as a quota stop

Status: accepted

Issue #158, part 2. Decided by Theo (answer A, relayed by the coordinator on 2026-11-09).

- **Three in 30 minutes, then a stop.** Claude Code gives `rate_limit` both for a spent plan and
  for a transient limit; the bridge tells them apart only by the session's plan windows (#249),
  and a session with no window reading stays a failure. The third such failed turn of one launch
  within 30 minutes, with no other turn ending in between, is recorded as a quota stop, so the
  #59 relaunch and the #158 resume take it. One or two failures is a passing limit the agent
  retries on its own.
- **The provider names its reasons.** `Provider::transient_limit_reasons` declares which failure
  reasons count; the default is none. Claude Code declares `rate_limit`. Codex names a spent
  plan itself (`usageLimitExceeded`), so it declares none.
- **The provenance is kept.** The stop's `inferred_from` (migration 22) says what it was inferred
  from; `torlek session show` prints it, the daemon logs it as a warning, and a stop whose
  windows show room tells the owner it was inferred.
- **The count lives in the database.** `session_limit_failures` is written in the report's own
  transaction, so a daemon restart neither loses nor doubles the count. Any other turn ending, a
  new launch, or the stop itself clears it.
- **Rejected:** counting OpenCode's `APIError`: it covers every provider error, so it would turn
  outages into quota stops. That gap stays open until OpenCode names a limit.

Evidence: `crates/cockpit-daemon/tests/quota_resume.rs`, `crates/cockpit-sessions/src/quota_stops.rs`,
`crates/cockpit-claude/tests/outcome.rs`.
