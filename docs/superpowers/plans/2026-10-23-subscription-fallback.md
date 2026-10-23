# Subscription Fallback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Continue exhausted Claude/Codex subscription tasks in native CLIs and OpenCode on the other subscription, then safely return to the original model after confirmed recovery.

**Architecture:** Add an explicit host-owned routing state machine beside existing dispatch logic. Native CLI replacements reuse sandbox launchers, bounded conversation export and ownership stores; OpenCode switches models within its existing session through its private-server plugin. Both paths share verified quota decisions and persisted return intent.

**Tech Stack:** Existing Rust workspace, serde_json, rusqlite, Bash/fish launchers and fake-command integration tests. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-23-subscription-fallback-design.md`

**Status (2026-10-24):** Tasks 1–6 and final review fixes verified by focused
regressions and `make test`; Task 7 reviewed application is pending.
Fresh review identified ten release-blocking findings, addressed with account,
interruption, admission, message-delivery, recovery and public-entry regressions.
Dedicated successful-transition audit/waiting-reason polish is deferred.
Exact commands, failures, host probes and rulings are in the execution ledger.
Theo confirmed an existing OpenCode
Anthropic `Claude OAuth` connection through `/connect`; keep OpenCode fallback in
the same conversation using existing connected accounts. Verify actual active
connection/billing/quota rather than treating the label as sufficient evidence.
Do not build authentication, copy tokens or bypass provider rejections.

## Global Constraints

- No Gemini integration or automatic API spending.
- OpenCode retains its session ID, conversation, agent, sandbox, task and team role; only its selected provider/model/variant changes.
- No two agents may execute the task in the same checkout during a handoff.
- Freshness ceiling: 600 seconds; raw quota values, matching execution account and applicable model limits only.
- Bound visible history to 100 messages/256 KiB; retain the latest actual user request separately.
- Never interrupt tools, background jobs, subagents, approvals, user questions or unsubmitted text.
- Preserve original conversation, exact model and effort; unknown identity pauses instead of defaulting.
- No dependency-manifest edits. Call out host-executed launcher changes before deployment.
- Git commits are authorized on feature branches; pushing is not. Applying still requires host review through `ask-to-run`.

## Review Focus

- Separate Codex quota-login and execution accounts: mismatch must never authorize fallback.
- Rounded percentages and unrelated Claude model limits: neither may trigger a false switch.
- Crash after process launch but before receipt: reconcile instead of launching another worker.
- Team runtime/epoch changes and uncertain messages: no inherited approvals or duplicate delivery.
- User cancellation, completion or manual billing/model change: automation must not resurrect or override the task.

---

### Task 1: Verified quota and runtime evidence

**Files:** Modify `apps/cockpit/src/usage.rs`, `apps/cockpit/src/codex.rs`, `apps/cockpit/src/paths.rs`, `config/waybar/.config/waybar/scripts/codex-quota.py`, `config/waybar/.config/waybar/scripts/codex-usage.sh`; test `apps/cockpit/tests/codex_quota_client.rs`, `apps/cockpit/tests/waybar_claude_scripts.rs` and usage unit tests. Add sanitized fixtures under `apps/cockpit/tests/fixtures/subscription-fallback/`.

**Interfaces:** Produce `usage::SubscriptionCapacity { Available, Exhausted, Unknown }` and `usage::subscription_capacity(agent: &str, model: &str, now: i64) -> SubscriptionCapacity`. New routing evidence must keep raw percentages, account match, freshness and model scope; do not derive decisions from `QuotaRow` display strings.

- [ ] Capture sanitized installed-CLI contracts for quota-stop events, Codex transcript records, runtime model/effort and account identity. Inspect installed help/source before choosing launch arguments. Store shape-only fixtures with CLI versions; never copy actual credentials or conversation content. If a required contract is unavailable, report that blocker before implementing a guessed protocol.
- [ ] Write parameterized failing tests: raw `99.6` remains available; `100.0` exhausts; unrelated Opus exhaustion does not block Sonnet; stale/failed/past-reset/invalid evidence is unknown; execution/quota account mismatch is unknown; fresh recovery enables return.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit usage::tests` and the two affected integration targets; confirm the new assertions fail for the intended missing behavior.
- [ ] Implement capacity evaluation and minimal private account-binding evidence using verified contracts. Do not expose account data in quota output, logs or audits; preserve existing display/cache behavior.
- [ ] Rerun those tests and commit the passing evidence/parser change on the feature branch.

### Task 2: Bounded Claude/Codex handoff context

**Files:** Modify `apps/cockpit/src/remote/conversation.rs`, `apps/cockpit/src/remote.rs`, `apps/cockpit/src/paths.rs`; create `apps/cockpit/src/subscription_fallback.rs` and export it from `apps/cockpit/src/lib.rs`; test `apps/cockpit/tests/claude_remote.rs` and new `apps/cockpit/tests/subscription_fallback.rs`.

