# Native Codex Cockpit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. In this harness use the available `executing-plans` skill for native execution; do not assume an unavailable skill can be loaded.

**Goal:** Make native Codex CLI a managed cockpit agent without bypassing greywall, user approvals or the native terminal UI.

**Architecture:** Extend the existing agent row/helper/launcher/report-bridge contract. Keep Codex runtime and hooks inside greywall; host routing uses a namespaced cockpit ID bound to the verified native thread. Collaboration is a separate dependent plan, not part of this prerequisite's delivery semantics.

**Tech Stack:** Rust 2024, existing cockpit/desktop crates, shell launchers, Codex 0.161.0 native CLI and documented lifecycle hooks; no new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-20-native-codex-cockpit-design.md`; parent `docs/superpowers/specs/2026-10-20-linked-cockpit-sessions-design.md`.

## Global Constraints

- Keep Codex's native terminal UI, approvals and authentication.
- No installs, dependency/manifest edits, login changes, account migration, global policy weakening or automatic host reloads.
- Never use `--last` to identify a managed session.
- Changed hooks remain awaiting the user's native trust review; never use `--dangerously-bypass-hook-trust`, auto-trust or invent a trust database edit.
- The app-server, bridge and all Codex execution remain inside greywall; no server runs product code with host privileges.
- All three native UIs must eventually pass the dependent collaboration conformance suite; this plan alone does not complete todo `1d3fd318`.
- Do not push, install or apply partial feature work. Commit only named requested files on the existing feature branch after the relevant tests pass.

## Review Focus

- Native UUIDs overlap Claude's UUID namespace: Codex must retain its own explicit agent identity and namespaced cockpit ID (Task 1).
- Missing/untrusted hooks look like idle sessions: show unavailable/unknown, never invent readiness (Task 2).
- A CLI update removes a hook/flag or changes shutdown behavior: fail with a useful capability error, not another agent's behavior (Tasks 1–3).
- Restart/resume accidentally picks another native thread: exact saved mapping or visible refusal, never `--last`/display-name matching (Task 3).
- A user's existing Codex hooks/MCP/auth config exists: preserve it and require native trust review for changed hooks (Task 2).

---

## File boundaries and interfaces

New identifiers below are proposed project code, not assertions of existing upstream APIs.

- `apps/cockpit/src/codex.rs`: `pub fn cockpit_id(native_thread: &str) -> Result<String, String>`; rejects empty/oversized/control-bearing IDs, returns `cdx_` + SHA-256 hex. `pub fn translate_hook(payload: &serde_json::Value) -> Result<serde_json::Value, String>` emits normalized cockpit reports with `agent=codex`, namespaced `session`, native thread and parent metadata.
- `apps/cockpit/src/bin/cockpit-codex-hook.rs`: stdin JSON → translated report via existing authenticated socket, stdout appropriate non-controlling hook JSON (`{}` for Stop/Interrupt), diagnostics to stderr. Reporting failure never returns a permission allow/deny or a continuation instruction.
- `apps/cockpit/src/agents.rs`, `sessions.rs`, `lib.rs`: registration, startup files, explicit ID pattern and capability inventory; no guessed native features.
- `config/bin/.local/bin/greywall-agents/codex`, new `config/greywall/.config/greywall/learned/codex.json`: managed isolation using shared guards; profile permits only reviewed required paths.
- New `config/codex/.codex/hooks.json`: isolated hook source pointing at the installed bridge. Before creating, check existing target ownership through approved host inspection; if unmanaged content exists, prepare a reviewed merge rather than replace it.
- New `config/bin/.local/bin/claude-sessions-agents/codex.sh`: full existing `AGENT_VERBS` contract with `agent_codex_*` names; unsupported features refuse explicitly.
- `apps/desktop/src/registry.rs` and `tests.rs`: native-to-cockpit mapping bound to verified ancestry, not trusted report claims; Codex is an agent process for lifecycle checks.
- `apps/cockpit/tests/native_codex.rs`: executable/launcher/hook tests using `common::sandbox::Sandbox`, fake tmux/greywall/vendor CLI, no network/model calls.
- Existing `agents.rs`, `greywall_launcher.rs`, `session_state_hook.rs`, `claude_sessions_script.rs` integration tests extend only where their contract needs Codex.

### Task 1: Pin the installed native contract and collision-free reports

**Files:** Create `apps/cockpit/src/codex.rs`, `apps/cockpit/tests/native_codex.rs`; modify `apps/cockpit/src/lib.rs`, `apps/cockpit/src/agents.rs`, `apps/desktop/src/registry.rs`, `apps/desktop/src/tests.rs`.

**Consumes:** Installed 0.161.0 help already inspected via host-approved read-only commands; current official hook schema. **Produces:** `cockpit_id`, `translate_hook`, `codex` agent metadata and verified native-thread binding.

- [ ] Read the installed hook schema/help and save minimal synthetic payload fixtures for start, prompt, permission, stop, interrupt, end and subagent events in the test source. Do not copy real prompts, credentials or transcripts. Verify native thread versus root session identity before mapping.
- [ ] Write failing tests `codex_ids_do_not_alias_claude_uuids`, `hook_translation_retains_native_thread_and_agent`, `subagent_events_do_not_replace_root_card`, `unknown_native_events_do_not_report_idle` and `native_id_claims_cannot_rebind_another_pane`. Assertions include `cockpit_id(uuid) != uuid`, exact `cdx_` length 68, stable repeat output, distinct thread outputs, rejected controls/oversize, and verified sender target winning over payload fields. Set native-thread max to 256 UTF-8 bytes and fail beyond it.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test native_codex`; expect compilation/test failure for missing new functions. Record the actual failure.
- [ ] Implement those exact functions and explicit `codex` metadata. Update any hardcoded array lengths and test fixture assumptions. Add only capabilities whose installed contract has been verified; missing lifecycle data is unknown, not waiting. Bind native IDs only after the same peer/PID checks already used for managed sessions.
- [ ] Run the native Codex target and `cargo test --manifest-path apps/Cargo.toml -p desktop --lib codex`; require all selected tests pass.
- [ ] Commit this cohesive change with explicit file staging: `feat(cockpit): recognize verified native Codex sessions`. Do not stage the mockup or unrelated work.

