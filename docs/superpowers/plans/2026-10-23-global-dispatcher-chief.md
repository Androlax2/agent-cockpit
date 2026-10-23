# Global Dispatcher Chief Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One approved chief coordinates existing agents across projects, advances approved work from recorded evidence, and interrupts Theo only for decisions.

**Architecture:** Extend the existing authenticated team IPC and host-owned SQLite state with a separate global chief grant and a revision-bound dispatch ledger. Keep existing task-team isolation and native inbox adapters; the daemon, not the LLM or sidebar, enforces grants, ownership, completion and delivery. While the chief owns scheduling, the legacy sidebar dispatcher cannot compete with it.

**Tech Stack:** Existing Rust workspace, rusqlite, serde/serde_json, Ratatui, existing Bash helper and Claude/OpenCode/Codex adapters. No new dependencies or manifest changes.

**Spec:** `docs/superpowers/specs/2026-10-23-global-dispatcher-chief-design.md` (approved).

## Global Constraints

- "One chief across all cockpit projects, not one per project."
- "New scope and recruitment require Theo's approval. Sensitive actions retain their approvals."
- "Notifications only for decisions Theo needs to make; progress summaries on demand."
- "A todo's existence, status, priority, or agent-authored text does not prove authorization."
- "A busy worker receives no new task." "Sleeping sessions are unavailable in V1."
- "A receipt, worker claim or independently marked-done todo cannot satisfy these gates."
- "Never blindly resend or reassign uncertain work."
- No new project mounts, host execution authority, arbitrary cross-project worker discovery, automatic replacement/recruitment or product work by the chief.
- Product implementation waits for plan approval and an execution-method choice. UI implementation additionally waits for approved design evidence; this plan does not invent layouts/keybindings.
- Existing feature-branch commit permission applies; no push/PR without an explicit request. Preserve unrelated work. Every changed host-running helper/adapter must be called out at handoff.

## Review Focus

1. Chief unavailable or paused while a second sidebar runs: no fallback starts duplicate or unapproved work (Tasks 2, 4).
2. Todo text or a cross-project dependency changes after approval: invalidate the affected revision; never silently widen authorization (Tasks 2, 5).
3. Test succeeds on a different revision, QA is superseded, or apply installation fails after git am: no completion or dependent dispatch (Task 5).
4. Human input, a queued prompt, a restart, sleep, or team-membership change races delivery: defer or cancel without typing over input (Tasks 3, 4).
5. A flood of agent events, terminal controls, or repeated blockers: bounded safe output, one decision per blocked action, no LLM wake loop (Tasks 6, 7).

## Scope, evidence and design checkpoints

This is one feature, not a new orchestration framework. Ship it disabled by default until all tasks pass. Tasks 1–6 implement the backend and integration; Task 7 is a **blocking UI design handoff**, not permission to improvise a screen. After its design is approved, append concrete frame references and UI tests before implementing its controls. No partial backend work should be presented as the finished chief.

Observed existing code:
- `apps/osd/src/teams.rs` authenticates peers via verified runtime and supplies protected host controls.
- `apps/cockpit/src/teams/store.rs` owns schema version 2 and durable team deliveries.
- `apps/cockpit/src/app.rs::dispatch_queue` sends via `cockpit say`; `start_sessions_for_handed_work` can start a session. Both must yield while chief scheduling owns the queue.
- `apps/cockpit/src/todo.rs` allows agent-originated items and dependency releases to enter the queue. Therefore queued is not dispatch approval.
- `Handoff.tests` and QA findings are currently free text. They are not structured successful-test/QA evidence.
- `config/bin/.local/bin/cockpit::apply_reviewed` marks trailers done before `after_reviewed_apply` finishes. Do not use that todo transition or `dotfiles.applied` as completed-install evidence.
- `claude-run-approve` records host-observed exit status after human approval. Reuse it as a source, not a new command runner. Never auto-execute tests with host rights because the chief requested them.

For V1, automatically usable test evidence is an explicitly approved host-run result bound to an approved gate and a protected reviewed revision, or an explicit host evidence attestation Theo approves. Ordinary sandbox tool output remains an agent report, not host-observed proof. Unsupported projects/sources produce one missing-evidence decision; do not invent a generic CI or sandbox execution broker. QA may supply a structured attestation as its approved role; identify that provenance honestly, not as independently proven test execution.

