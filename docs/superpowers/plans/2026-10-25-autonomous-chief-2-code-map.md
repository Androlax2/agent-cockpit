# Plan 2 code map (read-only survey at 3483bec)

Line numbers are approximate; re-check before editing.

## §1 Wake path
- `chief/store.rs`: `schedule_wake` 307-322 (skips failed grant or admission < 30 s ago; needs a `chief_events` row past `delivered_seq`, kinds except status/coordinate/reprioritized; one outstanding wake, index `chief_one_queued_wake` 233). `coordination_event` 262; `observe_coordination` 324-357; `material_workers` 291-300. `claim` 375-425 (wakes only; non-wake kinds → Obsolete 401-412); `wake_ready` 302-305; `finish_for` 431-476 (inserts `chief_turns`, advances `delivered_seq`; non-wake attempts cancelled 446-452); `agent()` 478-526; `charge_mutation` 363-371 (20 per turn); `context_remaining` 617-621 (50 records); `event_context` 529-579 (unknown kinds → snapshot 553); `context_records` 645-661 (chief, workers, decisions without text); `clear_dispatch_leftovers` 760-772.
- osd: `Registry::tick_queues` (registry.rs:1260) → `chief::tick` (osd/chief.rs:196-216) → `workers_at(observe=true)`; `Receive` (osd/teams.rs:349-362) calls `observe`; `turn` gate osd/teams.rs:300-302 (only `!needs_input && state=="waiting"`).
- Wake text: `teams.rs:271-275` (`envelope_text` chief_wake).

## §2 Stops and questions
- `cockpit-needs-input` → hook `NeedsInput` (session-state.sh:238-248) → `~/.cache/claude/input-<session>.question`; at Stop (303, 351-359) `state=needs-you, needs_input=true, message=<q>`; daemon keeps `Entry.request` (registry.rs:1403-1405, Entry 1048-1086), `since` changes on every transition (1460).
- cockpit.ts: `form.created` → Notification elicitation_dialog (195-197, `data.form{id,sessionID,title,fields[]}`); `permission.asked` (188-194); `session.execution.*` → Stop "Finished, waiting for you" (168-172); synthetic turn reports `UserPromptSubmit` source system (164-166); hook clears the saved question only for source user (270-274; test opencode_plugin.rs:231).
- Native telemetry counts (`pending_forms`, `pending_permissions`) in `registry.chief_telemetry` (osd/chief.rs:8-39, workers_at 90-102); sandbox-reported.
- Form text lives in `opencode.db` `session_message` assistant `content[]` part `{type:"tool", name:"question", state:{status:"running"|…, input:{questions:[{header,question,options:[{label,description}],multiple}]}}}`.
- Phone form/permission answering (remote.rs 667-782) types keys into the pane: not draft-safe, not reusable for the Chief.
- OpenCode form API in the installed bundle: `client.session.form.reply({sessionID, formID, answer})`, `form.cancel`, `form.list`; HTTP `POST /session/:id/form/:formID/reply {answer}`; `answer` keyed by field key; errors FormAlreadySettledError/FormNotFoundError. Unverified live; not used anywhere in the repo.

## §3 Conversation and task reading
- `paths::opencode_database()` (paths.rs:92-95, env `COCKPIT_OPENCODE_DB`); table `session_message(id, session_id, type, seq, time_*, data)`.
- Reader pattern `opencode_messages` (remote/conversation.rs:114-140): read-only, 200 ms busy timeout, caps; it drops `synthetic` and non-text parts (`visible_text` 68-74, 133).
- `TodoList::load()`: `text()` todo.rs:159, `notes()` 165, `session()` 172, `status()` 160; osd uses it in `live_sessions` (osd/teams.rs:74).
- Masking: `approval::redact_secrets(text) -> (String, usize)` (approval.rs:649-669), `approval::escape_hiding(text, before, after)` (252).
- Note: `opencode.db` is readable by every OpenCode sandbox; the masking holds for what the daemon hands over.

## §4 Delivery
- cockpit.ts receive timer 286-300 (3 s, root session) → `deliverTeamMessage` 265-285: `ctx.session.synthetic({sessionID, id:"msg_cockpit"+message_id, text, description:teamRow(envelope), metadata, delivery:"queue", resume:true})`, checks `admitted.id`, receipt received/uncertain; `teamRow` 253-264.
- `MessageKind` teams.rs:203; `envelope_text` 271-314.
- `ChiefAgentAction` chief.rs:28-34 (deny_unknown_fields; validators 98-113); `cockpit-team chief` verb (bin/cockpit-team.rs:47).

