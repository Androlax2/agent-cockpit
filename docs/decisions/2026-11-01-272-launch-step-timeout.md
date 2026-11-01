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