**User-approved admission adjustment:** begin assignments with OpenCode's documented native synthetic inbox (`delivery: queue`), preserving drafts rather than proving an empty composer. Verify this live before activation. Claude/Codex remain observable but non-dispatching until verified. Keep host-idle/role/runtime/ownership checks, pending human-input precedence, focus/gaming/sleep guards and bounded native delivery; if an idle check races execution, the native inbox queues without interruption. No pane typing, hidden draft inspection, invented native API, or bypass of approval/input dialogs.

**User-approved telemetry adjustment:** the private OpenCode runtime's plugin publishes raw native activity/inbox facts via existing runtime-authenticated agent IPC, not a new host API or a self-certified `ready` flag. The daemon derives availability and verifies the reporting runtime matches the worker. Telemetry truth is not independently host-proven; false availability may queue work while busy but cannot authorize scope, duplicate ownership, bypass permission/live-verification gates or interrupt native execution. Protected host-native observation remains a host-only path; the plugin uses a separate narrow raw-telemetry action and cannot submit a host observation or verification.

## File map and proposed interfaces

Existing names below were inspected; **new** files/types/actions below are proposed implementation interfaces, not claims they already exist.

| Files | Responsibility |
|---|---|
| New `apps/cockpit/src/chief.rs`, `chief/store.rs`, `chief/evidence.rs`; modify `src/lib.rs` | Typed requests; grants, briefs, claims, event/decision records; evidence gating |
| Modify `apps/cockpit/src/teams.rs`, `teams/store.rs`, `bin/cockpit-team.rs` | Nested chief actions; schema migration; shared receive/receipt transport and CLI |
| New `apps/osd/src/chief.rs`; modify `src/lib.rs`, `teams.rs`, `registry.rs`, `bin/osd.rs` | Peer-verified global access, authoritative readiness, event intake and recovery |
| Modify `apps/cockpit/src/app.rs`, `queue.rs`, `todo.rs`, `app/continuity_ui.rs` | Single scheduling owner, manual-action arbitration and todo/continuity projection |
| Modify `apps/cockpit/src/bin/ask-to-run.rs`, `bin/claude-run-approve.rs`, `teams.rs`; modify `config/bin/.local/bin/cockpit` | Optional evidence bindings; host result publication; structured QA results; reviewed apply identity |
| New `apps/cockpit/src/chief/view.rs`; modify `app.rs` only after Task 7's gate | Approved chief controls and short on-demand summaries |
| New `apps/cockpit/tests/chief_{protocol,store,dispatch,evidence,events,ui}.rs`, `tests/common/chief.rs`; new `apps/osd/src/chief_tests.rs` | Boundary tests and small shared fixtures |
| Existing `team_*`, `opencode_team.rs`, `native_codex.rs`, `cockpit_script.rs`, `approval_terminal.rs`, `claude_todo.rs` | Compatibility regression coverage |

Use `teams::{TaskKey, Role, VerifiedSession, TeamError, TeamReply, TransportOutcome}`; do not add Chief to `Role`.
Define the following new typed contract in `chief.rs` (all wire structs deny unknown fields):

- `WorkRef { task: TaskKey, revision: i64 }` and `WorkBrief { work: WorkRef, brief: String, role: Role, prerequisites: Vec<WorkRef>, gates: Vec<GateRequirement> }`.
- `GateKind::{Tests, Qa, Apply}`; `GateRequirement { kind: GateKind, source: String, expected: Value }`. Validate each kind's exact expected fields in `evidence.rs`; explicit not-applicable decisions are part of the approved brief, not mutable chief flags.
- `WorkerState { session: VerifiedSession, role: Option<Role>, ready: bool, blocked_by: Vec<String>, observed_at_ms: i64 }`. Only the daemon constructs readiness from host facts. A standalone role is explicitly host-bound; never trust an agent's role claim.
- `ChiefAgentAction::{Context { after: i64, limit: usize }, Dispatch { submission_key: String, work: WorkRef, worker: String }, Reprioritize { submission_key: String, ordered: Vec<WorkRef> }, Accept { assignment: String }, Status { assignment: Option<String>, summary: String }, Decision { work: Option<WorkRef>, key: String, question: String, recommendation: String }, Coordinate { submission_key: String, work: WorkRef, recipient: String, body: String }}`. Execution ruling: Coordinate needs the explicit key like other message submissions; identical content with a new key is an intentional new message, not a silently dropped replay.
- `ChiefHostAction::{Enable, Pause, Resume, Disable, ApproveWork, RevokeWork, BindWorker, Reconcile, Resolve, RequestSummary, AttestEvidence, ImportEvidence}`. Every variant has a `request_id`; mutating an existing record carries its expected revision/epoch. Enable takes an exact verified session; ApproveWork takes the complete hashed immutable brief, not just a todo id. Reconcile takes the assignment and handling/safe-stop evidence. AttestEvidence displays exact work revision, gate, source/result and commit before confirmation. ImportEvidence takes only `work: WorkRef`, `gate: GateKind` and `source_id: String`; the daemon resolves the host-owned source record, never trusts a caller-supplied success/result/path.
- `Evidence { id: String, work: WorkRef, kind: GateKind, commit: String, source: String, result: Value }`; source is exactly `host_run`, `qa`, `reviewed_apply` or `host_attestation`. Only authenticated approved sources or host approval may write it. The chief and worker cannot emit arbitrary evidence.