**Interfaces:** Extend existing `remote::conversation(id: &str) -> Result<Value, String>` to Codex using verified native-thread mapping. Produce `subscription_fallback::handoff_context(session: &Session) -> Result<String, String>` for the launcher; context is data, not an approval.

- [ ] Write failing public-entry tests for Codex visible user/assistant messages, reasoning/tool exclusion, 100-message/256-KiB bounds, partial streaming records, symlinks, malformed records, mismatched native IDs and latest-user-request retention beyond the bounded tail.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test subscription_fallback` and `--test claude_remote`; confirm the new tests fail.
- [ ] Implement bounded Codex transcript lookup with the same no-follow/regular-file checks as Claude. Reuse the existing visible-message representation and limits; keep reasoning and tool payloads out.
- [ ] Build context containing task identity, original request, historical messages, truncation notice and instruction to inspect current files before continuing. Pass large context through a private bounded file/verified CLI input mechanism, not an oversized command argument or shared log.
- [ ] Rerun the affected targets and commit once passing.

### Task 3: Crash-safe routing decisions and persistence

**Files:** Extend `apps/cockpit/src/subscription_fallback.rs`, `apps/cockpit/src/paths.rs`; test `apps/cockpit/tests/subscription_fallback.rs`.

**Interfaces:** Produce serializable `Route` records retaining original runtime/model/effort, current owner, checkout, task and transition generation. Produce `RoutingStore::load(path: &Path) -> Result<RoutingStore, String>` and `RoutingStore::reconcile(sessions: &[Session], now: i64) -> Result<Vec<Transition>, String>`; `Transition` distinguishes fallback, return and waiting. Keep effects in Task 4, not in the pure decision function.

- [ ] Write failing tests for both routing directions, no usable subscription, confirmed post-reset recovery, completed task, manual cancellation, working/approval/background boundaries and unsupported/API-billed/user-terminal exclusions.
- [ ] Add crash/restart tests for persisted intent, uncertain start, duplicate dispatchers and old runtime reports. Assert one transition generation and at most one executable task owner.
- [ ] Run the new integration target and confirm the intended failures.
- [ ] Implement the smallest explicit state machine with host-owned per-lineage locking, bounded validated records and atomic writes. Corrupt state fails loudly and remains intact; uncertain launch waits for reconciliation rather than retrying.
- [ ] Rerun the integration target and commit once passing.

### Task 4: Native sandbox launch, resume and periodic dispatch

**Files:** Modify `config/bin/.local/bin/cockpit`, `config/bin/.local/bin/cockpit-agents/claude.sh`, `config/bin/.local/bin/cockpit-agents/codex.sh`, `apps/cockpit/src/app.rs`, `apps/cockpit/src/bin/cockpit-codex-hook.rs`, `apps/cockpit/src/codex.rs`, `apps/osd/src/registry.rs`; extend `apps/cockpit/tests/subscription_fallback.rs`, `apps/cockpit/tests/native_codex.rs`, `apps/cockpit/tests/cockpit_script.rs` and `apps/osd/src/tests.rs`.

**Interfaces:** Add host manager verb `cockpit subscription-route <source-session-id> <transition-generation>`. It loads trusted transition state rather than accepting arbitrary history/agent/path arguments. New `App::dispatch_subscription_routes(&mut self)` runs before queue/prompt/sleep dispatch. Use Task 1's verified CLI contracts for detached launch and original-thread resume with continuation context.

- [ ] Write fake-launcher tests covering source retirement before replacement task execution, exact original model/effort on return, same checkout, context delivery without terminal typing, fresh identity receipt and API-key-free subscription launch.
- [ ] Test quota exhaustion while a tool is active, a prompt is typed, an approval is open and a provider is stopped by quota. Assert only verified safe boundaries allow transfer; quota text alone cannot masquerade as readiness.
- [ ] Test launch failure, lost receipt, sidebar restart, missing executable/auth and double dispatch. Assert no second worker, no cleared history and an actionable paused reason.
- [ ] Run the four affected integration/unit targets and confirm new failures.
- [ ] Implement the manager verb, verified lifecycle/model evidence and periodic dispatch. Recheck identity, safety and generation immediately before exiting/launching; use the existing greywall launchers unchanged in their isolation semantics. Exclude routing-held sessions from queue, restart, prompt and sleep dispatch.
- [ ] Rerun affected tests and commit once passing. Explicitly flag these launcher/hook edits as host-executed changes for review.

### Task 5: Transfer task, prompt and team ownership

**Files:** Modify `apps/cockpit/src/continuity/tasks.rs`, `apps/cockpit/src/todo.rs`, `apps/cockpit/src/prompts.rs`, `apps/cockpit/src/teams.rs`, `apps/cockpit/src/teams/store.rs`, `apps/osd/src/teams.rs`, `apps/cockpit/src/subscription_fallback.rs`; extend `apps/cockpit/tests/subscription_fallback.rs` and the existing team store/daemon tests in their owning modules.

**Interfaces:** Add explicit host-only replacement operation keyed by transition generation, old authenticated runtime and newly verified runtime. Preserve task/role, rotate team epoch and update active ownership exactly once. A sandbox report alone cannot request or authorize replacement.

- [ ] Write failing integration tests for active TODO/issue assignment, continuity trail and queued messages following the replacement, including a new user message arriving during a handoff.
- [ ] Test team role retention, old runtime rejection, cross-project/cross-checkout rejection, uncertain-message non-replay and crash recovery between stores. Assert no inherited approvals and no duplicate active task assignment.
- [ ] Run the new routing integration target plus affected existing team/continuity tests; confirm intended failures.
- [ ] Implement idempotent ownership transfer with persisted generation/receipts. Gate task execution until all required bindings succeed; an incomplete transfer remains paused and recoverable. Do not fabricate a fresh team approval or silently unlink the member.
- [ ] Rerun affected tests and commit once passing.

### Task 6: Same-session OpenCode subscription fallback

**Files:** Modify `config/opencode/.config/opencode/plugins/cockpit.ts`, `apps/cockpit/src/subscription_fallback.rs`, `apps/osd/src/registry.rs`, `apps/osd/src/teams.rs` (reuse its authenticated report/peer-verification patterns, not team membership for authorization); extend `apps/cockpit/tests/opencode_plugin.rs`, `apps/cockpit/tests/opencode_team.rs`, `apps/cockpit/tests/subscription_fallback.rs` and `apps/osd/src/tests.rs`. Modify `config/bin/.local/bin/greywall-agents/opencode` only if required to prevent an injected API key from overriding a verified subscription selection.

**Interfaces:** Use documented `ctx.session.get`, `ctx.model.list`, `ctx.session.switchModel({sessionID, model})`, integration connection metadata and the session event stream. Produce a host-verified route exchange bound to the reporting runtime/session/generation using the existing authenticated report transport. Do not expose a shared unauthenticated server or write the OpenCode database. Inspect V2 OpenAPI/installed types for exact event/continuation/variant shapes before implementing them.

- [ ] Extend the existing Node plugin harness with fake session, model, connection and host-route responses. Write failing tests for subscription exhaustion → eligible alternate model → exact original provider/model/variant on fresh recovery, keeping the same session/agent/task and without global default changes.
- [ ] Test disabled/tool-incapable models, wrong-account quota, API-key and unverified OAuth connections, missing subscriptions, stale recovery, transient 429s, active tools/subagents/forms/permissions and manual model/variant/connection changes. Assert no API spend, false exhaustion or interruption of active work.
- [ ] Test exactly-once continuation, consumed inbox non-replay, crashes after switch before receipt, duplicate events, plugin cleanup and child-session isolation. Test unchanged team membership/runtime and pending message delivery.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test opencode_plugin --test opencode_team --test subscription_fallback`; confirm intended new failures.
- [ ] Implement the plugin's session-local application of host-verified route decisions. Discover candidate IDs from enabled tool-capable models, filter by proven subscription connection/account, preserve prior per-subscription selection where known and otherwise keep catalog order. Persist original selection and generation before switching; verify the selected model before acknowledging. Continue only the unfinished request once, using the documented native inbox semantics.
- [ ] Reconcile model selection on plugin reload and cancel return on manual selection. Unavailable or uncertain contracts pause with a reason, never fall back to terminal typing, global defaults or a paid API key.
- [ ] Rerun the three integration targets and affected OSD tests; commit once passing. Flag any host-executed launcher edits before application.

