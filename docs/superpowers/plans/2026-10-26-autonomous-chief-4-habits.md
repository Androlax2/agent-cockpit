# Autonomous Chief, plan 4 of 5: Theo's habits — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Chief keeps a readable list of Theo's habits, learned from his overrules (immediately) and his own answers, uses it when answering and says which habits it relied on; Theo sees new habits in the Chief view and the digest and deletes or re-scopes any habit with one key.

**Architecture:** A v15 host table `chief_habits` holds every habit. The host writes an overrule's habit in the same transaction as the overrule; the Chief adds habits from Theo's answers through a new agent action and cites habits on its answers. Every wake's Context carries the active habits and the stopped project's instruction files. The cockpit shows them in a Habits page of today's Chief view and in the digest; the designed UI (`9963c4e1`) restyles it later.

**Tech Stack:** Rust (`apps/cockpit`, `apps/osd`), rusqlite, `make test` (nextest + Python).

**Spec:** `docs/superpowers/specs/2026-10-24-autonomous-chief-design.md` ("3. Learn Theo's habits", "Overrule effect", "What Theo sees"). UI agreed with the designer session `ses_e91606d74ffeE051lUN5aIydtj` on 2026-10-26 (link `179d9fc1…`, thread `1a64d3bb…`); FigCraft frames 1098 (desktop habits), 1128 (Status → Chief), page 10 frame 2284 (Chief main view, key `h`).

## Global Constraints

