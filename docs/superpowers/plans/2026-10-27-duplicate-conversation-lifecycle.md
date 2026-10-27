# Duplicate Conversation Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A managed OpenCode conversation has one launch, and exiting it closes all verified managed copies left by earlier launches.

**Architecture:** Serialize OpenCode starts and exits by conversation ID. Discover managed copies through the host's tmux inventory and the conversation identity placed in each tmux session's environment when it was created, independently of the daemon snapshot. Resume reuses an existing copy without changing its directory or ownership. Preserve daemon report authentication and launch ownership checks.

**Tech Stack:** Existing Bash helpers, flock, tmux, and Rust script integration tests; no dependencies.

**Spec:** Theo approved Option A in this conversation for todo `a9178c3c`: a shared per-conversation launch lock, reuse of the existing pane, and exit of every verified managed copy, preserving daemon ownership checks. Related todo `235d6996` describes the duplicate-resume path.

## Global Constraints

- No registry ownership/authentication changes, sandbox changes, or broad process-name kills.
- Never close another conversation merely because a stale snapshot points to its pane.
- Resume, restore, wake and restart all use the same constructor; preserve team tokens, prompts, models, and terminal ownership.
- Do not execute host lifecycle commands against real conversations while developing.
- `config/bin/.local/bin/cockpit` and `cockpit-agents/opencode.sh` run with Theo's rights; flag their changes at handoff. No dependency or Makefile changes.

## Review Focus

- Concurrent resume/restore before any plugin report starts one process.
- A dead pane or stale daemon row is not mistaken for an active managed launch.
- A reused pane belonging to another conversation is never sent exit keys.
- Launch locks do not leak into tmux/agent processes or deadlock restart and team wake.
- An inventory failure fails visibly instead of authorizing another launch or falsely claiming exit succeeded.

---

### Task 1: Shared start deduplication

**Files:** `config/bin/.local/bin/cockpit-agents/opencode.sh`; `apps/cockpit/tests/cockpit_script.rs`.

**Interfaces:** Keep `start_opencode(dir, session_id, model, arguments...)`, returning the existing or newly created managed tmux target on stdout. Add `opencode_conversation_panes(id)`, returning tab-separated managed target and pane IDs; derive identity from host tmux state, not session reports.

- [ ] Add failing entry-point tests: resume focuses an existing conversation without `new-session`; restore and wake reuse the same constructor; concurrent resume/restore starts once before a plugin report; dead panes do not prevent a fresh start.
- [ ] Run focused `cargo test --manifest-path apps/Cargo.toml -p cockpit --test cockpit_script opencode` and capture the expected new failures.
- [ ] Implement exact-ID inventory and a per-conversation flock in the constructor. Keep the lock through inventory and tmux creation; release it before any long-lived process or viewer attachment can inherit it. On multiple existing copies, reuse one; do not silently kill duplicates during resume.
- [ ] Re-run focused tests, including existing model/prompt/team-token and epilogue tests. Use controlled fake host state and an isolated tmux fixture where needed, never real sessions.

### Task 2: Close every verified managed copy

**Files:** `config/bin/.local/bin/cockpit-agents/opencode.sh`; `config/bin/.local/bin/cockpit`; `apps/cockpit/tests/cockpit_script.rs`.

**Interfaces:** Keep per-pane `agent_opencode_exit(target, pane, id)`. Add `exit_opencode_conversation(id)` to acquire the same conversation lock, inventory its managed panes, thaw each when necessary and invoke the existing per-pane exit. Route OpenCode conversation IDs through this helper from `exit_session`; preserve the generic startup-target and other-agent paths.

- [ ] Add failing entry-point tests for two copies with a stale snapshot, a reused unrelated pane, one failing copy that must not prevent closing the others, and launch/exit serialization.
- [ ] Implement inventory-driven exit with exact identity checks and bounded existing graceful-exit/kill behavior. Preserve diagnostics and error propagation. Do not fall back to an unverified stale snapshot pane.
- [ ] Re-run all OpenCode, restart, sleep/wake, restore and exit script regressions. Verify no daemon authentication code changed.

### Task 3: Review, full verification and reviewed application

**Files:** Only the above implementation/test files and this plan; a small README correction only if current usage guidance becomes wrong.

- [ ] Request an independent read-only review of lifecycle races, identity matching, unrelated-pane protection and lock lifetime; resolve important findings.
- [ ] Run `git diff --check`, shell syntax checks and the complete project `make test`. If this sandbox still lacks the shared full-test lock, ask Theo before requesting a host run; do not silently bypass the lock.
- [ ] Commit only this task's files on its feature branch, with `Todo: a9178c3c` and `Todo: 235d6996` only if both are fully addressed. Preserve unrelated untracked files.
- [ ] Export with `git format-patch origin/main -o .outgoing`. Obtain Theo's approval for host testing/application as required by the current request; preserve `cockpit apply`'s patch confirmation and run live-copy tests afterward.
- [ ] Check the todo list, close only items actually resolved, and report exact test results and anything not live-tested.
