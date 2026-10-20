# FigCraft side-by-side review implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Read and answer the existing cockpit conversation on the left while viewing the existing FigCraft window on the right.

**Architecture:** The FigCraft agent offers side-by-side viewing when the user's request involves visually reviewing or approving designs. After consent, an explicit host command arranges the two existing windows on workspace 6; a separate stop command restores their previous workspaces, floating state, geometry and fullscreen state. No keyboard shortcut, automatic window movement, application start or FigCraft commands.

**Tech Stack:** Hyprland Lua API; Rust integration tests invoking lua5.4 with a recording `hl` stand-in.

**Spec:** Approved in conversation: cockpit left, FigCraft right, no integration or sandbox permission changes. The user rejected keyboard shortcuts: the agent must offer this contextually and wait for consent, including restoration.

## Global constraints

- No sandbox, launcher, MCP forwarding, dependency manifest or process-launch changes.
- No install, build, reload, host-window manipulation or ask-to-run without further approval.
- No git writes; leave changes in this checkout for review.
- Moving/resizing must not issue canvas navigation, document selection or zoom commands. FigCraft itself may respond to viewport resizing; preserving its internal zoom cannot be guaranteed by Hyprland.
- Use the installed `/usr/share/hypr/stubs/hl.meta.lua` and official API documentation to verify dispatcher parameters before implementing them.

## Review focus

- Missing cockpit or FigCraft: notify, with no partial window rearrangement.
- Multiple FigCraft windows: use the focused FigCraft, otherwise refuse to guess.
- Consent: the skill must offer the arrangement before requesting host execution, never rearrange windows merely because FigCraft is connected.
- Scaled/rotated monitors and nonzero monitor origins: use logical workspace geometry, not physical pixel dimensions.
- A window closed during review: restore the survivor without reopening anything.

---

### Task 1: Reversible host-window arrangement

**Files:**
- Create `config/hyprland/.config/hypr/figcraft-review.lua`.
- Create `apps/cockpit/tests/figcraft_review_lua.rs`.

**Interfaces:**
- Produces a Lua module with `start()` and `stop()`; consumes `hl` window and workspace queries and dispatchers. Repeated start/stop calls are idempotent.
- Select exactly one `claude-sessions` window and an `ai.storyteller.figcraft` window (focused first, otherwise uniquely present).
- Entry comes from an explicitly approved host command, not the focused application's shortcut. Place the pair on workspace 6 on the cockpit's monitor, left/right 50:50, then focus the cockpit.
- Save both windows' original workspace, floating state, geometry and fullscreen states before changing anything. Restore surviving windows on stop. Never resolve a saved window to a different process/window.

