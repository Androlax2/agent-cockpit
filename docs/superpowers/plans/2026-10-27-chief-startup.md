# One-action Chief startup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Theo previously selected inline execution. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Start and enable one dedicated OpenCode Chief session from one explicit Start Chief action.

**Architecture:** The host owns a durable startup reservation in the existing protected SQLite store. It records a native session identity before launching, reconciles that exact session against verified host observations, and uses the existing Enable checks to activate it. The UI displays host state; opening or refreshing it never launches anything.

**Tech Stack:** Existing Rust, rusqlite, host IPC, Bash launcher and terminal UI. No new dependencies.

**Spec:** User-approved `design-plans/previews/19-chief-startup-wireframe.png` and the startup decisions recorded in this conversation. Existing Chief behavior is documented in `docs/superpowers/plans/2026-10-26-chief-ui.md`. Final startup frame/state mappings from the Designer are required before UI implementation.

## Global constraints

- Off copy: “Chief is off”; “Chief handles routine questions from your agents and brings important decisions to you.”; “Start Chief”. No session picker.
- Off → Starting → Checking → On. Show actual failure causes. Offer Try again only after authoritative confirmation that the attempt cannot leave a running session.
- Opening/closing/reopening the view must not create, cancel or repeat a launch.
- Preserve exact-live-session checks, native OpenCode admission verification, product-worker exclusion and reviewed replacement/disable behavior.
- Dedicated working folder was accepted when Theo said Continue after the recommendation. Proposed implementation location: `~/IdeaProjects/cockpit-chief`, resolved from host HOME. This path is a builder choice, not a previously supplied user path.
- Do not create that host folder or launch a real Chief during implementation/tests. Test with isolated HOME and fake launch commands. Host application and live verification use ask-to-run.
- Do not overwrite an existing folder's instructions or configuration. Reject symlink/non-directory collisions and report them; retain existing sandbox-launcher checks.
- Preserve concurrent Designer, phone-fix and other checkout edits. No dependency-manifest changes. Desktop pinned escalations remain deferred.

## Review focus

1. Two clients submit different Start requests concurrently: one launch only (Task 1).
2. Host crashes between recording launch intent and starting the process: retain Checking, never silently relaunch (Task 2).
3. Launch succeeds but activation is rejected: keep tracking the same session, show the reason, never offer a second launch (Task 2).
4. A late or restarted session reports the same native ID with a different runtime: reverify identity; never transfer a grant from stale evidence (Task 2).
5. A working folder already exists with user content or a symlink: preserve content and reject unsafe collisions (Task 2).

## Task 1: Durable startup state and host protocol

**Files:** `apps/cockpit/src/chief.rs`, `apps/cockpit/src/chief/store.rs`, `apps/cockpit/tests/chief_protocol.rs`, `apps/cockpit/tests/chief_store.rs`.

**Proposed interfaces:** Add `ChiefHostAction::Start { request_id: String, previous_attempt: Option<String> }`; `None` starts the first attempt, while retry must name the conclusively failed current attempt. No caller-selected session, directory or agent. Add a `startup` object to the host snapshot with attempt ID, revision, state, reserved native session ID, verified identity when available, error and `can_retry`. State values: `starting`, `checking`, `active`, `failed`; absent means no startup attempt. These names are new implementation proposals, not existing APIs.

- [ ] Add protocol tests rejecting extra directory/session/agent arguments and malformed request IDs.
- [ ] Add store tests through `store::host`: repeated request ID is idempotent; different concurrent requests share one unresolved attempt; an active/paused Chief cannot be replaced through Start; failed retry requires the latest attempt to be conclusively settled. A delayed initial Start or retry naming an older attempt cannot restart a newly failed attempt.
- [ ] Run `cargo nextest run --release --manifest-path apps/Cargo.toml --test chief_protocol --test chief_store`; confirm new tests fail for the absent behavior.
- [ ] Extend the existing migration and action journal. Store one globally unresolved attempt with its identity and launch state; enforce uniqueness transactionally, not with a UI lock. Preserve existing migration/reopen tests and grants.
- [ ] Add snapshot tests proving attempts survive store reopen, errors are retained, and uncertain attempts never expose `can_retry: true`.
- [ ] Run the same test command and confirm success.

## Task 2: Launch and reconcile the reserved session

**Files:** `apps/osd/src/chief.rs`, `apps/osd/src/chief_tests.rs`, `apps/osd/src/chief_workflow_tests.rs`, `apps/cockpit/src/chief/store.rs`, `config/bin/.local/bin/cockpit`, `config/bin/.local/bin/cockpit-agents/opencode.sh`; add `apps/cockpit/tests/chief_startup.rs` for isolated launcher fixtures.

**Consumes:** Task 1's protected attempt and Start action. **Produces:** snapshot progress and exactly one existing Enable operation for the verified reserved session.

