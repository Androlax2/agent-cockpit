# Team Member Sleep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let approved teammates sleep and resume their membership through cockpit-managed wake, including waking for queued team messages.

**Architecture:** Extend the existing protected team database with a one-use sleep permit. The host prepares it before exit; the daemon consumes it only for a verified replacement launched with that permit. Reuse the host socket, existing detached restore paths, wake lock and native message delivery.

**Tech Stack:** Rust, SQLite through installed rusqlite, existing bash launchers; no new dependencies or services.

**Spec:** `docs/superpowers/specs/2026-10-23-team-member-sleep-design.md`

## Global Constraints

- Automatic reattachment applies only to cockpit-managed sleep/wake. `r` on an asleep card remains a wake; restarting a live session or manually resuming history does not reattach.
- Same session, agent, canonical project, original checkout, task, role and membership epoch; old process dead, new process verified.
- Permit lifetime: seven days, matching sleep-note expiry. Revocation, replacement, explicit exit and note deletion cancel pending permission.
- Sandbox `AgentAction` gains no host lifecycle authority. Read permit provenance outside the sandbox, never from report JSON.
- Preparation transaction blocks new sends/claims; incoming messages may still queue. Pending or uncertain deliveries block preparation.
- No receipt reassignment or automatic retry of uncertain deliveries. Preserve queued messages across sleep, wake and daemon restart.
- Unknown membership or failed preparation means stay awake. Preserve all existing non-team safety checks and Codex exclusion.
- Keep tokens out of snapshots, ordinary logs and agent response JSON; inheritance through the trusted launcher is acceptable.
- No dependency-manifest changes. No host configuration, greywall pin or unrelated cleanup changes.

## Review Focus

1. A message arriving between preparation and exit must not be submitted to a disappearing process (Task 1).
2. Daemon restart after native launch but before reattachment must not launch another replacement or discard queued messages (Tasks 1–2).
3. Revoked membership or deleted sleep metadata must not be reconstructed from a stale token (Tasks 1–3).
4. A permit in sandbox-controlled JSON/environment must not acquire trusted provenance (Task 2).
5. Two simultaneous wake requests must lead to one replacement and one delivery, not just one visible card (Tasks 3–4).

## File map

- `apps/cockpit/src/teams/store.rs`: schema migration, existing message entry points and recovery.
- New `apps/cockpit/src/teams/store/sleep.rs`: permit transactions, within the existing store module (not a service layer).
- `apps/cockpit/src/teams.rs`, `apps/osd/src/teams.rs`: host lifecycle protocol and daemon orchestration.
- `apps/osd/src/registry.rs`: trusted resume-token extraction and periodic wake processing.
- `apps/cockpit/src/bin/cockpit-team.rs`: small host-only CLI branches using `client::host_request`; existing socket peer check is mandatory.
- `config/bin/.local/bin/cockpit`, `cockpit-agents/claude.sh`, `cockpit-agents/opencode.sh`: sleep/wake handoff and launcher propagation.
- `apps/cockpit/src/sleep.rs`, `apps/cockpit/src/app.rs`: remove known-team exclusion only after lifecycle works; preserve unknown-membership checks.
- Existing team/store/protocol/launcher/script/column tests; README and idle-sleep spec updated last.

---

### Task 1: Protected sleep permits and message continuity

**Files:** create `apps/cockpit/src/teams/store/sleep.rs`; modify `apps/cockpit/src/teams/store.rs`; test `apps/cockpit/tests/team_store.rs`.

**Interfaces:** expose `store::sleep` from store.rs. Define `SleepPermit` with token, session, agent, project, checkout, old runtime, task key, role, epoch, expiry, lifecycle state and optional replacement runtime. Do not serialize the token into normal snapshots.

- `prepare(connection: &mut Connection, member: &VerifiedSession, checkout: &str, now_ms: i64) -> Result<Option<String>, TeamError>`: `None` only for a known non-member; a mismatched existing membership is an error.
- `cancel(connection: &mut Connection, session: &str, token: &str) -> Result<(), TeamError>`: idempotent cancellation, never revokes unrelated membership.
- `bind(connection: &mut Connection, token: &str, replacement: &VerifiedSession, checkout: &str, now_ms: i64) -> Result<bool, TeamError>`: daemon has already checked the note and old-process death; repeat all database identity/epoch/expiry checks transactionally.
- `pending_wakes(connection: &Connection, now_ms: i64) -> Result<Vec<SleepPermit>, TeamError>`: only valid prepared permits with deliverable queued traffic.
- `begin_wake(connection: &mut Connection, session: &str, token: &str, now_ms: i64) -> Result<(), TeamError>` and `status(connection: &Connection, session: &str, token: &str) -> Result<Value, TeamError>` for durable launch/acknowledgement state. Status responses never echo the token.

