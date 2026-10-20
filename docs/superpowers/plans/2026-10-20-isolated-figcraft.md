# Isolated FigCraft Instances Implementation Plan

> **For agentic workers:** Execute inline using the executing-plans skill. Write regression tests first; no subagents or git writes are authorized by this plan.

**Goal:** Allow agents in different project checkouts to use independent FigCraft windows without closing existing windows or widening cross-project access.

**Architecture:** Reserve a distinct host control port permanently for each canonical checkout in host-only state. Start each new FigCraft with checkout-specific configuration/recovery storage and automation roots. Launchers forward only that checkout's verified instance port and configure its MCP connection explicitly.

**Tech Stack:** Existing shell launchers and Rust cockpit integration tests; no new dependencies.

**Spec:** Option A selected in this conversation: isolated instances per checkout, including parallel clones; preserve existing windows, project confinement, and stale-forward protections. Ask before any operation requiring ask-to-run.

## Global Constraints

- Never stop or replace an existing FigCraft window.
- Canonical checkout paths, not project basenames, identify instances.
- No port may be reassigned to another checkout, even after its window closes.
- Port reservations and instance configuration must be inaccessible to sandbox writes; create their parent before sandbox mounts are constructed.
- Keep the legacy foreign-sandbox-forward check before starting an app on any port.
- Never treat a TCP listener alone as proof of a correctly confined FigCraft instance.
- Each new instance gets separate XDG_CONFIG_HOME: upstream startup restores Recovery before installing automation roots.
- Preserve unrelated MCP servers and agent settings; do not migrate unrelated OpenCode configuration.
- No dependency-manifest changes, commits, host application, or desktop checks without separate authorization.

## Review Focus

1. A foreign FigCraft appearing first in pgrep must not hide the correct instance.
2. Concurrent starts must not assign the same port to different checkouts or create duplicate windows for one checkout.
3. A closed app's port retained by an old sandbox must never be reassigned to another checkout.
4. Root prefixes, spaces in roots, repeated automation-root flags, and parallel clone paths must not broaden access through string matching.
5. Both agent launchers must connect MCP to the exact port they forward, not silently fall back to 7979 when disconnected.

## Task 1: Allocate and start isolated instances

**Files:** `config/bin/.local/bin/craft-open`, a shared FigCraft helper under `config/bin/.local/bin/greywall-agents/`, `config/greywall/.config/greywall/greywall.json`, `apps/cockpit/tests/craft_open.rs`.

- [ ] Add failing command-level tests: another checkout's window stays alive while this checkout starts; reuse finds the matching window anywhere in the process list; two checkouts receive different ports and configuration directories.
- [ ] Add failing tests for stable reservations after app exit, serialized simultaneous opens, unavailable listeners, foreign stale forwards, exact/repeated automation roots, and distinct parallel clones.
- [ ] Run `cargo test -p cockpit --test craft_open` from `apps`; confirm failures exercise existing single-instance behavior rather than fixture errors.
- [ ] Implement shared discovery using argument boundaries rather than flattened command-line substring matches. Report only instances confined to exactly the requested canonical checkout.
- [ ] Serialize reservation/startup with a host-only lock. Persist port ownership under a dedicated host-only FigCraft directory; never recycle reservations across checkouts. Skip ports already listening, used by another app, or forwarded by foreign sandboxes. Retain reservations across restarts and fail explicitly on conflicts.
- [ ] Start with separate XDG_CONFIG_HOME and exact read/write roots. Reuse a matching confined instance without closing any other process. Preserve restart-when-idle and queued continuation behavior.
- [ ] Hide reservation/configuration storage through the sandbox policy, creating its parent before launch. Re-run tests.

## Task 2: Wire both agents and connection detection

**Files:** `config/bin/.local/bin/greywall-agents/launcher-common.sh`, `config/bin/.local/bin/greywall-agents/claude`, `config/opencode/.config/opencode/opencode.jsonc`, `apps/cockpit/tests/greywall_launcher.rs`, `apps/cockpit/tests/opencode_plugin.rs`.

- [ ] Add failing launcher tests for two simultaneous checkout instances: each agent gets only its own port; no matching instance grants no FigCraft forward. Cover a matching second process, foreign root prefixes, repeated roots, and disconnected launches.
- [ ] Share the discovery implementation with craft-open; only host-side launchers choose and forward ports.
- [ ] Export the selected control port, clearing inherited values when disconnected. Supply Claude's figcraft-app entry through its supported --mcp-config option, without --strict-mcp-config. Configure OpenCode's existing figcraft-app command to consume the selected port explicitly through a shell; missing ports must fail closed. Preserve other servers.
- [ ] Verify the resulting MCP command executes figcraft-cli with exactly the forwarded port using a logging executable fixture, not source-text assertions.
- [ ] Run `cargo test -p cockpit --test greywall_launcher --test opencode_plugin` from `apps`.

## Task 3: Report multiple connections correctly and document the change

**Files:** `apps/cockpit/src/sessions.rs`, `apps/cockpit/src/app.rs`, `apps/cockpit/src/tests.rs`, `README.md`, `config/claude/.claude/CLAUDE.md`.

- [ ] Add a failing cockpit test with two connected checkout instances and a third unrelated session: show badges on both served checkouts only.
- [ ] Replace single-root discovery with all running roots per craft app and update the badge membership check. Keep the existing session-side TCP connection check.
- [ ] Update the existing connection instructions to describe per-checkout instances and remaining conflict failures, not a global singleton. Identify changed host-run scripts explicitly in the handoff.
- [ ] Run `cargo test` from `apps`, plus shell syntax checks and `git diff --check`. Report all failures verbatim; show the diff stat.
- [ ] Ask Theo before any ask-to-run request for a live two-window verification or applying/reloading dotfiles. The sandbox cannot perform desktop verification: figcraft --help attempted window startup and returned Could not find wayland compositor; upstream main/control_server source confirms independent localhost listeners but is not a live installed-version check.
- [ ] Mark todo f1f879e1 done only after the implementation and required verification are complete; distinguish tested checkout changes from unapplied host changes.

## Implementation status

- Tasks 1 and 2 implemented and regression-tested. Task 3's multi-instance badge handling and documentation are implemented.
- Self-review added tests/fixes for compact `-fPORT` stale forwards, explicit write denial of reservation storage, and rejection of generic port grants targeting a foreign checkout's reserved port. Unmanaged legacy windows are not adopted because their shared recovery storage cannot establish checkout isolation; they are left open.
- Final full suite: **961 passed; 0 failed; 51 test targets**, using `env -u CLAUDE_SESSIONS_STATE_HOME -u OPENCODE -u CLAUDECODE cargo test --release --no-fail-fast` after Theo authorized `cargo build --release --workspace` in this checkout. Initial debug runs lacked wrapper binaries; inherited launcher environment also caused unrelated test failures. No tests were disabled or unrelated source changed.
- Shell syntax checks and `git diff --check` passed. Existing `unused_mut` warning at `apps/cockpit/src/tests.rs:185` remains unchanged.
- Host application, live multi-window verification, git writes and todo completion remain pending authorization/verification. No ask-to-run request has been made.