### Task 2: Sandboxed launcher and non-authorizing lifecycle bridge

**Files:** Create hook binary, hooks source and profile listed above; modify Codex launcher, `apps/cockpit/src/sessions.rs`, `apps/cockpit/tests/native_codex.rs`, `apps/cockpit/tests/greywall_launcher.rs`. Add an installed helper wrapper only if current build/install tooling does not already expose Cargo autodiscovered bins; inspect Makefile's cockpit target first and keep the change narrowly listed.

**Consumes:** Task 1 translation and binding. **Produces:** a sandboxed managed runtime that publishes lifecycle events and can use existing ask-to-run/todo tooling.

- [ ] Write failing launcher tests `codex_uses_shared_project_guards`, `codex_does_not_start_in_live_dotfiles`, `codex_receives_project_answer_paths`, `codex_keeps_other_agent_credentials_hidden`, `hook_reporting_never_approves_native_permissions`, `untrusted_hooks_are_not_bypassed`, `existing_codex_hook_content_is_preserved`. Assert fake greywall receives the Codex profile/shared settings; no bypass flag, broad allow-path, host socket or other-project answers; all reporting event outputs omit permission decisions and continuations.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test native_codex`; record the intended failing assertions.
- [ ] Implement `cockpit-codex-hook` by calling Task 1 functions and the existing `approval::file_request(kind: &str, request: &Value) -> Result<(), Refusal>` with `session-request`; use a dedicated normalized report rather than piping Codex into Claude UUID inference. Handle hook-specific stdout shape, bounded stdin, clear errors and no fail-open readiness.
- [ ] Make Codex launcher source `launcher-common.sh` with `agent=codex`; review every newly inherited mount/secret behavior. Add a minimal static profile and hooks source, retaining existing user config. Do not enable auto-profile learning for regular managed launches; if required policy is unavailable, report the exact blocker. Ensure installed binary/config paths are read-only within greywall and Codex cannot read host team state.
- [ ] Extend nested-agent detection and startup-file freshness reporting for Codex; ensure root attribution works when tools spawn another CLI. Native permission requests only mark attention.
- [ ] Rerun the two selected integration targets; require pass. Native hook trust must still be pending for Theo when first installed; document it rather than bypassing it.
- [ ] Commit named changes: `feat(cockpit): sandbox Codex and report native lifecycle`.

### Task 3: Native terminal lifecycle and safe capability refusals

**Files:** Create `config/bin/.local/bin/claude-sessions-agents/codex.sh`; modify `config/bin/.local/bin/claude-sessions`, `apps/cockpit/src/agents.rs`, `apps/cockpit/src/app.rs` only where new-agent choice or capability wiring requires, `apps/cockpit/tests/agents.rs`, `apps/cockpit/tests/native_codex.rs`, `apps/cockpit/tests/claude_sessions_script.rs`.

**Consumes:** Verified binding and launcher. **Produces:** all existing `AGENT_VERBS` for native Codex; pick/start/view/resume/restart/close without another CLI's shortcuts.

- [ ] Extend fake vendor CLI/tmux tests with `codex_new_starts_native_tui_in_greywall`, `codex_resume_uses_exact_native_id`, `codex_restart_preserves_thread_mapping`, `closing_codex_does_not_archive_or_delete_history`, `codex_missing_thread_refuses_instead_of_resuming_last`, `codex_unsupported_features_name_the_capability`. Test new/background/parallel launch ownership and ask-to-run attention attribution.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test agents --test native_codex --test claude_sessions_script`; expect missing helper/behavior failures.
- [ ] Implement all required helper verbs, namespaced managed tmux prefix `claude-retry-codex-`, native `resume` by exact ID, prompt handling and clean lifecycle using the inspected CLI flags. Launch cwd and arguments via argv arrays, never shell-interpolated text. Preserve native approvals; missing input/interrupt/exit semantics require verification, not invented key sequences.
- [ ] Keep existing human `say` behavior separate from collaboration. For Codex `say`, use verified native queue only against its exact managed runtime; if namespace-safe queue discovery has not been established, return a named capability refusal until the collaboration adapter task proves it. Do not type blindly into the TUI as a fallback.
- [ ] Replace synthetic tests that already use `codex` as a fictitious extra agent with a distinct fixture-only name, preserving their original assertions. Do not delete those tests to accommodate real Codex.
- [ ] Rerun selected targets and native desktop tests; require pass before committing `feat(cockpit): manage native Codex terminal sessions`.

