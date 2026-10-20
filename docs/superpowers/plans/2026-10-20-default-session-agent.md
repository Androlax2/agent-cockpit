# Default Session Agent Implementation Plan

## Execution ledger

- Pre-flight: Task 2 consumes Task 1's three string settings through the existing manager command; no interface conflict.
- Ruling: The installed V2 TUI has no --model flag (host CLI help verified). Pass a new-session-only model reference to the existing cockpit plugin, which creates the managed session with ctx.session.create({id, model}); resumes receive no preference. Cost if wrong: startup integration failure, covered by adapter/plugin tests.
- Ruling: Read the project-scoped catalog using the verified `api --standalone get /api/model` command via the existing sandbox launcher. Run it only when the model/effort picker needs it, in a bounded worker; do not start the shared service. Cost if wrong: catalog startup latency or a visible picker error.
- The execution skill's workspace helper is unavailable (reported to the dotfiles todo list). This plan retains execution notes; no helper was recreated.
- Task 1: implemented string preference validation and preservation, legacy identity separation, new-conversation-only model references, and catalog dispatch. New regression tests were observed failing before implementation and passing afterward.
- Task 2: implemented Settings rows, project-scoped model/variant picks, default creation flows, explicit-agent palette action, and help. Existing phone settings and identity-based session operations are retained.
- Ruling: named `claude` in Fish must explicitly select Claude, just as named `opencode` already does. Otherwise changing the global default would make the command named Claude run OpenCode. Cost if wrong: one extra explicit agent argument in the host Fish route.
- Ruling: settings honor CLAUDE_SESSIONS_STATE_HOME like the Rust cockpit, because OpenCode changes XDG_STATE_HOME inside its sandbox. Cost if wrong: commands would read different preferences; covered by a failing-then-passing manager regression test.
- Final review: independent read-only reviewer found two Important recovery gaps. Fixed malformed model replacement without reading the old value, and added the visible `d OpenCode default` reset on model/effort rows, independent of catalog access. Three regression tests observed RED then GREEN; `cargo test --lib`: `test result: ok. 270 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.
- Final review: corrected the plan's explicit-model-argument criteria below. V2's full TUI does not accept those arguments; the persisted per-session selection remains changeable in OpenCode itself.
- Real startup smoke initially failed with HTTP 404. A temporary test harness first used an ignored direct-file plugin setting, then auto-discovery exposed an actual setup failure: the installed plugin context throws `Session.NotFoundError`, not the HTTP schema's `SessionNotFoundError`. A new regression test observed RED then GREEN; the plugin handles both specific tags and rethrows other errors.
- Host smoke after that fix: `PASS: new empty session starts with opencode/minimax-m3.1-flash-free#low`; `PASS: plugin reload preserves its existing model and effort`; `Removed the temporary empty smoke-test session`. No prompt was sent. The temporary harness and its project configuration were removed.
- Host-executed files changed: `config/bin/.local/bin/claude-sessions`, its `claude-sessions-agents/opencode.sh` adapter, and `config/fish/.config/fish/config.fish`. No live apply, service restart, git write, or dependency change performed.
- Verification prerequisites: the user approved building the clone's release `osd` binary because desktop unit tests alone do not produce that executable. Test commands clear inherited CLAUDE_SESSIONS_STATE_HOME, CLAUDE_SESSIONS_AGENT, and OPENCODE, without changing the host environment. The unmodified-environment workspace run had failures listed below; the clean workspace run passed all 991 tests before the final review fixes.
- ShellCheck could not run: `shellcheck: command not found` (exit 127), reported through claude-todo blocked. Bash and Fish syntax checks pass.
- Task 1: complete. Task 2: complete. Final command: `env -u CLAUDE_SESSIONS_STATE_HOME -u CLAUDE_SESSIONS_AGENT -u OPENCODE cargo test --release --workspace --no-fail-fast` from `apps/`: 53 test targets, 995 passed, 0 failed, 0 ignored. Full output: `/tmp/opencode/default-session-verified.log`. `git diff --check`, `bash -n` for the manager/adapter, and `fish --no-config --no-execute` for Fish config all exited 0. Existing unused-mut and unused-method test warnings remain; no tests were disabled.

### Initial whole-workspace verification failures

The first run lacked the release osd executable and inherited session state pointing outside test fixtures. These were not suppressed or disabled. After the approved prerequisite build and environment-isolated run, every target passed.

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

An earlier package-only test also failed `a_message_said_to_a_session_is_audited_by_its_hash` before its release executable prerequisite existed.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add global Settings choices for the default session agent and OpenCode's default model and effort.

**Architecture:** Keep preferences in the existing cockpit settings.json, written through claude-sessions. Resolve the launch default separately from the historical Claude fallback used to identify existing sessions. Apply OpenCode model and variant selection only when creating a new conversation.

**Tech Stack:** Rust cockpit TUI, Bash manager and agent adapters, jq, existing Rust integration harness.

