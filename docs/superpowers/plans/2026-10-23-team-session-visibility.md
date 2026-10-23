# Team Session Visibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans for the recruited Developer to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This plan needs Theo's review before product edits.

**Goal:** Make linked sessions, their activity and their handoffs visible beneath shared task headings in the existing desktop sessions panel.

**Architecture:** Extend the existing read-only team snapshot, retain it on App's existing poll cadence, and build explicit selectable rows for team headings and real sessions. Keep all terminal actions resolved by session identity rather than treating a team heading as a session index. Use a small pure grouping module only for this view; preserve the existing team modal and session action handlers.

**Tech Stack:** Rust, Ratatui, serde_json, rusqlite; existing App fixture and team store tests. No dependencies or manifest changes.

**Spec:** `docs/superpowers/specs/2026-10-23-team-session-visibility-design.md`; approved frames 47 and 119 in `team-visibility.figcraft`; `DESIGN.md` and `design/tokens.*`.

## Global Constraints

- Preserve standalone sessions, existing terminal focus and existing session actions.
- Existing left/right focus behavior stays unchanged.
- Membership comes from the daemon team snapshot, not labels, branch names or cwd guesses.
- `Turn ended` is not task completion. Review approval never implies applied or deployed.
- No dependencies or manifest changes; no schema migration; no Makefile, hook, launcher or Kitty edits.
- User-written/agent-written text remains escaped. Refreshing, folding and navigating must not wake sessions or open a different terminal accidentally.
- FigCraft pixel dimensions describe the reference canvas, not new runtime font or CSS configuration.

## Review Focus

- Same task IDs in different projects and identical human titles must never merge teams (Task 1/2 tests).
- A collapsed member must remain the correct target for search, next-needing-me and terminal actions (Task 3 tests).
- Missing snapshots, planned QA, sleeping members and absent activity must not become false success or false live members (Task 1/2 tests).
- A refresh, restart or disappearance must preserve identity-based selection and never act on the next numerical index (Task 3 tests).
- Long Unicode names, terminal control characters and small buffers must not overwrite reasons, escape sanitization or panic (Task 2/3 tests).

---

### Task 1: Expose task-scoped handoff and standing-QA evidence

**Files:** Modify `apps/cockpit/src/teams/store.rs`; test `apps/cockpit/tests/team_store.rs`. Inspect `apps/osd/src/teams.rs::host_snapshot` without changing its authentication rules.

**Interfaces:** Preserve `store::snapshot(connection: &Connection, task: Option<&TaskKey>, after: Option<i64>, limit: usize) -> Result<Value, TeamError>`. Add `handoffs` and `qa_plans` arrays. Handoff entries carry `handoff_id`, `project`, `task_id`, `developer`, `status`, `created_at` and the stored JSON `payload`; QA plans carry `project`, `task_id`, `agent`. Add `project` and `task_id` to escalation entries. Existing fields remain compatible. Current handoffs/plans must remain present even when recent history exceeds the supplied limit.

- [ ] Write store-entry-point regression tests named `snapshot_exposes_current_handoffs_and_qa_plans`, `snapshot_scopes_handoff_evidence_by_project_and_task`, and `snapshot_preserves_current_handoffs_beyond_history_limit`. Create data through the existing recruitment/handoff helpers, not unrelated production DB writes. Assert exact task identities, current versus superseded handoffs, absent QA plans and escalation attribution.
- [ ] Run `cargo test --manifest-path apps/cockpit/Cargo.toml --test team_store snapshot_`; confirm the new assertions fail before implementing fields.
- [ ] Extend the existing snapshot SQL/serialization only. Keep the same task filter and protected connection boundary; expose only data necessary to explain status. Do not parse free-form findings as approval.
- [ ] Rerun the filtered command and `cargo test --manifest-path apps/cockpit/Cargo.toml --test team_store`; require zero failures.
- [ ] Commit only this task's verified files on the current feature branch if its repository authorization permits it.

### Task 2: Group authoritative membership and render readable team rows

**Files:** Create `apps/cockpit/src/teams/sidebar.rs`; export it from `apps/cockpit/src/teams.rs`; modify the sessions rendering/checking portions of `apps/cockpit/src/app.rs`; test `apps/cockpit/src/tests.rs` and `apps/cockpit/tests/team_ui.rs`.

**Interfaces:** Add `teams::sidebar::SidebarRow` with explicit `Team { task: TaskKey }`, `Session { session_id: String }`, and non-terminal `PlannedQa { task: TaskKey }`/`UnavailableMember { task: TaskKey, session_id: String, role: Role }` variants. Add `pub fn grouped_rows(snapshot: &Value, sessions: &[Session], collapsed: &HashSet<String>) -> Result<Vec<SidebarRow>, TeamError>`; collapsed keys are serialized `TaskKey`, not titles. All sessions must appear once, with team headers interleaved inside existing project order. Keep summaries/data derivation in this one view module rather than introducing services or generic mapper layers.

App retains the snapshot and an explicit loading/ready/error status beside `team_checked`; keep the existing ten-second cadence. Optional new fields missing from an older snapshot mean evidence unavailable, not a fabricated empty handoff history. App owns the rendered target vector and selected target identity; `self.sessions` still represents actual sessions. Use existing `Session` activity fields and timestamp semantics. Reuse `selected_session()` as the action boundary in Task 3.

