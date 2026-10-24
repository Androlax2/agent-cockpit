# Plan 1 inventory: what to remove and keep (read-only survey at `d18e509`)

Line numbers are approximate; re-check before editing.

## §1 Removed code

- `apps/cockpit/src/chief.rs`: `WorkRef` (20-23, 182-193), `WorkBrief` (27-33, 195-223), `WorkProposal` (38-51), `GateKind` (55), `GateRequirement` (59-64), `Evidence` (128-135, 303-315), `source()` (165-170), `review_payload()` (139-143), `MAX_ACTIVE_WORK` (12). `ChiefAgentAction`: remove ProposeWork, Dispatch, Reprioritize, Accept, Coordinate; `Status` loses `assignment`; `Decision` loses `work`; `SummaryResponse` loses `evidence_ids`. `ChiefHostAction`: remove ApproveWork, RevokeWork, Reconcile, AttestEvidence, ImportEvidence, BindWorker.
- `apps/cockpit/src/chief/evidence.rs`: whole file.
- `apps/cockpit/src/chief/store.rs`:
  - `host_action` arms: BindWorker 98-117, ApproveWork 118-135, RevokeWork 136-139, AttestEvidence 140-144, ImportEvidence 145, Reconcile 146-163, assignment branch of ReconcileDelivery 177-182; simplify Resolve event 193-195; request-id match 32-35; error text 64-65.
  - Delete functions: 215-288 (`cancel_stopped_inbox`, `delivery_assignment`, `current_briefs`, `revoke_work`); 303-526 (`publish_source`, `import_source`, `replay_sources`, `material_event`, `record_evidence`, `record_evidence_in`, `complete_ready`, `complete_ready_in`, `latest_gate_evidence`, `current_evidence`, `bind_handoff`, `invalidate_dependents`, `reaches_task`, `require_unclaimed`, `invalidate`); 528-531 `scheduling_owned`; 549-637 (`approved_work`, `ready_worker`, `worker_epoch`, `dependencies_complete`, `dispatch`, `queue_inbox`); 653-725 (`record_status` assignment part, `worker_action`, `coordinate`); 944-1008 `validate_inbox` (non-wake kinds become Obsolete); 1359-1370 `summary_reference_is_current`; 1610-1650 (`latest_revision`, `propose_work`); 1785-1801 `owned_work`.
  - Strip: `observe` 730, 735-747, 749 (keep 750-752); `recover` 761; `observe_coordination` 904-909; `claim` 1036/1054; `finish_for` 1078-1086, 1115-1146 (keep wake branch 1090-1114); `agent()` 1150-1151, 1158-1167, 1172-1186, 1214, 1219, 1224-1248, `chief_submissions` 1251-1253; `event_context` 1282-1308; `context_snapshot` 1419; `context_records` 1424-1427, 1434-1441, 1447-1454; `review_records` CTEs `selected_proposals`/`selected_assignments`/`selected_work`, categories binding/proposal/work/assignment/source/evidence, 1562-1585, 1590; `is_product_worker` 16-19 assignment/binding clauses; `runtime()` 534-538 and `worker_role()` 540-547 binding parts.
