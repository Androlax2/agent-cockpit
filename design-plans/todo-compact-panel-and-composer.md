# Compact TODO panel and unified add/edit popup

Written against: `ee77d7399ff6b4e59cf2c305683b42f181247cd5`.
Status: user selected mockup **A — Compact**. Product source has not been changed for this redesign. This is a handoff for the implementing agent; implementation, git writes, and live application require their own authorization.

Builder check: fresh read-only reviewer found “nothing important left”.

## Evidence chain

- Surface: cockpit terminal TODO panel and its `t` / `e` flows, not the remote phone UI.
- Problem: the user's screenshot repeats `dotfiles`, `Task`, and `by Claude` across nearly every row; kinds and metadata compete with titles. The user explicitly requested choosing kind when pressing `t`.
- Design evidence: user selected A after viewing the local interactive mockup at `design-plans/.superpowers/brainstorm/1512552-1792511469/content/todo-panel-and-composer.html`. The mockup is illustrative; sample kind assignments are not instructions to reclassify actual TODOs.
- Owner: `apps/cockpit/src/app.rs`: `build_todo`, `todo_subtitle`, `handle_todo`, `todo_hints`, `panel_help`, `draw_panel`, `App::handle`, `App::render`, and terminal-event decoding in `read_key`.
- Runtime: `build_todo` creates `Line` segments and `Part` metadata consumed by `draw_panel`; `handle_todo` currently opens `edit_in_popup` and only then asks `pick_todo_kind`. This is a two-stage flow, not a single popup.
- Identity: existing Tokyo Night terminal palette, defined by kitty's 16 colors and consumed via `app.rs` indexed color constants. No `DESIGN.md` exists in this checkout.
- Explicit exceptions: none documented.
- Uncertainty: screenshot is wide; the same design must work at the existing 60-column default and narrower sizes. Current key decoding discards Enter modifiers, so the mockup's CMD+Enter cannot be assumed to work without changes.

## Design decision

Use one compact line per TODO, with extra context reserved for the selected item. Put kind, title, and notes in one native terminal overlay for both add and edit, replacing the editor-then-picker sequence. This is not a web UI and must not depend on a browser or preview server.

### Compact list

- In a scoped project, show the project once in the panel heading, not in each typed row. In All projects, retain each item's project; GitHub rows retain their issue identity.
- Show a stable, left-aligned kind column. Leave ordinary Task cells blank; show Feature, Bug, Improvement, Chore, Research, and an explicit unknown indicator. Do not remove or rewrite stored categories.
- Reserve a stable priority area so title starts do not shift with the number of priority marks. Preserve four levels above normal and low priority.
- Remove repeated `by Claude` from row metadata. Show source, reporter, routing, status, kind, and notes in the selected-item summary and full details instead. Keep a compact Design/setup-report indicator on exceptional rows so routing is not silently hidden.
- In Open, render sections **In progress**, **Proposed** (when nonempty), then **To do**. Keep existing `todo::todo_order` within each section; this is display order only, never queue priority. Single-status tabs omit unnecessary section headers.
- Keep a compact queued marker. A status section replaces repeated `in progress` text on every row.
- Render a selected-item summary in `PanelState.fixed_bottom` only while Todo is focused and at least four scrolling rows remain. Limit the summary to three lines: full title (ellipsized if necessary), kind/status/source/routing, then the first nonempty note line or a useful empty hint. `Enter` continues to show the full content. Hide the summary first when height is constrained.
- Put status tabs and the active kind filter in the panel's fixed area, where they remain readable. The filter must remain visible ahead of long scope labels. Do not change tab hotkeys or count semantics. Keep action hints visible: `t Add`, `e Edit`, `K Kind`, `f Filter`, `Enter Details`.

### Press t / e: unified popup

- `t` retains the existing project-selection rule: viewed session's project, otherwise the existing project picker. Then open one native terminal overlay with Kind, Title, Notes (optional), and an Add TODO button. Initial kind is Task; initial focus is Title.
- `e` opens the same overlay prefilled with the selected typed item's values and a Save changes button. Preserve unknown persisted categories unless the user explicitly chooses a supported kind. GitHub edits remain on GitHub.
- `Tab` / `Shift+Tab` cycle through Kind, Title, Notes, Save. Arrows on Kind choose one of the six values. Enter in Notes inserts a newline; Enter on the Save button submits. Field focus is visually explicit.
- **Reliable save shortcut: CMD+S** (maps to `Key::Ctrl('s')`). Also support CMD+Enter only if terminal decoding demonstrably preserves its modifier; do not label an unsupported shortcut. This is a technical refinement of the browser mockup, which assumed CMD+Enter.
- `Esc` cancels the entire draft; no item, edit, category, or queue entry is persisted. Global `q`, `t`, `d`, `j/k`, digits, Tab, etc. must be text or form input while editing, never underlying panel actions.
- Validate a nonblank title before any persistence. Retain title and notes limits/sanitization from `todo.rs`; keep the overlay open with an inline error when validation fails. A successful save persists title, notes, and category together; no intermediate item with missing notes/category is visible to queue dispatch.
- Preserve kind/status filters after cancellation. After save, keep the saved item selected and visible, clearing a mismatched kind filter as the existing `reveal_todo` behavior does. Add opens To do; edit preserves proposal approval and status.
- Draft text must survive resize and refresh. Constrain the overlay to the terminal; at tiny sizes show a readable resize hint rather than overflowing or discarding the draft.

