# Global Ideas Pane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give pending ideas a discoverable global pane with their source project/session and existing approve/decline actions.

**Architecture:** Add `[4] Ideas` to the existing terminal panels without changing the project-scoped Todo tabs. Read the same proposed items as the header counter, independent of the viewed session and Todo filters. Reuse TodoList approval, decline and proposal details rather than creating a second store.

**Tech Stack:** Rust, ratatui, crossterm; existing App test fixtures.

**Spec:** Approved chat request: a dedicated global Ideas pane showing proposals and the project/session that submitted each; implement directly using existing terminal UI patterns, without a Designer.

## Global Constraints

- Preserve existing terminal styling, numbered panels 0–3, Todo scope/filters, and proposal approval semantics.
- Add no dependencies and change no manifests, launchers, Makefiles or host configuration.
- Pending proposals only; approval moves an item to To do without starting work.
- Never invent session provenance; show stored identity when the source is no longer running, and an explicit unknown source when absent.
- Ask Theo before any ask-to-run operation; do not apply changes to the running copy without that approval.

## Review Focus

- A viewed session in another project must not hide a globally counted idea.
- A Todo category filter must not hide ideas in the new pane.
- Exited or absent source sessions must remain identifiable without false attribution.
- Approval/decline must remove only the selected pending idea and preserve other proposals.
- Empty and short terminal layouts must remain navigable without indexing or layout failures.

---

### Task 1: Global Ideas panel

**Files:**
- Modify: `apps/cockpit/src/app.rs` — panel state, rendering, input, header destination, help and palette.
- Test: `apps/cockpit/src/tests.rs` — public App input/render/store behavior and affected layout expectations.

**Interfaces:**
- Consumes: `TodoList.items`, `Item.source()`, `Item.project()`, `Item.why()`, `TodoList.approve()`, `TodoList.decline()`, existing session snapshots and proposal detail formatting.
- Produces: panel index 4 (`Ideas`), accessible with `4`, Tab/BackTab, the action palette and the header idea indicator; `y` approves, `n` declines, Enter shows details.

- [x] **Step 1: Write failing behavior tests.** Drive `App::handle`, `App::build` and rendering using the existing scratch-home fixture. Verify:
  - With a viewed `homelab` session, proposals in `greywall` and `agent-cockpit` both render in Ideas, while ordinary To do items do not.
  - An active Todo kind filter does not exclude a pending idea.
  - Proposal rows show title, destination project, submitting session identity and reason; missing/live/exited sources remain accurately labeled.
  - Pressing `4`, clicking the header indicator and cycling panels reaches Ideas without changing the viewed session or Todo filters.
  - `y` changes only the selected idea to `todo`; `n` removes only the selected idea and retains the existing declined-title protection. Neither dispatches work.
  - After the last idea is resolved, the pane renders an explicit empty state and the header count disappears.
  - Status, all action panels and narrow/short buffers render safely with the fifth panel.

- [x] **Step 2: Run the new tests and confirm expected behavioral failures.**

Run: `cargo test --release --manifest-path apps/Cargo.toml -p cockpit --lib ideas`

- [x] **Step 3: Implement the smallest complete panel.** Extend existing panel metadata/state/layout/key cycling to index 4. Keep independent proposal selection and use the same proposed-status predicate as the global counter. Render compact proposal cards using the existing line primitives. Keep Ideas collapsed to its border when unfocused, and show its pending count on that border. Add a header hit target without replacing the existing Status hit target. Resolve source session labels from snapshots only when identities match; retain stored identity otherwise. Use existing TodoList mutations and detail formatting; rebuild/clamp after decisions. Update keyboard help and the palette without changing host tmux bindings.

- [x] **Step 4: Verify targeted and full suites.**

Run: `cargo test --release --manifest-path apps/Cargo.toml -p cockpit --lib`

Run: `make test` (includes compilation; obtain approval if needed under current execution permissions).

Report all failures verbatim. Inspect `git diff --check` and `git diff --stat`.

- [x] **Step 5: Commit verified requested changes on the current feature branch.** Use trailer `Todo: e2536107`. Do not push or apply without Theo's approval. The pending Designer recruit is not needed and must not be used to delegate this work.

## Verification results

- Baseline cockpit library: 585 passed, 0 failed.
- Initial Ideas regression run: 0 passed, 10 failed as expected (the pane did not exist).
- Initial library integration run: 589 passed, 6 failed; updated the old four-panel layout/navigation expectations for the fifth panel.
- Additional source-provenance regressions failed before fixing unknown and externally running source sessions.
- Final cockpit library: 597 passed, 0 failed.
- `make test`: 35 Python tests and 1,751 Rust tests passed; zero failures.
- Fresh-context read-only review: no Critical, Important or Minor findings.
- No dependencies, host settings, launcher scripts or Makefiles changed. No host application performed yet.
