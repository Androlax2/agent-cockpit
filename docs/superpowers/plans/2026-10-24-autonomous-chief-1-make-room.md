# Autonomous Chief, plan 1 of 5: make room — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the Chief's work-dispatching machinery so the todo queue runs while the Chief is on, keeping enable/disable, admission, readiness, wake delivery, summaries and decisions.

**Architecture:** Delete code paths, not tables: removed tables stay in the schema unused (`user_version` stays 10) and are dropped in plan 2's migration. An idempotent cleanup in `migrate` clears leftover rows from the 2026-10-24 test run. Legacy queue infrastructure (`scheduling_reservations` owner legacy/observed, `legacy_launches`, `ReconcileLaunch`, host-launch decisions) stays.

**Tech Stack:** Rust (crates `cockpit`, `osd` in `apps/`), rusqlite, bash (`config/bin/.local/bin/cockpit`), `make test`.

**Spec:** `docs/superpowers/specs/2026-10-24-autonomous-chief-design.md` (section "What changes in the existing Chief"). Inventory with line references: `docs/superpowers/plans/2026-10-24-autonomous-chief-1-inventory.md` (§1 removed code, §4 tests), taken at `d18e509`; re-check line numbers before editing.

## Global Constraints

- Enabling the Chief must never pause the todo queue (spec anti-goal).
- Keep: verified OpenCode delivery path and receipts; admission verification; readiness checks incl. visible-session guard (`fd4f35e`) and focus publishing (`baafca2`); Enable/Pause/Resume/Disable; RequestSummary/SummaryResponse; decisions + Resolve; ReconcileDelivery; ReconcileLaunch.
- Remove: work proposals, ApproveWork/RevokeWork, approval hashes, evidence gates/attestation/import, QA-result evidence, Chief assignments (Dispatch/Accept/Reprioritize/Coordinate, assignment Status), prerequisites, `BindWorker`/bindings, Chief scheduling ownership.
- Keep assignment-less `ChiefAgentAction::Status { summary }` (field `assignment` removed).
- No schema version change; removed tables untouched except the row cleanup in Task 5.
- `config/bin/.local/bin/cockpit` runs outside the sandbox with Theo's rights: its diff must be called out to Theo at apply.
- `make test` green after every task. No `Todo:` trailer in this plan: `b77a0881` stays open until plan 5.
- Out of scope here (plan 2): the Chief's conversation read access, and exempting the Chief's own turns from the 300 s `QUEUE_IDLE_S` guard.

## Review Focus

1. A Chief grant that is active or paused while the todo queue has ready work: the queue still launches (owned by Task 1 regression test).
2. A database from the 2026-10-24 run with Chief-owned reservations, open `missing_evidence`/`host-assignment-*` decisions and queued non-wake inbox rows: after upgrade nothing blocks the queue and no stale banner stays (Task 5 test).
3. Daemon unreachable: the queue still refuses to launch (fail closed), with a message that no longer mentions Chief ownership (Task 1 test).
4. A todo item in `todos.json` still carrying a `chief_assignment` field: status changes are allowed and the field is ignored (Task 1 test).
5. A stuck legacy launch with the Chief never enabled: its decision still appears and `ReconcileLaunch` still resolves it from the Chief view (kept test `unknown_legacy_launch_keeps_ownership_until_exact_host_safe_stop`, plus Task 2 view test).

---

### Task 1: The todo queue runs while the Chief is on

**Files:**
- Modify: `apps/cockpit/src/chief/store.rs` (`reserve_legacy_kind` ~1713-1720, `require_unreserved` ~1698-1707, `reconcile_todos` ~1779, remove `scheduling_owned` ~528-531 and `owned_work` ~1785-1801 callers outside assignment code)
- Modify: `apps/osd/src/teams.rs` (`host_snapshot` ~400-401: drop `chief_scheduling_owner`, `chief_owned_work`)
- Modify: `apps/cockpit/src/queue.rs` (field `chief_scheduling_owner`, `QueueBlock::ChiefOwned`, `OwnershipUnknown`)
- Modify: `apps/cockpit/src/app.rs` (~919-921 `project_chief`, ~1125, ~1156-1161 `next_queued`, ~3140, ~3160), `apps/cockpit/src/app/continuity_ui.rs` (~219, ~235-238), `apps/cockpit/src/todo.rs` (~404, ~574-601, ~766-813 `chief_controls`)
- Test: `apps/cockpit/tests/chief_dispatch.rs`, `apps/cockpit/src/tests.rs`, `apps/osd/src/chief_tests.rs`

