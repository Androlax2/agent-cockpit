# Confirmed TODO Filings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans or superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Agent filing commands report a confirmed save or an explicit failure, and completed work no longer consumes a daily filing allowance.

**Architecture:** Extend the existing protected filing receipts to blocker reports and stack proposals. The CLI waits briefly for the column's persisted result; transport acknowledgment alone is not success. Remove daily caps at their existing checks, preserving open-item caps and duplicate handling.

**Tech Stack:** Rust, existing Unix report socket, JSON queue and receipts, Cargo integration tests.

**Spec:** Approved in this conversation: confirmed save or explicit failure; remove the five-per-day limit; retain the three-open-item cap. The five-second receipt wait and timeout exit code below are implementation choices.

## Global Constraints

- Keep the existing three-open-item caps at their current scope; no new cap on ordinary same-project additions.
- Use existing request ownership validation, receipt destinations, and persistence; no dependencies or UI changes.
- Cover `add`, `propose`, `blocked`, `stack`, and the `greywall` alias.
- Return exit 1 for a rejected filing with its reason; exit 124 after five seconds without confirmation, naming the request and advising receipt lookup instead of duplicate submission.
- Accepted results name the saved item and project. Declined proposals explicitly report that nothing was added.
- Preserve existing authorization and duplicate behavior, including exact host-approved cross-project handoffs.
- Build/install/deploy commands require the existing user approval rules; do not edit dependency manifests.

## Review Focus

- Transport accepted but the column never replies: bounded wait, nonzero exit, recoverable request ID.
- Disk write fails: no accepted receipt and no dispatchable phantom blocker.
- Duplicate blocker: receipt names the existing persisted item.
- Stack alias and blocker receipts: daemon verifies ownership just as for additions.
- A completed fifth item: sixth filing succeeds; a fourth simultaneous open item is still rejected.

### Task 1: Remove daily filing caps

**Files:** `apps/cockpit/src/todo.rs`, `apps/cockpit/src/tests.rs`.

**Interfaces:** Keep `TodoList::apply_updates(&mut self) -> usize` and filing APIs unchanged.

- [x] Update the daily-cap regression to file and complete six blocker reports, asserting each application succeeds. Add equivalent coverage for cross-project additions and proposals, including stack proposals.
- [x] Run `python3 config/bin/.local/bin/agent-build cargo test --release --manifest-path apps/Cargo.toml -p cockpit --lib` and confirm failures at sixth-filing assertions.
- [x] Remove `BLOCKED_DAILY_MAX`, `ELSEWHERE_DAILY_MAX`, and their rejection branches. Update the capped-handoff approval regression to reach the three-open cap instead of the daily cap.
- [x] Run the same library tests; retain the existing fourth-open rejection and duplicate regressions.

### Task 2: Confirm all agent filings

**Files:** `apps/cockpit/src/bin/cockpit-todo.rs`, `apps/cockpit/src/todo_requests.rs`, `apps/cockpit/src/todo.rs`, `apps/cockpit/tests/claude_todo.rs`, `apps/cockpit/src/tests.rs`, `apps/osd/src/tests.rs`.

**Interfaces:** Extend existing `todo_requests::hold` and `finish` handling to `blocked` and `stack`. Add CLI `wait_for_receipt(id: &str) -> i32`, using the existing answers key and receipt format.

- [x] Write CLI regressions for accepted, rejected, and unconfirmed filings through the report socket. Assert saved ID/project and exit 0 only after an accepted receipt; rejection reason and exit 1; missing receipt and exit 124. Exercise every filing action, including the alias.
- [x] Add host-side tests for blocker save failure, blocker duplicate receipts, stack receipts, and daemon-verified destinations. Run the focused tests and confirm the missing behavior fails.
- [x] Assign a request ID to every filing. Wait up to five seconds, checking receipts every 50 milliseconds; share receipt interpretation with the existing `receipt` command and keep pending distinct from rejected.
- [x] Retain blocker and stack requests through the existing ownership checks. Remember blocker filing identity for new and duplicate reports, use fallible persistence, and finish receipts for all four filing actions.
- [x] Update transport-only CLI tests to expect unconfirmed status rather than success. Update help/output to describe confirmed results and timeout recovery. Offer exact-request approval only for eligible rejected cross-project additions.
- [x] Run `python3 config/bin/.local/bin/agent-build cargo test --release --manifest-path apps/Cargo.toml -p cockpit --test claude_todo`, the cockpit library tests, and OSD tests. Run the repository's `make test` once build authorization is available; report every failure.
- [ ] Review the diff, run `git diff --check`, show the diff stat, and commit only the requested files on this existing feature branch once checks pass.
- [ ] Export with `git format-patch origin/main -o .outgoing`; request `cockpit apply` through `ask-to-run`, followed by host `make test`, under the repository's deployment workflow.

## Verification record

- Baseline: 681 cockpit library tests passed.
- Daily-cap regressions failed at the sixth filing before implementation. A fixture initially tried to complete an unapproved proposal; corrected it to follow approval, then all 682 library tests passed.
- Filing regressions failed before implementation: blocker/stack request IDs were refused, and the CLI returned without the persisted item ID.
- Focused checks: 684 cockpit library, 41 TODO CLI, and 231 OSD tests passed.
- Full clone `make test`, approved host request `576be59e58d2`: 2,051 Rust tests and 77 Python tests passed, exit 0.
- Independent review found a post-publication sync ambiguity. A fault-injection regression reproduced the incorrect rejection; the fix reports pending and retries the exact retained request. The regression then passed, including reload before retry and duplicate prevention.
- After the review fix: complete Rust suite 2,052 passed, 0 failed; Python suite `Ran 77 tests` / `OK`.

## Delivery decisions

- Reviewed the staged diff while missing Git identity blocked committing. The review fix was verified RED→GREEN and with the complete suite; no other product changes followed review.
- Status changes, deletion, and `approve-request` confirmation remain outside this filing fix. Existing retained-request storage bounds remain; the five-per-day filing cap is removed.
- This clone has no `origin/main`. Export only this change from recorded starting commit `a908526736463dbc49276c57adeff326c2cdf92c`; the host apply checks compatibility before approval. No unrelated baseline commits are included.
- Theo approved using `Androlax2 <theo.benoit16@gmail.com>` for this commit only, without changing Git configuration.
- First apply was refused before modifying the running copy: the clone had inherited three unshipped session-link commits. Fetched deployed `origin/main` at `23a47bc` and rebased only the TODO fix onto it.
- On deployed main, TODO persistence ends at atomic rename and has no later fallible sync step. The review's sync ambiguity and its test hook belonged to the unshipped base; their workaround is omitted from the rebased fix. Save-failure, duplicate, cap, receipt, and daemon ownership regressions remain.
- Rebased verification against deployed main: complete Rust suite 2,018 passed, 0 failed across 58 targets; Python suite `Ran 77 tests` / `OK`. The lower total reflects omission of the unshipped session-link feature, not disabled tests.