### Task 4: Verify native parity and document the adapter prerequisite

**Files:** Modify `apps/cockpit/tests/native_codex.rs`, `README.md`, `config/bin/.local/bin/claude-sessions-agents/README.md`; update parent plan's prerequisite checkbox.

**Consumes:** Tasks 1–3. **Produces:** independently working native agent integration and an explicit readiness result for the team-messaging plan.

- [ ] Add end-to-end fake-entrypoint tests showing exact todo/project attribution, native needs-input/approval attention, unsupported UI actions, closed-session removal and failed starts, with all three agent kinds covered through the existing dispatch entrypoints.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test native_codex --test agents --test greywall_launcher --test session_state_hook --test claude_sessions_script` and `cargo test --manifest-path apps/Cargo.toml -p desktop --lib`; preserve actual output and investigate failures systematically.
- [ ] Document native launch choice, required hook trust, version/capability checks, lifecycle and the fact that registration is not yet collaboration-ready. Load `oss-docs` before README edits. No misleading claim that every CLI operation is supported.
- [ ] Commit the named tested changes: `docs(cockpit): document native Codex lifecycle and limits`.
- [ ] After reviewed application of the complete requested work, request a host smoke test through `ask-to-run`/`ask-to-guide`: actual native TUI, user-reviewed hook trust, correct card, exact resume, permission request, close. Before manual steps ask readiness; no login or billed model turn without explicit approval. Mark this task incomplete if that verification cannot run.

## Execution and handoff

Read the parent collaboration plan before choosing runtime endpoints, so native launch does not create an incompatible global daemon. Keep this prerequisite on the same feature branch or use a separately approved branch arrangement; no stack/rebase is authorized implicitly. Use focused tests first; full-suite/apply/reload are owned by the parent completion task and require the normal explicit host approval. Do not mark todo `1d3fd318` done here.