Wire additions: `AgentAction::Chief { request: ChiefAgentAction }`, `HostRequest::Chief { action: ChiefHostAction }`, and `HostRequest::ChiefSnapshot { after: i64, limit: usize }`. Use existing receive and receipt actions for delivery. Add `AgentAction::QaResult` for structured role-bound results; do not parse a `Findings` message for pass/fail.

Core **new** store signatures (responses use bounded JSON like the existing team store):
```rust
pub fn migrate(connection: &mut rusqlite::Connection) -> Result<(), TeamError>;
pub fn host(connection: &mut rusqlite::Connection, action: &ChiefHostAction,
    live: &[WorkerState], now_ms: i64) -> Result<TeamReply, TeamError>;
pub fn agent(connection: &mut rusqlite::Connection, sender: &VerifiedSession,
    action: &ChiefAgentAction, live: &[WorkerState], now_ms: i64) -> Result<TeamReply, TeamError>;
pub fn observe(connection: &mut rusqlite::Connection, live: &[WorkerState], now_ms: i64) -> Result<(), TeamError>;
pub fn claim(connection: &mut rusqlite::Connection, recipient: &VerifiedSession,
    idle_turn: &str, now_ms: i64) -> Result<Option<teams::Dispatch>, TeamError>;
pub fn finish_for(connection: &mut rusqlite::Connection, recipient: &VerifiedSession,
    dispatch_id: &str, outcome: &TransportOutcome, now_ms: i64) -> Result<(), TeamError>;
pub fn record_evidence(connection: &mut rusqlite::Connection, evidence: &Evidence,
    now_ms: i64) -> Result<(), TeamError>;
```
`record_evidence` is a Rust API reached only after source validation, not an exposed arbitrary agent action. Use explicit enum matches, transactions and existing hashing; no generic workflow engine.

### Task 1: Protected protocol and schema

**Files:** new `chief.rs`, `chief/store.rs`; modify `lib.rs`, `teams.rs`, `teams/store.rs`; new `tests/chief_protocol.rs`, `tests/chief_store.rs`, `tests/common/chief.rs`.

**Interfaces:** produce the typed contract and `migrate` above. Extend the existing store's migration to version 3 in one transaction; reject future versions and preserve version-2 data. New tables are prefixed `chief_`: grants, worker bindings, work revisions, assignments, events, decisions, evidence, and delivery attempts. Store immutable approval hashes, verified runtime/epochs, submission fingerprints, cursor sequences and action receipts. Unique partial indexes enforce one active chief, one open claim per work identity (across revisions), one open assignment per worker, and one in-flight delivery per recipient across chief traffic.