- [ ] **Write failing tests:** `sleep_prepare_pauses_sends_and_claims_but_accepts_incoming_messages`, `sleep_prepare_refuses_pending_or_uncertain_delivery`, `sleep_bind_preserves_role_epoch_and_queued_messages`, `sleep_bind_rejects_wrong_identity_expiry_replay_and_revocation` (parameterized), `sleep_bind_is_idempotent_for_the_winning_runtime_only`, `sleep_cancel_restores_normal_delivery`, `sleep_recovery_does_not_resend_uncertain_attempts`.
  Assert prepare and claim/send serialization using real SQLite transactions; after bind check member context, queued `recipient_runtime`, unchanged epoch and at-most-once receipt ownership.
- [ ] **RED:** `cargo test --manifest-path apps/Cargo.toml -p cockpit --test team_store sleep_` fails on missing permit support.
- [ ] **Implement:** migrate the existing schema non-destructively to add one purpose-specific permit table. States: `prepared`, `launch_requested`, `waking`, `bound`, `failed`, `cancelled`. Store random tokens using the existing identifier generator. Preparation checks membership and relevant `queued`, `ready`, `submitting`, `uncertain` traffic in one transaction. Hook send/claim guards into their explicit entry points; also guard structured handoff/recruit/assess operations that depend on the paused sender. Bind updates the member runtime, queued recipient runtimes and idle marker atomically, then records the winning runtime. Keep received/uncertain records and attempt ownership unchanged. Extend recovery so authorized sleep continuations are considered before changed-runtime cancellation, without exempting unauthorized restarts.
- [ ] **GREEN:** targeted command, then full `--test team_store`; all pass, including existing approval/revocation/receipt tests.
- [ ] **Commit:** `Authorize one-use team continuations for cockpit sleep`.

### Task 2: Verify host authorization and wake sleeping recipients

**Files:** `apps/cockpit/src/teams.rs`, `apps/cockpit/src/bin/cockpit-team.rs`, `apps/osd/src/teams.rs`, `apps/osd/src/registry.rs`; tests `apps/cockpit/tests/team_protocol.rs`, `apps/cockpit/tests/team_client.rs`, `apps/osd/src/tests.rs` and existing daemon team tests discovered there.

**Consumes:** Task 1 store functions. **Produces:** host-only `HostRequest` variants `PrepareSleep { session_id }`, `CancelSleep { session_id, token }`, `BeginWake { session_id, token }`, `SleepStatus { session_id, token }`; CLI verbs `sleep-prepare <id>`, `sleep-cancel <id>`, `sleep-begin-wake <id>`, `sleep-status <id>`. Supply tokens over stdin JSON, not command-line arguments. CLI checks its id and payload size, then uses the existing private host socket. Never add these to `AgentAction`.

Define `trusted_resume_token(table: &dyn Processes, sender: &str) -> Option<String>` beside `trusted_spawn_token`; source `COCKPIT_TEAM_RESUME` exclusively from `first_process_outside`. Add optional trusted resume metadata to `Report` and `Entry`, populated only by authenticated report handling, not parsed JSON.

- [ ] **Write failing tests:** `sandbox_cannot_prepare_or_cancel_team_sleep`, `resume_token_in_report_or_inner_environment_is_not_trusted`, `verified_sleep_replacement_rejoins_before_startup_recovery`, `resume_refuses_live_old_runtime_or_different_checkout`, `deleted_or_replaced_sleep_record_blocks_reattachment`, `queued_team_message_launches_one_sleep_wake`, `wake_failure_keeps_queue_and_reports_error`, `daemon_restart_does_not_repeat_a_requested_wake`.
  Drive real host/agent request decoding and fake process tables; verify pending/consumed permit state, recorded detached commands and message state rather than mocked helper returns.
- [ ] **RED:** focused `cargo test --manifest-path apps/Cargo.toml -p cockpit --test team_protocol` and `cargo test --manifest-path apps/Cargo.toml -p osd --bin osd sleep` (confirm the existing osd test target before invoking).
- [ ] **Implement:** host handlers derive canonical identity and checkout from verified registry data. Read matching sleep notes safely: regular private files, no symlink following, validated id, matching token/session/expiry, and not explicitly ended. Never trust a claimed path. Rebind verified replacement entries before `store::recover` in `live_sessions`; missing notes invalidate pending permits, while consumed permits allow acknowledgement only for the recorded winner. A successful bind allows the host wake waiter to remove its note. Periodic daemon work reserves `launch_requested` before `start_detached(["cockpit", "wake", id])`; the script's begin-wake changes it to `waking`. A launch already requested at crash is not repeated. Use the existing 90-second restart timeout to mark missing acknowledgement failed and expose a token-free error in team snapshot. Add this work to the existing daemon/host snapshot loop, not the column's synchronous input path.
- [ ] **GREEN:** protocol/client tests and complete osd tests pass, including peer authentication and existing spawn binding tests.
- [ ] **Commit:** `Verify sleeping teammates and wake them for queued collaboration`.

### Task 3: Carry the handoff through real sleep/wake launchers

**Files:** `config/bin/.local/bin/cockpit`, `cockpit-agents/claude.sh`, `cockpit-agents/opencode.sh`, `apps/cockpit/src/sleep.rs`; tests `apps/cockpit/tests/cockpit_script.rs`, `apps/cockpit/tests/agents.rs`, `apps/cockpit/tests/greywall_launcher.rs`.