- [ ] Add host-entry tests using the existing Registry/System fixtures for duplicate Start, observation before launch, crash/reopen before dispatch, delayed reports, mismatched project/runtime, activation rejection, and eventual successful activation of the same attempt.
- [ ] Add fake-command launcher tests: correct dedicated folder; existing content preserved; symlink rejected; configured model/effort passed through; the reserved ID reaches `--session`; nonzero launch result retains its cause. No real tmux/OpenCode launch.
- [ ] Run `cargo nextest run --release --manifest-path apps/Cargo.toml --test chief_startup` and `cargo nextest run --release --manifest-path apps/Cargo.toml -p osd chief`; confirm failures for the absent behavior.
- [ ] Add a Chief-specific internal launcher command, separate from team-spawn. Accept only a validated reserved attempt/session identity and the host-owned dedicated directory; do not assign Chief a Designer/Developer/QA role. Reuse `start_opencode`, default model/effort validation and sandbox launcher.
- [ ] Allocate and persist the native identity before dispatch. Use a persisted dispatch claim before invoking the launcher. A reclaimed in-flight claim is Checking, not permission to dispatch again. The host must not block the registry while waiting for startup.
- [ ] Record launcher completion/error through a host-controlled channel with attempt identity; sandbox-authored files are not proof that a launch failed or stopped. A timeout or missing report is uncertainty, not a failed attempt.
- [ ] Reconcile in the existing Chief host/tick path, not in snapshot reads. Match the reserved native ID, directory/project and verified live runtime. Run the existing admission checks and Enable path; use a stable per-attempt activation request ID so replay cannot create another grant/bootstrap.
- [ ] If admission fails while the session lives, remain Checking with the exact reason and continue observing that same session. Expose retry only when launch completion plus host runtime/terminal evidence establishes no session remains and no late dispatch is possible. Do not automatically kill an uncertain session.
- [ ] Verify the crash windows with fixtures: before dispatch claim, after claim before process start, after process start before result, and after Enable before recording completion.
- [ ] Run `cargo nextest run --release --manifest-path apps/Cargo.toml --test chief_startup` and `cargo nextest run --release --manifest-path apps/Cargo.toml -p osd chief`. Confirm every new test passes.

**Host-impact note:** Both shell files run with Theo's host rights after application. Explicitly call out their changes in the implementation handoff. Confirm the dedicated folder is supported by the normal sandbox profile workflow; if it requires host setup, request that through ask-to-run rather than bypassing it.

## Task 3: Connect the approved startup screen

**Files:** `apps/cockpit/src/chief/view.rs`, `apps/cockpit/src/app/chief_ui.rs`, `apps/cockpit/tests/chief_ui.rs`. Designer owns `DESIGN.md` and the FigCraft file.

**Consumes:** Task 1's snapshot and Start action; finalized approved frame IDs and keyboard mappings. **Produces:** one explicit startup action and host-driven progress/recovery rendering.

- [ ] Obtain the Designer's ready-for-development frame mappings. Draft frame IDs from coordination task `06fab667` are 2661/2667/2673/2677/2681; these are not evidence of final readiness.
- [ ] Add interaction tests: opening/refreshing produces no Start effect; explicit activation produces one; repeat input during pending acknowledgement cannot enqueue more; closing/reopening displays the existing attempt; failed reads never imply Off; retry follows the host's `can_retry` only.
- [ ] Run `cargo nextest run --release --manifest-path apps/Cargo.toml --test chief_ui` and confirm new tests fail.
- [ ] Replace only the Off startup interaction using the approved mappings. Send Start through existing host IPC, render Starting/Checking/error from the snapshot, and transition to the existing On view only after the active grant is observed.
- [ ] Run the Chief UI test suite. Check existing pause/resume/disable, answer, overrule and habit interactions remain passing.

## Task 4: Verify and hand off

- [ ] Run all affected Chief integration tests and OSD Chief tests once after integration. Use existing phone checks if shared snapshot serialization changes affect their fixtures. Record exact output, including failures.
- [ ] Review the diff against the approved startup requirements; explicitly inspect every launch/retry path and every new host command argument.
- [ ] Show the diff stat and distinguish this implementation's files from concurrent work. Hand the verified work to team QA through the existing supported handoff.
- [ ] Commit only reviewed task changes when checks and design readiness are satisfied; do not stage concurrent phone/design edits. Preserve task `1f838f53` as open while its deferred work remains.
- [ ] Follow project host application/review instructions after QA. Any live startup smoke test requires explicit approval and checks one session, one grant, reopen continuity and no duplicate launch.

## Review gate

This plan is ready for Theo's review, not an implementation-complete claim. Inline execution remains selected. Final visual mappings gate Task 3; the host launch/recovery tests gate activation. No real Chief session has been launched by this plan.
