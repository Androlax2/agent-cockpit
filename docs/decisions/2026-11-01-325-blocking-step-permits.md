# The sessions capability bounds its outstanding blocking steps with permits held until each step returns

Issue #325. Decided by the worker: report 03 asks for finite blocking ops behind a semaphore,
and its "cancellation has two facts" rule says when the permit is released.

- **One semaphore for the capability, `Settings::blocking_steps` permits (64 by default).**
  Every blocking step (`Sessions::blocking_by`, `crates/cockpit-sessions/src/lifecycle.rs`)
  takes one before `spawn_blocking` and moves it into the thread, so a timed-out step keeps
  its permit until it really returns. Hung tmux or provider calls can hold at most 64 of
  Tokio's 512 shared blocking threads; the rest of the daemon keeps its share.
- **The permit wait and the step share one step timeout.** A step that gets no permit in
  time is answered `busy` (`Error::StepsBusy`), retryable, never queued without bound.
- **Rejected:** refusing a session's new steps while one of its timed-out steps is
  outstanding (the issue's option B) as the bound: it limits threads per session, not in
  total, so many sessions or the periodic pass still drain the pool. Per-session ordering is
  kept only where it matters, for opening a terminal (#324).
- Evidence: `crates/cockpit-daemon/tests/launch_deadline.rs`,
  `a_step_is_answered_busy_while_a_timed_out_step_holds_every_permit` and
  `a_timed_out_step_gives_its_permit_back_once_it_returns`.
