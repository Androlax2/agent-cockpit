# Autonomous Chief, plan 3 of 5: what Theo sees — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Theo can see, without notifications, every answer the Chief gave (a count on each session card, a list per session and in the Chief view, a section in the daily digest and in on-request summaries) and overrule any answer with a typed correction that is delivered to the agent.

**Architecture:** The host store gets a v14 migration with a `chief_overrules` table and exposes Chief answers as review records and per-session counts. A new host action `Overrule` records the correction and queues a `chief_correction` inbox row that reuses the Theo-answer delivery path (native queue, no grace, no grant). The cockpit shows the data in today's UI (card badge, `i` details, `c` answers list, Chief view rows, digest text); the designed UI (todo `9963c4e1`) restyles it later.

**Tech Stack:** Rust (`apps/cockpit`, `apps/osd`), rusqlite, TypeScript OpenCode plugin (`config/opencode/.config/opencode/plugins/cockpit.ts`), `make test` (nextest + Python).

**Spec:** `docs/superpowers/specs/2026-10-24-autonomous-chief-design.md` ("What Theo sees", "Overrule effect"). UI decisions agreed with the designer session `ses_e91606d74ffeE051lUN5aIydtj` on 2026-10-25 (link `52674da0…`, thread `09a12d41…`); FigCraft page 05 frames 1053 (answers list) and 1075 (overrule), page 10 frame 2284 (Chief main view), `DESIGN.md` "Cockpit UI redesign" → Chief.

## Global Constraints

