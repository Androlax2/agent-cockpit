# Remaining release gates implementation plan

> **For agentic workers:** Use executing-plans to implement this plan inline, with one fresh independent full-delta review before the host-approved workflow push.

**Goal:** Give each remaining #191 release check an executable, bounded CI job and keep unmeasured acceptance explicit.

**Architecture:** Release tooling owns candidate selection and archive validation; existing daemon fixtures own protocol, recovery and responsiveness behavior. Jobs consume the same archive built by the release workflow. Upgrade checks use an earlier published release of the same channel, never a synthetic claim of a previous release.

**Tech stack:** Existing Python standard library, Rust test fixtures, GitHub Actions and Make.

**Spec:** Issue #191 and [report 13](../../design/greenfield/13-release-upgrades.md), subject to Theo's lane instructions.

## Constraints and review focus

- No local sustained load, new dependencies, live providers, publication, or history rewrite.
- Fresh temporary homes, stand-in providers and private tmux servers; bounded CI only for sustained measurements.
- Existing report targets: held control p95 ≤ 100 ms, p99 ≤ 250 ms, no response later than one second.
- All candidate claims must bind the unpacked manifest and source revision; latency must run the archive's daemon.
- Preserve the Alpha upgrade exemption. Missing previous beta release is a named refusal, not a pass.
- Preserve all gate dependencies and existing release-channel restrictions. Workflow push requires host approval.
- Test missing/tampered archives, unsafe or wrong-channel previous releases, failed commands, interrupted startup and threshold failures.

## Task 1: Native macOS execution and compatibility fixtures

Files: `.github/workflows/release.yml`, `Makefile`, `tools/release.py`, `tools/tests/test_release.py`.

- [x] Add failing wiring tests for native Intel macOS install, a compatibility job and publication dependencies.
- [x] Add the Intel runner consuming the universal archive and assert native architecture.
- [x] Add a focused Make target for existing real protocol, restart/interrupted-action and migration fixtures, run on the exact candidate revision.
- [x] Run release-tool tests; CI proves platform execution.

## Task 2: Bounded responsiveness of the packaged daemon

Files: existing `latency.rs` and `real_daemon/mod.rs` fixtures, Makefile and release workflow.

- [x] Add unit fixtures proving latency targets reject missing samples, failed requests and missed thresholds without generating load locally.
- [x] Add an ignored release gate: five minutes, at most two build stand-ins, two database writers, existing flood/stall/restart checks, enforced targets and saved report.
- [x] Allow the fixture to select the validated archive binaries explicitly, retaining an isolated dev profile and production wiring.
- [x] Wire a bounded CI job to verify/unpack the archive, run only this test, and upload its report even on failure.
- [x] Run focused non-load tests and compile/lint affected test targets locally; execute the sustained run only in CI.

## Task 3: Upgrade from the previous channel release

Files: `tools/release_upgrade.py`, `tools/tests/test_release_upgrade.py`, Makefile and release workflow.

- [x] Test previous-release selection and invalid/missing baselines using fake GitHub responses.
- [x] Select a published, earlier release of the same channel; download and validate its archive/checksum in CI only.
- [x] Drive old and new unpacked CLI/daemon binaries in one temporary home, stopping the old daemon before the new one opens persisted project/task state; assert retained state and normal post-upgrade writes.
- [x] Test orchestration through the command entry point with isolated fake processes, including failure/cleanup paths.
- [x] Wire Linux, macOS and WSL2 jobs. Alpha reports its explicit exemption; a beta without a baseline fails clearly.

## Task 4: Evidence and delivery

- [x] Update dated #191 decision, RELEASING guidance and coverage-map evidence without claiming unmeasured live outcomes.
- [x] Run focused tests and affected format/lint checks once on final source; record unavailable tools and failures.
- [x] Obtain independent full-delta review: actual `$code-review medium` on `0c004c6`, findings `[]`.
- [ ] Request host approval for the normal workflow branch push.
- [ ] Open a PR with Part of #191 until all acceptance is measured; wait for required CI and record exact archive/gate results before merge.
