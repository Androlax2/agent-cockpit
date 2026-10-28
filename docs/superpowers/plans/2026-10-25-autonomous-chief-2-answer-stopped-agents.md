# Autonomous Chief, plan 2 of 5: answer stopped agents — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When an OpenCode agent stops while its todo is in progress, the Chief reads the stop and either answers it (delivered through the verified native queue 30 s after the stop) or escalates it to Theo with a category and an optional suggested answer; every answer is logged.

**Architecture:** The host daemon detects stops from its session registry, builds a masked "stop bundle" from the host-side OpenCode database and the todo list, stores it on a `chief_stops` row and raises a `stop_detected` coordination event, which the existing wake path turns into a Chief turn. The Chief reads stops from Context and calls two new agent actions, `Answer` and `Escalate`. Answers become `chief_answer` inbox rows delivered by the worker's own plugin via `session.synthetic` (draft-safe, receipt = `msg_cockpit`+id); escalations become decisions Theo answers from the Chief view. Question forms are answered in a separate task after a live check of OpenCode's form API.

**Tech Stack:** Rust (`apps/cockpit`, `apps/osd`), rusqlite, TypeScript OpenCode plugin (`config/opencode/.config/opencode/plugins/cockpit.ts`), bash hook (`config/claude/.claude/hooks/session-state.sh`), `make test` (nextest + Python).