- [ ] Write protocol tests, including this exact rejection assertion; fixture builders create valid requests using the contract, not placeholder runtime strings in production.
  ```rust
  assert!(serde_json::from_value::<AgentAction>(json!({"action":"chief","request":{"action":"approve_work"}})).is_err());
  assert!(serde_json::from_value::<Role>(json!("chief")).is_err());
  ```
  Add `migration_preserves_v2_links_messages_and_spawns`, `migration_reopen_is_idempotent`, and `future_schema_is_refused`; assert old rows unchanged and no partial migration after injected failure.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test chief_protocol --test chief_store`; expect failures for the absent chief module/actions.
- [ ] Implement only serialization, validation and migration. Preserve existing private-directory/symlink checks. Limits: brief/body 4,000 characters, blocker 1,000, context page at most 50 records and below existing 65,536-byte agent reply bound; reject oversized single records rather than truncating approved briefs. Maximum 256 active approved work records and 100 open chief decisions; reject excess visibly.
- [ ] Repeat the focused command and `cargo test --manifest-path apps/Cargo.toml -p cockpit --test team_protocol --test team_store`; expect all pass.
- [ ] Commit only this task's explicit files: `feat: add protected chief protocol and state` (do not mark the feature todo fixed yet).

### Task 2: Verified chief grant and exact work authorization

**Files:** `chief/store.rs`; new `apps/osd/src/chief.rs`, `chief_tests.rs`; modify osd `lib.rs`, `teams.rs`, `registry.rs`; `tests/chief_store.rs`.

**Interfaces:** implement `store::host` and `store::agent` Context/Reprioritize; new daemon `chief::request(registry: &mut Registry, peer: u32, request: ChiefAgentAction) -> Result<TeamReply, TeamError>` and `chief::host(registry: &mut Registry, action: &ChiefHostAction) -> Result<TeamReply, TeamError>`. Reuse peer-runtime verification from `teams.rs`, exposing it `pub(crate)` rather than copying it. Chief state shares the opened team connection.

- [ ] Add table-driven boundary tests: ordinary agent global Context refused; selected chief sees bounded statuses from two projects; forged sender/runtime/approval fields refused; enable of an active team member refused; role binding requires host action; busy/unowned existing work remains observed, never reassigned. Snapshot excludes transcript/output/full_request/credential fields, even when the registry contains them.
- [ ] Add lifecycle tests for two-chief conflict, pause/resume, disable, same id/new runtime, daemon restart without team membership, and revision-bound revoke/edit. Assertion pattern: `assert_eq!(reply.result.unwrap_err().code, ErrorCode::Unauthorized);`. Grant runtime recovery must work for the chief and bound standalone workers, not only existing `members` rows.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p osd chief` and cockpit `--test chief_store`; expect failing grant/authorization tests.
- [ ] Implement the grant and explicit work approval; validate canonical project via existing host logic. Approve immutable brief+requirements hash only from host. Agent-originated queued todos remain unapproved. Reject dependency cycles, self-dependencies, missing revisions and changed briefs. Reprioritize only approved unclaimed records; no change to running assignments. Pause/absence does not relinquish scheduling ownership or fall back to legacy dispatch.
- [ ] Repeat both focused suites plus osd's existing team tests; expect pass. Commit: `feat: authorize one global chief and approved work`.

### Task 3: Atomic assignment and native-inbox delivery

**Files:** `chief/store.rs`, osd `chief.rs`, `teams.rs`, cockpit `teams.rs`, `bin/cockpit-team.rs`; new `tests/chief_dispatch.rs`; existing `team_watch.rs`, `opencode_team.rs`, `native_codex.rs`.

**Interfaces:** implement `agent` Dispatch/Accept/Status/Coordinate and `claim`/`finish_for`. Add nested `cockpit-team chief` JSON handling and a host-only `cockpit-team chief-host` JSON mode using existing host_request; the daemon still rejects sandbox peers regardless of the CLI command. Dispatch ids are durably namespaced so existing Receipt routes to exactly one store.

- [ ] Write concurrent/reopen tests for one task/two workers, two tasks/one worker, duplicate submission same/different contents, crash before claim/after irreversible claim/after receipt/before acceptance. Example invariant: `assert_eq!(active_assignments_for(&connection, &work), 1);` (test helper queries the protected fixture, not a production API). Unknown outcomes retain ownership. Acceptance is separate from transport receipt.
- [ ] Add tests for wrong project/role, stale readiness (>30 seconds), team mapped to another task, waiting-for-human, busy, queued human prompt, restart, asleep, and changed membership/runtime immediately before receive. Cross-project coordinate requires approved task/dependency relevance; arbitrary recipient/history probing returns no information.
- [ ] Run cockpit `--test chief_dispatch` and osd chief tests; expect assignment/delivery failures.
- [ ] Implement transactional claims and final revalidation. Standalone workers require explicit host role binding; team workers require the existing exact task/role approval. Add chief envelopes containing trusted approval revision/hash separately from untrusted commentary. Reuse `Receive`/`Receipt`; chief delivery and team delivery must mutually exclude in-flight and same-idle-turn claims. Route receipts by persisted dispatch ownership, not caller-supplied message type. Preserve bounded existing 2/10/30-second known-safe retry policy and uncertain-no-resend behavior.
- [ ] Exercise existing adapters with chief/assignment envelopes: native inbox admission once, stable ids, collaborator label, receipts, busy deferral and failure uncertainty. Do not add a second adapter polling loop. If any adapter cannot establish draft-safe native admission/task-boundary readiness, mark that adapter unavailable for automatic dispatch and surface the missing capability; do not substitute keystrokes. Keep a live-check gate before claiming all three apps supported.
- [ ] Repeat focused tests and `--test team_watch --test opencode_team --test native_codex --test team_store`; expect pass. Commit: `feat: dispatch chief assignments through verified inboxes`.

