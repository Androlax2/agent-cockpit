# Stable Session Navigation Implementation Plan

> **For agentic workers:** Use the `executing-plans` skill to implement this plan task-by-task in this session. Steps use checkbox (`- [ ]`) syntax for tracking. Do not delegate or write to git without fresh authorization.

**Goal:** Make active sessions easy to identify and navigate without moving rows when their activity changes.

**Architecture:** Keep the existing session registry, project grouping, cached polling, and terminal controls. Derive presentation from existing `Session` fields; add fixed summary and selected-details areas around the scrolling session list. No agent-side hooks, model calls, new dependencies, or new activity-reporting protocol in this version.

**Tech Stack:** Rust, ratatui, crossterm, existing fzf picker, existing cockpit test fixtures.

**Execution record:** Approved and implemented locally. Final unit suite: 296 passed, 0 failed; full `make cockpit test`: 1058 passed, 0 failed. Fresh review's two short-terminal findings were fixed with RED→GREEN regressions and a full-suite rerun. Commit and live apply remain pending authorization. Detailed progress: `.superpowers/sdd/2026-10-20-stable-session-navigation/progress.md` (local).

**Spec:** The approved design and implementation decisions below are self-contained. Visual evidence: `.superpowers/brainstorm/3324111-1792497058/content/stable-attention.html` (local, untracked mockup; not a runtime dependency).

## Approved design

- Keep A's stable project/session order, with B's attention cues.
- Use the overall task as the primary label and current activity as a separate secondary line.
- Show a fixed attention summary; highlight requests in place, without flashing.
- Provide “Next needing me”; move selection, not the list order.
- Show routine technical details for the selected session instead of repeating badges across every row.

## Proposed first-version decisions — review before implementation

1. **Honest activity:** show `Approval needed`, `Input needed`, `Needs you`, `Command running`, `Working`, `Ready`, or `Status unknown`. The mockup's “Testing”, “Investigating”, and “Designing” were illustrative; current reporting does not establish those activities. Generic `needs-you` is not necessarily a question, so do not promise question counts.
2. **Task names:** prefer the agent's generated session title over the full to-do text. Fall back to the linked to-do title, then branch/project. Never use the last user reply as task identity. Long labels are cell-width clipped with an ellipsis; selected details expose the complete title. This is not semantic summarization: a verbose native title can remain verbose.
3. **Find:** `/` opens the existing fzf-style picker over task and project names, then selects the match in the unchanged list. This adapts the mockup's inline search to existing terminal interaction patterns without a second filtering state.
4. **Next needing me:** retain existing `n` preview behavior (`claude-sessions view`), but target only actual attention requests, including approval/input while the agent is otherwise `working`. Plain `Ready` sessions are not requests. `Enter` retains its existing open/focus behavior.
5. **Information priority:** attention first; routine restart/update, branch, agent, model, effort, context, app, billing, queued-work and commit information lives in selected details and existing menus. Existing global failure/security notices remain visible; do not silently discard them.

## Global Constraints

- Keep stable project/session order; state changes must not reorder sessions.
- Preserve pins, terminal-session behavior, external-session exclusion, and current manager commands.
- No fabricated activity, last-tool-as-current-work inference, or last-message-as-title fallback.
- No dependency manifest, launcher, shell script, Makefile, or agent configuration changes.
- No extra filesystem/database/process calls on a keypress or draw; use the gathered snapshot.
- Use existing palette tokens; amber plus `!`/text indicates attention, blue indicates selection. Color is not the only signal.
- Keep keyboard navigation and mouse hit targets usable in narrow and short terminals.
- This plan does not authorize implementation, commits, installation, or reloads.

## Review Focus

1. Approval/input combined with `working`: count and navigate it as needing attention; do not wait for the agent's base state to change.
2. Ready/unknown/external sessions: do not count or jump to them as requests; never hide unknown status as healthy readiness.
3. Long Unicode task names and short terminal heights: preserve the task/request signal, cell boundaries, and a usable scrolling area.
4. Selected session exits or another session is added: retain selection by session ID where possible; never accidentally operate on a neighboring session.
5. Multiple requests for one session or changing request type: count the session once; summary, highlight and navigation must agree.

## Repository evidence

