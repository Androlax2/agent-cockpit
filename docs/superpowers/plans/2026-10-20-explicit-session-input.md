# Explicit Session Input Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An agent explicitly flags a blocking chat question, which stays visible on its cockpit card until the user answers.

**Architecture:** Add `claude-needs-input '<question>'`, a shell command that sends a `NeedsInput` event through the existing session-state hook and authenticated report socket. The hook remembers the question per session, preserves it through tool completion and turn end, and clears it on a new user prompt, session start, or session end. An explicit `needs_input` boolean in reports prevents Claude's screen reconciliation from replacing this state with ordinary idle/working state; existing card rendering displays the request.

**Tech Stack:** Bash, jq, existing Rust registry and integration tests. No dependencies or manifest changes.

**Spec:** User-approved scope in this conversation: explicit command, question on the card, survives reply completion, clears on answer, Claude and OpenCode support, mandatory agent instruction, no punctuation heuristics.

## Global Constraints

- Keep the existing permission/form detection and card layout.
- Use the existing authenticated socket; do not introduce another trust channel or accept an arbitrary session ID argument.
- Changes to host-executed scripts and sandbox configuration must be called out.
- Do not modify dependency manifests or push commits.
- Apply only through the host's existing patch-review workflow.

## Review Focus

- A question survives `PostToolUse`, `Stop`, and Claude's idle screen reconciliation.
- A fresh user prompt clears both the stored question and daemon protection.
- Other sessions never inherit the question.
- Missing session ID or blank question fails instead of silently appearing successful.
- Ordinary turn completion and permission dialogs keep their current behaviour.

### Task 1: Explicit question lifecycle

**Files:**
- Create: `config/bin/.local/bin/claude-needs-input`
- Modify: `config/claude/.claude/hooks/session-state.sh`
- Modify: `apps/desktop/src/registry.rs`
- Test: `apps/cockpit/tests/session_state_hook.rs`
- Test: `apps/desktop/src/tests.rs`

**Interfaces:**
- Command: `claude-needs-input '<question>'`; session comes from `CLAUDE_CODE_SESSION_ID`, working directory from the current directory. Reject missing/invalid session IDs and empty questions.
- Hook: `NeedsInput` event with `message`, `session_id`, and `cwd`; stored question under `~/.cache/claude/`, isolated by validated session ID.
- Report: optional boolean `needs_input`; true means preserve `needs-you` with the explicit question, false releases protection. Missing means no change to protection.
- Registry: store protection on the session entry; skip screen-state overrides while protected, without suppressing unrelated update detection.

- [ ] Write command/hook regression tests for valid Claude and OpenCode IDs, surviving tool/turn completion, clearing on prompt/start/end, session isolation, and invalid input. Write registry tests exercising parsed reports and pane reconciliation, plus false/missing/invalid boolean behaviour.
- [ ] Run focused tests and verify failures are caused by missing functionality.
- [ ] Implement the command and hook lifecycle; include explicit false on lifecycle resets. Propagate report failures through the command rather than silently succeeding.
- [ ] Implement boolean validation, storage, and reconciliation protection in the registry. Preserve existing request display and normal permission behaviour.
- [ ] Run both affected test suites; verify question text appears in the snapshot and disappears when answered.

### Task 2: Make the command available and required

**Files:**
- Modify: `config/greywall/.config/greywall/greywall.json`
- Modify: `config/claude/.claude/CLAUDE.md`
- Modify: `README.md` (existing cockpit usage section)

**Interfaces:**
- Allow reading the new shell command through the sandbox, following adjacent command entries.
- Agent instruction: call `claude-needs-input '<question>'` immediately before ending with a question whose answer is required to proceed. Do not signal optional offers; do not replace `ask-to-run` or `ask-to-guide`.

- [ ] Add the command's sandbox read path and executable permission.
- [ ] Document the instruction, lifecycle, and existing-session restart requirement; confirm how OpenCode consumes the shared instructions before changing any OpenCode configuration.
- [ ] Run Bash syntax checks, JSON validation, affected tests, and the full workspace test command (`cargo test` from `apps/`). Report failures verbatim; do not disable unrelated failing tests.
- [ ] Review `git diff --check` and diff stat, commit on the feature branch, export patches to `.outgoing`, and request host-side reviewed apply through `ask-to-run`.
- [ ] After approved apply, request `make -C ~/dotfiles cockpit test` and `claude-sessions reload` through `ask-to-run`; report actual outcomes and any required session restart.