### Task 4: One scheduling owner and consistent todo projection

**Files:** cockpit `app.rs`, `queue.rs`, `todo.rs`, `app/continuity_ui.rs`; osd `chief.rs`, `registry.rs`; `tests/chief_dispatch.rs`, existing queue/todo tests.

**Interfaces:** add host snapshot fields `chief_scheduling_owner` and `chief_owned_work` (including paused/unavailable chief and unresolved assignments). New `chief::reconcile_todos(connection: &mut Connection, todos: &TodoList, now_ms: i64) -> Result<(), TeamError>` observes old ownership/status, never interprets a todo change as authorization/completion. Existing todo/continuity state is a projection, not a second dispatch ledger.

- [ ] Add regressions proving two sidebar instances cannot dispatch while chief scheduling owns the queue, auto-start handed work is suppressed, unavailable host state fails closed, and disable does not resurrect/reassign outstanding chief claims. Manual handoff of a chief-owned task must use host claim arbitration or show the existing owner; no bypass through `give_todo`/queue UI. Preserve normal legacy behavior when no chief grant/claims have ever owned the affected work.
- [ ] Add crash/projection tests: SQLite claim succeeds and todo save fails; restarting repairs display without another assignment; agent marks done but completion gates remain pending; agent-added `--after` release does not dispatch unapproved work. Existing doing tasks are never adopted as freely dispatchable work.
- [ ] Run chief dispatch, existing todo and queue tests; expect arbitration/projection failures.
- [ ] Implement authoritative ownership checks before sidebar queue dispatch and handed-work spawn; fail closed on unknown chief ownership. Preserve human queued prompts, viewed-session exclusion, existing 300-second idle guard, gaming pause and sleep rules. Publish fresh host-side focus/pending-input readiness to the daemon; missing/expired readiness is not eligible. Wire a production OpenCode plugin raw-telemetry producer over existing authenticated IPC, with the daemon deriving availability (Theo approved this source-trust adjustment); no arbitrary ready flag, source impersonation, host observation/verification action or additional permissions from telemetry. Use documented native queued admission and preserve drafts instead of requiring composer-empty proof; keep unverified Claude/Codex adapters unavailable. The sidebar cannot invent native capabilities on an adapter's behalf. Live draft/queue verification remains required before assignments are activated. Never let two JSON writers overwrite each other's todo state: use existing update path and re-read before projection.
- [ ] Repeat tests and osd tests; expect pass. Commit: `feat: arbitrate chief and legacy task scheduling`.

### Task 5: Evidence-bound completion and dependency release

**Files:** new `chief/evidence.rs`; `chief/store.rs`, `teams.rs`, `teams/store.rs`, osd `chief.rs`; `bin/ask-to-run.rs`, `bin/claude-run-approve.rs`, `bin/cockpit-team.rs`; `config/bin/.local/bin/cockpit`; new `tests/chief_evidence.rs`; existing approval/apply/QA tests.

**Interfaces:** implement `record_evidence`; new `evidence::validate(requirement: &GateRequirement, evidence: &Evidence) -> Result<(), TeamError>`, `store::complete_ready(connection: &mut Connection, now_ms: i64) -> Result<Vec<WorkRef>, TeamError>`. Define `QaVerdict::{Pass, Findings}` and `QaResult { submission_key: String, task: TaskKey, handoff_id: String, verdict: QaVerdict, tests: Vec<String>, findings: String }`: exact approved QA runtime/epoch and commit are resolved by the daemon from the handoff. Structured QA reports attest the reviewer result, not command execution; their test descriptions do not satisfy the Tests gate. Keep legacy free-text Findings compatible but non-gating.

