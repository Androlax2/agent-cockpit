# TODO Kinds Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add six fixed TODO kinds, an easy picker, visible labels, and a kind filter.

**Architecture:** Keep the existing JSON `kind` field for special Design and Blocked routing. Store the new user-facing classification separately as `category`, exposed as `--kind` in the CLI. Extend the existing Rust terminal UI and fzf pickers; no new dependencies or new UI framework.

**Tech Stack:** Rust, serde_json, existing cockpit terminal rendering and fzf.

**Spec:** User-approved Option A in this conversation: Task, Feature, Bug, Improvement, Chore, Research; picker on add/edit; compact text badge; kind filter; CLI `--kind bug`; preserve Design and Blocked behavior.

## Global Constraints

- Exact category identifiers: `task`, `feature`, `bug`, `improvement`, `chore`, `research`.
- Existing uncategorized items are Task without requiring a migration.
- Preserve `kind: design` and `kind: blocked`, dispatch, queueing, approval, priorities, notes and status tabs.
- No dependency-manifest changes, installs, builds, deployment, or git writes without separate authorization.
- Reuse terminal UI conventions; no web UI or FigCraft files exist in this checkout.

## Review Focus

- Invalid or missing CLI kind values must fail before an update is sent; duplicate flags must not silently override.
- Crafted updates must not create an item with an unsupported category or alter routing.
- Cancelling an add/edit picker must leave the list unchanged.
- Filtering must not silently affect queue dispatch or hide all items without a visible active-filter label.
- Existing and unknown persisted categories must remain visible in the unfiltered list, with no destructive normalization.

## Implementation choices

### Execution record

- Pre-flight: Task 2 and Task 3 consume Task 1's Category and setter; interfaces agree. Worktree branch is `todo-208f0230`; git writes remain unauthorized.
- Ruling: skill bookkeeping scripts are missing (exit 127), reported through `claude-todo blocked`; progress is retained here instead. Cost: no generated task briefs or ledger automation.
- Task 1: complete. RED: missing Category/accessors/setter; GREEN: `cargo test --manifest-path apps/Cargo.toml -p cockpit --lib` → `265 passed; 0 failed`. Existing unrelated unused-mut warning at tests.rs:185 remains.
- Ruling: integration tests invoke wrappers fixed to `apps/target/release`; use `cargo test --release` (as the repository Makefile does). Debug-only CLI test run had 17 failures because release binaries were absent, not because of product behavior. Cost: longer initial test compilation.
- Task 2: complete. Release RED: 3 new behavior tests failed, 14 passed; GREEN: release library 265 passed and CLI 17 passed, 0 failed.
- Ruling: use uppercase `K` for changing kind; lowercase `k` is consumed by global up-navigation before Todo actions. The first UI run exposed that collision. Cost: Shift is needed for the shortcut; existing navigation remains intact.
- Task 3: complete. UI RED: 10 new tests failed, 265 passed; GREEN: 275 passed, 0 failed after fixing the shortcut collision.
- Final review: a fresh read-only reviewer found three P2 issues (blocked updates bypassing category validation, badges hiding titles at width 40, active filter hidden behind long project names). Added three regression tests, watched all three fail, and fixed them in one pass. Library GREEN: 278 passed, 0 failed; CLI GREEN: 17 passed, 0 failed.
- Final verification: `cargo test --release --manifest-path apps/Cargo.toml --no-fail-fast` → 993 passed, 15 failed, 0 ignored across the workspace. Ran the identical command against an untouched `git archive HEAD` snapshot under `/tmp/opencode/todo-kinds-baseline`; the exact same 15 tests fail there. No new failures. Full outputs: `/tmp/opencode/todo-kinds-final-suite.log` and `/tmp/opencode/todo-kinds-baseline-suite.log`.
- `git diff --check`: passed. No dependency manifests, host scripts, git history, live installation, or running services changed. Source changes under `apps/` need host application/reload before the running cockpit shows them.
- Handoff: initial commit `894ca98` exported successfully, but host application refused it because live main had advanced. With explicit user authorization, fetched live main `140276c` and replayed only this feature (all earlier inherited commits were already present upstream). Resolved three app.rs overlaps by retaining new session fields, default-agent help, and the newer metadata truncation logic while adding the Todo title-space budget. No unrelated live changes were discarded.
- Rebased verification: `python3 config/bin/.local/bin/agent-build cargo test --release --manifest-path apps/Cargo.toml --no-fail-fast` → 1296 passed, 8 failed, 0 ignored. Cockpit library: `363 passed; 0 failed`; TODO CLI: `17 passed; 0 failed`. Remaining failures are the same `claude_remote` test and seven `claude_sessions_script` tests already reproduced on unchanged baseline above; newer main fixes the other seven historical failures. `python3 -m unittest discover -s tools/tests -p test_agent_build.py` → `Ran 12 tests` / `OK`. Full Rust output: `/tmp/opencode/todo-kinds-rebased-suite.log`.

#### Existing workspace failures (reproduced on unchanged HEAD)