- `apps/cockpit/src/sessions.rs`: `Session::needs_attention()` already recognizes `needs-you`, `ask_to_run=approval`, and `ask_to_run=input`. `next_attention_index()` currently instead includes most non-working states; this mismatch must be corrected.
- `apps/cockpit/src/app.rs`: `build_sessions()` already groups and filters sessions but prefers linked to-do text over generated titles and repeats badges in each card. `draw_panel()` scrolls every content line, so putting a summary in `panel.lines` would not make it fixed.
- `PanelState::keep_selected_visible()` and `draw_panel()` share the scrolling height calculation. `App::draw()` also uses line counts to size panels; all three must account for fixed content.
- `Session.task` is the latest user prompt's first line, truncated to 60 characters by `session-state.sh`; it is not current agent activity. `PostToolUse` only sets `working`, even though the OpenCode plugin sends `tool_name`.
- `Session.request`/`detail` can describe a human request. `outcome` describes a completed turn, not present activity. `ask_to_run` is authoritative command-phase reporting.
- Generated OpenCode titles already come from read-only `session_v2` access; Claude titles come from `ai-title` records. Reuse `History` caching; do not add SQL in the draw path.

## File map

| File | Responsibility |
| --- | --- |
| `apps/cockpit/src/sessions.rs` | Activity presentation and consistent next-attention selection. |
| `apps/cockpit/src/app.rs` | Task-title precedence, compact cards, fixed panel chrome, selected details, shortcuts and search. Keep the existing module structure. |
| `apps/cockpit/src/tests.rs` | Fixture-driven rendering, navigation, identity, command and layout regression tests. |

Existing tests such as `cards_keep_their_place_when_states_change`, `grouped_todo_cards_keep_the_task_title_after_short_replies`, `grouped_opencode_cards_use_the_session_title_not_the_last_reply`, `ask_to_run_badges_stay_on_their_session_and_clear_when_finished`, and attention-marker tests need intentional expectation updates where presentation changes. Preserve their underlying guarantees, not obsolete badge placement.

---

### Task 1: Derive honest activity and unify attention navigation

**Files:** Modify `apps/cockpit/src/sessions.rs`; test in `apps/cockpit/src/tests.rs` and the existing `sessions.rs` test module.

**Interfaces:** Reuse `Session::needs_attention()`. Add `pub fn activity_label(session: &Session) -> &'static str` and `pub fn activity_text(session: &Session) -> String`. Preserve `next_attention_index(sessions: &[Session], selected: usize) -> Option<usize>`.

- [ ] **Write failing table-driven tests.** Snapshot a session with each relevant combination and assert its rendered label/text. Classify in this order: approval → input → `needs-you` → running command → working → waiting → unknown. `waiting` with a running command must say `Command running`, not `Ready`.

  Use `activity_labels_follow_reported_state_without_guessing` for the helper boundary:
  ```rust
  for (state, phase, expected) in [
      ("working", "approval", "Approval needed"),
      ("working", "input", "Input needed"),
      ("needs-you", "running", "Needs you"),
      ("waiting", "running", "Command running"),
      ("working", "", "Working"),
      ("waiting", "", "Ready"),
      ("unknown", "", "Status unknown"),
  ] {
      let mut row = session(SESSION_A, state, "dotfiles", "/r/dotfiles", 1);
      row["ask_to_run"] = json!(phase);
      let reported = sessions::Session::from_value(&row).unwrap();
      assert_eq!(sessions::activity_label(&reported), expected);
  }
  ```
- [ ] **Pin attention navigation through real keys.** With ready A, working B carrying approval, working C, needs-you D, and external E, pressing `n` selects/previews B then D then B. Assert no preview of A/C/E and no change to session ID ordering. With no requests, assert message `nothing needs you` and no manager command. A single request wraps to itself.
- [ ] **Run the new tests and observe the expected failures.** From `apps/`: `cargo test -p cockpit --lib attention`, then `cargo test -p cockpit --lib activity`.
- [ ] **Implement the helpers and narrow the candidate predicate** to `!session.is_external() && session.needs_attention()`. Labels must use the exact values in decision 1. `activity_text` uses collapsed request detail only while `state == "needs-you"`; otherwise use neutral phase/state copy. Never display a stale request/outcome as current work.
- [ ] **Re-run both commands.** All tests must pass, including cases where approval/input arrives while working and where unknown phase values fall back to a known base state.
- [ ] **Review the diff.** Commit only if separately authorized.

### Task 2: Separate stable task identity from activity and routine detail

**Files:** Modify `apps/cockpit/src/app.rs`; test `apps/cockpit/src/tests.rs`.

