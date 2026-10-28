# Chief Incremental Availability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Complete a frozen Chief snapshot and then consume routine availability updates without starting another snapshot.

**Architecture:** Extend the existing explicit event projection for host `availability_changed` events. Return only the worker fields already exposed by Context, never the raw event body or runtime identity. Keep snapshot recovery for unknown controls, new stop bundles and availability records that cannot fit the transport bound.

**Tech Stack:** Rust, rusqlite, serde_json; no dependency changes.

**Spec:** Theo's task b90b30c5: preserve every grant, admission, authorization and budget gate; regressions plus live Chief verification after reviewed host apply.

## Global Constraints

- Keep the 50-record admitted-turn budget and existing admission/mutation checks.
- Keep stopped-worker visibility and the existing frozen-snapshot continuation.
- No installs, dependency changes, raw event bodies or extra authority.
- Host apply and host verification require Theo's normal review.

## Review Focus

- Worker readiness, blocked reasons and disappearance must remain visible.
- Runtime identities and unrelated event body fields must never be exported.
- More than one turn of availability events must continue at the saved event cursor.
- New stops must still expose their masked bundles, not disappear into event summaries.
- Malformed or oversize availability data must retain bounded snapshot recovery, not skip events.

### Task 1: Project routine availability incrementally

**Files:** Modify `apps/cockpit/src/chief/store.rs`; test `apps/cockpit/tests/chief_events.rs`.

**Interfaces:** Existing `ChiefAgentAction::Context` and `ContextSnapshot`; no new action or authorization route.

- [x] Add failing regressions for snapshot completion followed by multiple availability-only turns, budget exhaustion/continuation, roster changes/redaction, stop visibility and malformed/oversize recovery.
- [x] Run the regressions and confirm the expected snapshot-restart failure.
- [x] Add the smallest explicit host availability projection; preserve other recovery and budget paths.
- [ ] Run Chief suites and full `make test`; obtain independent diff review and commit with `Todo: b90b30c5`.
- [ ] Export only this task, request normal host review/apply and host tests.
- [ ] Ask the active Chief to complete a snapshot, read its event cursor over subsequent admitted turns, and report incremental availability, bounded continuation and visible stalled workers. Iterate until the original restart symptom is absent.
