# FigCraft Floating Conversation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Read and reply to the existing agent conversation in a floating terminal on FigCraft's workspace, without displaying the cockpit or moving/resizing either main window.

**Architecture:** Attach a separate Kitty client directly to the existing agent's tmux session, never to `claude-manager`. Resolve the conversation through the daemon snapshot, and choose the FigCraft window explicitly when several are open. Use the host-only session-manager command and Hyprland rules; do not grant the sandbox compositor access.

**Tech Stack:** Bash, tmux, Kitty, Hyprland Lua, existing Rust integration-test harness.

**Spec:** User-approved conversation in this session: option A, an interactive view of the existing CLI; no session list, TODOs, or cockpit panels. FigCraft and the original CLI stay where they are.

## Global Constraints

- No second agent, conversation resume, detached pane, or movement of the original tmux pane.
- No new dependencies or dependency-manifest edits.
- Start and stop are explicit approved host operations, not automatic on MCP connection.
- Closing the floating client must not terminate the agent or its original viewer.
- Host-executed script and Hyprland changes must be identified in the handoff.
- Do not retain the obsolete side-by-side behavior behind `design-review start`.

## Review Focus

- Multiple projects/FigCraft windows must not silently attach an unrelated conversation.
- Stale/missing/dead session targets must fail before spawning a terminal.
- Arguments and window selectors must not become executable shell/Lua interpolation.
- A smaller second tmux client may reflow the shared agent display; this must not resize either main desktop window or silently change global tmux sizing policy.
- Closing/reopening or stopping the float must not close another terminal, kill the session, or accumulate duplicates.

---

### Task 1: Interactive conversation-only terminal

**Files:**
- Modify: `config/bin/.local/bin/claude-sessions`
- Test: `apps/cockpit/tests/claude_sessions_script.rs`

**Interfaces:**
- Consumes: daemon snapshot `sessions[].session`, `tmux_pane`, existing `CURRENT` selection, and tmux's `#{session_name}` lookup.
- Produces: `claude-sessions design-review start [session-id]`, `claude-sessions design-review stop`; start uses the current selection only when no explicit ID is supplied.

- [ ] Add failing integration tests: resolve Claude/OpenCode IDs to their existing agent tmux session; reject missing/dead targets and `claude-manager`; attach interactively without `-d`, `new-session`, `respawn-pane`, or launching an agent.
- [ ] Add failing tests for invalid/extra arguments and shell/Lua metacharacters, duplicate start, and stop targeting only the exact chat window. Extend the manager's existing compositor/terminal shims as necessary.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml --test claude_sessions_script design_review` and record the expected failures.
- [ ] Replace the old layout dispatch with explicit session resolution and a Kitty launch whose arguments directly invoke `env -u TMUX tmux attach-session -t =<resolved-session>`. Use a dedicated window class `figcraft-chat`; pass dynamic values as arguments, not shell source. Use the compositor's window PID/address to reuse/close only the intended float. Do not use a newly built product-specific chat UI.
- [ ] Run the targeted tests again and verify no existing viewer/current-selection state is rewritten. Add a test asserting absence of global/window sizing-option changes.

### Task 2: Workspace placement, lifecycle, and documentation

**Files:**
- Modify: `config/hyprland/.config/hypr/figcraft-review.lua`
- Modify: `config/hyprland/.config/hypr/apps/claude.lua`
- Modify: `apps/cockpit/tests/figcraft_review_lua.rs`
- Modify: `README.md`

**Interfaces:**
- Consumes: Task 1's dedicated Kitty window and resolved conversation target.
- Produces: explicit selection/placement of the chat on the selected FigCraft workspace, without side-by-side layout operations.