- Count text: `Chief ✓N`, dim, never amber; hidden when N = 0. N = answers with `source='chief'` and `delivery != 'cancelled'` whose stop has the same `task_id` as the session's latest stop.
- Delivery words: `queued` → `waiting`, `received` → `sent`, `uncertain` → `not sure` (red), `cancelled` → `cancelled`; all dim except `not sure`.
- Answer row: line 1 `HH:MM · <question>`; line 2 `Chief: <answer>`; line 3 (dim) `<delivery> · <reason>`. Each line truncated with `…`, never wrapped. Newest first. Empty list: `No Chief answers yet.`
- Keys (today's UI): `c` on the selected session card opens its answers list; in the list `o` overrule, Enter reason/details, Esc back. Overrule review keeps the Chief view's existing confirm (Enter, then `y` send / `n` back). `C` stays `/compact`.
- Correction text sent to the agent, exactly: `Correction from Theo to an earlier Chief answer ("<answer excerpt ≤ 200 chars>"): <correction>` + `\n\n` + `CORRECTION_LABEL`, where `CORRECTION_LABEL` = `"Correction from Theo, sent through the cockpit Chief view. Treat it as a new instruction inside your current task. If following it means undoing work or changing the task's scope, stop and ask Theo."`
- Correction ≤ 2,000 chars, single line (like Theo's typed answers). One overrule per answer; only answers with delivery `received` or `uncertain` can be overruled.
- No habit preview, no placeholder, no "habits used" line until plan 4 (never show fake data).
- Digest section and summary list: at most 5 answer lines, then `+K more`.
- Session names everywhere a session appears in plan-3 UI: `<label> · <project>`; fallback `ses_…<last 6 chars>`; never raw JSON.
- Schema: one migration to version 14, idempotent, inside the existing Immediate transaction.
- No phone changes (phone frames 1032/1061 stay with `9963c4e1`). No `Todo:` trailer (`b77a0881` stays open until plan 5). `make test` green and zero warnings after every task.

## Review Focus

1. Theo overrules an answer while the agent is mid-turn or showing a question: the correction waits for the next turn, is delivered through the native queue only, and never touches the draft (Task 2 workflow test).
2. A Chief answer to a later stop is queued when Theo's correction lands: the Chief answer is cancelled at claim time, so the agent never gets the Chief contradicting Theo (Task 2 test).
3. The session restarted (runtime changed) since the overruled answer: the correction is cancelled, never delivered to the new runtime, and shows `cancelled` (Task 2 test).
4. A session with 200+ answers, or answers with 2,000-char text and hidden characters: the card, list and digest stay one line per field, escaped, and the snapshot stays bounded (Task 1 and 3 tests).
5. The team database is missing or locked when the digest runs: the digest still prints, with one line `Chief: unavailable (<reason>)` (Task 5 test).

---

### Task 1: Schema v14, answer records and per-session counts

**Files:** Modify `apps/cockpit/src/chief/store.rs` (migration after the v13 block ~line 350; `review_records` ~1057; new `answer_counts`), `apps/osd/src/teams.rs` (`host_snapshot` ~408), `apps/osd/src/chief.rs` (`snapshot`: add `session_names`). Tests: `apps/cockpit/tests/chief_store.rs`, `apps/cockpit/tests/chief_events.rs`, `apps/osd/src/chief_tests.rs`.

**Interfaces:**
- v14: `chief_overrules(id TEXT PRIMARY KEY, answer_id TEXT NOT NULL UNIQUE REFERENCES chief_answers(id), session_id TEXT NOT NULL, runtime TEXT NOT NULL, correction TEXT NOT NULL, delivery TEXT NOT NULL CHECK(delivery IN ('queued','received','uncertain','cancelled')), created_at INTEGER NOT NULL, finished_at INTEGER)`; index `chief_answers(session_id, created_at)`.
- Review record kind `answer` (newest 200 only, all states): `{kind:"answer", id, stop_id, session_id, project, task_id, question, answer, reason, source, delivery, created_at_ms, overrule: null | {id, correction, delivery, created_at_ms}}`.
- Summary record gains `previous_completed_at_ms` (second-latest completed summary, or null).
- `pub fn answer_counts(connection: &Connection) -> Result<Vec<AnswerCount>, TeamError>`; `pub struct AnswerCount { pub session_id: String, pub count: u32, pub recent: Vec<Value> }` (`recent` = ≤ 3 newest counted answers as `{created_at_ms, question, answer, delivery}`), `Serialize`.
- `host_snapshot` adds `"chief_answers": [AnswerCount…]`; the chief snapshot reply adds `"session_names": {session_id: "<label> · <project>"}` from the registry.

- [ ] **Step 1: Failing tests** — `version_13_upgrades_to_14_with_an_empty_overrule_log` (seed v13 with answers; migrate; v14, answers kept, `chief_overrules` present; migrate again no-op); `the_review_lists_chief_answers_newest_first_and_bounded` (250 answers → 200 `answer` records, newest first); `an_overruled_answer_carries_its_correction`; `answer_counts_count_chief_answers_of_the_current_task_only` (theo source, cancelled, and an older task's answers not counted; recent ≤ 3); osd `the_host_snapshot_carries_chief_answer_counts`; osd `the_chief_snapshot_names_sessions` (label · project, fallback absent key).
- [ ] **Step 2: Run, verify they fail.** `cargo nextest run --manifest-path apps/Cargo.toml -p cockpit --test chief_store`
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run** `make test` → pass, zero warnings.
- [ ] **Step 5: Commit** `feat: expose chief answers and per-session counts`

### Task 2: Overrule an answer and deliver Theo's correction

**Files:** Modify `apps/cockpit/src/chief.rs` (`ChiefHostAction::Overrule`, its `request_id` arm and validation; `CORRECTION_LABEL` next to `THEO_ANSWER_LABEL`), `apps/cockpit/src/chief/store.rs` (`host_action`, new claim branch, receipt handling in `finish_for`, cancel rule in `claim_reply`), `apps/cockpit/src/teams.rs` (`envelope_text` branch for `chief_correction`), `apps/osd/src/teams.rs` (Receive gate: claim corrections on `answer_turn`), `config/opencode/.config/opencode/plugins/cockpit.ts` (row label only). Tests: `apps/cockpit/tests/chief_events.rs`, `apps/cockpit/tests/opencode_team.rs`, `apps/osd/src/chief_workflow_tests.rs`.

**Interfaces:**
- Consumes: Task 1 `chief_overrules`.
- `ChiefHostAction::Overrule { request_id: String, answer: String, correction: String }` (host only; `correction` 1–2,000 chars, single line, rejected if it contains `THEO_ANSWER_LABEL`, `CHIEF_ANSWER_BOUNDARY` or `CORRECTION_LABEL`).
- In one transaction: answer must exist with delivery `received` or `uncertain` and no overrule (else `Conflict` with a plain reason: "not sent yet", "cancelled", "already overruled"); the session must be live with the answer's stop runtime (else `Conflict`); insert `chief_overrules` (`queued`) and a `chief_inbox` row of kind `chief_correction`, epoch `THEO_ANSWER_EPOCH`, envelope `{kind:"chief_correction", source:"theo", overrule, answer_excerpt, text}`; emit an event.
- Delivery: claimable on `answer_turn` (waiting, or needs-you with `needs_input`); readiness = Theo-answer readiness (no grace, no grant; all other blockers apply); runtime must equal the overrule's runtime at claim, else cancel the overrule (`cancelled`). Receipt `msg_cockpit<id>` → `received`; otherwise `uncertain`. The plugin row reads `Theo's correction: …`; no question-clearing report for this kind.
- Cancel rule: `claim_reply` cancels a queued Chief answer (`source='chief'`) when an overrule for the same session was created after that answer's stop.

- [ ] **Step 1: Failing tests** — `overruling_a_sent_answer_queues_the_correction_with_its_label` (envelope text exactly per Global Constraints); `an_answer_not_sent_yet_cannot_be_overruled`; `an_answer_can_be_overruled_once`; `a_correction_to_a_restarted_session_is_cancelled`; `a_queued_chief_answer_is_cancelled_after_theo_corrects_the_session`; `a_correction_ignores_the_chief_grant_and_the_grace`; `a_correction_waits_while_the_session_is_viewed`; plugin `a_correction_is_delivered_through_the_native_queue_with_its_row_label` (synthetic `delivery:"queue"`, row `Theo's correction: …`, no `chief_answer` source report); osd workflow `theo_overrules_a_chief_answer_end_to_end` (production plugin; overrule `received`).
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement** as listed; reuse the chief_answer claim/readiness code, don't duplicate it.
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `feat: let Theo overrule a chief answer with a correction`

### Task 3: `Chief ✓N` on session cards, answers in `i`, `c` opens the list

**Files:** Modify `apps/cockpit/src/app.rs` (`check_team` ~4112: keep `chief_answers` in an `App` map; independent card line 1 ~1891 before the age, or line-2 prefix ~1910 when line 1 has no room; selected-card badges ~2068; `open_session_details` ~2292; `handle_sessions`: `c`), `apps/cockpit/src/app/chief_ui.rs` (open the Chief view at the answers page for one session). Test: `apps/cockpit/src/tests.rs`.

**Interfaces:**
- Consumes: Task 1 `host_snapshot.chief_answers`; Task 4 `pub(super) fn open_chief_answers(&mut self, session_id: Option<&str>) -> String` (Task 4 runs before Task 3).
- Card: `Chief ✓N · <age>` on line 1, dim; hidden at 0; same text in the selected-card badges.
- `i` details: a "Chief answers" block with the ≤ 3 recent rows (line 1 and 2 of the answer row format) and `c all answers`; absent when N = 0.
- If `c` is already bound in `handle_sessions` or the global handler, stop and report NEEDS_CONTEXT.

- [ ] **Step 1: Failing tests** — `a_card_shows_how_many_answers_the_chief_gave` (`Chief ✓2`, dim, no amber); `a_card_without_chief_answers_shows_no_count`; `session_details_list_the_recent_chief_answers`; `c_opens_the_answers_of_the_selected_session`; `a_long_answer_stays_on_one_line_with_an_ellipsis` (2,000 chars + a bidi char → one line, escaped, ends `…`).
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `feat: show chief answer counts on session cards`

### Task 4: Chief view: Needs you, Answers and the overrule flow

**Files:** Modify `apps/cockpit/src/chief/view.rs` (`Page::Answers(Option<String>)`, `Intent::Overrule(Value)`, root rows, answer rows, Detail, Note/Review for overrule, session names), `apps/cockpit/src/app/chief_ui.rs` (`open_chief_answers`), `README.md` (Chief section: counts, answers list, overrule, keys). Test: `apps/cockpit/tests/chief_ui.rs`.

**Interfaces:**
- Consumes: Task 1 `answer` records and `session_names`; Task 2 `ChiefHostAction::Overrule`, `CORRECTION_LABEL`.
- Root rows in order: `Needs you · N` (today's open decisions: escalations and delivery recoveries), `Answers · N`, `Chief controls`, `Workers`, `Request summary`.
- Answers page: rows per Global Constraints, filtered to one session when opened with a session id (title `Answers · <session name>`); footer `o overrule · ⏎ reason · esc back`. Overruled answers add `overruled · <correction delivery>` to line 3.
- `o` (or Detail action "Overrule") on an answer with delivery `received`/`uncertain` → Note box (single line, ≤ 2,000) → Review showing the exact delivered text (built with `envelope_text`, so the label is visible) → `y` submits `Overrule`. `o` on another answer shows the reason in words ("Not sent yet", "Cancelled", "Already overruled"). A refreshed snapshot invalidates an open review as for escalation answers.
- Escalation rows and decision details show session names; `Row::record` never falls back to raw JSON for `answer` records.

- [ ] **Step 1: Failing tests** — `the_root_shows_needs_you_and_answers_counts`; `the_answers_page_lists_rows_newest_first` (row format, delivery words, `not sure` red); `an_empty_answers_page_says_no_chief_answers_yet`; `overruling_shows_the_exact_text_then_sends_it` (review contains `Correction from Theo to an earlier Chief answer (` and `CORRECTION_LABEL`; `y` sends one `Overrule`); `an_answer_not_sent_yet_says_why_it_cannot_be_overruled`; `answers_name_the_session_not_its_id`.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `feat: list chief answers and overrule them from the chief view`

### Task 5: Chief answers in the daily digest and on-request summaries

**Files:** Modify `apps/cockpit/src/costs.rs` (`digest_lines` ~768 gets a `chief: &ChiefDigest` input; `print_digest` ~863 opens the team database read-only, reads the since-time, prints, then writes the new since-time), `apps/cockpit/src/chief/store.rs` (`answers_since`), `apps/cockpit/src/chief/view.rs` (Summary frame ~414 appends the host answer list), `README.md` (digest). Tests: `apps/cockpit/src/tests.rs` (digest tests ~5971), `apps/cockpit/tests/chief_ui.rs`.

**Interfaces:**
- `pub fn answers_since(connection: &Connection, since_ms: i64) -> Result<ChiefDigest, TeamError>`; `pub struct ChiefDigest { pub answered: u32, pub escalated: u32, pub overruled: u32, pub lines: Vec<DigestAnswer>, pub more: u32 }`; `pub struct DigestAnswer { pub session_id: String, pub question: String, pub answer: String }` (≤ 5 newest; `more` = the rest).
- Since-time: file `chief-digest-at` (epoch ms) in the same cockpit state directory as `digest-day`, written by `print_digest` only after the digest printed; missing or unreadable → last 24 h.
- Digest section, after the existing lines: `Chief answered N, escalated M, overruled K since <HH:MM or "yesterday HH:MM">`, then up to 5 lines `<session name> · <question> → <answer>` (each ≤ the digest width, `…`), then `+K more`. N = M = K = 0 → `Chief: nothing answered since <time>`. Database missing or error → `Chief: unavailable (<reason>)`, rest of the digest unchanged.
- Summary frame: below the Chief-written sections, `Answers since <previous_completed_at_ms or 24 h>` with the same ≤ 5 lines from the `answer` records.
- `config/bin/.local/bin/cockpit` stays unchanged.

- [ ] **Step 1: Failing tests** — `the_digest_lists_chief_answers_since_the_last_digest`; `the_digest_says_when_the_chief_answered_nothing`; `a_missing_team_database_leaves_the_digest_intact` (`Chief: unavailable (`); `the_digest_shows_at_most_five_answers_then_a_count`; `printing_the_digest_moves_the_since_time`; UI `a_summary_lists_the_answers_since_the_previous_summary`.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `feat: add chief answers to the daily digest and summaries`

Execution order: 1, 2, 4, 3, 5.