**Interfaces:**
- Produces: `QueueContext` without `chief_scheduling_owner`; `QueueBlock` without `ChiefOwned`/`OwnershipUnknown`; `reserve_legacy_kind(connection, task, worker, manual, now_ms)` no longer consults the grant or `chief_work_revisions`; queue blocks with new `QueueBlock::HostUnavailable` when the team host cannot be reached.

- [ ] **Step 1: Write the failing tests**
  - `chief_dispatch.rs`: `an_active_chief_does_not_pause_legacy_reservations` — setup with grant active (and again paused): `store::reserve_legacy(&mut c, &TaskKey{project:"path:/project-a".into(), task_id:"cccccccc".into()}, None, NOW)` is `Ok`.
  - `osd/src/chief_tests.rs`: `an_enabled_chief_leaves_reserve_task_and_reserve_manual_available` — through `crate::teams::host(... HostRequest::ReserveTask{automatic:false,..})` and `ReserveManual` with grant active: both `Ok`.
  - `src/tests.rs`: invert `chief_two_sidebars_yield_both_queue_sends_and_handed_work_spawns` into `an_enabled_chief_keeps_the_todo_queue_sending` (fake host snapshot without `chief_owned_work`; `app.dispatch_queue()` sends the queued todo); `a_todo_with_a_leftover_chief_assignment_field_can_change_status` (item JSON with `"chief_assignment":"x"`, `set_status` succeeds); rename the unknown-host test to `an_unreachable_team_host_blocks_queue_sends` asserting `QueueBlock::HostUnavailable`.
- [ ] **Step 2: Run them, verify they fail** — `cargo test --release --manifest-path apps/Cargo.toml -p cockpit --test chief_dispatch an_active_chief_does_not_pause` → Err "chief owns scheduling"; same for the others.
- [ ] **Step 3: Implement** — delete the ownership/protected checks in `reserve_legacy_kind`; drop owner `chief` handling from `require_unreserved` and `reconcile_todos`; remove the snapshot fields and every consumer (queue, app, continuity_ui, todo projection, `chief_controls`); `next_queued` no longer requires `chief_owned_work`. Invert `legacy_reservations_are_shared_by_sidebars_and_chief_dispatch` and `disabling_a_chief_does_not_release_its_unresolved_scheduling_ownership` to the new rule; delete projection tests `src/tests.rs` 7372, 7420, 7447, 7484, 7504 and `chief_archived_resume_cannot_bypass_the_scheduling_owner` (replace with a resume-allowed assertion). Dispatch code may still compile against reservations; it is removed in Task 4.
- [ ] **Step 4: Run** — `make test` → all pass.
- [ ] **Step 5: Commit** — `fix: keep the todo queue running while the chief is on`

### Task 2: A simpler Chief view

**Files:**
- Modify: `apps/cockpit/src/chief/view.rs` (pages ~34-36, `Intent::Attest`, `selected_proposals`, rows ~210-271, handler ~331-340, ~300-312, ~450-458, ~130-132, ~233-242), `apps/cockpit/src/app.rs:95` palette text
- Test: `apps/cockpit/tests/chief_ui.rs`

**Interfaces:**
- Produces: root rows exactly `Decisions`, `Workers`, `Chief controls`, `Request summary`; `Page` without `Work, Pending, Approved, WorkActions, Gates, GateActions, Sources, WorkerActions, Roles`; Workers is read-only (Enter shows detail, no actions).

- [ ] **Step 1: Write the failing test** — `the_chief_root_offers_only_decisions_workers_controls_and_summary`: render root, assert rows equal `["Decisions","Workers","Chief controls","Request summary"]` and no text contains "Work" as a row, "Approve", "Attest" or "Bind role".
- [ ] **Step 2: Run, verify it fails.**
- [ ] **Step 3: Implement** — remove listed pages, intents, state and handlers; Recovery keeps inbox and legacy-launch targets only. Delete `chief_ui.rs` tests 84, 111, 136, 143, 222, 235, 247, 258, 273, 303, 333, 342, 350, 382, 404, 415, 446, 560, 568; fix fixtures of kept tests that used proposal records.
- [ ] **Step 4: Run** — `cargo test --release --manifest-path apps/Cargo.toml -p cockpit --test chief_ui` → pass; then `make test`.
- [ ] **Step 5: Commit** — `refactor: reduce the chief view to decisions, workers, controls and summary`