## Reuse

- `Line`, `Part`, `segment`, `PanelState.fixed_top`, `PanelState.fixed_bottom`, `fit`, `put`, indexed colors, `width_of`, and the existing overlay-first pattern in `App::handle`.
- Existing `todo::Category`, `Item::category`, `todo_details`, `reveal_todo`, and project picker. Do not introduce another kind enum or invent routing semantics.
- Exemplars: `EffortMenu` and `SettingsMenu` for rendering/focus; selected session-details overlay for ownership of overlay state. The composer needs text fields and a draft, which these selection-only menus do not provide: a small dedicated `apps/cockpit/src/todo_composer.rs` module is appropriate. Register it in `apps/cockpit/src/lib.rs`; keep persistence and project ownership in the existing app/list layers.

## Changes and verification order

1. `apps/cockpit/src/tests.rs` and `apps/cockpit/src/app.rs`: write failing App-entry-point/render tests first, then implement compact rows, scoped project removal, Open section headings, selected summary, and fixed filter/header layout. Section headings must have `item: None`; row item indices must still match `todo_entries`, including clicks and scrolling. Verify every row action still targets the correct item after grouping and filtering.
2. `apps/cockpit/src/todo_composer.rs`, `apps/cockpit/src/lib.rs`, `apps/cockpit/src/app.rs`: add the native draft overlay and input handling with failing tests before code. Capture form events before global key handlers. Do not use an external editor or fzf for `t`/`e`; keep existing `K` and `f` quick pickers.
3. `apps/cockpit/src/todo.rs`: provide the smallest explicit save operation that validates the whole draft and persists once. Keep add/propose CLI payloads and JSON compatibility unchanged. Test persistence and cancellation through App actions and list reload, not only private editor helpers.
4. Update `todo_hints`, `panel_help`, full TODO details, and any existing tests whose expectations deliberately reflect the old two-stage UI. Do not disable tests. Record intentional expectation changes in the execution notes.

## Scope

- Inherit: cockpit TODO add/edit, row rendering, selected summary, counts, scrolling, keyboard and mouse interaction.
- Verify: unscoped lists, GitHub issue rows, proposals, empty views, queued items, Design items, setup reports, long titles/projects, Unicode, notes, all six kinds, and unknown stored values.
- Preserve: existing kind CLI, JSON fields, ids, priorities, status transitions, proposal approval, dispatch, queue behavior, per-session filters, and navigation outside the popup.
- Exclude: automatic kind inference, historical reclassification, custom tags, search, dependencies, remote UI redesign, new status semantics, deployment/build-script changes, or a new visual identity.

## Validation

- Product: `t` opens one form; selecting Bug, entering title/notes, and saving creates exactly one complete Bug item. `e` edits the same fields; Esc leaves the original byte-for-byte unchanged. A cancelled add creates nothing.
- Input: field traversal, multiline notes, cursor/backspace behavior, ordinary paste and Unicode, title validation, repeated save, literal shortcut characters in text, and saved drafts across resize/refresh. Inspect real terminal event handling before promising enhanced keyboard shortcuts.
- Interface: render at widths 40, 60, 100, and 130; test short terminal heights, long project names, long kind names, four priority marks, and queued Design/setup-report rows. Titles and active filter remain readable; no overlapping cells or out-of-bounds cursor positions.
- Selection: keyboard and click selection skip section headings; scrolling and selected summaries remain synchronized; `h`, `N`, `i`, `d`, `x`, `y/n`, `K`, and queue actions affect the selected item, not its pre-grouping index.
- System: queue dispatch ignores display grouping/filtering and never sees a partial form save. Proposal edits do not approve them. Unknown categories survive unrelated edits.
- Repository: `python3 config/bin/.local/bin/agent-build cargo test --release --manifest-path apps/Cargo.toml --no-fail-fast`. Previous sandbox run: 1296 passed, 8 failed; those eight were reproduced on an unchanged earlier baseline. Live host test target subsequently passed. Rerun and report actual results; do not assume the old failures remain.
- Run `git diff --check`. Show the diff stat and state what is preserved. No installs/builds, dependency edits, commits, patch exports, or host reload without fresh authorization.

## Stop conditions

- Current live main has advanced: inspect and reconcile before implementing, without overwriting unrelated session/UI changes. A previous patch needed rebasing for this reason.
- Existing input primitives cannot support a usable form without broad changes: propose the smallest isolated input extension before widening scope. No new dependency without approval.
- No mockup is reachable through localhost: use the already-tested self-contained local HTML file. Do not alter the sandbox, proxy, browser settings, or service configuration to make a design preview work.

## Design documentation

After acceptance and implementation validation, retain this file as the decision record and document any deliberate departures. The browser mockup is a visual aid, not production source or an instruction to change historical data.
