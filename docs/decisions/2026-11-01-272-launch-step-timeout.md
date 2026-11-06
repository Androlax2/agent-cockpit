# Every blocking step of a session operation has a deadline, so a hung launch fails

Issue #272. Decided by the worker: report 03 (proposed) rules that every exchange has a
deadline and that started blocking work cannot be aborted; option A, pre-empting a launch at
checkpoints, was already rejected in #230 as wider than the fix.

- **One bound, `Settings::step_timeout` (30 s), at one helper.** `Sessions::blocking` runs
  each blocking step (the provider building its command, tmux calls, start-file and process
  reads, resolving a directory) off the async workers and waits at most that long; then it
  answers `Error::StepTimedOut` (`internal`: the step may still take effect). A launch that
  times out fails like any failed launch: the session goes `failed`, the launch marker drops,
  and `end` or `resume` work. A restart's stop phase is bounded the same way.
- **The provider's `launch` now runs off the async workers.** Its contract says it does not
  block, but the issue names a provider step that blocks; this bounds it without trusting it.
- **A timed-out terminal open is discarded, not only abandoned:** the open may still create
  the pane and start the agent, so the terminal is closed and the execution stopped.
- **Database waits are not bounded.** The writer is shared: a hung writer stops every
  operation, a stop included, so a deadline on a launch's writes would not make it stoppable.
- **Rejected:** a deadline around the whole launch, which drops the launch future at any
  await, a transaction's commit included, and leaves what it prepared unknown.

Evidence: `crates/cockpit-daemon/tests/launch_deadline.rs`.

## Revisited 2026-11-01 (#324)

Decided by the worker. A launch opens its session's terminal only once every earlier step on
that terminal returned.

- **Each step that may close or open a session's terminal is counted until its thread
  returns; a launch waits, within one step timeout and before it prepares anything, for
  the count to reach zero** (`Sessions::wait_for_terminal_steps`,
  `crates/cockpit-sessions/src/lifecycle.rs`), and is refused `conflict`
  (`Error::TerminalBusy`) otherwise. A stale step therefore finishes before the newer
  launch's terminal exists, and the open's own `close_if_open` clears what it left.
- **Closes never wait.** The owner can still end a session whose launch hung in tmux; a stale
  step acting after an end touches no newer launch's terminal.
- **Rejected:** a generation the step checks right before it acts (option A): a tmux command
  already sent to a hung server still runs when it recovers, after the check. Per-launch
  terminal names (option B): sound, but the name is the daemon's and the viewer's handle too
  (`crates/cockpit-daemon/src/manager.rs`), and stale opens would leave orphan terminals that
  need a sweep; the fence gets the same guarantee inside the sessions crate.

Evidence: `crates/cockpit-daemon/tests/launch_deadline.rs`,
`a_resume_does_not_open_the_terminal_while_a_timed_out_open_is_outstanding` and
`a_resume_opens_the_terminal_once_the_timed_out_open_returned`.

## Revisited 2026-11-01 (#325)

Decided by the worker: report 03 asks for finite blocking ops behind a semaphore, and its
"cancellation has two facts" rule says when the permit is released. The sessions capability
bounds its outstanding blocking steps with permits held until each step returns.

- **One semaphore for the capability, `Settings::blocking_steps` permits (64 by default).**
  Every blocking step (`Sessions::blocking`, `crates/cockpit-sessions/src/lifecycle.rs`)
  takes one before `spawn_blocking` and moves it into the thread, so a timed-out step keeps
  its permit until it really returns. Hung tmux or provider calls can hold at most 64 of
  Tokio's 512 shared blocking threads; the rest of the daemon keeps its share.
- **The permit wait and the step each get one step timeout.** A step that gets no permit in
  time is answered `busy` (`Error::StepsBusy`), retryable, never queued without bound; one
  that queued is not cut short and misreported as hung.
- **Rejected:** refusing a session's new steps while one of its timed-out steps is
  outstanding (the issue's option B) as the bound: it limits threads per session, not in
  total, so many sessions or the periodic pass still drain the pool. Per-session ordering is
  kept only where it matters, for opening a terminal (the #324 section).

Evidence: `crates/cockpit-daemon/tests/launch_deadline.rs`,
`a_step_is_answered_busy_while_a_timed_out_step_holds_every_permit` and
`a_timed_out_step_gives_its_permit_back_once_it_returns`.
