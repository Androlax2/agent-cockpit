# Palette Action Homes Implementation Plan

## Resume checkpoint — 26 October 2026

Implementation and independent code review are complete in this checkout. User requested a pause before delivery.

- Sessions/cards/details: `u` Updates, `Q` queue explanations, `v` acknowledges captured displayed identities without answering. Todo row/details: `T` trail; row `i` opens details, details `i` toggles manual status and preserves inspected identity. PRs: Enter review, `o` GitHub, `l` Later/bring back, `t` trail.
- Status has independently selectable service links and digest/clones utilities, preserving action identity across quota-count changes. `N` supports blank/canvas modes, preserves fields, validates links and scrolls the active field into view in short columns. Canvas uses the existing default-agent, in-project launcher.
- Removed `/`, `:`, their picker code and attention-only filtering. Grouping, Updates and Queue remain. Migrated existing tests, including `apps/osd/src/chief_tests.rs` manual-status integration; updated README key documentation.
- Independent reviewer found a Todo-details trail panic on unreadable continuity and clipped canvas launcher explanation at 40×18. Both have failing-then-passing regression tests and fixes. Added Todo details status identity/removal coverage.
- Final `make test`: `Ran 77 tests in 8.628s`, `OK`; `2778 tests run: 2778 passed (1 slow), 0 skipped`. Log: `/tmp/opencode/palette-verified.log`. Earlier migration runs failed on old bindings/selection assumptions; those were corrected, not disabled.
- Still to do: final approved-frame comparison/status updates, coordinate designer-owned `DESIGN.md` and `.figcraft` inclusion, team QA, export only unapplied commits, `cockpit apply` with host review, running-copy `make test`. Do not mark `5a82d5dd` or `86822093` done yet. No host-script or dependency changes.
- Preserve concurrent Chief commits `d8c957a` and `f225e1d`, designer changes and `.design-wt`, and pre-existing untracked configuration files. The application checkpoint is separate from designer-owned changes.

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give all thirteen remaining palette actions their approved homes before removing `:` and `/`.

**Architecture:** Reuse the Rust terminal panels, grouped details, New Session form and continuity handlers. Keep effects at explicit key handlers; no new services or dependency changes. Status selection maps to named actions; detail actions retain the inspected item identity.

**Tech Stack:** Rust, existing terminal renderer and nextest fixtures; FigCraft for approved design evidence.

**Spec:** `DESIGN.md`, “Palette action homes · approved 26 October 2026”, lines 367–403; `design-plans/cockpit-session-list.figcraft`, page 2572, ready-for-development frames 2575, 2591, 2614, 2626, 2640. All five inspected through FigCraft, including their combined screenshot.

## Global Constraints

- Preserve grouped teams, grouped session details, scrolling help, lowercase `l` Later and `k` Links.
- Reuse existing terminal styling; no new visual system or Storybook dependency for this Rust TUI.
- Keep daily digest and clone cleanup behavior and schedules; only change their UI entry points.
- Preserve Updates, archived return notes, queue explanations and revision-aware deferrals. Only the attention filter is replaced by grouping.
- Enter never approves; acknowledging displayed updates never answers or approves. Todo `Q` still queues/unqueues.
- Preserve designer-owned uncommitted `DESIGN.md`, `.figcraft` and `.design-wt`; coordinate their commit separately.
- No install, dependency, host-script or migration changes. Apply and running-copy tests retain host approval.

## Review Focus

- A selected item disappears or changes while its details are open: never act on its replacement.
- An update arrives after details opened: `v` must acknowledge only the captured displayed identities.
- PR head revision changes or is unavailable: do not hide new work under an old deferral.
- Invalid canvas link or cancellation: keep correct form state and launch nothing.
- Narrow/short terminals and no sessions/projects/data: actions and explanations remain reachable.

---

### Task 1: Sessions continuity actions

**Files:** `apps/cockpit/src/app.rs`, `apps/cockpit/src/app/continuity_ui.rs`, `apps/cockpit/src/tests.rs`.

**Interfaces:** Reuse `run_continuity_action(&mut self, name: &str, screen: &mut dyn Screen) -> String`, `review_updates` and existing session-detail identity guard. Add captured update identities to session details, passed explicitly to `review_updates` for `v`.

- [ ] Add entry-point tests: Sessions `u` opens cross-session Updates with archived notes; `Q` opens Queue even with no session selected; `v` acknowledges only displayed identities. Repeat `u/Q/v` through session details. Cover late arrivals and vanished target.
- [ ] Run the new tests with `cargo nextest run --release --manifest-path apps/Cargo.toml -p cockpit --lib -E 'test(palette_homes_sessions)'`; observe missing-key failures.
- [ ] Wire card and detail keys, help and empty/unavailable states to existing handlers. Keep queue and updates readable when empty; never reinterpret their data as grouping.
- [ ] Rerun the focused tests; all pass. Retain existing continuity persistence and delivery tests.

### Task 2: Task trails and PR Later