- [ ] Add tests for each required gate: missing/failed/wrong revision/wrong commit/unapproved source; forged worker/chief evidence; duplicate conflicting evidence; superseded QA; not-applicable cannot be introduced after approval; independently done todo; dependency edited after approval; failure after source patch application; and invalidation after dependent work was queued or began. Assertion: completion returns `[]` until all exact required gates pass; then returns that work once without a host confirmation action.
- [ ] Run `--test chief_evidence`, `--test approval_terminal`, `--test cockpit_script`; expect evidence/completion failures, not an unrelated environment failure.
- [ ] Add an optional exact assignment/gate binding to an ask-to-run request, shown alongside its existing approval. A host-approved run publishes only id, verified work revision, effective command fingerprint, cwd identity, host-observed exit code and reviewed work identity to an atomic protected source record. Import it through host-only ImportEvidence; after a daemon restart, replay pending protected source ids idempotently. Validate Tests expectations as exactly command fingerprint, canonical protected cwd and approved reviewed revision; exit must be zero and before/after identities unchanged. Edited commands mismatch the approved gate unless Theo explicitly approves the revised binding. Declined, abandoned or unknown executions cannot pass. Require a protected reviewed checkout identity before/after the run; do not run host git in a sandbox-writable clone or automatically launch any command. For unsupported evidence paths, use the spec's explicit missing-evidence decision/host attestation, never a successful-looking log parser.
- [ ] Add structured QA intake authorized only for the current approved QA/handoff. Reuse existing handoff supersession and retain historical evidence; changes invalidate current gates. Ordinary agent actions cannot use the host evidence publisher.
- [ ] In the existing reviewed apply workflow, bind approved work to the reviewed source patch series and resulting protected live commits. Since git am changes commit ids, record the host-observed patch correspondence rather than requiring equal source/live SHAs. Emit successful apply evidence only after required after-apply/install succeeds; its failure remains failed even if trailers already say done. Carry result through daemon restarts via durable host evidence/outbox and idempotent ingestion. Never trust a clone's claimed destination or patch-to-work mapping without the displayed approval.
- [ ] Implement automatic gate evaluation in a transaction; release task ownership/dependencies only for the approved revision with complete evidence, and worker capacity only after safe-idle. No new privileged test runner or generic CI integration. Missing evidence creates one decision, not recurring approvals for completed tasks.
- [ ] Complete host-only Reconcile alongside the claim lifecycle: handled preserves assignment ownership; safe-stop cancellation requires confirmation the previous worker cannot execute it, cancels every unsent delivery and permits only a fresh-key dispatch after resolution. Never release uncertain ownership from a chief/worker message. Accept exact completed prerequisite revisions in approved new briefs; historical or invalidated revisions do not count.
- [ ] Repeat focused tests plus existing team handoff tests. Commit: `feat: advance chief work from recorded completion evidence`. Explicitly flag the approval binary and `config/bin/.local/bin/cockpit` host-rights changes for review.

### Task 6: Bounded observation, decisions and on-demand briefings

**Files:** `chief.rs`, `chief/store.rs`, osd `chief.rs`, `registry.rs`, `bin/osd.rs`; new `tests/chief_events.rs`; osd `chief_tests.rs`.

**Interfaces:** implement `observe`, agent Decision and host Resolve/RequestSummary. New daemon `chief::tick(registry: &mut Registry, now_ms: i64) -> Result<(), TeamError>` runs from the existing queue timer for deadlines/cooldowns, never to generate periodic LLM turns. Context cursors are monotonically sequenced and paginated; a missed retention window requires an explicit bounded snapshot.

#### Stage 6A: Required host-review integration from the approved Designer handoff

The Designer's approved artifact is `docs/superpowers/specs/2026-10-23-chief-controls-interaction-design.md`, references F1–F7 plus its follow-up choices/state contract. It supplies concrete approved UI evidence without modifying source. Implement this backend-only stage and review it separately before Stage 6B; it fills missing paths, not optional UI features.

- [ ] Add immutable agent-prepared exact work proposals with sender provenance, protected canonical project/revision/hash validation, bounded intake and no approval inheritance. A verified ordinary worker may propose only within its own canonical project; the active chief may propose across projects. Proposal text, roles, gates and prerequisites are untrusted until host approval; only the existing exact host approval action can grant work. Changed proposals create new immutable identities and invalidate frozen selection; never approve todo text or later backlog implicitly.
- [ ] Expand the host-only review snapshot with full verified identities/epochs/team bindings/blockers, immutable pending proposals and approved brief payloads/hashes, versioned decisions, assignment recovery identities, exact gate/evidence source records and summary request/result state. Keep pagination, escaped rendering boundaries and size limits; never expose these global records to ordinary agents. Host reads return structured records rather than reparsing prose.
- [ ] Reconcile claimed-work revocation with the approved spec: cancel conclusively unsent work, retain running/uncertain ownership and emit one durable host-authored stop-at-safe-boundary notice to the exact current worker. The notice is not product authorization, kill or rollback; it must remain deliverable under revoked work without reviving work permissions. Late receipts and chief replacement cannot resurrect cancelled work or drop ownership.
- [ ] Honor the Designer's subsequently approved stricter evidence safeguard: Attest is allowed only for a gate approved with `source: host_attestation`. The host must not substitute an attestation for `host_run`, `qa` or `reviewed_apply`; unsupported provenance requires a revised explicit work approval, not a silent fallback. Update the prior fallback regression to assert this restriction without disabling it.
- [ ] Test proposal provenance/limits/foreign-project rejection/stale approval, complete bounded host-only review records, claimed revocation queued/in-flight/accepted paths and late receipts, and exact attestation-source restriction. Focused RED/GREEN, required full suite and separate commit/review before 6B. No UI code in this stage.