- [ ] Add App fixture/buffer tests `team_sidebar_groups_members_without_duplicate_cards`, `team_sidebar_keeps_identical_titles_in_separate_tasks`, `team_sidebar_distinguishes_planned_qa_and_missing_members`, `team_sidebar_reports_unavailable_links_without_hiding_sessions`, and `team_sidebar_does_not_infer_activity_or_completion`. Assert visible group titles, Designer/Developer/QA roles, exact member identities, missing-report copy, error copy and no accidental terminal calls. Use the existing `team_host`/`team_snapshot` fixture rather than a new mock transport.
- [ ] Add `team_sidebar_title_uses_task_entry_or_exact_id` and `team_sidebar_attention_filter_keeps_team_context`. Assert that conflicting session labels cannot rename a task, missing task titles show the exact task ID, identical IDs across projects do not borrow titles, and a task-only blocker remains visible with all actual teammates counted.
- [ ] Run `cargo test --manifest-path apps/cockpit/Cargo.toml --lib team_sidebar`; verify new expectations fail.
- [ ] Implement the minimal grouping and cached snapshot consumption. Resolve project identities with the daemon's verified session mapping, including existing launch/conversation aliases. Count actual linked members separately from a planned reviewer. Attribute attention by task; retain the global team modal for exact approval/message details. Use explicit evidence when showing waiting/handoff labels; display unknown when evidence is missing.
- [ ] Resolve task heading copy from the exact existing todo/issue entry, falling back to `task_id`; no new snapshot title field or member-label heuristic. Group from the complete session set. In attention-only mode include complete groups with member or task attention, then filter truly independent sessions with the existing predicate.
- [ ] Render the approved terminal hierarchy with existing `Line`/`segment`/ANSI helpers. Preserve project headings, status glyphs and independent rows. Selected member text must use `FG` on `BG_HIGHLIGHT` without dim secondary text. Use terminal display width for ellipsis and preserve full text in details. Add no theme loader/CSS dependency.
- [ ] Rerun the filtered command and `cargo test --manifest-path apps/cockpit/Cargo.toml --test team_ui`; require zero failures. Inspect buffers at the default column width and at narrower widths.
- [ ] Commit only verified Task 2 files under the existing Git authorization.

### Task 3: Make grouped selection safe for every session action

**Files:** Modify `apps/cockpit/src/app.rs`; test `apps/cockpit/src/tests.rs`; keep grouping adjustments in `apps/cockpit/src/teams/sidebar.rs` if necessary.

**Interfaces:** Preserve `App::selected_session(&self) -> Option<Session>` but resolve it through the selected `SidebarRow` identity. Team/planned/unavailable rows return `None`. Keep `handle_sessions(&mut self, key: Key, screen: &mut dyn Screen) -> String`; handle Enter on team headings before session-specific commands. Store collapsed task keys in App memory, not a new state-file format.

- [ ] Add tests `team_sidebar_enter_toggles_without_opening_or_waking`, `team_sidebar_next_attention_expands_the_correct_team`, `team_sidebar_search_finds_a_collapsed_member`, `team_sidebar_refresh_preserves_selected_session_identity`, `team_sidebar_non_session_rows_cannot_run_session_actions`, and `team_sidebar_unicode_and_small_buffers_preserve_attention`. Assert exact logged session targets, unchanged terminal-focus behavior, unchanged standalone behavior, preserved sleep/restart handling and escaped control characters. Include pinning, attention-only filtering and disappearance in table-driven cases.
- [ ] Add `team_sidebar_next_attention_reaches_task_only_blockers`: with a collapsed task containing an escalation and no actionable member, `n` selects/expands the heading, Enter only folds/unfolds, and `i` opens the existing team modal without a terminal action.
- [ ] Run `cargo test --manifest-path apps/cockpit/Cargo.toml --lib team_sidebar`; confirm failures before changing navigation.
- [ ] Route every selected-session consumer through the identity boundary. Audit the direct `self.sessions.get(self.panels[1].selected)` and `self.sessions[index]` sites in build, selected details, details popup, search and next-attention; audit mouse row targeting and panel selection too. A header must not inherit an adjacent member's restart/push/exit action.
- [ ] Preserve selection by session/task identity on refresh; explicitly expand a collapsed team when search or `n` chooses its member. Collapsing a group selects its heading without changing the viewed terminal. A disappearing session must not trigger a side effect solely because another row inherits its index.
- [ ] Run `cargo test --manifest-path apps/cockpit/Cargo.toml --lib` and `cargo test --manifest-path apps/cockpit/Cargo.toml --test team_ui`. Then run repository `make test` with the required current-message/host approval. Record verbatim results, including failures; do not claim completion on filtered tests alone.
- [ ] Commit verified Task 3 files. Hand the completed commit, exact tests and FigCraft frame references to QA with `cockpit-team handoff`; do not apply an unreviewed implementation. Use host `ask-to-run` for any eventual `cockpit apply` and host test as required by project instructions.

## Handoff and execution gate

Recommended execution is one recruited Developer working through these dependent tasks, followed by fresh QA. The task boundaries share selection and snapshot interfaces, so parallel product edits would increase integration risk. Ask Theo to review this plan before the Developer begins; a recruitment brief must not be presented as personal authorization for installs, builds, git pushes or host commands.

Builder-check questions were resolved as explicit proposed calls in the spec: exact task-entry title with ID fallback; task-only attention selects its heading and uses `i` for the existing team modal; attention-only filtering keeps the entire affected team as context. Theo should review these with the plan before execution.

Design-only handoff verification: confirm frames 47/119 are ready for dev, JSON/CSS exports match the approved palette, DESIGN.md lint and DTCG export pass, and no product source changed. Live terminal behavior remains an implementation/QA check, not a property of the mockup.
