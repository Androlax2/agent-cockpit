# Phone Message Line Breaks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Phone replies, first messages, and queued messages retain paragraphs and indentation as one message.

**Architecture:** Stop flattening newlines in the existing phone composer. Permit LF in the message validators and queue, then deliver multiline terminal input using a named tmux buffer and bracketed paste before the existing submit key. Keep single-line terminal delivery unchanged, and keep todo titles single-line.

**Tech Stack:** Existing Rust, Bash, vanilla JavaScript, Cargo integration tests, Node UI tests, and Python browser tests. No dependencies added.

**Spec:** Theo approved this scope in chat for todo `a8ed6501`: preserve paragraphs in phone replies and first messages, including queued messages, using bracketed paste for Claude and OpenCode; retain control-character safeguards and add regression tests.

## Global Constraints

- Retain the 4000-character message limit and existing whitespace trimming at the outer boundary.
- Accept LF, normalize CRLF to LF, reject other controls; keep the phone's existing tab-to-space conversion.
- Preserve blank lines and interior indentation; never split one message into multiple submissions.
- Preserve Claude's idle Enter / working C-x Enter distinction and all dialog guards.
- No new layout, components, dependency changes, or unrelated refactoring.
- Host installation and live-pane checks require approval; never send test prompts to existing sessions without consent.

## Review Focus

- Embedded Escape/bracketed-paste terminators must be rejected, not interpreted (Task 1).
- Failed buffer load/paste must not press Enter or report successful delivery (Task 1).
- Concurrent sends must not overwrite a shared tmux buffer or leave message text in a buffer (Task 1).
- Queued/asleep delivery must preserve blank lines and indentation (Tasks 1–2).
- First-message support must not accidentally permit multiline todo titles (Tasks 2–3).

### Task 1: Safe multiline terminal delivery

**Files:**
- Modify: `config/bin/.local/bin/cockpit`
- Modify: `config/bin/.local/bin/cockpit-agents/claude.sh`
- Test: `apps/cockpit/tests/cockpit_script.rs`
- Reference: `config/bin/.local/bin/cockpit-agents/opencode.sh` already calls `submit_to_pane`.

**Interfaces:**
- Consumes: `say_to_session(id, message)`, `agent_claude_say(id, pane, message)`, and `submit_to_pane(pane, text)`.
- Produces: `type_into_pane(pane, text)` returns nonzero on failed delivery; both submission paths use it explicitly.

- [ ] Add failing parameterized tests for Claude idle/busy and OpenCode waiting/working. Input `First line\n\n  - second line` must reach one bracketed paste unchanged, followed by exactly the existing submit keys; no newline is sent as a raw key.
- [ ] Add failure tests for buffer load/paste, unique named buffers, cleanup, controls including ESC + `[201~`, and unchanged permission-dialog refusals. Update asleep-message coverage to accept LF but still refuse other controls.
- [ ] Run `cargo test --release --manifest-path apps/Cargo.toml -p cockpit --test cockpit_script` and confirm the new cases fail for the intended reason.
- [ ] Implement multiline-only `tmux load-buffer -b <unique-name> -` from stdin, then `tmux paste-buffer -d -p -r -b <name> -t <pane>`. Use `-r` to retain LF bytes; remove the named buffer on failure. Keep single-line `send-keys -l --` unchanged. Explicitly propagate failures before submit/audit calls, including when Bash errexit is suppressed by a caller's conditional.
- [ ] Relax the public `say` validator to permit LF only, preserving its length limit and rejection of other controls. Normalize CRLF before validation.
- [ ] Rerun the script integration suite; require all cases passing.

### Task 2: Server and queue accept paragraphs

**Files:**
- Modify: `apps/cockpit/src/remote.rs`
- Modify: `apps/cockpit/src/prompts.rs`
- Test: `apps/cockpit/tests/claude_remote.rs`
- Test: `apps/cockpit/src/tests.rs`

**Interfaces:**
- Consumes: Task 1's public `cockpit say` behavior and existing `prompts::add(session, text)` / `oldest(session)`.
- Produces: unchanged HTTP request/response schemas; `message` and `start_session` accept LF, while `add_todo` continues to use `one_line`.

- [ ] Add failing HTTP tests for multiline replies to both agents, working-session queue retention, CRLF normalization, multiline first prompts for clone/background starts, and rejection of controls/empty/overlong messages. Inspect recorded bytes/arguments rather than line-splitting logs.
- [ ] Add queue tests proving `First line\n\n  - second line` survives add/read/dispatch unchanged, and malicious controls still fail. Retain one-prompt-per-turn behavior.
- [ ] Run the affected HTTP and library tests and confirm intended failures.
- [ ] Add a narrowly named multiline message validator in `remote.rs`; normalize CRLF, trim boundaries, allow LF but no other control, enforce 4000 characters. Use it in `message` and `start_session`, not todo titles. Update `prompts::add` with the same contract without introducing an abstraction layer.
- [ ] Rerun the affected tests; require passing, including the existing first-message project/agent guards and queued dispatch tests.

### Task 3: Phone sends the original paragraphs

**Files:**
- Modify: `apps/cockpit/assets/remote/page.js`
- Test: `apps/cockpit/tests/remote_mobile_ui.mjs`
- Test: `apps/cockpit/tests/mobile_browser.py`
- Reference: `apps/cockpit/tests/remote_chat_ui.rs` runs the Node suites through Cargo.

**Interfaces:**
- Consumes: Task 2's unchanged `{text}` and `{prompt}` HTTP bodies accepting multiline content.
- Produces: `messageText(text)` preserves internal LF/blank lines/indentation, normalizes CRLF and converts tabs to spaces as before.

- [ ] Change the existing flattening regression to expect `Fix the login bug\n- check the cookie\n\n  - add a test  now`; replace the old warning test with coverage that paragraphs are not described as flattened.
- [ ] Add a first-message UI test checking the actual posted prompt retains paragraphs, plus a queued reply case and draft-preservation coverage.
- [ ] Run `node apps/cockpit/tests/remote_mobile_ui.mjs` and verify expected failures.
- [ ] Replace `oneLine` with `messageText` at reply and session-start call sites. Remove the obsolete "Line breaks are sent as spaces" hint and its toggling only; leave all controls, layout and draft handling unchanged.
- [ ] Update the Python browser scenario to expect the literal newline in the sent action and no obsolete warning.
- [ ] Rerun Node tests and the full `cargo test --release --manifest-path apps/Cargo.toml` plus `python3 -m unittest discover -s tools/tests -p 'test_*.py'`. Report every failure; request authorization before any separate build/install command needed by browser tests.
- [ ] Request a read-only whole-change review; fix material findings and rerun affected tests.
- [ ] Commit only this task's files on a feature branch, with `Todo: a8ed6501`. Export only the requested unapplied commits; never overwrite unrelated exports.
- [ ] Ask before `ask-to-run` applies the patches and runs host `make test`. Separately ask readiness for the desk check with Claude and OpenCode, including idle and queued delivery; do not claim live validation without it.
- [ ] Run `claude-todo done a8ed6501` only when the agreed work is finished; explicitly report any remaining live-validation limitation.