#### Stage 6B: Event-driven coordination

- [ ] Add fake-clock tests: 100 equivalent status events coalesce; no change yields zero wake-ups; one chief idle turn accepts at most 20 mutating actions and 50 event records; cooldown is 30 seconds and persists across reopen. Context reads and receipts cannot reset budgets. Pause/disable prevents wake-up; new event during a chief turn is available at next safe boundary; no failed-action reasoning loop.
- [ ] Add decisions tests: repeated same blocker/revision produces one decision and one attention transition; new evidence updates it; only host Resolve answers it; host approval prompts remain visible/actionable and never get auto-answered. Capacity/no-approved-work is silent. Chief loss or uncertain delivery is one actionable decision. More than 100 open decisions fails visibly without evicting unresolved ones.
- [ ] Run cockpit `--test chief_events` and osd chief tests; expect failing coalescing/budget/attention assertions.
- [ ] Implement source-labelled redacted snapshot and material-event comparison; never export registry request/detail/full_request fields. Summaries requested by host create one bounded chief request and return short completed/active/decisions/idle sections with evidence references and a clear unverified label. No automatic progress notifications. Host-generated critical failures must remain visible even if the chief is unavailable or suppresses them.
- [ ] Repeat focused tests plus daemon banner/notification tests; expect pass. Commit: `feat: coalesce chief work and decision notifications`.

### Task 7: Approved cockpit controls — design gate before UI implementation

**Files:** new `chief/view.rs`, `tests/chief_ui.rs`; modify `app.rs`, `lib.rs`, existing `tests/team_ui.rs`, `tests/sidebar_refresh.rs`. Exact interaction design/frame references must be appended after approval; no FigCraft file was found in this checkout during planning.

**Interfaces:** UI calls `client::host_request` with the defined Chief host actions; never writes chief SQLite directly. New `ChiefView::new(snapshot: Value) -> ChiefView`, `ChiefView::attention(snapshot: &Value) -> usize`, `ChiefView::detail(&self) -> Vec<String>` follow existing plain-text TeamView conventions. These names specify data boundaries, not an approved screen layout.

**Design gate satisfied by Designer handoff:** `docs/superpowers/specs/2026-10-23-chief-controls-interaction-design.md` states Theo approved the assembled design after its wireframes, individual choices and builder recheck. Use references F1–F7, the approved navigation tree and concrete state contract. The Task 7 designer-recruitment/drafting steps below are complete; do not recruit another Designer or redesign the approved controls. Missing task messaging link does not grant relinking authority. Product implementation was already authorized by Theo in this session; the artifact supplies design evidence, not extra permissions.

**Exact approved interaction/test addendum:** separate Chief view from existing action menu (no global shortcut); root Decisions/Work/Workers and roles/Chief controls/Request summary. Work has Awaiting approval and Approved; Space selects exact proposals, `a` freezes full batch review. Lists use j/k/arrows/Enter/Esc; full review scrolls PgUp/PgDn, Enter opens a distinct confirmation, only `y` there submits, `n`/Esc returns without submission. Changed/disappeared records cannot retarget an action or inherit consent. Every runtime/grant/revision/hash/source is fully inspectable, escaped and scrollable on narrow screens. Note entry is not consent; safe-stop and host attestation each need a separate initially unchecked Space-toggle assertion before review. Import selects a protected source record, never a caller path/id editor. Attest is restricted to approved host_attestation gates. Summary is explicitly requested once, retains timestamp and never auto-regenerates. Tab/BackTab keep ordinary panel navigation and never submit a suspended confirmation. Batch partial failure reports each actual result, never claims atomic success or blind retry. Transfer all seven test groups from the artifact's Required screen/interaction test evidence section into the actual UI tests; use no unapproved layout or key choices.

