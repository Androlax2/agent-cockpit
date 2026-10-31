# A stop of a session whose launch still runs is refused with `conflict`, never queued behind it

Issue #230. Decided by the worker: report 03 (proposed) already rules that a stop "neither holds
a control worker nor waits behind launches" (#217); of the issue's three options only A and B
keep that rule, and B is the one an M1 fix can carry.

- **Refuse, do not wait.** `end` and `sleep` take the session's lock through
  `Sessions::lock_to_stop` (`crates/cockpit-sessions/src/lifecycle.rs`). While a launch holds
  that lock (start, wake, resume, restart, a start-file restart, the periodic pass's pending
  restart) they answer `Error::LaunchInProgress`, `conflict` on the wire, at once. The stop
  lane's two workers are freed immediately, whatever a provider's startup takes.
- **A launch that starts while a stop waits refuses that stop too.** A stop queued behind a
  short holder (the periodic pass, a link, an `ensure_awake` on a running session) is woken
  when that holder turns into a launch, and refused, so it never ends up behind the launch.
  The cost: a stop that was ahead of a launch in the lock's queue may be refused although it
  would have run first. A retry serves it.
- **The marker is in memory.** Only a running launch counts; a session left `launching` by an
  error or a crash has no marker, so `end` still stops it as before (#157, #221).
- **Rejected:** A, pre-empting a running launch at a checkpoint: it needs cancellation points
  through the provider, process and tmux steps, wider than an M1 fix. C, accepting the wait:
  it contradicts report 03's stop-lane rule.

Evidence: `crates/cockpit-daemon/tests/stop_lane.rs` (`*_launching_session_*`).
