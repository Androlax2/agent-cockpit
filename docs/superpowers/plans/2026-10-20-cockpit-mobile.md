# Cockpit Mobile Reliability Implementation Plan

> **For agentic workers:** Use the executing-plans skill to implement this plan task-by-task.

**Goal:** Fix the reproduced mobile cockpit failures and verify the existing phone workflows in real Chromium.

**Architecture:** Keep the Rust HTTP API and vanilla JavaScript frontend. Correct frontend rendering and request lifecycles without redesigning the interface or adding production dependencies. Browser tests run the actual release server with temporary state and shimmed host commands.

**Tech Stack:** Rust, vanilla JavaScript/CSS, Python unittest, Playwright Chromium.

**Spec:** User-approved Option A in this conversation: fix confirmed mobile bugs and expand browser coverage, keeping the current UI.

## Global Constraints

- Preserve the existing UI, API contracts, pairing security, and two-tap approval confirmation.
- Do not change dependency manifests or operate on live sessions during testing.
- Test failures must remain visible; do not disable failing tests.
- Apply completed dotfiles changes only through the host approval workflow.

## Review Focus

- Initial unpaired rendering must not expose navigation or session controls.
- Notification links must load a live terminal without conflicting redraws.
- Session removal must restore the list and discard that session's draft.
- Failed requests must show actionable feedback without losing typed text.
- Polling and asynchronous tab loads must not erase feedback or overwrite another view.

### Task 1: Fix session rendering and pairing visibility

**Files:** `apps/cockpit/assets/remote/page.js`, `apps/cockpit/assets/remote/page.html`, `apps/cockpit/tests/mobile_browser.py`.

**Interfaces:** Preserve `refresh()`, `render()`, `renderDetail()`, `loadScreen()`, and the existing HTTP routes.

- [x] Add and run browser regressions for initial pairing visibility, notification links, session removal, draft preservation, and navigation.
- [x] Ensure `[hidden]` sections remain hidden despite authored display styles.
- [x] Remove conflicting initial detail redraws; reconcile removed sessions before choosing visible sections; clear removed session contents.
- [x] Run browser tests against rebuilt assets and verify those regressions pass.

### Task 2: Fix action feedback and stale asynchronous updates

**Files:** `apps/cockpit/assets/remote/page.js`, `apps/cockpit/tests/mobile_browser.py`.

**Interfaces:** Keep `post(path, body, note)` returning an action result and preserve approval command verification.

- [x] Add and run regressions for network failure feedback and approval results surviving polling.
- [x] Catch action/network failures at the UI boundary; preserve unsent text and meaningful server errors.
- [x] Preserve approval feedback when unchanged data is polled; ensure approval arming does not redraw unrelated session details.
- [x] Add and run a delayed-tab-response regression; prevent old tab requests from replacing the active view.
- [x] Expand coverage for messages, todo creation/start, agent selection, terminal refresh, and narrow layouts.

### Task 3: Verify and deliver

**Files:** The files above; no build scripts or dependency manifests need changing.

- [x] Run the complete mobile browser suite against rebuilt release assets; capture phone screenshots for inspection.
- [x] Run `cargo test -p cockpit --test claude_remote` (expected: 43 tests pass).
- [x] Run the repository test command `make test`; report every failure, including unrelated failures.
- [x] Review `git diff --check`, diff stat, and final changes; commit on the existing feature branch.
- [ ] Export patches, request host approval to apply them, run live cockpit tests and reload through `ask-to-run`.
- [ ] Mark todo `710fb6e3` done only after successful verification and delivery; otherwise report the remaining blocker.

## Execution record

- Task 1: complete. Eight targeted Chromium regressions passed after the first rendering fixes.
- Task 2: complete. Reproduced and fixed request errors, stale tab responses, duplicate submissions, cross-session drafts, approval feedback loss and long-title overflow. Final browser run: `Ran 36 tests in 24.227s`, `OK`.
- Full repository verification: `make test` passed all 1,250 tests (61 test binaries/doc-test runs), including all 43 remote HTTP tests. Existing warning: `cockpit/src/tests.rs:260`, unused `mut`; not changed.
- Earlier red runs included the original 43-test missing-release-binary setup failure, browser regressions, and one temporary Python indentation error. No failing test was disabled.
- `node --check`, Python syntax checking and `git diff --check` passed.
- Repeat browser run: `Ran 36 tests in 25.605s`, `OK`. Inspected screenshots of Sessions, detail, Todo, Pull requests and Usage. The isolated fixture has no usage database, and the Usage tab correctly displays that error rather than hiding it.
- Review: self-review of the complete change, per the user's explicit selection of execution without subagents.
- Ruling: browser tests use actual release assets and HTTP routes with isolated state and shimmed host commands. Physical iOS/Safari and native push delivery are not claimed as verified; those require a paired real device.
- Ruling: the prescribed skill workspace helper is absent and reported through `claude-todo blocked`; progress is recorded here instead. No task verification depends on that helper.
- Ruling: export only this task's commit from base `7fd163a`, not the two inherited commits ahead of `origin/main`. This avoids delivering unrelated changes; a cross-commit dependency would surface in patch application or live tests.