**Consumes:** Task 2 CLI and host responses: prepare returns JSON `{ "token": null }` for a checked non-member or `{ "token": "..." }` for a team permit; status returns lifecycle state and winning runtime without credentials. **Produces:** optional `team_resume` field in the existing private asleep-note JSON; host launch environment `COCKPIT_TEAM_RESUME` on detached Claude/opencode restores only.

- [ ] **Write failing tests:** `team_sleep_prepares_before_exit_and_records_private_permit`, `unknown_team_membership_prevents_sleep`, `failed_exit_cancels_team_permit`, `claude_and_opencode_wake_launch_with_host_resume_permit`, `team_sleep_note_survives_launch_until_binding_acknowledged`, `ordinary_restart_does_not_pass_team_resume_permit`, `r_on_asleep_teammate_uses_authorized_wake`, `concurrent_wakes_launch_once_and_keep_queue`, `team_exit_expiry_and_invalid_note_cleanup_revoke_pending_resume`.
  Use the real manager entry points with fake team CLI/daemon replies; verify order in shim logs, mode 0600, token absent from log/snapshot output, explicit status failures, and no note deletion before acknowledgement.
- [ ] **RED:** `cargo test --manifest-path apps/Cargo.toml -p cockpit --test cockpit_script team_` fails before adding lifecycle calls.
- [ ] **Implement:** prepare through the host CLI for all sleeps after pane checks but before writing the note/exiting. A failed preparation leaves the process running. Cancel any created permit on a last-moment refusal or genuinely failed exit; do not discard the note after an exit that actually finished. Wake uses the existing note lock, requests begin-wake, and sets the launch-only resume variable. Pass it explicitly to both detached launch commands (do not leak it into unrelated starts/restarts). For team wakes retain the note through startup and check host status asynchronously: remove it only after binding acknowledgement; timeout keeps the note and reports failure without silently launching a second process. Keep the ordinary non-team wake behavior unchanged. Explicit exit deletes the note and existing queued prompts, marks ended, and attempts host cancellation; absent daemon cannot recreate a deleted note. Expiry and stale-note cleanup preserve the same revocation ordering.
- [ ] **GREEN:** full `cockpit_script`, `agents`, `greywall_launcher` tests pass. Inspect both actual restore argv/env paths and the statusline/Remote Control waiter interaction.
- [ ] **Commit:** `Preserve team identity across managed sleep and detached wake`.

### Task 4: Enable teammate sleep and verify the complete behavior

**Files:** `apps/cockpit/src/sleep.rs`, `apps/cockpit/src/app.rs`, `apps/cockpit/src/tests.rs`, `README.md`, idle-sleep design document; integration tests in existing team/script test files as needed.

**Consumes:** completed Tasks 1–3; do not enable this early.

- [ ] **Write failing tests:** replace the team rejection case in `block_reasons` with `known_team_member_can_sleep`; replace manual `s` refusal test with `s_can_sleep_a_known_teammate`. Keep/add `unknown_membership_blocks_manual_and_automatic_sleep`. Add end-to-end lifecycle coverage using real store/daemon request handling: prepare, exit, incoming message, one detached wake, verified rebind, claim and receipt; assert same task/role/epoch and exactly one received message. Add revoked-before-wake and manual-resume-without-permit negative cases.
- [ ] **RED:** `cargo test --manifest-path apps/Cargo.toml -p cockpit --lib team` exposes existing teammate exclusions.
- [ ] **Implement:** remove `SleepBlock::Team` and the corresponding unconditional manual-sleep check, pruning unused context fields/callers/fixtures. Retain `team_known` gating. Keep the existing assigned-todo, draft/background-work, unsupported-agent and ordinary idle checks. README says teammates may sleep, incoming team messages wake them, and a live-session restart still requires fresh approval. Do not redesign cards or phone controls.
- [ ] **GREEN:** `make test` from the repository root; report every failure verbatim, including unrelated failures. Confirm the old unverified approval-terminal test still has its bounded wait. Review the final diff for any path that grants membership without host-issued provenance or replays uncertain delivery.
- [ ] **Commit:** `Enable idle and manual sleep for approved teammates`, trailer `Todo: 491ae1b6` only once the feature is verified.

## Integration and handoff

- Feature branch/workspace is already isolated; inspect status before edits and use current applied main as the integration base. Do not overwrite other sessions' host edits.
- Use the previously selected subagent-driven method unless Theo changes it: one implementer and one scoped reviewer per task, then a whole-branch security/lifecycle review. Keep shared interfaces consistent between tasks.
- Apply via the project's existing export/`cockpit apply` workflow and host approval prompt; no extra chat permission before `ask-to-run`. Changes to launch scripts run with host rights and must be called out.
- Run `make -C ~/agent-cockpit test` after apply. If host reload is blocked by unrelated pin/profile drift, report it rather than adjusting those settings.
- Mark todo 491ae1b6 done only with a precise completion/verification summary. No claim of live team reattachment until a real smoke check has been approved and run; automated integration coverage is mandatory regardless.