```text
claude_remote:
  yes_and_no_from_the_phone_press_the_dialog_keys
test result: FAILED. 42 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.18s

claude_sessions_script:
  compact_is_typed_only_at_the_prompt
  exit_types_slash_exit_at_an_idle_prompt
  handoff_types_rc_at_an_idle_prompt
  prompt_commands_pause_between_the_text_and_its_enter
  restart_exits_then_resumes_the_same_conversation
  say_sends_at_the_prompt_and_queues_while_working
  say_refuses_while_a_dialog_is_open_and_keeps_the_text_out_of_the_log
test result: FAILED. 159 passed; 7 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.95s

session_state_hook:
  a_session_started_from_a_shell_reports_even_under_the_daemon
test result: FAILED. 24 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.70s

desktop --lib:
  tests::a_credits_session_is_marked_in_the_snapshot_and_the_reboot_record_until_it_switches_back
  tests::a_request_held_or_refused_is_audited_without_its_command
  tests::a_sessions_agent_is_in_the_snapshot_and_the_reboot_record_and_claude_by_default
  tests::a_terminal_session_is_marked_in_the_snapshot_and_the_reboot_record
  tests::sessions_are_remembered_across_reboots
  tests::the_daemon_writes_its_messages_to_the_shared_log
test result: FAILED. 120 passed; 6 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.60s
```

#### Final implementation choices

- Use a small `todo::Category` enum with `ALL`, `as_str()`, `label()`, and `parse(&str) -> Option<Self>` shared by the CLI and UI.
- `Item::category() -> Option<Category>`: missing category is Task; unknown stored category is None. Show the raw unknown value in details rather than rewriting it.
- GitHub issues remain managed on GitHub, are visible under All, and are excluded from specific category filters; no guessed label-to-category mapping.
- Add a kind picker using existing fzf after editing a title/notes and before saving. Preselect the current category for edits; Task for additions. Escape cancels the entire operation.
- Add `K` to change the selected typed item's kind without opening its editor and `f` to pick All or a category. Filters are scoped per viewed session, like existing status tabs; All is the initial filter.
- Show a compact kind label for each typed item, including Task, without replacing Design/Blocked labels. Keep the active filter visible in the subtitle and counts consistent with the filter. If a newly added/edited item no longer matches the active filter, reset to All so the saved item is visible.

### Task 1: Persist validated classifications without changing routing

**Files:** `apps/cockpit/src/todo.rs`, `apps/cockpit/src/tests.rs`.

**Interfaces:** Produce `todo::Category` as above, `Item::category() -> Option<Category>`, and `TodoList::set_category(&mut self, id: &str, category: Category) -> bool`.

- [x] Add failing tests through list add/edit/save/load and `apply_updates`: all six categories survive; missing is Task; unknown persisted values survive; malformed/unsupported incoming categories reject the whole add/propose; Design and Blocked routing survives recategorization.
- [x] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --lib` and confirm the new tests fail for the missing behavior.
- [x] Implement the enum, accessors, setter, and validation of an optional `category` update field before adding an item. Keep missing categories omitted for backward compatibility. Explicit non-string values are invalid.
- [x] Rerun the library tests and verify passing results.

### Task 2: Expose kinds through the real CLI

**Files:** `apps/cockpit/src/bin/claude-todo.rs`, `apps/cockpit/tests/claude_todo.rs`.

**Interfaces:** Consume `todo::Category`; `add` and `propose` accept `--kind <identifier>` and send `category` independently of `--design`. Existing `blocked` behavior stays unchanged.

- [x] Add failing executable-level tests for every kind on add/propose, combining kind with notes/why/design, explicit Task, missing/invalid/duplicate kind flags, and list/show displaying the classification. Preserve the existing test that unclassified add has no routing `kind`.
- [x] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test claude_todo` and confirm the new tests fail (then use release wrappers as recorded).
- [x] Extend argument validation and update payloads; display classifications in list/show; update usage and module documentation. Reject malformed flags with exit 2 and no socket update. Do not accidentally treat a title or notes containing `--design` as a routing flag.
- [x] Rerun CLI and library tests and verify passing results.

### Task 3: Add terminal pickers, badges, and filtering

**Files:** `apps/cockpit/src/app.rs`, `apps/cockpit/src/tests.rs`.

**Interfaces:** Consume Category and list setter. Add `todo_categories: HashMap<String, Option<Category>>` (None means All) alongside `todo_tabs`; centralize the selected category/filter application in testable methods rather than testing external fzf internals.

- [x] Add failing app-entry-point tests for badges/details, category filtering across status tabs and viewed sessions, GitHub visibility, empty filtered lists, cancellation, changing a selected kind without changing title/notes/routing, and keeping the saved item visible. Include a regression that queue dispatch ignores the UI filter.
- [x] Run library tests and confirm the new tests fail.
- [x] Add fzf pickers reusing `screen.outside`, wire add/edit and `K`/`f` actions, update action help tables, subtitle/counts and details. Verify unknown stored categories remain in All, and never overwrite one just by opening/cancelling an editor.
- [x] Run the entire workspace suite with release wrappers and report results above, including failures reproduced on baseline. Test execution is permitted; do not separately run an install/build command.
- [x] Review `git diff --check` and `git diff --stat`. No commits or deployment in this plan. Explain that changes under `apps/` require host-side application/reload before the running cockpit shows them.
- [x] After implementation and verification, marked TODO 208f0230 done.
