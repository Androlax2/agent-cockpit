# Session-stop characterization and diagnostics implementation plan

> **For agentic workers:** Use the `executing-plans` workflow to implement this plan inline, task by task, after approval. Independent final review is a separate gate.

**Goal:** Characterize delayed host reaping through the real supervisor and make unconfirmed stops explain their actual phase without changing termination proof.

**Architecture:** `cockpit-processes` returns bounded, structured failure evidence with `Termination::Unconfirmed`; `cockpit-sessions` emits it through its existing logger. Process observations remain diagnostic only. No logging dependency, sidecar persistence or new capability layer is needed.

**Tech stack:** Existing Rust, Tokio, rustix, Turso fixtures and session tracing.

**Spec:** [Issue #537](https://github.com/Androlax2/agent-cockpit/issues/537), the user's bounded investigation assignment, and the coordinator report copied into `.handoff/537-lifecycle-investigation.md` (local evidence, not a committed dependency).

## Status and constraints

- Implemented and locally verified; uncommitted handoff. Initial focused run: 97 passed, 1 ignored live-systemd test. After the recovered-identity review fix: 36 affected tests passed, 1 ignored live-systemd test. These are separate runs, not a combined test total.
- Independent review's Important finding is fixed and covered by an observed RED/GREEN regression. Installed medium review is complete; two Minor follow-ups are recorded: ordinary parallel cargo-test subreaper isolation, and final-phase capture when a launching scope becomes identifiable during its last wait observation.
- Worker owns todo `5c009b71`; release worker owns #523 and its candidate gate.
- Isolated branch `greenfield/537-stop-diagnostics`, base `c8ba4022bf4409a811286f9feb67502d238e0313`. Attachment blocker `da66afee` is resolved.
- Main subsequently advanced to `74f27fc3e76ca6a0011d23ef28c4e7280778faec` through #525; comparison shows provider-fixture/docs changes only, with process/session sources unchanged. No history rewrite is needed for this plan.
- Preserve boot/start-token identification, conservative reservations, no replacement before confirmed end, whole-containment signalling and all existing error propagation.
- No stress/load loops, duplicate broad suites, additional #444 runs, installs, dependencies, manifest changes, publication, merges, tags or history rewrites.
- The coordinator demonstrated Linux zombie group presence, not the cause of CI executions 615 and 640. A successful characterization does not close #537.

## Review focus

- Reaping a leader while a live descendant remains must not release occupancy (task 1).
- An unidentified execution must neither receive signals nor be described as killed (tasks 2–3).
- Malformed, disappearing or unreadable process records must produce incomplete evidence, never termination proof (task 2).
- Diagnostic collection must remain bounded under large process inventories and concurrent failures (task 2).
- A stop refusal must retain the original session execution and checkout, with no replacement launch (task 3).

## Task 1: Hosted reaper characterization

**Files:** extend `crates/cockpit-processes/tests/hosted.rs`; add Linux-only fixture support under `crates/cockpit-processes/tests/common/` only if the descendant fixture needs it. Retain existing common helpers.

**Interface:** Tests call existing `prepare_hosted`, `confirm_hosted`, `release_hosted`, `terminate`, `observe`, `execution` and `occupying_executions`. Fixture controls reap permission explicitly; no production testing hooks.

- [x] Add `an_unreaped_hosted_leader_keeps_its_reservation_until_reaped`. Launch a TERM-ignoring program with an explicit ready handshake; gate the host's wait/reap. Use fixture-only 100 ms TERM grace and 200 ms kill wait, with the existing 10-second readiness ceiling. Verify exit without reaping, assert zombie state, unconfirmed result, `cancel_requested`, and occupied execution. Release the reaper, then assert `Phase::Ended(EndReason::Terminated)` and confirmed termination.
- [x] Add the companion scenario with a dead leader and a live, TERM-ignoring descendant. A fixture-owned subreaper controls adoption/reaping. Assert occupancy both before and after leader reaping, then call real termination and verify the descendant was killed. An unreaped descendant alone still retains occupancy; reaping it finally permits confirmation.
- [x] Give both scenarios a 25-second outer work deadline, with bounded failure cleanup of all fixture-owned children and no unbounded thread joins. Exercise cleanup when the readiness handshake fails, using the same fixture rather than a stress loop.
- [x] Run the focused hosted target once. These tests characterize existing behavior and are expected to pass before production changes; they are not a fabricated red/green lifecycle fix.

## Task 2: Structured, bounded stop evidence

**Files:** `crates/cockpit-processes/src/{model,lib,supervisor,platform,systemd}.rs`; new `crates/cockpit-processes/src/diagnostics.rs`; affected hosted/reattach tests.

**Proposed interface:** `Termination::Unconfirmed { diagnostics: Box<TerminationDiagnostics> }`. Keep the confirmed variant unchanged. Export the diagnostic types from `lib.rs`; remove `Copy` where the owned evidence requires it.

`TerminationDiagnostics` contains the execution identity/phase used by the attempt, `UnconfirmedReason::{Unidentified, KillWaitElapsed}`, final presence classification, elapsed duration, TERM/KILL results and a `ProcessSnapshot`. Signal entries are optional: absence means not attempted. Distinguish successful delivery from ESRCH; systemd success means the existing command was accepted or the unit was subsequently observed inactive, not proof that a signal was delivered. Other signal errors retain the existing `Err` path and errno/error detail.

- [x] Extend the unidentified reattach test to require `Unidentified` with neither signal attempted. Extend the zombie fixture to require `KillWaitElapsed` and both actual signal results. Observe the missing diagnostic contract before implementing it.
- [x] Capture the actual final presence result in the termination wait rather than inferring it from the error message or making a later snapshot authoritative. Update `Supervisor::terminate` and its private signalling/wait helpers; preserve ordering and the existing observation-based state transitions. The separate final persisted-phase edge remains a Minor review follow-up.
- [x] Implement a Linux failure-only `ProcessSnapshot` collector. Implemented hard caps: one admitted collector at a time, no waiting queue, 100 ms collection budget, 4096 directory entries examined, 32 matching group members and 32 parent records, 4096 bytes per stat read. Inspect the recorded leader first, then bounded group membership. Store PID/PPID/PGID/SID/state/start ticks and parent identity metadata only; never retain comm, cmdline, environment or raw stat text.
- [x] Keep finite filesystem work off the async executor. A timed-out caller must not free the collector slot while its blocking worker still runs. Mark admission-busy, deadline, entry/member limits, unsupported platform/containment, absent records, malformed records and read errors explicitly; a scan is never an atomic or authoritative process census.
- [x] Mark Linux process-group evidence unavailable for macOS and systemd containment in this bounded first pass; still report phase, identity and signal outcomes there. Never substitute a process-group snapshot for whole-cgroup evidence.
- [x] Test parsing and caps with isolated synthetic stat directories, including comm fields containing spaces/parentheses and a sentinel secret. Assert no raw text leaks. Test early timeout/busy behavior and unreadable/disappearing records. Verify the real gated fixture exposes its zombie and current parent identity.
- [x] Run focused process tests and inspect all `Termination` consumers. Snapshot failures must not change an unconfirmed result into success or conceal a supervisor error.

## Task 3: Session diagnostics and refusal contract

**Files:** `crates/cockpit-sessions/src/{lifecycle,error}.rs`; new `crates/cockpit-sessions/tests/stop_error.rs`; extend `crates/cockpit-daemon/tests/session_end_failed.rs` for entry-point refusal coverage.

**Interface:** Keep `Error::StopUnconfirmed { session, execution }` and wire error classification unchanged. Emit the structured process evidence at `stop_previous_agent` and `discard_launch`, where the session ID is available. Avoid a second duplicate warning in `finish_stop`.

- [x] Add a public error-display test that rejects the unconditional `after SIGKILL` assertion. Implemented message: `session {session}'s agent termination is unconfirmed: {execution} was not observed ended; the session keeps it`.
- [x] Update the stored transition reason to `its agent termination could not be confirmed`; detailed phase belongs in the structured warning. Keep `finish_stop` restoring Running on refusal and returning the same error.
- [x] Through the daemon socket fixture, make a running execution unidentifiable, attempt restart and assert conflict, the original execution/checkout retained and no second provider launch. Assert the returned text does not claim SIGKILL. Restore fixture identity before cleanup. This pins the new wording/logging wiring to the existing conservative refusal contract; it is not the CI failure reproduction.
- [x] Verify the diagnostic event contains session/execution correlation and no fixture secret; check both normal stop refusal and failed-launch cleanup call sites.

## Verification and handoff

Run only after implementation approval; stop on missing offline prerequisites rather than installing. Use `python3 config/bin/.local/bin/agent-build` as the existing runner, with `CARGO_BUILD_JOBS=2`, `NEXTEST_TEST_THREADS=2` and `--locked --offline`.

- [x] Iteration: `cargo nextest run --manifest-path Cargo.toml -p cockpit-processes --test hosted -j 2`; then focused `hosted`, `lifecycle`, `reattach` and diagnostic tests after the related change.
- [x] Final affected checks: process crate tests, session error test, daemon `session_end_failed`, `sessions`, and `restore_stalled_stops`; no unchanged broad reruns. Use the existing private home/tmux/provider fixtures only.
- [x] Formatting: `cargo fmt --manifest-path Cargo.toml --all -- --check`. Clippy: affected processes/sessions/daemon/resource-admission packages, `--all-targets --locked --offline -j 2 -- -D warnings`. Check new public documentation with warnings denied. Run installed changed-file Markdown checks for this plan and the eventual decision record.
- [x] Run the actual installed OpenCode `code-review.md` medium recipe on the full final delta: eight inline angles, deduplication and its prescribed output. Record the exact command-definition hash, reviewed revision/diff, findings and dispositions. This is distinct from the required independent final review; both are recorded in the local evidence directory below.
- [x] Obtain independent full-delta review, address material findings, and rerun only affected checks justified by changes. Report exact commands, results and any missing prerequisites.
- [x] Record the diagnostic-only architectural decision under `docs/decisions/` after implementation. Prepare diff stat and local handoff evidence for the coordinator. Leave #537 open pending actual failed-path evidence and the release owner's separately authorized corrected-candidate gate.

## Completion evidence

Theo approved the structured-return approach, bounded fixture/check budget, locked dependency fetch and recovered-identity review correction. Exact commands, RED/GREEN results, review provenance and dispositions live in `.superpowers/sdd/2026-11-02-537-stop-diagnostics/`. The decision record is `docs/decisions/2026-11-02-537-stop-diagnostics.md`. Local handoff: `.handoff/537-stop-diagnostics-handoff.md`. This work does not establish the cause of CI executions 615/640 or accept a release candidate.