- [ ] **Stop before UI implementation.** Request a Designer via the cockpit team flow and wait for Theo's recruitment approval. Offer the visual companion for mockups. If FigCraft is needed, follow its workspace/readiness and restart workflow; do not open it unannounced. Approve wireframes/controls for selecting chief, pause/resume/disable, exact brief batch approval/revocation, worker role binding, decisions/recovery and on-demand summary. Use the UI skills and design workflow then; this backend plan is not design evidence.
- [ ] Append approved design references, exact controls and key/focus behavior, then concrete screen tests here. A fresh builder must be able to implement without choosing a layout. This step must be reviewed before proceeding; until then this task is intentionally blocked, not implementation-ready.
- [ ] Write `chief_controls_show_exact_brief_revision_and_scope_before_approval`, `old_confirmation_cannot_approve_new_revision`, `chief_decisions_preserve_selection_and_human_prompts`, `paused_and_unavailable_states_are_not_off`, `summary_request_does_not_enable_progress_notifications`, and control-character/long-project/empty-list rendering cases. Assert escaped text, full ids available and host-action payloads matching displayed revision/hash. Keyboard tests use approved interactions, not newly guessed bindings.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test chief_ui --test team_ui --test sidebar_refresh`; expect new UI assertions to fail before implementation.
- [ ] Implement only the approved controls using existing Ratatui/plain-text view patterns, preserving ordinary focus/navigation. Show failed storage/delivery as errors, not optimistic success. Attention reflects unresolved decisions, not ordinary successful progress.
- [ ] Repeat focused tests; expect pass. Commit: `feat: add approved global chief controls`.

### Task 8: Whole-feature verification, docs and reviewed apply

**Files:** existing protocol/store/adapter/apply/todo tests, `README.md`; all actual host-running changes listed individually in the handoff. Do not modify Makefile, dependency manifests or global instructions incidentally.

**Interfaces:** no new APIs; verify the feature through host enable → exact work approval → chief dispatch → worker accept → structured evidence → completion → dependent dispatch → one needs-you decision → host resolution → disable. Include two different projects and mixed supported agents.

- [ ] Write end-to-end boundary tests for the full sequence and restart at each irreversible transition, with fake approved command results and no real host execution. Run focused chief+team+adapter suites and confirm all pass. Add missing spec regressions rather than disabling failing existing tests.
- [ ] Load the OSS documentation skill before updating README: document enabling, approving a bounded backlog, compatible-agent limitations, evidence provenance, decisions-only notifications, recovery and disable. Do not promise generic verified sandbox tests or all-app live support without evidence.
- [ ] Once execution is explicitly authorized, run `make test` from the clone (it includes builds); record the command and full test outcomes verbatim, including failures. No build/test was run while writing this plan. Inspect `git diff --check`, status and diff stat; list changed host-running files. Request whole-branch review according to the selected execution workflow.
- [ ] After required tests/review pass, commit final requested work with `Todo: 5fa54c26`. Export using `git format-patch origin/main -o .outgoing`; use `ask-to-run --why 'This applies the reviewed chief changes and reloads the cockpit. Review the patch list before approving; it may rebuild and restart cockpit services.' --cwd <clone-root> 'cockpit apply <clone-root>'`. Do not push or directly edit the running copy.
- [ ] Through `ask-to-run`, run `make -C ~/agent-cockpit test` after apply. A live two-project/three-app interaction check needs the user's readiness first and guided steps where necessary. Prove no duplicate wakes, unchanged permission prompts and no legacy auto-spawn under chief ownership. If blocked, report the exact missing capability rather than weaken boundaries.
- [ ] Run `cockpit-todo list`; mark `5fa54c26` done only after implementation, verification and reviewed application succeed. Report what changed/what stayed/test output and any unverified adapter or evidence source. Design/plan alone do not close this todo.

## Self-review and handoff status

Spec coverage: lifecycle/visibility (Task 2), work authorization and priorities (2), atomic dispatch/coordination/delivery (3), old scheduler/manual actions/todo claims (4), completion/evidence/dependencies (5), events/decisions/summaries (6), host controls/design (7), regression/live/apply workflow (8).

Review Focus conditions are pinned to the named tasks' tests. Contract types and new entry points are defined above; implementers must preserve those names or update every consumer/test together. Numeric bounds are explicit builder choices within the approved spec, not extra user decisions.

**Design status:** Task 7 now has an approved Designer artifact and concrete UI-test addendum above. Stage 6A fills its identified required backend intake/review/revocation gaps before UI implementation. Live OpenCode admission verification and normal whole-feature review/apply remain release gates; the design artifact does not certify those tests or grant extra permissions.