**Spec:** `docs/superpowers/specs/2026-10-24-autonomous-chief-design.md` (sections 1, "What a Chief answer authorises", "Question forms", "What Theo sees", "Rework", "Constraints"; Theo's 2026-10-25 decisions on trigger and timing). Code map with line references: `docs/superpowers/plans/2026-10-25-autonomous-chief-2-code-map.md`.

## Global Constraints

- Trigger: every stop of an OpenCode session (state `waiting`, or `needs-you` with `needs_input`, or an open question form) whose todo has status `doing`. Permission prompts are never a Chief stop.
- Chief answer delivery: ≥ 30 s after the stop (`CHIEF_ANSWER_GRACE_S = 30`); still blocked when the session is viewed, a human prompt is queued, a permission prompt is open, Theo is gaming, telemetry is stale, or the visible-session guard fails. The 300 s `QUEUE_IDLE_S` stays for everything else.
- The Chief's own turns are exempt from `idle_guard`; all its other readiness checks and the 30 s wake rate limit stay.
- Stop bundle: conversation text only (user, assistant text parts, synthetic), never tool outputs or attachments; secrets masked with `approval::redact_secrets`, hiding characters escaped with `approval::escape_hiding`; caps: last 20 messages, 2,000 chars per message, 16,000 chars per bundle; plus todo text and notes. The running `question` tool input (header, question, options) is included.
- Every Chief answer message ends with this exact host-added boundary text: `"Chief answer on Theo's behalf. It authorises edits, builds, tests and commits inside your current task only. Installs, dependency changes, pushes, merges, default-branch or other Git writes your project does not already allow, anything outside the task, and anything security-related still need Theo. Permission prompts, ask-to-run and cockpit apply are always Theo's."`
- Escalation categories exactly `security`, `scope`, `not_sure`; `suggested_answer` optional (≤ 2,000 chars); reason ≤ 300 chars.
- Answer log row: session, stop id, question excerpt (≤ 300 chars), answer text, reason (≤ 300 chars), created time, delivery state.
- Schema: one migration to version 13 that drops plan 1's unused tables and adds `chief_stops`, `chief_answers` and decision columns; idempotent, inside the existing Immediate transaction.
- Fail closed: stale or missing data → no stop, no delivery. A stop is superseded when the session's state boundary (`state:since`) changes.
- No `Todo:` trailer (b77a0881 stays open until plan 5). `make test` green and zero warnings after every task.

## Review Focus

1. Theo is typing in the stopped session's composer when the Chief answer arrives: the draft stays untouched (native queue path only; test the envelope is delivered via `synthetic`, never `cockpit say`/pane typing).
2. The agent resumes on its own (or Theo answers) before the 30 s grace: the stop is superseded and no answer is delivered (Task 5/6 test).
3. A stop bundle containing an API token or a tool output with secrets: the token is masked and the tool output never appears in Context (Task 4 test).
4. The Chief answers a stop whose session has since been relinked/restarted (runtime changed): delivery fails closed, nothing reaches the new runtime (Task 6 test).
5. After a Chief answer, the next stop does not resurrect the old `needs_input` question (Task 6 test on the question-clearing path).

---

### Task 1: The Chief's own turns skip the idle guard

**Files:** Modify `apps/osd/src/chief.rs` (`workers_at`, idle_guard push), `apps/cockpit/src/chief/store.rs` (add `is_chief`). Test: `apps/osd/src/chief_tests.rs`.

**Interfaces:** Produces `pub fn is_chief(connection: &Connection, session_id: &str) -> Result<bool, TeamError>` (true for an active or paused grant).

- [ ] **Step 1: Failing tests** — `the_chief_is_woken_without_waiting_for_the_idle_guard` (Chief session idle 40 s, all else ready: `blocked_by` has no `idle_guard`, wake claimable); `workers_still_wait_for_the_idle_guard` (non-Chief session idle 40 s: `blocked_by` contains `idle_guard`). Update the existing assertion that the guard applies to the Chief.
- [ ] **Step 2: Run, verify they fail.** `cargo nextest run --manifest-path apps/Cargo.toml -p osd chief_tests::the_chief_is_woken`
- [ ] **Step 3: Implement** `is_chief`; skip the `idle_guard` push when it returns true.
- [ ] **Step 4: Run** `make test` → pass, zero warnings.
- [ ] **Step 5: Commit** `fix: let the chief take its turns without the idle guard`

### Task 2: The todo queue skips the Chief's own session

**Files:** Modify `apps/osd/src/teams.rs` (`host_snapshot`: add `chief_session`), `apps/cockpit/src/queue.rs` (`QueueContext.chief_session: Option<String>`, `QueueBlock::Chief`), `apps/cockpit/src/app/continuity_ui.rs` (`queue_context`, queue view text). Test: `apps/cockpit/src/tests.rs`.

**Interfaces:** Produces `QueueBlock::Chief` with display text `"the Chief's own session"`.

- [ ] **Step 1: Failing test** — `the_todo_queue_never_offers_work_to_the_chief_session`: fake host snapshot with `chief_session` = an idle session that matches a queued todo's project; `app.dispatch_queue()` makes no `reserve_task` request and logs no `queue:` WARN; `queue::block` returns `QueueBlock::Chief`.
- [ ] **Step 2: Run, verify it fails.**
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `fix: keep the todo queue from offering work to the chief session`

### Task 3: Schema v13: drop dispatch tables, add stops, answers and escalation fields

**Files:** Modify `apps/cockpit/src/chief/store.rs` (new `upgrade_to_13`, `upgrade_events` trigger list, `clear_dispatch_leftovers`), `apps/cockpit/src/teams/store.rs` (`migrate_with`, both version range checks to `0..=13`), `apps/cockpit/src/teams/links.rs` (range check). Test: `apps/cockpit/tests/chief_store.rs` and every hard-coded `user_version` 12 assertion listed in the code map §7.

**Interfaces:** Produces tables:

- `chief_stops(id TEXT PRIMARY KEY, session_id TEXT NOT NULL, runtime TEXT NOT NULL, boundary TEXT NOT NULL, project TEXT NOT NULL, task_id TEXT NOT NULL, bundle TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('open','answered','escalated','superseded')), created_at INTEGER NOT NULL, UNIQUE(session_id, boundary))`
- `chief_answers(id TEXT PRIMARY KEY, stop_id TEXT NOT NULL REFERENCES chief_stops(id), session_id TEXT NOT NULL, question TEXT NOT NULL, answer TEXT NOT NULL, reason TEXT NOT NULL, source TEXT NOT NULL CHECK(source IN ('chief','theo')), delivery TEXT NOT NULL CHECK(delivery IN ('queued','received','uncertain','cancelled')), created_at INTEGER NOT NULL)`
- `chief_decisions` new nullable columns `session_id`, `stop_id`, `category` (CHECK `security|scope|not_sure`), `suggested_answer`; `work`/`assignment` columns dropped by rebuild or left unused (implementer's choice, recorded).
- `chief_inbox`/`chief_delivery_attempts` rebuilt without `assignment`; indexes `chief_inbox_inflight`, `chief_one_queued_wake`, `chief_one_inflight_recipient` and `review_*` triggers re-created.
- Dropped: `chief_evidence`, `chief_assignments`, `chief_work_revisions`, `chief_work_proposals`, `chief_sources`, `chief_handoffs`, `chief_apply_reviews`, `chief_qa_results`, `chief_submissions`, `chief_worker_bindings` (and their triggers/indexes).

- [ ] **Step 1: Failing tests** — `version_12_with_dispatch_leftovers_upgrades_to_13_keeping_wakes_and_decisions` (seed v12 DB with dispatch rows, a queued wake, an open decision; migrate; assert v13, wake and decision kept, dropped tables absent, new tables present; migrate again changes nothing); `a_fresh_database_is_created_at_version_13`; re-point the rollback-injection test at a table that still exists in v13.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement** `upgrade_to_13` with FK-safe order (null child `assignment`, rebuild children, drop parents) per code map §7.
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `refactor: migrate the chief store to version 13 for stops and answers`

### Task 4: Masked stop bundles

**Files:** Create `apps/cockpit/src/chief/bundle.rs`; modify `apps/cockpit/src/chief.rs` (`pub mod bundle`), `apps/cockpit/src/remote/conversation.rs` only to share the read-only DB open helper if needed. Test: `apps/cockpit/tests/chief_bundle.rs`.

**Interfaces:** Produces

```rust
pub struct StopBundle { pub question: Option<FormQuestion>, pub last_assistant: String, pub recent: Vec<BundleMessage>, pub task_text: String, pub task_notes: String }
pub struct FormQuestion { pub header: String, pub question: String, pub options: Vec<String> }
pub struct BundleMessage { pub role: String, pub text: String }
pub fn stop_bundle(database: &Path, session_id: &str, todo: &crate::todo::Item) -> Result<StopBundle, TeamError>
```

(serde-serialisable; role ∈ `user|assistant|synthetic`).

- [ ] **Step 1: Failing tests** (SQLite fixture like `claude_remote.rs` §8): `a_bundle_keeps_conversation_text_and_drops_tool_outputs`; `a_bundle_masks_tokens_and_escapes_hiding_characters` (planted `ghp_…` → masked; bidi char escaped); `a_bundle_includes_the_open_question_form` (running `question` tool → `FormQuestion`); `a_bundle_respects_its_size_caps` (20 messages, 2,000 chars each, 16,000 total); `a_missing_database_yields_an_error_not_an_empty_bundle`.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement** with read-only open and 200 ms busy timeout.
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `feat: build masked stop bundles for the chief`

### Task 5: Detect stops and show them to the Chief

**Files:** Modify `apps/osd/src/chief.rs` (`observe_stops` in `tick`), `apps/cockpit/src/chief/store.rs` (`record_stop`, `supersede_stops`, `stopped_session` record in `context_records`, `stop_detected` event), `apps/cockpit/src/teams.rs` (`envelope_text` chief_wake: instructions for stops, `Answer`/`Escalate`, the authorisation boundary and the escalation rules from the spec). Test: `apps/osd/src/chief_tests.rs`, `apps/cockpit/tests/chief_events.rs`.

**Interfaces:**

- Consumes Task 3 tables, Task 4 `stop_bundle`.
- Produces `pub fn record_stop(connection, session: &VerifiedSession, boundary: &str, task: &TaskKey, bundle: &StopBundle, now_ms) -> Result<Option<String>, TeamError>` (stop id; `None` if the boundary was already recorded) and `pub fn supersede_stops(connection, session_id, current_boundary) -> Result<(), TeamError>`; Context record `{"kind":"stopped_session","stop":id,"session":…,"project":…,"task_id":…,"bundle":…}`.
- Stop rule: OpenCode session, not the Chief, todo found for the session with status `doing`, state `waiting` or `needs-you` without an open permission (telemetry `pending_permissions == 0`), boundary `"{state}:{since}"`.

- [ ] **Step 1: Failing tests** — `a_stopped_worker_with_an_open_todo_wakes_the_chief_with_its_bundle`; `a_stop_without_an_open_todo_is_ignored`; `a_permission_prompt_is_never_a_chief_stop`; `a_new_state_boundary_supersedes_the_open_stop`; `the_same_boundary_is_recorded_once`.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement** (bundle built in osd with `paths::opencode_database()`; bundle failure → no stop, WARN logged once per boundary).
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `feat: wake the chief when an agent stops on an open task`

### Task 6: Chief answers reach the agent and are logged

**Files:** Modify `apps/cockpit/src/chief.rs` (`ChiefAgentAction::Answer { stop: String, text: String, reason: String }`), `apps/cockpit/src/chief/store.rs` (`agent` arm, `chief_answer` inbox kind, `claim`/`finish_for` branches with answer readiness, `chief_answers` row), `apps/osd/src/teams.rs` (`Receive` turn gate allows a `chief_answer` for `needs_input`/`waiting` sessions), `apps/osd/src/chief.rs` (answer readiness: `CHIEF_ANSWER_GRACE_S`), `apps/cockpit/src/teams.rs` (`envelope_text` for `chief_answer`: answer text + exact boundary text), `config/opencode/.config/opencode/plugins/cockpit.ts` (`teamRow` "Chief answer"; clear the saved question after a Chief answer), `config/claude/.claude/hooks/session-state.sh` only if clearing is done there. Tests: `apps/cockpit/tests/chief_events.rs`, `apps/cockpit/tests/opencode_team.rs`, `apps/osd/src/chief_workflow_tests.rs` (`native_inbox`).

**Interfaces:** Consumes Task 5 stops. Produces `pub const CHIEF_ANSWER_GRACE_S: i64 = 30;`.

- [ ] **Step 1: Failing tests** — `a_chief_answer_is_delivered_through_the_native_queue_with_the_boundary_text` (plugin harness: `synthetic` called with `delivery:"queue"`, text ends with the exact boundary string, receipt `msg_cockpit`+id; `chief_answers.delivery` = `received`; stop `answered`); `a_chief_answer_waits_for_the_thirty_second_grace`; `a_chief_answer_is_not_delivered_while_the_session_is_viewed_or_a_prompt_is_queued`; `a_superseded_stop_cannot_be_answered`; `an_answer_to_a_restarted_session_fails_closed`; `after_a_chief_answer_the_old_needs_input_question_does_not_return`.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `feat: deliver and log chief answers to stopped agents`

### Task 7: Escalations Theo answers from the Chief view

**Files:** Modify `apps/cockpit/src/chief.rs` (`ChiefAgentAction::Escalate { stop, category, reason, suggested_answer: Option<String> }`, `ChiefHostAction::AnswerDecision { request_id, decision, expected_revision, answer }`), `apps/cockpit/src/chief/store.rs` (escalate → decision with session/stop/category/suggested_answer, stop `escalated`; `AnswerDecision` resolves and queues a `chief_answer` with `source='theo'` in one transaction; stop supersede also supersedes its decision), `apps/cockpit/src/chief/view.rs` (Decision detail shows category, reason, question excerpt, suggestion; rows "Send the suggestion" (only with suggestion) and "Write my own answer"; review shows the exact text), `apps/osd/src/chief.rs` (banner text includes the category), `README.md` (Chief section: stops, answers, escalations, 30 s). Tests: `apps/cockpit/tests/chief_events.rs`, `apps/cockpit/tests/chief_ui.rs`.

- [ ] **Step 1: Failing tests** — `an_escalation_becomes_a_decision_with_category_and_suggestion`; `answering_a_decision_queues_the_text_for_the_agent_and_resolves_it`; `a_stale_decision_revision_cannot_be_answered`; `when_the_agent_moves_on_its_escalation_closes_itself`; UI `a_decision_with_a_suggestion_offers_send_suggestion_and_write_my_own` and `the_answer_review_shows_the_exact_text_to_send`.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `feat: let Theo answer chief escalations from the chief view`

### Task 8: Answer question forms (after a live check)

Controller-led: before dispatch, verify live with Theo that the running OpenCode exposes `session.form.reply({sessionID, formID, answer})` to the server plugin `ctx` (or only to the TUI plugin client), and the exact `answer` shape for a single-choice form. Record the result in the ledger; if the API is unavailable, park this task with a ruling (forms then escalate to Theo) and continue.

**Files (if available):** `apps/cockpit/src/chief.rs` (`ChiefAgentAction::AnswerForm { stop, choice: String, reason }`), `apps/cockpit/src/chief/store.rs` (`chief_form_reply` inbox kind), `config/opencode/.config/opencode/plugins/cockpit.ts` (record the live `form.created` id per session; on `chief_form_reply` call `form.reply` with the verified shape; receipt), tests in `apps/cockpit/tests/opencode_team.rs`.

- [ ] **Step 1: Failing tests** — `a_chief_form_answer_replies_to_the_open_form_by_id`; `a_form_answer_for_a_settled_form_reports_uncertain_and_never_types`.
- [ ] **Step 2–5:** run RED, implement, `make test`, commit `feat: let the chief answer opencode question forms`.
