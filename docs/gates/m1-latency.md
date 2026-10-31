# Gate M1: latency run and restart fixtures (#36)

Proof for [gate #37](https://github.com/Androlax2/agent-cockpit/issues/37)'s "latency run
recorded" and "executions survive daemon restart" items, and for report 15's claim "control
path stays responsive under load". The workload is defined in
[the decision record](../decisions/2026-10-31-36-latency-workload.md); the harness is
`crates/cockpit-daemon/tests/latency.rs`.

## Recorded run, 2026-10-31

- Machine: AMD Ryzen 7 7800X3D (8 cores, 16 threads), 30 GiB RAM, Arch Linux, kernel
  7.2.11-arch1-1 x86_64, tmux 3.8, `/tmp` on tmpfs.
- Toolchain: cargo 1.99.1, cargo-nextest 0.9.150; release profile.
- Commit: `bfbf0b629a7eb1425656a3652f84c75b753d09e5`.
- From 20:06:59 to 20:37:04 CET. The machine was not idle: other agent sessions were
  building and testing in parallel clones. Load average 11.7 at the start, 35.9 at the end
  (16 of it from the run's own build processes).
- Command:

  ```sh
  COCKPIT_LATENCY_REPORT=report.md cargo nextest run --release --manifest-path Cargo.toml \
    -p cockpit-daemon --test latency --run-ignored only --no-capture
  ```

  Outcome: `PASS [1804.384s] cockpit-daemon::latency recorded_latency_run`.
- The harness was cleaned up after this commit (same loads, probe and report; reattach also
  checks the agent is alive; the daemon's stderr goes to a file). A 30 s release run of the
  final harness passed: p95 6.2 ms, p99 10.3 ms.

| Request | Count | Failed | p50 ms | p95 ms | p99 ms | Max ms |
| --- | --- | --- | --- | --- | --- | --- |
| all control (held connection) | 36000 | 0 | 3.6 | 45.1 | 74.0 | 193.3 |
| ping | 7200 | 0 | 2.8 | 7.8 | 14.5 | 74.0 |
| session-list | 7200 | 0 | 28.9 | 71.6 | 105.9 | 193.3 |
| agent-reports | 7200 | 0 | 3.3 | 24.8 | 59.7 | 152.8 |
| approval-queue | 7200 | 0 | 3.3 | 10.5 | 23.3 | 103.3 |
| provider-list | 7200 | 0 | 2.9 | 7.7 | 13.9 | 64.2 |
| connect+ping | 1800 | 0 | 4.0 | 12.5 | 23.0 | 62.8 |

Targets (report 03), on all control requests of the held connection:

- p95 ≤ 100 ms: met (45.1 ms).
- p99 ≤ 250 ms: met (74.0 ms).
- No answer later than 1 s: met (0 later; slowest 193.3 ms).

Loads, all answered, none busy, none failed: 354 restarts of the stalled agent, 32550
session starts and ends, 1834 thousand item adds and deletes over two connections; 16 build
stand-ins; the flooding agent ran to the end; the stalled change-feed viewer stayed open.

Restart: the daemon was killed (`kill -9`) with its sessions running and was ready again
after 64 ms; every session was reattached with its launch unchanged; agent launches 16467
before, 16467 after.

## What this does not prove (gaps)

- **One machine, Linux only.** Report 03 asks for named hardware; macOS and WSL 2 have no
  recorded run. The targets stay `validate` for them.
- **Stops are not measured.** `session-end`, `session-sleep`, `session-restart` and
  `resource-stop` wait in the bulk lane, so under a full bulk lane they can be answered
  `busy`; report 03 reserves capacity for them. Decided in #217 (option B, a small stop lane);
  until that lands, this run measures status and approval requests only.
- **Keystrokes are not measured.** Terminal input goes from the attached client to tmux and
  never through the daemon (see the decision record).
- **`session-list` is the slowest request** (p99 105.9 ms, above the 100 ms p95 target on its
  own). It reads every session and its launch; it grows with the number of sessions.
- **Memory was still rising at the end**: 27 MiB at the start, 47 MiB at the peak, which was
  the last sample. Thirty minutes do not show whether it levels off; a longer run would.
- **A restart interrupted by a crash** is dropped instead of finished (#205); the fixture
  asserts only that one agent runs.

## Fixtures in CI

- `latency::control_requests_are_answered_while_every_load_runs_and_a_restart_reattaches`:
  the same harness for 3 s with one build process; asserts the harness, not the targets.
- `restart_reattach::a_killed_daemon_reattaches_the_running_session_without_launching_it_again`.
- `restart_reattach::a_daemon_killed_while_it_restarts_a_stalled_agent_leaves_one_agent_running`.

To record another run, use the command above (`COCKPIT_LATENCY_SECONDS` shortens it) and add
a section here with the machine, commit, command, report and gaps.
