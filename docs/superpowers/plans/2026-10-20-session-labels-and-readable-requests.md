# Session Labels and Readable Requests Implementation Plan

> **For agentic workers:** Use `executing-plans` inline, with test-driven development and a fresh read-only final review. No commits or live apply without fresh authorization.

**Goal:** Remove generic status filler, display automatically supplied short task labels, and make the full selected request readable without disturbing stable session rows.

**Architecture:** Extend the existing authenticated session-report socket with an optional `label` field. Each agent publishes its own 3–6-word label during its normal work using a shared command and instructions; the daemon retains it across reports/restarts. The cockpit keeps the original task title for details/search and uses the label only for the list. Requests get reserved wrapped space in selected details and a scrollable full-detail popup.

**Tech Stack:** Rust/ratatui, existing Bash/jq session hook and socket reporting. No dependencies or extra model calls.

**Execution status:** Implemented locally and reviewed. Both Important review findings were fixed with RED→GREEN regressions. Final full release suite: 1144 passed, 0 failed across 58 targets; one pre-existing `unused_mut` warning. The first full run hit unrelated `handoff_install::a_destination_outside_the_repository_is_refused` / BrokenPipe; no unrelated test/helper was changed or disabled. Commit/live apply remain pending approval.

**Spec:** Approved chat description: do all three improvements; automatic 3–6-word labels during normal work, original titles retained, labels arrive on each session's next turn, repeated generic filler removed without moving rows, full requests readable. Inline execution continues the already selected workflow.

## Global Constraints

- Keep project/session order and two reserved rows per session; state changes must not shift rows.
- Do not rename native sessions or overwrite to-do text; preserve the original title in details and search.
- No model calls, assistant tool permissions, or background sessions solely for label generation.
- Label reports must not clear a pending question, change agent activity, steal another session's terminal target, or discard ordinary report fields.
- Label validation: 3–6 whitespace-separated words, at most 60 Unicode characters, normalized single spaces, no terminal controls or tmux formats. Reject invalid input; never silently clip a label.
- Shared agent instructions: publish a descriptive action/object label once the task is understood, then only when the overall task changes. Keep its language aligned with the user's task. Reporting a label is session metadata, not permission to edit project files.
- No git writes or live installation until separately authorized. Explicitly disclose changes to host-executed scripts and global instructions.

## Review Focus

1. Metadata-only updates must publish immediately while preserving `state`, `since`, requests, pending-input flag and original task.
2. Missing labels and old snapshots must still show existing title fallbacks, not blank cards; restart persistence must work.
3. Control characters, malformed values, overlong/multi-line labels, and wrong-pane reporters must be rejected at the boundary.
4. Long Unicode/multiline requests must remain fully reachable in short/narrow terminals; details must never acknowledge/approve anything.
5. Label publication during a pending question, or a popup open while the source session exits, must not accidentally resume or act on another session.

## Task 1: Agent-published automatic labels

**Files:** `config/bin/.local/bin/claude-session-label` (new executable wrapper); `config/claude/.claude/hooks/session-state.sh`; `config/claude/.claude/CLAUDE.md`; `apps/desktop/src/registry.rs`; `apps/desktop/src/tests.rs`; `apps/cockpit/src/sessions.rs`; `apps/cockpit/tests/session_state_hook.rs`.

**Interfaces:** `claude-session-label '<3–6 words>'` sends `hook_event_name: TaskLabel, label` through the same hook as `claude-needs-input`. Add optional `label: Option<String>` to `Report`, `label: String` to registry `Entry` and cockpit `Session`; absent fields preserve previous values. Reuse existing `validate_text`, `report_from` and snapshot/load paths.

