# Sidebar Idle CPU Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans or superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove expensive idle refreshes without slowing keyboard input or visible session updates.

**Architecture:** Watch relevant files with Linux inotify through the existing libc dependency. Separate full data refreshes from cached presentation and periodic maintenance. Coalesce notifications, retain a periodic safety refresh, and explicitly signal completion of background fetches so their results appear promptly.

**Tech Stack:** Rust, libc/inotify, crossterm, ratatui; no dependency changes.

**Spec:** Chat-approved Option A: file-change-driven refreshes, immediate keyboard/session updates, a slow safety refresh, unchanged interface.

## Global Constraints

- Preserve the existing interface and update speed; session switching currently checks every 40 ms, ordinary data refreshes every 250 ms.
- No dependency manifest changes, daemon protocol changes, or new services.
- Keep queue/sleep dispatch, pairing, provider refreshes, costs, reviews, timers, and command completion working without file activity.
- Log watcher failures and fall back rather than silently freezing the sidebar.
- Apply via the project's existing host approval flow and preserve unrelated host changes.

## Review Focus

- Atomic file replacement must not detach the watch from its parent directory.
- Missing directories, directory replacement, and notification overflow must recover or fall back.
- The sidebar's own log/archive/continuity writes must not trigger a refresh loop.
- Worker completion and terminal resizing must update the screen even with no filesystem changes.
- Bursts of reports must be coalesced without postponing session switching or starving keys.

---

### Task 1: File-change detection and refresh scheduling

**Files:** Create `apps/cockpit/src/app/refresh.rs`; modify `apps/cockpit/src/app.rs` to declare the module.

**Interfaces:** A watcher owns and closes its nonblocking inotify descriptor; construction accepts explicit file/directory targets. A refresh scheduler accepts elapsed time and dirty notifications, exposing whether a full refresh is due. Keep implementation details inside the module.

- [ ] Write tests for file writes, atomic replacement, deletion/recreation, unrelated files, idle scheduling, and coalescing bursts. Use temporary directories and real inotify, not host state.
- [ ] Run the new tests and confirm they fail before implementing.
- [ ] Implement filtered parent-directory watches with close-write, moved-to/from, create/delete and watch-invalidated/overflow handling. Ignore access events and unrelated log/archive/temp files. Watch runtime snapshot/current/usage/todo updates, statusline and design inputs, state todo/settings/sleep/prompt/restart inputs; map remaining inputs during wiring.
- [ ] Keep viewed-session notifications prompt (within the existing 40 ms check cadence), ordinary full refreshes coalesced to at most once per 250 ms, and a five-second full safety refresh. Re-establish missing watches during safety refreshes. Failed coverage uses the old 250 ms fallback until restored, with a diagnostic rather than silent data loss.
- [ ] Run watcher/scheduler tests, including directory replacement and bounded recovery.

### Task 2: Wire the sidebar without losing timer or worker updates

**Files:** Modify `apps/cockpit/src/bin/cockpit-sidebar.rs`, `apps/cockpit/src/app.rs`; background completion notifications in `apps/cockpit/src/{costs,reviews,status,usage}.rs` and a small shared notification primitive as needed.

**Interfaces:** Preserve `App::tick()` for existing callers and tests. Introduce a cached maintenance path that does not invalidate gathered session data or spawn tmux. Expose explicit background-completion notification at each worker completion, using a cheap shared generation counter rather than copying worker results every loop.

- [ ] Add regression tests that idle iterations do not gather sessions repeatedly, worker completion requests a redraw, and maintenance remains due without input events.
- [ ] Run tests and confirm failure before wiring.
- [ ] Use the scheduler in the real sidebar loop. Stop rereading `cockpit.current` every 40 ms; use watched notifications and safety refresh instead. Full data refresh follows relevant changes; timer maintenance and cached time-dependent presentation run once per second. Keys and pending command collection remain responsive.
- [ ] Signal background completion explicitly from costs, reviews, usage and provider workers. Consume the notification at the existing short input wait cadence, scheduling cached redraw or full refresh according to the result's needs.
- [ ] Handle terminal resize explicitly instead of relying on unconditional redraw every tick. Review self-induced watched writes and guard against feedback loops.
- [ ] Run library and terminal tests.

### Task 3: End-to-end regression tests, measurement and application

**Files:** Create `apps/cockpit/tests/sidebar_refresh.rs`, using `apps/cockpit/tests/common` PTY and fake-command conventions.

- [ ] Write real sidebar tests with a fake tmux call log: idle for several seconds and assert no four-per-second session gathers; atomically replace snapshot/current and verify prompt visible updates; send keyboard input and resize while idle; ensure unrelated log writes cause no refresh storm.
- [ ] Verify background results display without further file changes and a missing watch target recovers when created.
- [ ] Run `make test` with output saved and check the actual exit status, not a filtered pipeline's status.
- [ ] Inspect the diff, commit with `Todo: dce1dd1b`, export only this task's commits, and request host application using `ask-to-run`.
- [ ] Run host tests, then measure live CPU over three minutes using process CPU-time deltas. Compare with the previous 1.3%; report the actual number and any remaining periodic work rather than promising literal zero.
- [ ] Check the todo list and close dce1dd1b only after successful application and verification.