**Interfaces:** Add `fn session_task_title(&self, session: &Session, branch: &str) -> String` on `App`. It consumes existing cached `Session.title`, linked `self.todos.items`, branch, and project. Store its results in `App::session_task_titles: HashMap<String, String>` keyed by session ID for rows, details and Task 4's picker. Add `branches: HashMap<String, String>` to `Gathered` and `session_branches: HashMap<String, String>` to `App`; collect branches in `gather_sessions`, not on a keypress. Initialize maps empty in `App::new`. Consume Task 1's activity helpers.

- [ ] **Write failing fixture/render tests.** Cover a long linked to-do and shorter native title (native wins); empty native title (to-do wins); no title/to-do (branch/project wins); short reply `Yes` (never becomes the identity); and both single-session and grouped projects (same task-first hierarchy).

  Name the precedence test `native_titles_identify_tasks_before_long_todo_prompts`; with a native title `Free disk space` and a longer linked to-do, assert:
  ```rust
  assert_eq!(app.session_task_titles[SESSION_A], "Free disk space");
  assert!(!app.panels[1].lines.iter().any(|line| line.texts() == "Yes"));
  ```
- [ ] **Test compact-row guarantees.** Each card has exactly two content rows: bold task title and muted activity. Need-for-input markers and labels remain distinguishable when selected. Unselected rows do not reserve space for agent/effort/git/restart badge trains. Complete technical data stays available to the selected-details builder rather than being dropped.
- [ ] **Run `cargo test -p cockpit --lib title` and the new compact-card tests; observe failures.**
- [ ] **Implement title precedence and the two-line card rendering.** Use the existing `fit`/`width_of` cell-width functions, not byte slicing or automatic word deletion. Do not add a second generated-title cache or alter native titles in storage. Carry both complete title and complete activity in selected details when clipping occurs.
- [ ] **Re-run title and card tests.** Include CJK/combining characters and two similar prefixes: the selected detail must still let the user distinguish them.
- [ ] **Review the diff.** Commit only if separately authorized.

### Task 3: Add fixed summary and selected-session details without breaking scroll

**Files:** Modify `apps/cockpit/src/app.rs` (`PanelState`, `draw`, `draw_panel`, `build_sessions`, `handle_click`); test `apps/cockpit/src/tests.rs`.

**Interfaces:** Add `fixed_top: Vec<Line>` and `fixed_bottom: Vec<Line>` to `PanelState`, empty for other panels. Add `fn session_summary_lines(&self) -> Vec<Line>` and `fn selected_session_detail_lines(&self) -> Vec<Line>` on `App`; consume Tasks 1–2. Add `fn content_height(&self) -> usize` on `PanelState` for panel allocation. Keep scroll indices relative to `lines`, not fixed regions.

- [ ] **Write failing buffer tests at 120×40, 80×24 and 40×12.** Render enough cards to scroll; select the last session. Assert the attention summary stays at the same panel position, the selected card is visible, and fixed detail never overlaps scrolling content or borders. Other panels' scrolling/collapse behavior must be unchanged. With fewer than three interior rows, show the minimal summary and available card space, omitting detail first.
- [ ] **Test counts.** Fixture categories must produce `2 need you`, `4 working`, `3 ready`. Count each visible session once; approval/input takes precedence over working. Keep unknowns visible with an explicit unknown count. Zero requests says `Nothing needs you`; an empty list has zero counts and no stale detail.

  Name the fixed-region test `attention_summary_stays_visible_while_session_cards_scroll`. After selecting the final session and rendering its short viewport, assert:
  ```rust
  assert!(app.panels[1].scroll > 0);
  assert!(app.panels[1].fixed_top.iter().any(|line| line.texts().contains("2 need you")));
  ```
  Also assert the corresponding buffer row remains at the same screen coordinate before/after scrolling; a field-only assertion is insufficient.
- [ ] **Test click mappings.** Summary `Next needing me` invokes the same action as `n` and never activates the selected session's `Enter` action. Selected effort remains clickable in its new detail position. Blank fixed regions do not select or open rows.
- [ ] **Run `cargo test -p cockpit --lib` and observe the new failures.**
- [ ] **Implement fixed regions and height accounting.** Finalize `self.sessions` before building summary/details so they never read the previous tick's list. Keep the attention summary to one row and the shortcut/count row to one row. Priority in narrow space: attention count, `n` hint, then other counts. Limit selected details to three rows; expose the unabridged source title, branch, activity/request, and existing technical indicators without overwriting the task/activity rows (the terminal renderer may still cell-clip at its viewport width). Remove the repeated global multiline restart banner in favor of a compact count and selected-session detail; retain existing actions and non-routine failure/security notices.
- [ ] **Preserve amber attention in selected rows.** Update `draw_panel`'s selection recoloring rules, currently preserving red/blue. Keep `!` and the text label; use blue selection styling without erasing the attention signal. No animation.
- [ ] **Re-run `cargo test -p cockpit --lib`.** Update presentation-specific tests to assert the new locations while retaining all command/capability constraints.
- [ ] **Review the diff.** Commit only if separately authorized.

