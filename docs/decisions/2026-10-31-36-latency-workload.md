# The M1 latency run measures control requests to the daemon binary under a fixed synthetic workload

Issue #36. Decided by the worker: report 15 leaves "hardware and workload definition for the
latency targets" open to Theo, and #36 needs a definition to record a run. Theo may replace it;
the harness takes the duration from `COCKPIT_LATENCY_SECONDS`, and the loads are in one file
(`crates/cockpit-daemon/tests/latency.rs`).

- **"Input latency" is the round trip of control-lane requests** (`ping`, `session-list`,
  `agent-reports`, `approval-queue`, `provider-list`) on a held connection, plus a new
  connection's handshake and `ping` each second. Keystrokes to an agent go from the attached
  terminal to tmux and never pass through the daemon, so the daemon's share of "input" is what
  a client asks to show status and answer approvals.
- **Latency counts from when the request was due**, every 50 ms, not from when it was sent: a
  stall delays the requests behind it, and each of those counts as late (no coordinated
  omission).
- **The real binary, killed for real.** The daemon runs as a separate process, with the
  production wiring (Claude Code found on `PATH`, here a script; tmux under `TMUX_TMPDIR` in
  the test home), so the probe and the loads never share its runtime.
- **The loads, all at once:** one build stand-in per CPU (`head -c 64M /dev/urandom |
  gzip -1 > /dev/null`, looped: CPU-bound, no toolchain needed; output to a file would sit in
  memory when the test home is on tmpfs); a session whose agent
  writes to its terminal without pause; a change-feed subscriber that never reads; a session
  whose agent ignores `SIGTERM`, restarted back to back (each restart holds a bulk worker for
  the 5 s stop grace); two owner connections adding and deleting items back to back; one
  connection starting and ending a session back to back.
- **Targets are reported, not asserted.** Report 03's numbers stay `validate`; a missed target
  is recorded with its gap in `docs/gates/m1-latency.md`. The run fails only on a refused or
  failed control request, or a restart that loses or relaunches a session.
- **The recorded run uses the release profile**, as installed; the CI smoke variant runs 3 s with
  one build process and asserts only the harness.
- **Rejected:** an in-process daemon (the probe and loads would share its Tokio runtime and
  distort it); a real `cargo build` as the build load (needs a toolchain and network, and its
  length differs per machine); driving the owner's control lane to saturation (one human owner
  does not send hundreds of requests at once; lane saturation is already covered by
  `overload::control_requests_are_answered_while_bulk_work_saturates_its_workers`).