- [ ] Replace old layout expectations with failing Lua tests: only the chat window can be moved/resized/focused; cockpit and FigCraft retain workspace, floating, geometry, and fullscreen values.
- [ ] Add tests for a unique FigCraft window, multiple FigCraft windows with one focused, ambiguity refusal, named/special workspaces, scaled/rotated monitors, and missing/closed FigCraft. Choose the design before spawning the terminal; do not choose the first arbitrary window.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml --test figcraft_review_lua` and record the expected failures.
- [ ] Replace the snapshot/restore layout with chat-only placement. Match the dedicated class, place it on the selected FigCraft workspace, and keep it movable/resizable. Use the existing guide's initial width of 560 logical pixels and height of 85% of the monitor's usable area, clamped to fit. Never pin it across workspaces. Preserve the original CLI session and existing compositor safety checks where relevant.
- [ ] Update the README's Design reviews paragraph: explicit conversation target, conversation-only view, replying, close/stop semantics, and the shared terminal reflow trade-off. Replace—not supplement—the old side-by-side instructions. Check the repository's tracked agent instructions for that obsolete command guidance and update any affected references without changing unrelated policies.
- [ ] Run both targeted integration targets, `bash -n config/bin/.local/bin/claude-sessions`, and `git diff --check`; record exact results. Run the full cockpit suite when authorized, reporting unrelated failures without disabling tests.
- [ ] Inspect `git diff --stat`, commit only requested changes, export patches to `.outgoing`, and request the host-reviewed apply through `ask-to-run` as specified in `CLAUDE.md`. Run the host cockpit checks and reload after approval. Offer a short live smoke test before opening/moving windows; mark todo `deb9c6cd` done only after successful completion.

## Execution ledger

- Base: `51d2e13` on `feature/figcraft-floating-chat`; original `todo-deb9c6cd` branch preserved after upstream main was rewritten. Fetch ran through approved host request `085cc6633e6c`.
- Pre-flight: tasks 1 and 2 share the session-manager/Lua launch boundary. Ruling: Lua launches Kitty with the existing `hl.exec_cmd` API and per-exec rules, rather than shell spawning then racing to place the new window. Bash validates/resolves the session; Lua selects FigCraft before any launch. This avoids an initial tiled window and duplicate launch races; cost if wrong: the compositor launch boundary needs adjusting.
- Ruling: accept an optional session ID on stop too; without one close only a unique chat, refusing ambiguity. Stop must still work after its agent exits or disappears from the snapshot. Cost if wrong: an extra argument for ambiguous cleanup.
- The executing-plans workspace helper is absent (reported with `claude-todo blocked`); this existing plan records progress instead. No helper was recreated.
- UI context: terminal/window management only; no web frontend or dependency installation is needed.
- Task 1: implemented; script regression tests observed RED (4 failures, 2 passes), then GREEN: `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 205 filtered out; finished in 0.18s`.
- Task 2: implemented; Lua behavior tests observed RED (12 failures, 2 passes), then GREEN (14 passed). Two slow-launch regressions were then observed RED and fixed: duplicate mapped clients close; cancelled slow launches stay cancelled. Final targeted run: 16 passed, 0 failed.
- Test-harness correction: the fake compositor assigned a string workspace ID after movement; real Hyprland exposes numeric IDs. Corrected the fake rather than weakening production validation.
- `bash -n config/bin/.local/bin/claude-sessions` and `git diff --check`: exit 0. Full project `make test` launched with inherited agent/state markers cleared; result pending.
- Host-executed changes: session-manager shell command, Hyprland floating-chat module and window rule. Agent guidance and its existing eval prompts now request the float rather than side-by-side movement. No dependency manifests, permissions, build scripts, or sandbox rules changed.
- First full-suite attempt was interrupted by the tool's 120-second timeout: `make: *** [Makefile:132: test] Terminated`. Rerun with no tool timeout completed successfully. No failing test was disabled.
- Independent whole-change review: two Important findings, no Critical or Minor. Fix pass: stale/reused tmux pane ownership and stop-after-first-map with another delayed client. Both reproduced RED, then GREEN. Added `osd owns-pane` using the existing process inventory and trusted outside-sandbox launcher environment, matching the conversation, pane and tmux server PID before launch. This adds `apps/desktop/src/{registry.rs,tests.rs,bin/osd.rs}` to the file scope; no new dependency or permission.
- Ruling: also provide `start <session-id> --figcraft-window <address>` when several designs are open. The approval terminal itself steals focus, so a focus-only selector cannot reliably identify the intended design through ask-to-run. Explicit-address script/Lua regressions observed RED→GREEN. Cost if wrong: callers must supply a fresh window address when a design closes/reopens.
- Reviewer limit: actual compositor mapping/focus/close behavior still needs a live smoke test after host-reviewed application. Existing `hl.exec_cmd` usage matches the surrounding scratchpad implementation. No finding was deferred.
- Final verification: `env -u CLAUDE_SESSIONS_STATE_HOME -u OPENCODE -u CLAUDECODE make test` exited 0: **1252 passed, 0 failed**, across 61 target results. Log: `/tmp/opencode/figcraft-chat-final-suite.log`. Three pre-existing compiler warnings remain: `unused_mut` in `cockpit/src/tests.rs`, two `unused_must_use` in `desktop/src/tests.rs` outside the changed tests. No test disabled.
- Final: fixed stale-pane ownership — script rejection and process-identity/pane/server tests RED→GREEN, including current `--session-id` versus the source `--resume` conversation. Final: fixed late client after stop of a mapped chat — `stopping_a_mapped_chat_also_cancels_any_outstanding_slow_client` RED→GREEN. Whole project suite green after both fixes.
- Shell syntax, staged whitespace, and updated eval JSON checks: exit 0. Final diff before application: 13 files, covering implementation, regression tests, agent guidance and this plan; no unrelated changes staged. Host application and live-compositor smoke test remain pending.