### Task 4: Preserve selected identity and add Find without shuffling the list

**Files:** Modify `apps/cockpit/src/app.rs`; test `apps/cockpit/src/tests.rs`.

**Interfaces:** Add `fn find_session(&mut self, screen: &mut dyn Screen) -> String` on `App`. Reuse existing `fzf(prompt, lines, extra)` via `screen.outside` and Task 2's title helper. Picker entries carry a session ID, not an index or title used as an identifier.

- [ ] **Write failing navigation tests.** `/` opens a picker with project and task text; choosing a duplicate-titled session selects the correct ID. Cancel leaves selection and order unchanged. It must not run `open`, `view`, or any approval command just by selecting a search match.

  Name the duplicate-title test `finding_a_session_selects_its_id_without_opening_it`; after the fixture's fzf shim chooses `SESSION_B`, assert:
  ```rust
  assert_eq!(app.sessions[app.panels[1].selected].session, SESSION_B);
  assert!(fixture.calls("claude-sessions").is_empty());
  ```
- [ ] **Write selection-lifecycle tests.** Attention changes, new sessions, pin changes, and redraws keep the selected ID when it still exists. If it exits, clamp to a remaining item and clear old details/hit targets; no automatic manager command is sent. Stable means state changes do not sort sessions; explicit pinning may still reorder them.
- [ ] **Run the new tests through `cargo test -p cockpit --lib`; observe failures.**
- [ ] **Implement search and identity retention.** Bind `/` only in Sessions and preserve the other panels' keys. Use cached branch/title presentation, not a fresh git lookup on the keypress. After selecting, let the normal keep-selected-visible path scroll to the item. Update footer/help/palette copy: `n` is `Next needing me`, not `peek` at any idle session.
- [ ] **Re-run `cargo test -p cockpit --lib`.** Verify that repeated `n` actions change only selection/view, never session order or approval state.
- [ ] **Review the diff.** Commit only if separately authorized.

### Task 5: Verify the complete terminal surface and prepare an authorized live apply

**Files:** Tests and the implementation files above only; no build-wrapper changes.

**Interfaces:** No new runtime interface. Consumes the finished navigation surface and the existing dotfiles apply workflow.

- [ ] **Run the complete suite using the repository's expected binary layout:** from the clone root, `make cockpit test`. Explicit build permission is required before running this command. This builds the release binaries that the shell-wrapper integration tests expect; do not mistake missing wrapper binaries for navigation failures or disable those tests.
- [ ] **Inspect the rendered fixture and real terminal at wide, narrow and short sizes.** Check fixed summary, long title/request clipping, selected effort hit target, non-color attention recognition, and navigation when nothing needs the user. Do not invent live sessions or approvals just to populate the UI; use fixtures for those states.
- [ ] **Report exact test failures/warnings and `git diff --stat`.** Existing warnings are not authorization to edit unrelated code. No claims that mockup “Testing” or “Investigating” labels became live telemetry.
- [ ] **Request fresh authorization to commit and apply.** Prior authorization covered the already-applied external-session fix, not this redesign. Do not stage, commit, export or reload on the strength of that old approval.
- [ ] **Once authorized**, synchronize with the live `origin/main`, preserve others' work, commit only scoped changes, export the relevant patches, and request `claude-sessions apply-dotfiles <clone>` through `ask-to-run`. After successful approval/build, run `make -C ~/dotfiles cockpit test` and then `claude-sessions reload` through `ask-to-run`. Surface any failures before proceeding.
- [ ] **Confirm the live result with the user.** A request should be easy to find using `n`; its row stays in the same project position. Session records and ordinary terminal controls remain intact.

## Self-review

- Approved layout maps to Tasks 2–4; reliable activity and matching attention behavior map to Task 1.
- Each Review Focus case has explicit fixture/render/key-path coverage in Tasks 1–4.
- Title helper, activity helpers and fixed-region interfaces are named once and reused consistently.
- No runtime coupling to the HTML mockup, no new reporting layer, and no modifications to host-executed scripts.
- Remaining approval question: accept the five first-version decisions, particularly honest generic activity instead of invented detailed activity and native-title-first rather than new semantic summarization.