### Task 3: Remove evidence producers and QA-result evidence

**Files:**
- Delete: `apps/cockpit/src/chief/evidence.rs`, `apps/cockpit/tests/chief_evidence.rs`
- Modify: `apps/cockpit/src/bin/ask-to-run.rs` (`--evidence` ~26, ~125-133, ~155), `apps/cockpit/src/bin/cockpit-run-approve.rs` (~77-98, ~118-124, ~151-158, ~187-191), `apps/cockpit/src/bin/cockpit-team.rs` (HELP 9-14/34-36, `qa-result` 42/55, `evidence-apply-*` 166-184, confirm list 214-215 keeps ReconcileDelivery/ReconcileLaunch), `apps/cockpit/src/teams.rs` (`AgentAction::QaResult`, `QaResult`, `QaVerdict`, `qa_brief_text` structured branch), `apps/cockpit/src/teams/store.rs` (`bind_handoff` 718, `complete_ready_in` 721/760, `qa_result` 726-763, `structured_qa_required` 792-793), `config/bin/.local/bin/cockpit` (`apply_reviewed` second argument, ~2504-2509, ~2541-2548, ~2571-2573, dispatch ~2785)
- Test: `apps/cockpit/tests/cockpit_script.rs`, `apps/cockpit/tests/team_store.rs`, `apps/osd/src/chief_workflow_tests.rs`

**Interfaces:**
- Produces: `ask-to-run` rejects `--evidence` as an unknown option; `cockpit apply <clone>` takes one argument; `cockpit-team` has no `qa-result` or `evidence-apply-*` verbs; spawned QA reports findings only.

- [ ] **Step 1: Write the failing tests** — `cockpit_script.rs`: `apply_takes_only_the_clone` (`cockpit apply <clone> extra` exits non-zero with usage); `approval_terminal.rs` or `run_requests.rs`: `ask_to_run_rejects_the_removed_evidence_option`; `chief_workflow_tests.rs`: keep `ordinary_spawned_qa_keeps_findings_only_reporting_and_automatic_exit` and add `a_chief_never_requires_structured_qa` (generated QA brief contains no `qa-result`).
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement** — remove the listed code; delete tests `cockpit_script.rs:4149`, `approval_terminal.rs:29`, `team_store.rs` 10-57, `chief_workflow_tests.rs` 432, 744, 775 and helpers `proposal`, `approve`, `bind`, `queued_workflow`, `GitBoundary` once unused.
- [ ] **Step 4: Run** — `make test` → pass.
- [ ] **Step 5: Commit** — `refactor: remove chief evidence producers and structured QA results`

### Task 4: Remove the dispatch protocol and store code

**Files:**
- Modify: `apps/cockpit/src/chief.rs` (remove `WorkRef`, `WorkBrief`, `WorkProposal`, `GateKind`, `GateRequirement`, `Evidence`, `source()`, `review_payload()`, `MAX_ACTIVE_WORK`; variants listed in Global Constraints; `Decision.work` field; `SummaryResponse.evidence_ids`; `Status.assignment`)
- Modify: `apps/cockpit/src/chief/store.rs` (functions listed in the inventory §1; strip `observe`, `recover`, `observe_coordination`, `claim`, `finish_for`, `agent`, `event_context`, `context_snapshot`, `context_records`, `review_records`, `is_product_worker`, `runtime`, `worker_role`, `decision_record` callers pass no work/assignment; `schedule_wake` text without "evidence")
- Modify: `apps/cockpit/src/teams.rs` (`envelope_text`: drop `host_stop_notice` and `chief_assignment`/`chief_coordinate` branches; reword `chief_wake` text), `apps/osd/src/chief.rs` (drop ProposeWork/ApproveWork checks; `host_role_unbound` no longer added to `blocked_by`), `apps/osd/src/teams.rs` (drop `replay_sources`/`complete_ready` on open 41-44; task-id validation at ~422 and ~446)
- Create: `TaskKey::validate(&self) -> Result<(), TeamError>` in `apps/cockpit/src/teams.rs` (8 lowercase hex or `owner/repo#N`, ≤256 chars, non-empty project ≤300) replacing `WorkRef::validate` in osd.
- Test: `chief_protocol.rs`, `chief_store.rs`, `chief_events.rs`, `chief_dispatch.rs`, `common/chief.rs`, `osd/src/chief_tests.rs`, `osd/src/chief_workflow_tests.rs`