- [x] Write a Lua recording fixture following `apps/cockpit/tests/game_mode_lua.rs`.
- [x] Add tests asserting left/right geometry, both windows on workspace 6, focus returned to the cockpit and no exec/application commands.
- [x] Add tests asserting restoration on stop, repeated start/stop idempotence, missing-window refusal, multiple-FigCraft refusal/selection, logical monitor geometry and survivor restoration.
- [x] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test figcraft_review_lua`; confirm failures are due to the missing implementation.
- [x] Implement `start()`/`stop()` and their saved arrangement in the new Lua module, using verified dispatcher parameters only. Report invalid prerequisites through the existing `hl.notification.create` convention.
- [x] Run the same test command; require all tests to pass.

### Task 2: Agent-proposed review, without shortcuts

**Files:**
- Modify `config/bin/.local/bin/claude-sessions`.
- Modify `config/claude/.claude/skills/figcraft-design/SKILL.md`.
- Modify `README.md`.
- Extend `apps/cockpit/tests/claude_sessions_script.rs` and add regression coverage for the skill's consent instructions using the existing skill-contract test patterns.

**Interfaces:**
- Consume `require("figcraft-review").start()`/`.stop()` from Task 1 through explicit host commands `claude-sessions design-review start` and `claude-sessions design-review stop`.
- When asked to review/approve boards, compare alternatives or give visual feedback, the agent offers: conversation left, FigCraft right, without reopening the document. It waits for consent before ask-to-run, following the existing host-approval policy.
- Do not offer merely on connection, on every board, or after a refusal in the same review. At the end, offer restoration; do not move windows without consent.

- [x] Add regression tests asserting exact start/stop dispatch, invalid-action rejection, no app launch, no shortcut addition, and skill instructions covering contextual offers, consent before ask-to-run, refusal and restoration.
- [x] Run the focused tests and confirm the new command/instruction tests fail.
- [x] Add the explicit commands, contextual skill instructions and documentation; require both applications already open. Do not modify bindings.lua.
- [x] Run focused tests and the full workspace suite; earlier failures and final passing verification recorded below.
- [x] Review `git diff --check` and `git diff --stat`. Hyprland's new `figcraft-review.lua` and the `claude-sessions` command run on the host after application; no host application/reload has been performed.
- [x] No host application/reload or ask-to-run performed. Implementation and regression verification complete; live-window validation remains unperformed. Todo 0a9a9f1b can be closed; deployment is separate.

## Execution record

- Pre-flight: Task 1 produces Lua start/stop consumed by Task 2's explicit host command; interfaces agree. Existing isolated branch: todo-0a9a9f1b.
- Ruling: keep the execution record here without helper scripts or git writes; the execution skill's workspace helper is absent and this task forbids git writes.
- Ruling: reject grouped/pinned windows rather than break unrelated layout; supporting them would need additional state handling.
- Ruling: test executable arrangement and command boundaries, not exact skill prose. Testing guidance rejects source-text assertions as evidence of agent behavior; contextual offers require scenario evaluation.
- Task 1: complete. Focused test observed RED (module absent): 0 passed, 8 failed; then GREEN: 8 passed, 0 failed.
- Task 2 commands: RED 0 passed, 3 failed (unknown subcommand); GREEN 3 passed, 0 failed, 166 filtered out. Updated skill triggers and global FigCraft instructions so read-only visual reviews also load the contextual offer.
- Special-workspace restoration: additional test RED (used negative ID), corrected selector to named special workspace, GREEN 9/9 layout tests.
- Ruling: restoring a tiled window returns it to compositor tiling rather than reconstructing the entire tiling tree; exact saved geometry is meaningful for floating windows. Documented explicitly in README.
- Scenario evaluation: separate read-only agents simulated original committed versus updated instructions. Original 2/4 (missing initial offer and restoration offer), updated 4/4; no real app/approval calls performed. Skill role/todo bookkeeping is not a window operation.
- Full debug suite failed at unrelated wrapper prerequisites: a_recorded_text_marked_fp_is_kept_only_as_its_hash and verify_says_whether_the_chain_holds, `claude-audit is not built yet: make -C ~/dotfiles cockpit`. Re-running the repository's documented test command (`cargo test --release --quiet --manifest-path apps/Cargo.toml`), which produces the release executables its wrappers expect.
- Correction: release test builds cockpit executables for its integration tests, but does not produce the desktop `apps/target/release/osd` executable; only its unit-test harness exists. Full suite cannot pass without the normal release build. Waiting for explicit build authorization; no host installation or reload requested.
- Final review fixed: dispatcher failure results were ignored and restoration state could be discarded. Failure-injection tests RED, checked dispatches plus preserved recovery state GREEN.
- Final review fixed: bare names such as `previous` are reserved workspace selectors. Explicit `name:previous` regression RED, prefixed named selectors GREEN.
- Final focused verification: layout `test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`; CLI `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 166 filtered out`. `bash -n`, `git diff --check`, skill validator passed.
- Final review minor (deferred): cross-monitor restoration/workspace destruction is not covered by the fixture (both windows currently share one simulated monitor).
- Full release suite with `--no-fail-fast`, before the final three layout regressions: 905 passed, 15 failed, 0 ignored. Full output: `/tmp/opencode/figcraft-review-full-suite.log`. No tests disabled.

Full-suite failures (verbatim names):

```text
yes_and_no_from_the_phone_press_the_dialog_keys
compact_is_typed_only_at_the_prompt
exit_types_slash_exit_at_an_idle_prompt
handoff_types_rc_at_an_idle_prompt
prompt_commands_pause_between_the_text_and_its_enter
restart_exits_then_resumes_the_same_conversation
say_refuses_while_a_dialog_is_open_and_keeps_the_text_out_of_the_log
say_sends_at_the_prompt_and_queues_while_working
a_session_started_from_a_shell_reports_even_under_the_daemon
tests::a_credits_session_is_marked_in_the_snapshot_and_the_reboot_record_until_it_switches_back
tests::a_request_held_or_refused_is_audited_without_its_command
tests::a_sessions_agent_is_in_the_snapshot_and_the_reboot_record_and_claude_by_default
tests::a_terminal_session_is_marked_in_the_snapshot_and_the_reboot_record
tests::sessions_are_remembered_across_reboots
tests::the_daemon_writes_its_messages_to_the_shared_log
```

The first group calls the absent release `osd classify`; the desktop tests set temporary `XDG_STATE_HOME` but inherit `CLAUDE_SESSIONS_STATE_HOME=/home/theo/.local/state`, which takes precedence in `paths::state_home` and is correctly read-only in the sandbox. Test-process-only removal of that inherited override would keep state in the temporary fixtures, not grant host access. Full CLI target after the review fix also reported 162 passed, 7 failed (the same classifier-dependent failures). No unrelated production/test harness changes made.

- User explicitly authorized the local release build and test-process-only state isolation. `cargo build --release --manifest-path apps/Cargo.toml`: `Finished release profile [optimized] target(s) in 2.15s` (successful). No install, commit, patch application, compositor reload or host window manipulation.
- First isolated verification: `env -u CLAUDE_SESSIONS_STATE_HOME cargo test --release --quiet --manifest-path apps/Cargo.toml --no-fail-fast` reported 922 passed, 1 failed: `a_session_started_from_a_shell_reports_even_under_the_daemon`, `left: []`, `right: ["waiting"]`.
- Root cause: that fixture removes `CLAUDECODE` but retains `OPENCODE=1` in its fake `claude` ancestors; the unchanged nesting guard correctly suppresses their reports. Removing both markers only from the test invocation models the unmarked interactive shell that the fixture intends. Focused verification: `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 24 filtered out`.
- Final isolated verification PASSED: `env -u CLAUDE_SESSIONS_STATE_HOME -u OPENCODE -u CLAUDECODE cargo test --release --quiet --manifest-path apps/Cargo.toml --no-fail-fast`. All 52 target results `ok`: 923 passed, 0 failed, 0 ignored. Full output: `/tmp/opencode/figcraft-review-final-suite.log`. No test or hook disabled/modified; fake-process/socket tests remain isolated from the real session.
- Task 2: complete. Fresh-agent review's two Important findings fixed via RED→GREEN regressions; full workspace suite green. No commit, host application, permission changes, or live-window verification. Cross-monitor fixture coverage remains the declared deferred minor.