- `apps/cockpit/src/chief/view.rs`: pages Work, Pending, Approved, WorkActions, Gates, GateActions, Sources, WorkerActions, Roles (34-36); `Intent::Attest` (39); `selected_proposals` (74, 139, 326-340, 382, 404, 431, 477); rows 210-219, 238-239, 243-271, 300-301, 306, 312, 331-340, 450, 453-458; guards 130-132; dispatch texts 233, 242.
- `apps/osd/src/chief.rs`: ProposeWork project check 146-157, ApproveWork check 177-186; `host_role_unbound` push 71-76 removed.
- `apps/osd/src/teams.rs`: 41-44 (`replay_sources`, `complete_ready` on open); 400-401 snapshot fields; QaResult route 331; `WorkRef::validate` uses at ~422, ~446 → `TaskKey::validate`.
- `apps/cockpit/src/teams.rs`: `AgentAction::QaResult` (176), `QaResult`/`QaVerdict` (213-224), `envelope_text` branches 232-249 (keep `chief_wake` 228-230, reworded), `qa_brief_text` 289-297.
- `apps/cockpit/src/teams/store.rs`: `bind_handoff` 718, `complete_ready_in` 721/760, `qa_result` 726-763, `structured_qa_required` 792-793.
- Binaries: `cockpit-team.rs` HELP 9-14, 34-36; `qa-result` 42, 55; `evidence-apply-*` 166-184; confirm list 214-215 (keep ReconcileDelivery/ReconcileLaunch). `cockpit-run-approve.rs` 77, 86-98, 118-124, 151-158, 187-191. `ask-to-run.rs` 26, 125-133, 155.
- Shell `config/bin/.local/bin/cockpit`: `apply_reviewed` second argument 2449-2451, 2504-2509, 2541-2548, 2571-2573, dispatch 2785.
- Queue ownership: `queue.rs` 17, 22, 30-31, 53-57; `app/continuity_ui.rs` 219, 235-238; `app.rs` 95, 919-921, 1125, 1156-1161, 3140, 3160; `todo.rs` 404, 574-601, 766, 772, 810-813.

## Kept (do not remove)

`native_telemetry`, `legacy_auto_ready`, `workers_at`, Enable admission gate, snapshot, tick (osd `chief.rs`); Enable/Pause/Resume/Disable/Resolve/RequestSummary/ReconcileDelivery/ReconcileLaunch; `chief_inbox` wakes, `chief_delivery_attempts`, `chief_turns`, `chief_events`, `chief_decisions`, `chief_attention`, `chief_summary_requests`, `chief_admission_verifications`, `chief_grants`; legacy `scheduling_reservations` (owners legacy/observed), `legacy_launches`, `settle_launch`, `reserve_legacy*`, `observe_resumed`, `reconcile_todos`; host-launch decisions (`observe_coordination` 922-924, `recover` 759); `require_non_chief`; OpenCode plugins unchanged.

## §4 Tests

- Delete whole: `apps/cockpit/tests/chief_evidence.rs`; `chief_protocol.rs` except `context_pages_reject_oversize_records_instead_of_truncating` (133); `common/chief.rs` `work`, `brief` (8-17).
- `chief_dispatch.rs`: keep 10, 442, 916, 968; rewrite with `reserve_legacy` 463, 475, 484; invert 399, 390; delete the rest.
- `chief_events.rs`: delete 31, 54, 73, 89, 113, 136, 305, 452, 776, 783; keep and rewire 153, 164, 190, 206, 221, 247, 268, 290, 326, 356, 372, 398, 432, 501, 523, 537, 550, 561, 582, 602, 613, 655, 667, 677, 692; swap fixtures in 625, 636, 712, 754.
- `chief_store.rs`: keep 8, 185 (part), 273, 284, 294, 309, 357, 382, 397, 408; delete 44, 68, 85, 107, 122, 135, 146, 172, 207, 318, 344, 371.
- `chief_ui.rs`: delete 84, 111, 136, 143, 222, 235, 247, 258, 273, 303, 333, 342, 350, 382, 404, 415, 446, 560, 568; keep others, fixing proposal fixtures.
- `team_store.rs`: delete 10-57; keep 83, 100, 113. `cockpit_script.rs:4149` and `approval_terminal.rs:29` delete.
- `src/tests.rs`: simplify fake host 298-318, 3302-3324; keep 7337 (renamed), 10354; invert 7351, 7527; delete 7372, 7420, 7432, 7447, 7484, 7504; 7327 drop field.
- osd `chief_tests.rs`: keep 60, 76, 92, 116, 131, 147, 193, 218, 242, 255, 293, 302, 372, 423, 526, 544, 568, 598, 761, 773, 811; adjust 557, 740, 44, 475, 705; delete 175, 271, 398, 450, 870.
- osd `chief_workflow_tests.rs`: keep 289, 792; delete 322, 361, 407, 432, 744, 775 and helpers `proposal`, `approve`, `bind`, `queued_workflow`, `GitBoundary`.