**Interfaces:**
- Consumes: Task 1's queue without ownership.
- Produces: `ChiefAgentAction = Context | ContextSnapshot | SummaryResponse{request_id, sections} | Decision{key, question, recommendation} | Status{summary}`; `ChiefHostAction = Enable | Pause | Resume | Disable | Resolve | RequestSummary | ReconcileDelivery | ReconcileLaunch`; `TaskKey::validate`.

- [ ] **Step 1: Write the failing tests**
  - `chief_protocol.rs`: `removed_dispatch_actions_no_longer_parse` — `serde_json::from_value::<ChiefAgentAction>` of `{"action":"propose_work",..}`, `dispatch`, `accept`, `coordinate`, `reprioritize` all `Err`; `ChiefHostAction` of `approve_work`, `revoke_work`, `attest_evidence`, `import_evidence`, `bind_worker`, `reconcile` all `Err`.
  - `teams.rs` unit test: `task_keys_accept_todos_and_issues_only` (`"aaaaaaaa"` ok, `"org/repo#1"` ok, `"AAAAAAAA"`, `"abc"`, `"org/repo"` err).
  - `osd/src/chief_tests.rs`: `a_standalone_session_is_ready_without_a_bound_role` (fresh facts, admission verified, no team: `blocked_by` empty).
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement** — remove per the inventory; rewire kept tests that used `Status{assignment:None}` to `Status{summary}`, swap `Dispatch`/`ApproveWork` fixtures in `chief_events.rs` 625, 636, 712, 754 for decisions or worker records; delete tests listed in the inventory §4 for `chief_dispatch.rs`, `chief_events.rs`, `chief_store.rs`, `chief_protocol.rs`, `chief_tests.rs` (175, 398, 450, 870, 271) and `chief_workflow_tests.rs` (322, 361, 407); keep the legacy-launch, CLI-route and wake-recovery tests listed there.
- [ ] **Step 4: Run** — `make test` → pass; `cargo build --release --manifest-path apps/Cargo.toml` shows no dead-code warnings for removed items.
- [ ] **Step 5: Commit** — `refactor: remove chief work dispatch, assignments and bindings`

### Task 5: Clean leftover data and rewrite the docs

**Files:**
- Modify: `apps/cockpit/src/chief/store.rs` (new `pub fn clear_dispatch_leftovers(transaction: &rusqlite::Transaction) -> Result<(), TeamError>` called from `teams::store::migrate` on every open)
- Modify: `apps/cockpit/src/todo.rs` (loading drops a `chief_assignment` field), `README.md` (rewrite the "Global dispatcher Chief" section ~181-368 to ~60 lines: what the Chief does now, enable/disable, readiness, summaries, decisions; link the design)
- Test: `apps/cockpit/tests/chief_store.rs`

**Interfaces:**
- Produces: `clear_dispatch_leftovers` deletes `scheduling_reservations` rows with owner `chief`; marks open `chief_decisions` whose `work` or `assignment` is non-null, or whose key starts with `host-assignment-`, as `superseded`; marks `chief_inbox` rows of kinds other than `chief_wake` in states queued/submitting/uncertain as `cancelled`. Idempotent.

- [ ] **Step 1: Write the failing test** — `upgrading_clears_dispatch_leftovers`: seed a migrated DB with a chief-owned reservation, an open `missing_evidence` decision with `work` set, and a queued `chief_assignment` inbox row; run `teams::store::migrate`; assert no chief reservation, the decision state `superseded`, the inbox row `cancelled`, and `store::reserve_legacy` for that task `Ok`; running migrate twice changes nothing.
- [ ] **Step 2: Run, verify it fails.**
- [ ] **Step 3: Implement** `clear_dispatch_leftovers`; strip `chief_assignment` on todo load; rewrite README section.
- [ ] **Step 4: Run** — `make test` → pass.
- [ ] **Step 5: Commit** — `chore: clear chief dispatch leftovers and document the simpler chief` with trailer `Todo: b77a0881` omitted (plans 2-5 remain); host apply via `cockpit apply`, calling out the `cockpit` script diff to Theo, then `make -C ~/agent-cockpit test`.