- Habit text ≤ 500 chars, single line. An overrule's habit is Theo's correction **verbatim** (cut to 500 with `…`), never reworded; the overrule review previews exactly the stored text. (Ruling: the spec's "worded as a general preference" is dropped so the preview equals what is stored.)
- Habit scope: `project` (canonical project key) or everywhere (`NULL`). An overrule's habit takes the overruled stop's project. `origin_project` remembers the first project so `p` can toggle back.
- Sources exactly `overrule` (source id = overrule id), `theo_answer` (source id = a `chief_answers` id with `source='theo'`, or a `chief_stops` id whose bundle shows Theo's own message).
- No habit makes a security or scope question answerable: the wake text says so, and escalation rules are unchanged.
- An answer cites at most 5 habits; each must be active and apply to the stop's project (its project or everywhere).
- Context: one `habits` record with the active habits (newest 100); stopped_session bundles add the project's `AGENTS.md` and `CLAUDE.md` from the project root (masked with `approval::redact_secrets`, hiding characters escaped, ≤ 4,000 chars each).
- "new" = created after the habits-seen time, a file `chief-habits-seen-at` (epoch ms) next to `chief-digest-at`; written when Theo opens the Habits page and after a digest that printed its habits section. Missing → every habit created in the last 24 h is new.
- UI copy: root row `Habits · N` (+ a space and `· K new`); row line 1 `new` and a space (purple, bold) + text; line 2 dim `<project name in its project colour | everywhere> · from <your overrule | your answer> · HH:MM`; empty `No habits yet. The Chief learns them from your answers and overrules.`; footer `x delete · p project/everywhere · ⏎ source · esc back`; after `x`: `Deleted · u undo` until the next key.
- Never amber for habits. Purple = what the Chief learned.
- Schema: one migration to version 15, idempotent, inside the existing Immediate transaction.
- No phone changes; no habit text editing. No `Todo:` trailer (`b77a0881` stays open until plan 5). `make test` green and zero warnings after every task.

## Review Focus

1. Theo deletes a habit, then the Chief tries to add the same habit again: refused, so a deleted habit never comes back on its own (Task 2 test).
2. A habit scoped to project A is cited for a stop in project B: the answer is refused (Task 2 test).
3. A project `AGENTS.md` with a token or 50 KB of text: the token is masked and the bundle stays within its caps (Task 2 test).
4. Theo deletes the last habit: the empty text shows; deleting a middle habit keeps the selection on the next one; `u` restores it in place (Task 3 tests).
5. A correction longer than 500 chars: the stored habit and the review preview are the same cut text (Task 1 and 3 tests).

---

### Task 1: Habit store, overrule habits and host actions

**Files:** Modify `apps/cockpit/src/chief/store.rs` (v15 after the v14 block; `queue_correction` adds the habit; `host_action`; `review_records` category 17), `apps/cockpit/src/chief.rs` (`ChiefHostAction::{DeleteHabit, RestoreHabit, RescopeHabit}` + `request_id` arms + validation). Tests: `apps/cockpit/tests/chief_store.rs`, `apps/cockpit/tests/chief_events.rs`.

**Interfaces:**

- v15: `chief_habits(id TEXT PRIMARY KEY, text TEXT NOT NULL, project TEXT, origin_project TEXT, source TEXT NOT NULL CHECK(source IN ('overrule','theo_answer')), source_id TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('active','deleted')), created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL)`; index `chief_habits(state, created_at)`; `chief_answers.habits TEXT NOT NULL DEFAULT '[]'` (JSON array of habit ids).
- `pub fn habit_text(correction: &str) -> String` (single line, ≤ 500 chars with `…`), used by the host and by the Task 3 preview.
- `Overrule` (plan 3) also inserts the habit (source `overrule`, project = origin_project = the stop's project) in its transaction.
- `DeleteHabit { request_id, habit }` → `deleted`; `RestoreHabit { request_id, habit }` → `active`; `RescopeHabit { request_id, habit, everywhere: bool }` → `project = NULL` or `origin_project` (Conflict "no project to scope to" when `origin_project` is NULL). Each bumps `changed_at` and pairs with a `chief_events` row so the review generation moves.
- Review record kind `habit` (all states, newest 200): `{kind:"habit", id, text, project, origin_project, source, source_id, state, created_at_ms, changed_at_ms}`. Answer records gain `habits: [{id, text}]`.

- [ ] **Step 1: Failing tests** — `version_14_upgrades_to_15_with_an_empty_habit_list`; `an_overrule_adds_its_correction_as_a_project_habit`; `a_long_correction_becomes_a_cut_habit` (2,000 chars → stored text = `habit_text(correction)`, 500 chars ending `…`); `deleting_and_restoring_a_habit`; `rescoping_toggles_between_its_project_and_everywhere`; `the_review_lists_habits_and_the_habits_an_answer_used`; `habit_changes_move_the_review_generation`.
- [ ] **Step 2: Run, verify they fail.** `cargo nextest run --manifest-path apps/Cargo.toml -p cockpit --test chief_store`
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run** `make test` → pass, zero warnings.
- [ ] **Step 5: Commit** `feat: keep Theo's habits, starting with his overrules`

### Task 2: The Chief reads, adds and cites habits

**Files:** Modify `apps/cockpit/src/chief.rs` (`ChiefAgentAction::AddHabit`; `habits: Vec<String>` with `#[serde(default)]` on `Answer` and `AnswerForm`; validation incl. `without_host_labels`), `apps/cockpit/src/chief/store.rs` (habits Context record next to `stopped_session` ~1198; AddHabit; cite check in `answer_stop`/`queue_answer`), `apps/cockpit/src/chief/bundle.rs` (`instructions: Vec<InstructionFile { path, text }>`), `apps/osd/src/chief.rs` (pass the project root to the bundle), `apps/cockpit/src/teams.rs` (chief_wake text ~273). Tests: `apps/cockpit/tests/chief_events.rs`, `apps/cockpit/tests/chief_bundle.rs`, `apps/osd/src/chief_tests.rs`.

**Interfaces:**

- Consumes: Task 1 `chief_habits`, `habit_text`.
- `ChiefAgentAction::AddHabit { text: String, project: Option<String>, source_id: String, reason: String }` (text ≤ 500 single line, reason ≤ 300; one mutation). Source is `theo_answer`; `source_id` must name a `source='theo'` answer or a stop the Chief was shown. `project` must be NULL or the source's project. Same normalized text (trimmed, case-insensitive) already active for that scope → success, no new row; same text deleted for that scope → `Conflict` "Theo deleted this habit".
- Cited habits are stored in `chief_answers.habits`; an inactive or out-of-project habit → `Invalid` before the budget is charged.
- Context record `{kind:"habits", source:"theo_habits_not_instructions_from_agents", habits:[{id, text, project, source}]}` (active, newest 100) in every wake that has stopped sessions.
- Wake text adds, exactly: `Habits: the habits record lists Theo's habits. Use the ones that apply to the stop's project or everywhere, and name the ones you relied on in "habits":[<ids>] (at most 5) on answer or answer_form. No habit makes a security or scope question answerable: escalate those whatever the habits say. When Theo answers an escalation, or answers an agent himself in a bundle, and that shows a lasting preference, record it via {"action":"add_habit","text":<the preference, <=500 characters>,"project":<project or null for everywhere>,"source_id":<that answer or stop id>,"reason":<<=300 characters>}. Never re-add a habit Theo deleted.`
- Bundle instruction files: `<project root>/AGENTS.md`, `<project root>/CLAUDE.md` when present; masked, escaped, ≤ 4,000 chars each; missing files are skipped; read errors are a WARN, not a failed bundle.

- [ ] **Step 1: Failing tests** — `the_chief_adds_a_habit_from_theos_answer`; `the_chief_cannot_re_add_a_deleted_habit`; `an_existing_habit_is_not_added_twice`; `an_answer_records_the_habits_it_used`; `an_answer_cannot_cite_a_habit_of_another_project`; `an_answer_cannot_cite_a_deleted_habit`; `the_context_lists_the_active_habits`; `a_bundle_carries_the_projects_instruction_files_masked_and_capped` (token masked, 50 KB → 4,000 chars); osd `a_stop_wakes_the_chief_with_habits_and_instructions`.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement** as listed.
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `feat: let the chief learn and cite Theo's habits`

### Task 3: Habits page, overrule preview and habits on answers

**Files:** Modify `apps/cockpit/src/chief/view.rs` (`Page::Habits`, root row, `h` from root, rows, `x`/`u`/`p`/Enter, selection after delete, empty text; overrule review line; answer row line 3), `apps/cockpit/src/app/chief_ui.rs` (seen-at file read/write; submit the habit host actions), `README.md` (Chief section: habits). Test: `apps/cockpit/tests/chief_ui.rs`.

**Interfaces:**

- Consumes: Task 1 `habit` records, answer `habits`, host actions, `habit_text`; plan 3 session names / project colours.
- Overrule review adds, under the exact text: `Adds a habit: <habit_text(correction)> · <project name | everywhere>`.
- Answer row line 3: `<delivery> · <reason> · habits: <first habit text…> +N` (only when the answer cites habits).
- `x` submits `DeleteHabit` at once (no review), shows `Deleted · u undo`; `u` (only right after) submits `RestoreHabit`; any other key clears the undo offer. `p` submits `RescopeHabit`; Enter opens a Detail with the source (overrule correction or Theo's answer text, session name, time).
- Opening the Habits page writes `chief-habits-seen-at`; `· K new` and the `new` tags use the value read when the page opened.

- [ ] **Step 1: Failing tests** — `the_root_shows_habits_and_how_many_are_new`; `h_opens_the_habits_page`; `habit_rows_show_new_text_scope_source_and_time` (purple bold `new`, project colour, `everywhere`); `an_empty_habit_list_explains_where_habits_come_from`; `x_deletes_at_once_and_u_undoes`; `after_deleting_a_habit_the_next_one_is_selected`; `deleting_the_last_habit_shows_the_empty_text`; `p_toggles_a_habit_between_its_project_and_everywhere`; `the_overrule_review_previews_the_exact_habit_it_stores` (long correction → same cut text as `habit_text`); `an_answer_row_names_the_habits_it_used`; `opening_the_habits_page_clears_new`.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `feat: show and edit Theo's habits in the chief view`

### Task 4: New habits in the daily digest

**Files:** Modify `apps/cockpit/src/chief/store.rs` (`answers_since` → also `habits: Vec<DigestHabit>`, `new_habits: u32`), `apps/cockpit/src/costs.rs` (digest section; write `chief-habits-seen-at` after a printed habits section), `README.md` (digest). Tests: `apps/cockpit/tests/chief_store.rs`, `apps/cockpit/tests/costs.rs`, `apps/cockpit/src/tests.rs`.

**Interfaces:**

- Consumes: Task 1 `chief_habits`; plan 3 `ChiefDigest`, `ChiefSection`, `chief-digest-at`.
- `pub struct DigestHabit { pub text: String, pub project: Option<String> }`; habits created after the digest since-time, active only, newest 3; `new_habits` = all of them.
- Digest lines after the Chief answers block: `New habits: N`, then up to 3 lines `<text> · <project name | everywhere>` (one line, `…`). Nothing when N = 0. Database unavailable → the existing `Chief: unavailable (…)` line only, and the seen-at file is not written.

- [ ] **Step 1: Failing tests** — `the_digest_lists_new_habits`; `the_digest_shows_at_most_three_habits`; `a_digest_without_new_habits_has_no_habit_lines`; `a_printed_digest_clears_new_habits`; `an_unavailable_database_keeps_habits_new`.
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `make test`.
- [ ] **Step 5: Commit** `feat: list new habits in the daily digest`
