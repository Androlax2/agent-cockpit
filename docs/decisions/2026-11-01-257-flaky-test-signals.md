# Flaky tests wait on a real signal; the capture stream drops output its pending snapshot shows

Issue #257 (also #222, #224, #264, #265, #267). Decided by the worker: each flake was a read
before the thing it checks had happened, and a longer timeout only hides that.

- **The capture stream drops a pane's output while a snapshot of that pane is pending.**
  tmux runs a control client's commands in order, so `%output` that arrives before the
  `capture-pane` reply was on screen when the capture ran (`crates/cockpit-tmux/src/capture.rs`).
  This makes the documented order (snapshot, then output) true for a busy pane; the regression
  test is `crates/cockpit-tmux/tests/capture_ordering.rs`.
- **Fake-agent launch logs are read through `common::once_logged` / `common::still_logged`**
  (`crates/cockpit-daemon/tests/common/mod.rs`). The agent logs from its own shell after tmux
  starts it, so a test waits for a launch before reading it or stopping that agent. A check that
  no further launch happened watches the log for `common::SETTLE` (500 ms) instead of reading once.
- **The niceness and idle-scheduling tests wait until the leader exec'd the program.** Before
  that the leader is still the `nice`/`chrt` shim, and `inspect` truly reports the inherited value.
- **Rejected:** making the fake provider's launch return only after the agent logged (#257
  option B): a provider only builds the command; the daemon spawns it, so that would need a
  product hook for a test.
- **Rejected:** keeping the test-side tolerance of leading output in `tests/reattach.rs`: it
  hid a broken ordering contract every client would have to work around.