### Task 7: End-to-end verification and reviewed application

**Files:** Extend `apps/cockpit/tests/subscription_fallback.rs`; update this plan's checkboxes with actual outcomes.

**Interfaces:** Existing logs/audit events and message area report fallback, waiting, return and failure without exposing history/account secrets. No new UI layout or settings screen.

- [x] Add complete public-entry fake-runtime journeys: Claude → Codex → original Claude model, Codex → Claude → original Codex model, and OpenCode subscription-model fallback → exact original provider/model/variant in the same conversation. Include user cancellation, both exhausted, recovery, team member and restart during uncertain launch.
- [x] Run focused tests, then the full suite through `make test` (release Cargo workspace tests). Final local result: Python 35 / OK, cockpit library 596 passed, daemon library 206 passed, all integration targets passed. Exact output is in the execution ledger; no test disabled.
- [x] Run `git diff --check`, review the whole branch and show the diff stat. Commit completed requested changes with `Todo: 1797cdd6`; do not push.
- [ ] Export with `git format-patch origin/main -o .outgoing`; use `ask-to-run --cwd <clone-root>` for reviewed `cockpit apply <clone-root>`, explaining host launcher/hook changes and the rebuild/reload it performs. Never bypass approval.
- [ ] Run host `make -C ~/agent-cockpit test` through `ask-to-run` and report verbatim results. Check `cockpit-todo list`; mark `1797cdd6` done only after the requested behavior is implemented and verified.