- [ ] Write failing hook tests using the real wrapper and report socket fixture: valid label appears without `state`, `message` or `needs_input`; invalid label exits 2 without a report; a pending question survives label publication and subsequent Stop.
- [ ] Write failing daemon tests: metadata-only update publishes a label without changing `since`, state, task/request; the label survives subsequent reports without the field and snapshot reload; invalid string types, 2/7 words, 61 characters, controls and tmux formats are rejected. Existing cross-pane rejection must also cover label updates.
- [ ] Run `cargo test -p cockpit --test session_state_hook` and `cargo test -p desktop --lib label`; observe failures before adding implementation.
- [ ] Implement the wrapper, the hook event, boundary validation, metadata update and snapshot round-trip. Normalize valid input once; use shared Rust label validation for the daemon boundary and compatible jq checks in the hook. Existing NeedsInput sticky processing must be unaffected.
- [ ] Add the small global instruction adjacent to existing per-session reporting instructions, naming the exact command, 3–6 words/60 characters, when to report, and no extra calls for naming alone.
- [ ] Re-run both test commands and all desktop unit tests. Review only scoped changes; do not commit.

## Task 2: Short card labels with stable, quieter rows

**Files:** `apps/cockpit/src/app.rs`; `apps/cockpit/src/sessions.rs`; `apps/cockpit/src/tests.rs`.

**Interfaces:** Keep `session_task_titles` as complete original title cache. Add `fn session_display_title(&self, session: &Session) -> &str` on `App`, preferring nonempty `Session.label` over cached original title/project. Search entries include label and original title. `activity_text` keeps request and meaningful command-phase copy but returns an empty string for generic Working/Ready/Unknown filler.

- [ ] Write failing rendering tests: `label: "Free disk space"` overrides a verbose original only on the card, while original source remains in selected detail/search; absent/empty labels keep original fallback; replies and routine state changes do not replace labels.
- [ ] Write a stability test asserting equal item-row positions between Working, Ready, and Needs-you snapshots; generic filler is absent but request text and Command running/Input needed detail remain.
- [ ] Run `cargo test -p cockpit --lib label` and `cargo test -p cockpit --lib filler`; observe failures, then implement using existing cache/palette/row owners.
- [ ] Re-run the whole cockpit unit suite and update only obsolete wording expectations. No keypress I/O or extra title generation.

## Task 3: Wrapped selected request and a scrollable full-detail view

**Files:** `apps/cockpit/src/app.rs`; `apps/cockpit/src/tests.rs`.

**Interfaces:** Add `SessionDetails` popup in `app.rs` using existing overlay conventions and `approval::wrap`, with owned original title, label, request and metadata text plus scroll offset. `App` holds `session_details: Option<SessionDetails>`. Bind `i` in Sessions to open details; Up/Down or j/k scroll, Esc/i/Enter close without manager commands. Request text gets its own wrapped fixed-detail rows, never shares a row with the technical badge train.

- [ ] Write failing buffer/key tests: long request's beginning is readable in selected fixed detail; `i` reveals its complete content through scrolling at 40/80 columns; controls never approve/open/preview a session; pending details remain tied to the original ID/text even if the registry changes; Esc returns to unchanged list order/selection.
- [ ] Write narrow/short rendering tests ensuring fixed details are omitted before card space, popup text is clipped only by its viewport and scrolls to the last line, and no cached click targets activate underlying actions while the popup is open.
- [ ] Run `cargo test -p cockpit --lib request_detail`; observe failures. Implement with cached data only and no new terminal backend/pager process.
- [ ] Re-run `cargo test -p cockpit --lib` and all hook/daemon tests; preserve existing enter/open and n/preview semantics.

## Task 4: Final verification and handoff

- [ ] Run `make cockpit test` with explicit build approval; report failures verbatim. Wrapper tests must execute the new helper, not inspect its source text.
- [ ] Dispatch one fresh read-only review focused on the five Review Focus cases. Fix Important/Critical findings once, with RED→GREEN regression tests and a complete-suite rerun.
- [ ] Report `git diff --stat`, unchanged native titles/session controls, test outcomes, and host-executed script/global-instruction changes. Do not install, commit, restart agents or apply to live without fresh approval. Existing sessions publish labels only once they receive the updated instruction on a subsequent turn/restart.

## Self-review

The shared hook/registry label path covers both Claude and OpenCode without provider-specific API calls. Existing authorization and pane ownership checks are retained. Two-row geometry remains invariant; original task titles remain available. A dedicated cached-text popup makes full requests reachable independently of fixed-footer height.
