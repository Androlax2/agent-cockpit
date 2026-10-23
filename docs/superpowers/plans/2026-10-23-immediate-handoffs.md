# Immediate Handoff Sessions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Start fresh sessions for direct cross-project handoffs and setup blocker reports on the next cockpit tick, regardless of existing sessions.

**Architecture:** Run handed-work dispatch before the ordinary queue's 30-second throttle and idle-session assignment. Reuse the existing background launcher and todo status tracking; remove the active-project exclusion for these items.

**Tech Stack:** Rust cockpit application and its existing fixture-based tests.

**Spec:** Chat approval of Option A for todo `7c2eb8cf`.

## Global Constraints

- Preserve queue-off, gaming pauses, dependency waits, design routing, and proposal approvals.
- Ordinary manually queued work retains its existing idle-session behavior.
- A handoff is a task request, not authorization for additional privileges.
- No dependency changes. Background-launch naming may change to prevent same-second collisions exposed by batching.

## Review Focus

- Existing idle or working sessions must not absorb or prevent fresh handoff sessions.
- A recently checked queue must not delay a new handoff.
- Repeated ticks must not launch the same item twice.
- Multiple handoffs to one project must each launch fresh sessions.
- Blocker reports lack `queued_by`; include them explicitly without enabling ordinary todos.

### Task 1: Dispatch handed work immediately into fresh sessions

**Files:**
- Modify: `apps/cockpit/src/app.rs` (dispatch order and handed-work selection).
- Modify: `apps/cockpit/src/todo.rs` (blocker dispatch documentation).
- Modify: `config/bin/.local/bin/cockpit-agents/claude.sh` (collision-resistant background session names).
- Test: `apps/cockpit/src/tests.rs`.
- Test: `apps/cockpit/tests/cockpit_script.rs` (two successful same-second launches).

**Interfaces:**
- Consumes: `App::dispatch_queue(&mut self)`, `App::start_sessions_for_handed_work(&mut self, &queue::QueueContext)` and existing `TodoList` update fixtures.
- Produces: Same interfaces, with immediate fresh-session routing for eligible handed items.

- [x] Add regression tests covering all five review conditions and preserved pause/dependency gates; update the old idle-session handoff expectation to fresh launch while retaining its authorization assertion.
- [x] Run targeted tests with `cargo test --release --manifest-path apps/Cargo.toml -p cockpit handed` and confirm failures identify the old routing.
- [x] Dispatch handed work before idle reuse and throttling; select blocker reports as well as existing handed work, and remove project/session exclusions.
- [x] Run targeted tests and `make test`; record any failures without disabling tests.
- [ ] Review the diff, commit with `Todo: 7c2eb8cf`, export patches and request host apply/test approval through `ask-to-run` as required by `CLAUDE.md`.