**Files:** `apps/cockpit/src/app.rs`, `apps/cockpit/src/app/continuity_ui.rs`, `apps/cockpit/src/tests.rs`.

**Interfaces:** Reuse `TrailTarget`, `task_key`, `DeferralTarget::review` and `change_deferral`. Extend `SessionDetails` with an optional inspected Todo key, separate from the session target. Generalize `toggle_later(&mut self) -> String` to distinguish Sessions and focused PR context.

- [ ] Add `palette_homes_trails_*` tests: Todo row/details `T` opens that task's trail; `t` remains Add. Refresh/reorder or removal cannot change the inspected Todo target.
- [ ] Add `palette_homes_pr_*` tests: `l` defers exactly 1200 seconds, keeps row visible with return time, toggles back; `t` opens its trail. Known revision changes cancel effective deferral; missing revision shows the existing explanation. Enter starts review and `o` opens GitHub. Chief log `l` remains its own action.
- [ ] Run both groups using the same nextest command with an OR filter; observe failures.
- [ ] Replace the external Todo detail pager with the existing in-column detail surface for `i`/Enter, retaining its current text. Route only `T` through the captured Todo key; existing row actions remain reachable after closing details.
- [ ] Wire PR keys and render active revision-aware deferral expiry. Update contextual hints/help; rerun tests.

### Task 3: Status Services and Utilities

**Files:** `apps/cockpit/src/app.rs`, `apps/cockpit/src/tests.rs`.

**Interfaces:** Add `StatusAction` variants for Claude status, Codex status, Digest and Clones, with an App selection map aligned to Status item indices. Reuse `status::PAGE`, `status::CODEX_PAGE`, `manager_async`, and `manager_in_background`.

- [ ] Add `palette_homes_status_*` tests selecting each row through navigation and activating Enter; assert exact existing URL/command. Quota rows must never accidentally run a utility when counts refresh. Cover error/unknown provider status and a short terminal.
- [ ] Run focused tests and observe failures.
- [ ] Make each provider independently selectable, even at wide widths; append Utilities with Daily digest and Clones review/cleanup. Preserve usage, spending, Chief and PR sections. Use named action mapping rather than raw index arithmetic at dispatch sites.
- [ ] Show selected action in footer and Enter behavior in Status help; rerun tests.

### Task 4: Canvas start in the existing N form

**Files:** `apps/cockpit/src/app/new_session.rs`, `apps/cockpit/src/app.rs`, `apps/cockpit/src/tests.rs`.

**Interfaces:** Extend the form's field enum with StartFrom and CanvasLink; retain blank-session state while switching modes. Extract the existing validated canvas launch into `launch_design_session(&mut self, directory: &str, canvas_link: &str) -> Result<String, String>`, reusing `sessions::canvas_address` and `sessions::design_prompt`.

- [ ] Add `palette_homes_canvas_*` tests for valid links, invalid links, Escape, project filtering and mode round-trips retaining blank fields/clone choice. Assert the existing `cockpit new <project> <design prompt>` call without silently adding agent/clone options. Test all three existing blank-session agents.
- [ ] Run focused tests and observe failures.
- [ ] Add the explicit start option and canvas-link field using existing form drawing/input. Canvas mode shows launcher defaults and in-project behavior rather than inactive agent/clone controls; verify actual launcher behavior in `config/bin/.local/bin/cockpit` before final copy. Keep invalid input/form open with an error; CMD+S remains submit.
- [ ] Rerun focused tests and existing New Session tests.

### Task 5: Remove obsolete entry points and deliver

**Files:** `apps/cockpit/src/app.rs`, `apps/cockpit/src/app/continuity_ui.rs`, `apps/cockpit/src/tests.rs`, `apps/cockpit/tests/chief_ui.rs`, `README.md`.

**Interfaces:** Remove `PALETTE_ONLY`, `palette_entries`, `run_palette_only`, `pick_action`, `PALETTE_FZF_OPTIONS`, `find_session` and `attention_only` after Tasks 1–4 pass. Keep `select_sidebar_target`, all data/storage handlers and existing non-palette actions.

- [ ] Rewrite palette-driven test helpers to use real new entry points. Replace removed-search/filter tests with reachability/grouping assertions where their underlying guarantee still matters. Add tests that `:` and `/` do nothing and never spawn a picker.
- [ ] Remove obsolete dispatch and filtering, and update each context's help plus README's key list to the approved map. Load oss-docs before README edits.
- [ ] Run `make test` and `git diff --check`; require all checks to pass. Review all thirteen mappings against the spec and render representative short/wide terminal snapshots.
- [ ] Request independent review, address findings, compare with approved FigCraft frames, and set only verified implemented frame statuses to completed. Coordinate designer-owned file inclusion rather than overwriting their work.
- [ ] Commit on the feature branch with applicable Todo trailers, export only unapplied commits, request `cockpit apply`, and run `make -C /home/theo/agent-cockpit test` through ask-to-run.
- [ ] Hand the tested commit to team QA with exact tests/frame IDs. Mark only fully completed todos done; report remaining key-map work explicitly.