## §5 Decisions
- `chief_decisions` (store.rs:935-941): id, scope_key, decision_key, work, assignment, question (1..1000), recommendation (NOT NULL 1..1000), state open|resolved|superseded, revision, resolution, created_at, resolved_at; unique open (scope_key, decision_key); cap 100 open (966-975).
- `decision_record` 269-289 (coalesce, revision bump, attention); banners osd/chief.rs:200-214; agent `Decision` 510-516 (reserved keys ignored); host `Resolve` chief.rs:61 / store.rs:114-120; host snapshot decision records 705-706.
- View: Decisions page view.rs:196-197, `DecisionActions` 217-226, `Intent::Resolve` → Note → `finish_note` 241-251 → Review → `Effect::Submit` → app/chief_ui.rs:68-80.

## §6 Idle guard and queue
- `QUEUE_IDLE_S = 300` queue.rs:7; `workers_at` pushes `idle_guard` osd/chief.rs:123 for every session; `wake_ready` needs empty blocked_by; `legacy_auto_ready` osd/chief.rs:55,59; test asserting the guard on the Chief chief_tests.rs:156; `require_non_chief` store.rs:20-25.
- Queue WARN loop: `dispatch_queue` app.rs:1172-1207 (WARN 1193); `host_snapshot` osd/teams.rs:393-415; `queue_context` app/continuity_ui.rs:215-220; `QueueContext` queue.rs:10; `queue::block` 49; test `an_enabled_chief_keeps_the_todo_queue_sending` src/tests.rs:8157.

## §7 Schema
- `user_version` 12; `upgrade_schema` teams/store.rs:89-173 (accepts 0..=12 at 92, early return ≥10 at 94); `links::migrate` teams/links.rs:531-604 (10..=12 at 534); `migrate_with` teams/store.rs:80-87 (upgrade_schema → links::migrate → clear_dispatch_leftovers, one Immediate transaction).
- Leftover tables: `create_schema` 902-976 (`chief_work_revisions`, `chief_assignments`, `chief_evidence`, `chief_worker_bindings`; also `upgrade_worker_history` 887-900); `upgrade_review` 734-744 (`chief_work_proposals`); `upgrade_evidence` 138-149 (`chief_sources`, `chief_handoffs`, `chief_apply_reviews`, `chief_qa_results`); `upgrade_inbox` 164-176 (`chief_submissions`); `upgrade_events` 235-241 creates `review_*` triggers for a hard-coded list including dropped tables; `upgrade_scheduling` 746-756 reads `chief_assignments`.
- FKs: `chief_assignments` → `chief_work_revisions` (927); `chief_evidence` → `chief_work_revisions` (947); `chief_inbox.assignment` → `chief_assignments` (166); `chief_delivery_attempts.assignment` → `chief_assignments` (950). Rebuild children before dropping parents; `foreign_keys` cannot change inside the transaction.
- Hard-coded version asserts: tests/chief_store.rs 20, 90, 103, 136, 144, 205, 207 (future-schema test uses 13 at 109-111); tests/team_store.rs 1599, 2012; teams/store/tests.rs 19, 32; rollback injection tests/chief_store.rs:186-194; schema-8 upgrade tests/chief_events.rs:472-486; tests/common/chief.rs:7-17.

## §8 Harnesses
- cockpit: `chief_events.rs` (`setup`, `admit_events` 48-55), `chief_store.rs`, `chief_ui.rs` (`snapshot`, `decision` fixtures 10, 178), `common/chief.rs` (`admit` 24-31), `opencode_team.rs` (production cockpit.ts under node, fake synthetic, `SHIM_TEAM_DELIVERY`), `opencode_plugin.rs`, `session_state_hook.rs`, `claude_remote.rs:697-760` (session_message fixture), `src/tests.rs` queue fixtures.
- osd: `tests.rs` `team_bench` 205-220, `Fixture` 31-52 (set `COCKPIT_OPENCODE_DB` per test); `chief_tests.rs` `enable`/`admit_chief` 12-41, summary route 146-170, readiness 383-427, attention 75-127, native telemetry harness 567-654; `chief_workflow_tests.rs` `INBOX_HARNESS` + `native_inbox()` 79-174 (needs node and the cockpit-team binary).
