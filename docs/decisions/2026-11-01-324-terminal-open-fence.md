# A launch opens its session's terminal only once every earlier step on that terminal returned

Issue #324. Decided by the worker, with the reason below.

- **Each step that may close or open a session's terminal is counted until its thread
  returns; a launch's open waits, within its step timeout, for the count to reach zero**
  (`Sessions::open_terminal`, `crates/cockpit-sessions/src/lifecycle.rs`), and is refused
  `conflict` (`Error::TerminalBusy`) otherwise. A stale step therefore finishes before the
  newer launch's terminal exists, and the open's own `close_if_open` clears what it left.
- **Closes never wait.** The owner can still end a session whose launch hung in tmux (#272);
  a stale step acting after an end touches no newer launch's terminal.
- **Rejected:** a generation the step checks right before it acts (option A): a tmux command
  already sent to a hung server still runs when it recovers, after the check. Per-launch
  terminal names (option B): sound, but the name is the daemon's and the viewer's handle too
  (`crates/cockpit-daemon/src/manager.rs`), and stale opens would leave orphan terminals that
  need a sweep; the fence gets the same guarantee inside the sessions crate.
- Evidence: `crates/cockpit-daemon/tests/launch_deadline.rs`,
  `a_resume_does_not_open_the_terminal_while_a_timed_out_open_is_outstanding` and
  `a_resume_opens_the_terminal_once_the_timed_out_open_returned`.
