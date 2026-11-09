# A quota-stopped session resumes once the daemon's own plan-window readings show the reset

Status: accepted

Issue #158. Decided by Theo (2026-11-04 scope, answers A A A relayed by the coordinator).

- **The reset is two readings.** One shows a window spent or the account blocked, taken
  after the stop or just before it for a window spent past it; a later one shows room; then the daemon sends one continue message. The
  agent's own stop text and a provider's `resets_at` are not trusted alone: a clock skew or a
  stale reading would resume a session into the same limit.
- **One durable record per stop, claimed before the message.** `session_quota_stops` holds
  it; the resume pass sets `resumed_at` in a transaction before `submit`, and gives the claim
  back only when the message certainly did not leave. A daemon that dies mid-send never sends
  twice. The agent working again, stopping again, or its session ending drops the stop.
- **Named accounts go to the owner until per-account readings exist.** The daemon reads only
  the owner's own sign-in (`default`, #58), so a stop on a named account with no relaunch to
  move it (#59) becomes a needs-you dialog, as does a stop whose windows show room. Any stop
  still waiting five hours on, or an hour past the reset a reading said, goes to the owner too.
- **A #59 move is the reset.** The relaunch on an account with capacity marks the stop, and it
  is due at once. An asleep session is woken at the reset, then sent its continue once its
  agent waits. Coordinators go first; a worker waits while a due coordinator starts or is busy.
- **Rejected:** timing the resume from the stop plus the five-hour hold (resumes into a
  spent seven-day window); reading each named account's windows now (that is the follow-up).

Evidence: `crates/cockpit-daemon/tests/quota_resume.rs`, `crates/cockpit-daemon/src/quota_resume.rs`,
`crates/cockpit-sessions/src/quota_stops.rs`.

## Revisited 2026-11-09 (#158)

A #59 move refused before it stops the agent hands the stop to the owner at once, instead of
once it is overdue: the session stays on its spent account, and five hours is too long to wait
for a move that a refusal, such as a changed sandbox setting (#969), will keep refusing. The
periodic pass still tries the move. A failure past the refusal is not handed over: a passing
error is retried, and a failed launch fails the session, which shows it. The issue's scope asks
for it (item 2); the coordinator chose it over leaving the stop to the overdue hand-over.
Evidence: `cockpit-daemon` `sandbox::a_quota_stop_whose_account_move_is_refused_asks_the_owner_at_once`.
