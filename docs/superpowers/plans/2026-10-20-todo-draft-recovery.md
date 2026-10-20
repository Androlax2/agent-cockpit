# TODO draft protection implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent cockpit reloads from losing TODO editor input, and recover interrupted drafts.

**Architecture:** Hold a shared advisory file lock throughout TODO editing and saving; the host reload command waits for an exclusive lock before replacing the sidebar. Store new editor drafts and their destination metadata in the host-only persistent state directory. Recover abandoned nonempty drafts at startup using stable item IDs, retaining files if saving fails.

**Tech Stack:** Existing Rust cockpit, Bash manager, filesystem locks; no dependency changes.

**Spec:** Option A approved in this conversation: defer reloads while editing and recover interrupted drafts; retain the editor and project assignment.

## Global constraints

- No dependency manifest changes, new editor, or unrelated UI redesign.
- Host-executed change: `config/bin/.local/bin/claude-sessions` reload behavior.
- Preserve unknown legacy runtime drafts: they lack trustworthy project metadata. The reported original is already recreated; do not import it again.
- Lock must cover the final list save, not just the editor subprocess.
- Recovery preserves project, directory, notes, and edited-item identity.

## Review focus

- Reload during editing waits until the item is durably saved.
- Crash after list save but before draft deletion does not duplicate an item.
- Failed list writes retain the draft and surface an error, not success.
- Empty drafts cancel without creating an item; failed editor startup does not destroy an existing draft.
- Recovery never imports another active editor's draft or guesses a legacy draft's project.

## Task 1: Protect the edit-to-save boundary

Execution: inline, approved by Theo. Workspace helper unavailable (exit 127), reported through `claude-todo blocked`; progress stays here. Pre-flight: both tasks share the editor/save boundary; the lock will remain held until draft persistence succeeds or recoverable files are retained.

Task 1: lock regressions observed RED (unprotected editor; reload interrupted editing), then GREEN (1 App test and all 3 reload tests). Host script change is limited to waiting on `claude-todo-editor.lock` around sidebar replacement.

Task 2: five recovery regressions observed RED, then GREEN. Additional coverage checks idempotent post-save residue, a child editor surviving its sidebar, editor startup failure, and retry after the active editor releases its lock.

Final review: independent reviewer found two Important issues (popup startup race; blank title promoting notes), both reproduced RED. Fix pass: child checks draft ownership under its shared lock before opening, and parser respects the literal first line.

Ruling: a popup delayed until after startup recovery retired its draft aborts before accepting input, rather than opening an untracked file. This closes the ownership race without a daemon or extra protocol; cost if wrong: the user reopens an editor that had not yet accepted text.

Final minor (deferred): reviewer requested a controlled blocked-final-save contention test. The editor and reload tests cover editing contention and the RAII guard spans the entire final save, but a filesystem-write stall is not separately injected.

Verification: the first `make test` was terminated by the tool's 120-second timeout (`make: *** [Makefile:135: test] Terminated`), not a failing assertion; rerun without a timeout.

Final verification: `make test` passed: 1,281 Rust tests (62 groups), 12 Python tests; zero failed or ignored. Both Important review fixes passed their RED→GREEN regressions; `bash -n config/bin/.local/bin/claude-sessions` and `git diff --check` passed. Existing unrelated compiler warnings were not changed.

Export note: this parallel clone has no `origin/main` ref (only `origin/parallel-1249`); export only the requested commit against its starting commit `1dd90cb5659ab8093e636869287eadec8aaed336`. Host application still checks every patch against the live tree before asking for approval.

Integration: first host application safely refused stale patch contexts; nothing applied. Fetched `origin/main` and rebased this unpushed change onto the latest live checkout, preserving concurrent FigCraft work and the new TODO category picker/filter. Draft metadata now also preserves the selected category before saving; cancellation still discards the draft. Existing category/cancellation tests and all 384 cockpit library tests pass. Export now uses `origin/main` normally.

Post-integration verification: `make test` passed: 1,327 Rust tests and 12 Python tests; zero failures.

**Files:** `apps/cockpit/src/app.rs`, `apps/cockpit/src/tests.rs`, `config/bin/.local/bin/claude-sessions`, `apps/cockpit/tests/claude_sessions_script.rs`.

- [ ] Add a manager regression test holding the editor lock: spawn `reload`, assert no sidebar respawn until release, then assert successful respawn.
- [ ] Add App entry-point tests proving both `t` and `e` hold the same lock through editor execution and saving.
- [ ] Run the focused tests and observe the missing-lock failures.
- [ ] Implement the shared editor lock in Rust and exclusive reload lock in Bash; propagate lock errors to the cockpit message.
- [ ] Run focused tests, including normal reload and existing editor tests.

## Task 2: Persist and recover editor drafts

**Files:** `apps/cockpit/src/app.rs`, `apps/cockpit/src/todo.rs`, `apps/cockpit/src/tests.rs`, and a focused draft module if needed; `README.md`.

- [ ] Add startup recovery tests for interrupted additions/edits, original project and notes, empty cancellation, repeated recovery, and failed persistence retaining the draft.
- [ ] Observe the tests fail before implementing recovery.
- [ ] Persist each editor buffer with project/directory/stable item ID metadata before opening it. Keep drafts until a successful save; recover only tracked, inactive drafts at startup under the editor lock.
- [ ] Make save failures observable on this path without changing unrelated TODO actions. Never remove recoverable text after an editor or filesystem error.
- [ ] Document deferred reloads and persistent draft recovery in the existing TODO documentation.
- [ ] Run focused tests, the cockpit suite, and the repository test command; report failures verbatim.
- [ ] Review diff, commit requested changes on this feature branch, export patches, and request host approval to apply. Run host cockpit tests and reload through `ask-to-run`; mark `a23519e2` done only after verification.
