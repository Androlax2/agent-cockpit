# Mobile Session Chat Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the mobile cockpit a simple, ADHD-friendly chat that supports replies and inline command approvals.

**Architecture:** Retain the Rust remote service and vanilla browser assets. Add a bounded, read-only conversation reader for active sessions; render actual messages rather than terminal snapshots. Preserve the current message/queue and command-approval execution paths.

**Tech Stack:** Rust, rusqlite, serde_json, vanilla JavaScript/CSS, existing Cargo integration-test fixtures.

**Spec:** `docs/superpowers/specs/2026-10-20-mobile-chat-design.md`

## Global Constraints

- Desktop UI unchanged; the shared message handler may be corrected to distinguish chat questions from dialogs.
- No dependency or manifest changes, arbitrary remote shell, automatic approvals, or new command-execution API.
- All history access authenticated, limited to active sessions, bounded, read-only and rendered as text rather than HTML.
- Use the existing palette, labeled controls, focus indicators and touch targets of at least 44px.
- Preserve exact-command checks, second-tap Run confirmation and agent-specific dialog protection.

## Review Focus

- A slow response from session A must never replace session B's conversation.
- Polling and status transitions must preserve per-session drafts, focus and reading position.
- A missing, malformed or locked transcript must produce an explicit unavailable state, not fabricated messages.
- An approval belonging to another session must not interrupt the open conversation or be approved accidentally.
- A narrow viewport and virtual keyboard must leave the reply action reachable without horizontal scrolling.

---

### Task 1: Allow replies to ordinary OpenCode questions

**Files:**
- Modify: `config/bin/.local/bin/claude-sessions-agents/opencode.sh`
- Test: `apps/cockpit/tests/claude_sessions_script.rs`
- Test: `apps/cockpit/tests/claude_remote.rs`

**Interfaces:** Keep `agent_opencode_say(id, pane, message)` and `POST /api/message/:id` unchanged. Consume the existing `state` and `needs_input` snapshot fields.

- [ ] Add failing manager-entry-point tests: `needs-you` with `needs_input: true` submits the reply; `needs-you` without that flag refuses and emits no pane input. Keep waiting/working behavior covered.
- [ ] Run `cargo test --manifest-path apps/cockpit/Cargo.toml --test claude_sessions_script` and record the failing regression.
- [ ] Adjust only the ordinary-question case. Investigate the report lifecycle before trusting the flag; if it can remain true during a real dialog, correct that distinction rather than bypassing dialog protection.
- [ ] Add/extend HTTP tests proving mobile chat replies reach the manager and genuine dialog restrictions remain.
- [ ] Run both targeted integration suites and commit only this fix on the existing non-default branch, after checking branch and worktree status.

### Task 2: Expose real conversation text

**Files:**
- Create: `apps/cockpit/src/remote/conversation.rs`
- Modify: `apps/cockpit/src/remote.rs`
- Modify: `apps/cockpit/src/bin/claude-remote.rs`
- Test: `apps/cockpit/tests/claude_remote.rs`

**Interfaces:** Introduce `remote::conversation(id: &str) -> Result<Value, String>` and a new authenticated `GET /api/conversation/:id` route. Return `{messages: [{id, role, text}], truncated: bool}`; roles are `user` and `assistant`. Return an explicit error for unavailable history. The existing screen route remains unchanged.

- [ ] Inspect actual provider storage formats read-only before defining parser fixtures. Claude transcript discovery already exists in `sessions.rs`; OpenCode uses `paths::opencode_database()` and `session_message`, demonstrated in `costs.rs`. Verify where actual text parts live instead of assuming a schema from billing records. Never read credentials.
- [ ] Add failing HTTP tests for each verified provider format, chronological ordering, authenticated access, unknown/inactive sessions, transcript truncation and missing/locked/malformed history. Assert that system, reasoning and tool payloads are absent and markup remains literal text.
- [ ] Run `cargo test --manifest-path apps/cockpit/Cargo.toml --test claude_remote` to observe failures.
- [ ] Implement the verified formats using bounded file reads/read-only SQLite queries with parameter binding and a timeout. Cap output to the latest 100 messages and 256 KiB of text, preserving UTF-8 boundaries; report truncation. Do not follow arbitrary paths supplied by phone requests.
- [ ] Run the HTTP suite, verify no source writes, and commit the reader and route.

### Task 3: Replace the mobile terminal-first layout

**Files:**
- Modify: `apps/cockpit/assets/remote/page.js`
- Modify: `apps/cockpit/assets/remote/page.html`
- Create: `apps/cockpit/tests/remote_chat_ui.rs`
- Test: `apps/cockpit/tests/claude_remote.rs`

**Interfaces:** Consume the conversation response from Task 2. Reuse `/api/message`, `/api/decide`, `/api/screen`, existing capability flags and session approval metadata. No new execution interface.

- [ ] Load the narrow UI/accessibility and webapp-testing guidance before UI implementation. Use existing test tooling; no automatic `npx` installation. If a live frontend/browser run requires unavailable tools or explicit server/build authorization, request it instead of silently skipping verification.
- [ ] Add browser-behavior tests for ordered speaker turns, Send/Queue, per-session drafts, no duplicate submit, network-error draft retention, focus/scroll preservation during polling, and stale response rejection after switching sessions. Use the repo's Node-from-Rust test pattern if no browser harness exists; do not substitute source-string assertions for behavior tests.
- [ ] Add failing behavior tests for session-scoped inline approval cards, exact command payload, second-tap Run confirmation, Decline and terminal disclosure. Test that ordinary chat questions show a reply box rather than Yes/No controls or an instruction to use the computer.
- [ ] Run `cargo test --manifest-path apps/cockpit/Cargo.toml --test remote_chat_ui` to record failures.
- [ ] Implement compact session navigation, actual chat messages, a labeled sticky composer, inline approvals, plain-language statuses and secondary controls. Keep existing pairing, notifications, todos, pull requests and usage functional. Safely scope async updates by session and preserve drafts without persisting private text to browser storage.
- [ ] Run targeted suites and a mobile-viewport interaction check. Test narrow widths, long text/code, keyboard-open composer, scrolling up, approval errors and offline recovery. Verify literal markup cannot execute.
- [ ] Commit the mobile change. Show the diff stat and report exact test outcomes, including any unrun checks.

### Task 4: Apply reviewed changes

- [ ] Confirm no unrelated changes were included; run the relevant Cargo suites and existing full-suite command if authorized. Never run build/install commands without explicit current authorization.
- [ ] Export completed commits using `git format-patch origin/main -o .outgoing`.
- [ ] Use `ask-to-run` for `claude-sessions apply-dotfiles /home/theo/IdeaProjects/.parallel/dotfiles/parallel-1829`, preserving the host review.
- [ ] Through `ask-to-run`, run the project-required host checks and reload after successful apply. Clearly identify that the changed manager shell script runs with host rights.
- [ ] Report what works on mobile, what remains unchanged on desktop, and exact verification results. Do not claim deployment success unless host application and checks succeed.