**Spec:** Approved in chat: global Settings defaults for Claude or OpenCode, with OpenCode model and effort choices; affect new sessions only and preserve explicit agent choices and existing sessions.

## Global Constraints

- No dependency or manifest changes.
- Claude remains the initial default when no preference exists.
- Preserve explicit agent choices and existing conversation selections.
- Retain sandbox launchers, session ownership checks, and current safety boundaries.
- Do not commit, stage, rebase, deploy, or run standalone build/install commands without authorization.
- Changes to config/bin/.local/bin/claude-sessions and its agent adapter run with host rights after application and must be called out.
- OpenCode V2 model references are provider/model#variant; variants must come from the selected model's metadata, not an invented effort list.

## Review Focus

- Malformed settings must not silently select a different agent or overwrite the file.
- Changing the default must not reclassify old Claude reports or alter resumed sessions.
- Explicit --agent must override the saved launch default; model/effort choices made within OpenCode must survive plugin reload and resume.
- Selecting a different model must clear an incompatible saved effort; models without variants must remain usable.
- Catalog failures and unavailable selections must show an error, not hide models or silently replace the user's choice.

### Task 1: Persist and apply launch defaults

**Files:**
- Modify: config/bin/.local/bin/claude-sessions
- Modify: config/bin/.local/bin/claude-sessions-agents/opencode.sh
- Test: apps/cockpit/tests/claude_sessions_script.rs
- Test: apps/cockpit/tests/opencode_terminal.rs

**Interfaces:** Extend `claude-sessions setting` with string preferences `default-agent`, `opencode-model`, and `opencode-effort`. Empty model/effort means OpenCode's own default. Keep existing boolean setting behavior unchanged. Keep DEFAULT_AGENT=claude as the legacy identity fallback, not the configurable launch choice.

- [ ] Confirm the installed V2 interactive model-selection flag and catalog access contract using official V2 docs and existing integration code. The sandbox's `opencode --help` invokes a launcher that currently fails with permission errors; if local inspection is needed, request host-side help via ask-to-run rather than bypassing the launcher. Do not guess a catalog command or endpoint.
- [ ] Add integration tests for initial Claude selection, a persisted OpenCode default, explicit Claude/OpenCode overrides, unchanged boolean settings, invalid agents, malformed JSON preservation, and recovery through the setting command even when a saved launch preference is invalid.
- [ ] Run `cargo test --test claude_sessions_script` from apps/cockpit and verify the new behavior tests fail before implementation.
- [ ] Extend setting validation/persistence and resolve the saved agent only for commands that create new sessions. Preserve id-based dispatch for resume/restart/restore and legacy reports.
- [ ] Add adapter tests proving new OpenCode sessions receive the saved model plus optional #variant as one environment value, and resume/restart/restore receive no new defaults. Cover foreground, background, parallel, review, and terminal creation through existing entry points. Plugin tests and a real startup smoke must prove that an existing session's selection survives reload.
- [ ] Run the added tests to establish failures, then apply defaults only at new-conversation adapter call sites; do not alter shared start_opencode behavior for resumed conversations.
- [ ] Run `cargo test --test claude_sessions_script --test opencode_terminal --test agents` from apps/cockpit. Preserve all failures verbatim in the handoff.

### Task 2: Expose defaults in Settings and honor them in creation flows

**Files:**
- Modify: apps/cockpit/src/settings.rs
- Modify: apps/cockpit/src/app.rs
- Modify: apps/cockpit/src/tests.rs
- Modify: README.md

**Interfaces:** Add Settings rows for Default agent, OpenCode model, and OpenCode effort. Reuse existing picker and manager command patterns. Model choices use verified OpenCode catalog IDs; effort choices use that model's actual variants. An explicit provider-default choice clears the override. Keep an explicit agent-choice action available when the usual new-session flow uses the saved default.

- [ ] Inspect existing picker, Settings keyboard/mouse, and asynchronous work patterns before editing; keep catalog loading off the UI event loop. Select the smallest verified catalog interface from Task 1, scoped to the active project, without starting unrelated services or reading credentials.
- [ ] Add tests for the three Settings rows, keyboard/mouse selection, cancellation without writes, visible catalog errors, models without variants, and clearing incompatible effort on a model change.
- [ ] Add creation-flow tests for the saved default, explicit agent overrides, and todo handoff: reuse an existing target session without changing its agent or model; apply defaults only when creating a new one.
- [ ] Run `cargo test --lib` from apps/cockpit to verify the new tests fail, then implement the Settings and creation-flow wiring using the existing settings command as the writer.
- [ ] Update help text and README with the defaults, override action, new-session-only behavior, and model-specific effort. Do not change OpenCode's unrelated global configuration.
- [ ] Run `cargo test` from apps/cockpit and report the exact results, including pre-existing failures. Run `git diff --check` and show `git diff --stat`.
- [ ] Stop before git writes or live application unless authorized. Mark todo 10e3cbc8 done only after implementation and verification are complete, explicitly identifying whether the changes have been applied live.
