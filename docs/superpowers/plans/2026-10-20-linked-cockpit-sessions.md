# Linked Cockpit Sessions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. In this harness use the available `executing-plans` skill for native execution; do not assume an unavailable skill can be loaded.

**Goal:** Let explicitly approved Designer, Developer and QA sessions exchange visible, reliable task messages across Claude Code CLI, OpenCode and native Codex CLI.

**Architecture:** A daemon-owned typed collaboration protocol, host-owned transactional SQLite state and authenticated per-session intake, with narrow adapters for each native UI. The host sidebar controls immutable link-request approval/revocation and shows task groups/history; it does not own scheduling. Native delivery is separate from the existing human text queue and must prove input safety before automatic wake is enabled.

**Tech Stack:** Existing Rust 2024 cockpit/desktop crates, rusqlite, serde, Ratatui, peer-credential Unix sockets, native CLI/plugin bridges; no new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-20-linked-cockpit-sessions-design.md`. Prerequisite: `docs/superpowers/plans/2026-10-20-native-codex-cockpit.md` and its spec.

## Global Constraints

- Approval covers the exact task ID, exact session IDs and assigned roles. Changing any of these requires fresh approval.
- Team approval grants messaging only. Message contents never constitute user authorization for product work, scope changes, commands, installs, filesystem/network permissions, Git writes, merges or releases.
- Retries are bounded, back off and fail visibly. Durable duplicate detection prevents duplicate wake-ups and automatic QA requests across restarts.
- Approval is rechecked before every delivery and wake-up. Unlinking cancels pending deliveries and retries.
- If submission may have succeeded but its receipt is missing, stop automatic retries and visibly ask Theo to reconcile. Do not risk a duplicate resend.
- Each clarification question gets at most three request/reply rounds, then escalates to Theo if unresolved. Transport retries do not consume clarification rounds.
- Initial transport attempt plus at most three safe retries, after 2, 10 and 30 seconds.
- Maximum message body: 4,000 characters, matching the existing prompt limit.
- Same canonical project; distinct Designer/Developer/QA sessions, at most one active task-team membership per session. No role is tied to a provider or UI.
- QA reports findings and never fixes product code. A QA pass is not a merge/release/todo-completion authorization.
- No new mounts, shared checkouts, SSH access, secrets, filesystem permissions or network grants. No dependency/manifest changes without asking.
- No automatically launched teams, unreviewed global plugin changes, permission-relay capability, bypass flags or silently reduced UI support.

## Review Focus

- Two sidebars approve different request revisions: stale approval is refused and only the displayed exact tuple is granted (Tasks 2/7).
- Crash occurs between irreversible intake and receipt: no resend or duplicate wake, even if delivery must be reconciled manually (Tasks 3/4).
- Native UI has a draft, permission prompt or user interrupt: no collaborator keystrokes, draft mutation or interrupt; human input keeps precedence (Tasks 1/4).
- Latest handoff supersedes a QA request mid-review: results retain their original commit and cannot approve newer work (Task 6).
- Native CLI adapter/identity changes after a restart or update: fail closed with a visible reason instead of choosing another session/provider (Tasks 1/3/8).

---

## File map and shared types

All new function/type/command names below are proposed local implementation interfaces, not existing functions or upstream API claims. Avoid a generic broker framework: plain enums, structs, functions and one SQLite connection owned by the daemon suffice.

Create:

- `apps/cockpit/src/teams.rs`: typed protocol, validation, deterministic IDs/keys and envelope formatting. Child modules `teams/store.rs` (durable transactions), `teams/client.rs` (bounded authenticated client), `teams/view.rs` (Ratatui overlay and stable grouping).
- `apps/cockpit/src/bin/cockpit-team.rs`: sandbox-safe agent CLI; accepts one bounded JSON object on stdin and outputs a typed JSON result. No approval/edit/revoke subcommands.
- `apps/desktop/src/teams.rs`: verified sender binding, host-only control routing, scheduling, final authorization and native adapter coordination. Add `pub mod teams` in desktop lib.
- `apps/cockpit/tests/team_protocol.rs`, `team_store.rs`, `team_client.rs`, `team_adapters.rs`, `team_ui.rs`, `team_workflow.rs`; `apps/desktop/src/team_tests.rs` registered under cfg(test) for daemon integration with its fake System.
- A minimal Claude channel implementation only if Task 1 verifies an approved way to enable it. Its exact transport module is `apps/cockpit/src/teams/channel.rs`; reuse the existing binary `cockpit-team` in an explicit channel mode, not a new installed dependency or external chat service.

Modify only required integration sites:

- `apps/cockpit/src/lib.rs`, `paths.rs`, `agents.rs`, `sessions.rs`, `app.rs`; preserve existing human `prompts.rs` behavior.
- `apps/desktop/src/bin/osd.rs`, `registry.rs`, `tests.rs`, `lib.rs`; keep GTK-loop work bounded and asynchronous transport reads off the loop.
- `config/opencode/.config/opencode/plugins/cockpit.ts`, `config/claude/.claude/hooks/session-state.sh`, native Codex bridge from the prerequisite; no body parsing as permission approval.
- Relevant launchers and agent helpers only to attach the native adapter to the exact managed runtime. No terminal `say` fallback for autonomous messages.
- `README.md` and `config/bin/.local/bin/claude-sessions-agents/README.md` after loading `oss-docs`.

**Protocol types** (`teams.rs`):

- `TaskKey { project: String, task_id: String }`, `Role::{Designer, Developer, Qa}`.
- `Member { session_id: String, role: Role }`; `Proposal { submission_key: String, task: TaskKey, members: Vec<Member>, reason: String }`.
- `AgentAction::{Propose, Send, Reply, Assess, Handoff, Findings, Receipt, Status, Receive}`. Sender/role are not input authority; derive them from peer binding. `Receive` is a bounded read of the caller's eligible addressed inbox, never a global list.
- `HostAction::{Propose, Snapshot, Edit, Approve, Reject, Unlink, Resolve, Reconcile}`; each mutation carries an opaque host request ID and, when changing existing state, expected revision. Host Propose creates the same pending immutable proposal as agent Propose; it does not implicitly approve it. Snapshot carries task/cursor filters and changes no state. `Approve` includes the complete displayed mapping hash. Reconcile offers mark handled/cancel only, no resend.
- `MessageKind::{Clarification, Reply, Handoff, QaRequest, Findings, Notice}`; `DeliveryState::{Queued, Ready, Submitting, Received, Cancelled, Failed, Uncertain}`.
- `Submission { submission_key: String, recipient: String, kind: MessageKind, body: String, thread_id: Option<String>, handoff_id: Option<String> }`.
- `VerifiedSession { session_id: String, agent: String, project: String, runtime: String }`; `runtime` is a trusted incarnation fingerprint based on launcher ancestry/start time and verified native binding, not a caller-provided pane number.
- `TeamError` is a closed serializable error code plus display message. Avoid unlinked-target discovery through detailed errors. `TeamReply { request_id: String, result: Result<serde_json::Value, TeamError> }`.

**Store interface** (`teams/store.rs`): `open(path: &Path) -> Result<Connection, TeamError>`; `propose(connection: &mut Connection, sender: &VerifiedSession, proposal: &Proposal, now_ms: i64) -> Result<TeamReply, TeamError>`; `control(connection: &mut Connection, action: &HostAction, live: &[VerifiedSession], now_ms: i64) -> Result<TeamReply, TeamError>`; `accept(connection: &mut Connection, sender: &VerifiedSession, action: &AgentAction, now_ms: i64) -> Result<TeamReply, TeamError>`; `claim(connection: &mut Connection, recipient: &VerifiedSession, now_ms: i64) -> Result<Option<Dispatch>, TeamError>`; `finish(connection: &mut Connection, dispatch_id: &str, outcome: &TransportOutcome, now_ms: i64) -> Result<(), TeamError>`; `recover(connection: &mut Connection, live: &[VerifiedSession], now_ms: i64) -> Result<(), TeamError>`; `snapshot(connection: &Connection, task: Option<&TaskKey>, after: Option<i64>, limit: usize) -> Result<serde_json::Value, TeamError>`.

`Dispatch` contains immutable message/recipient/task/member epochs, attempt ID and envelope. `TransportOutcome::{Received { native_id: String }, NotSubmitted { reason: String }, Uncertain { reason: String }}`. No generic success boolean, no retry on ambiguous I/O.

**Daemon interface** (`desktop/teams.rs`): `request(registry: &mut Registry, sender_pid: u32, action: AgentAction) -> Result<TeamReply, TeamError>`; `host_control(registry: &mut Registry, action: HostAction) -> Result<TeamReply, TeamError>`; `tick(registry: &mut Registry, now_ms: i64) -> Result<(), TeamError>`. Registry owns the optional team connection; failure to open disables collaboration visibly without breaking ordinary sessions.

### Task 1: Prove native intake for all three UIs before enabling wakes

**Files:** Create `apps/cockpit/tests/team_adapters.rs`; extend `apps/cockpit/src/agents.rs` with a verified collaboration capability inventory, and record evidence in a short adapter-contract section in this plan. No product dispatcher yet.

**Consumes:** Completed native Codex prerequisite; existing Claude/OpenCode runtime contracts. **Produces:** tested native intake/readiness/receipt contract for each UI or an explicit blocker requiring Theo's decision.

- [ ] Read installed CLI/version contracts and minimal upstream references: Claude channels at `https://code.claude.com/docs/en/channels` and its linked reference; OpenCode V2 plugin/session prompt-admission docs; Codex installed queue help and current hooks/app-server schema. Do not call guessed APIs, start billed turns or change global config during discovery.
- [ ] Write executable adapter-contract tests around synthetic native runtimes with fixtures for idle, busy, draft input, permission dialog, receipt loss and duplicate intake. Assert no `tmux send-keys` occurs, busy is deferred, draft bytes are unchanged, permission state is not answered and one irreversible admission causes at most one turn. Include old/unsupported API errors returning unsupported rather than falling back.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test team_adapters`; expect missing adapters/capability failures and record them. Fake tests cannot prove a native feature exists: accompany each with the exact version/schema evidence and later approved live smoke test.
- [ ] Choose the narrow native path: OpenCode in-process plugin `ctx.session.prompt` with queued delivery and stable native admission ID if verified; Codex native `queue` scoped to its own sandbox runtime or private native app-server; Claude native MCP channel, not the existing `agent_claude_say`. Never use Claude permission-relay capability.
- [ ] Treat Claude channel enablement as a separate explicit approval prerequisite: current official docs call custom channels research preview and may require a custom allowlist or development flag. Do not silently set `--dangerously-load-development-channels`, install a plugin, change org policy or weaken sandbox rules. If no approved supported intake is available, stop this task, report the precise blocker and ask Theo; do not mark Claude supported through terminal typing.
- [ ] Pin exact native signatures/config/receipt semantics in the adapter-contract section and make the contract tests pass against those synthetic interfaces. Prove native receipt identity rather than treating a socket write/MCP notification as proof of completed native admission. Non-idempotent paths use one irreversible claim and uncertainty escalation, never a blind resend.
- [ ] Commit only tested contract/capability changes: `test(cockpit): pin native collaboration adapter contracts`.

### Task 2: Exact-tuple approval and durable host-only state

**Files:** Create `teams.rs`, `teams/store.rs`, `team_protocol.rs`, `team_store.rs`; modify cockpit `lib.rs`, `paths.rs`.

**Consumes:** Shared types above. **Produces:** `open`, `propose`, `control`, `snapshot`; immutable proposal revisions and per-member epochs.

- [ ] Write table-driven protocol tests for exact task IDs, distinct role/session mapping, empty/unknown roles, wrong projects, hostile controls, 4,000/4,001-character bodies and forged approval fields. Use strict deserialization (`deny_unknown_fields` on action payloads) and no executable body interpretation.
- [ ] Write persistent public-store tests: `pending_request_grants_no_messaging`, `only_the_displayed_revision_can_be_approved`, `changing_task_session_or_role_requires_fresh_approval`, `one_session_has_one_active_team`, `same_project_separate_checkouts_can_link`, `member_revocation_cancels_old_epochs`, `approved_qa_addition_preserves_unchanged_developer_epoch`, `duplicate_proposal_returns_original_request`. Open/close the database between actions to prove persistence. Assert rejected edits/approvals leave prior mappings unchanged.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test team_protocol --test team_store`; record the intended failure before implementation.
- [ ] Implement the typed actions and `paths::team_database() -> PathBuf` at `audit::dir().join("teams.sqlite")`, inside the already hidden persistent host directory. Validate directory/database type and permissions; refuse symlinked state, initialize 0700/0600, foreign keys, bounded SQLite busy timeout and schema version. Test WAL/SHM remain in that hidden directory. No sandbox direct DB access or broad new state mount.
- [ ] Implement transactional immutable request revisions, exact mapping hash, approvals/rejections, unique active membership and per-member epochs. V1 maxima: 3 members, 16 KiB wire request, 100 pending messages per team, 10 new requests/messages per verified sender per minute; count duplicate submission replays without creating work. Errors reject excess, never silently truncate. Persist rate counters for restart resistance. `snapshot` paginates at most 100 entries and never opens transport or runs project Git config.
- [ ] Rerun protocol/store targets, including symlink, locked/corrupt DB and disk-write-error cases. Require no partial grant on a failed transaction. Commit named files: `feat(cockpit): persist exact approved team memberships`.

### Task 3: Durable messages, bounded retry and crash recovery

**Files:** Modify `teams/store.rs`, `team_store.rs`; define `Dispatch` and outcomes in `teams.rs`.

**Consumes:** Approved mappings and membership epochs. **Produces:** `accept`, `claim`, `finish`, `recover` with separate transport and clarification accounting.

- [ ] Write tests `replayed_submission_returns_original_message`, `conflicting_duplicate_payload_is_rejected`, `receipts_survive_daemon_restart`, `uncertain_submission_is_never_retried`, `retry_budget_survives_restart`, `unlinked_retry_never_resumes`, `concurrent_claims_wake_at_most_once`. Use a file-backed DB reopened between every crash boundary, not only in-memory fixtures.
- [ ] Pin scheduler assertions using a fake clock: attempt 1 at t=0; known-pre-submit failure schedules +2, then +10, then +30 seconds; the fourth failed attempt becomes Failed once. Busy/offline waiting does not consume attempts. Duplicate replies and transport attempts leave thread rounds unchanged. Unknown in-flight state recovered after restart becomes Uncertain with no retry.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test team_store`; capture the intended failures.
- [ ] Implement unique sender/submission keys, message/attempt/receipt records and irreversible durable claims. Recheck exact current task/roles/member epochs and runtime on `claim`. `finish(NotSubmitted)` alone may schedule a bounded retry. `Received` deduplicates by original admission/message; `Uncertain` requires reconciliation. `recover` never converts Submitting back to Ready.
- [ ] Make revocation and final claim share serialization. Once an envelope has crossed the documented irreversible native boundary it is submitted and cannot be recalled; mere reservation/preparation is cancellable. Do not return a reusable delivery lease that could be exercised after revocation. Test revoke-before-boundary and boundary-before-revoke as distinct linearized outcomes.
- [ ] Rerun tests including competing connections, duplicate late receipts, clock rollback (no retry burst), disk-full/failed-commit simulation and failed reconciliation. Commit: `feat(cockpit): make collaborator delivery crash safe`.

### Task 4: Authenticated agent client, daemon scheduling and native adapters

**Files:** Create `teams/client.rs`, `cockpit-team.rs`, `desktop/teams.rs`, `team_client.rs`, `desktop/team_tests.rs`; modify `osd.rs`, `registry.rs`, desktop/cockpit libs, relevant bridge/launcher files fixed by Task 1, `opencode_plugin.rs`, `team_adapters.rs`.

**Consumes:** Store APIs and Task 1 pinned native contracts. **Produces:** authenticated sandbox operations and daemon-owned dispatch independent of the sidebar.

- [ ] Write socket/daemon tests `payload_cannot_choose_sender_or_role`, `other_team_history_is_not_disclosed`, `sandbox_cannot_submit_host_control`, `unverified_runtime_is_refused`, `sidebar_absence_does_not_stop_delivery`, `native_receipt_cannot_ack_another_recipient`, `idle_wakes_once_busy_waits_for_turn_end`. Drive real request decoding and existing peer-PID/ancestry mapping, not a mock caller's asserted sender string.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test team_client --test team_adapters` and `cargo test --manifest-path apps/Cargo.toml -p desktop --lib team_`; record expected failures.
- [ ] Add bounded `team-request` handling to the report socket with a typed JSON reply. The new client `pub fn request(action: &AgentAction) -> Result<TeamReply, TeamError>` uses the existing socket path, write/read deadlines and maximum response size; no DB access. `cockpit-team <action>` reads JSON on stdin; action verb must match its tagged input. Exit nonzero with a useful error for refusal/unreachable/unsupported.
- [ ] Derive verified caller and canonical project from trusted ancestry/known launcher binding; do not run sandbox-controlled Git configuration on the host. Same basename is not canonical project proof. Use verified primary-checkout/parallel-clone inventory; refuse ambiguous unrelated roots with the same name. Restrict Receive/Status to the caller's addressed task data, never another recipient's entire history.
- [ ] Implement `request`, `tick`, adapter attachment and receipt handling using pinned Task 1 contracts. Recheck current approval at actual native intake/wake, not only enqueue time. If a reverse-polling adapter cannot make that boundary safe under revocation, fail its capability and stop rather than treating a stale lease as authorization.
- [ ] Native adapters must not overwrite draft input, use a human user role as permission, interrupt a busy turn or run a host shell from message text. OpenCode adapter attaches only to the managed root session, not children or another server. Codex uses the verified native thread/runtime mapping. Claude channel uses no permission relay and handles MCP IDs/framing with bounded parsing and approved enablement only.
- [ ] Preserve human prompt priority by deferring when a human queue entry/input/needs-you state is pending. For native non-idempotent intake, durable preclaim plus one submission; receipt timeout becomes Uncertain. Poll/heartbeat work is bounded/backed off and disposed with the runtime; no hot loops or UI-thread waits.
- [ ] Audit link/message/receipt/failure/revocation events using trusted metadata and content fingerprints; keep raw bodies only in protected team history, not journal. Persist unread/escalation state and publish host-only view data, not bodies into the sandbox-readable general session snapshot.
- [ ] Rerun client, adapter, desktop tests and existing OpenCode hook tests. Add a fixture-only fourth adapter to prove protocol code has no three-agent branching outside native dispatch. Commit named files: `feat(cockpit): route authenticated team messages across native UIs`.

### Task 5: Clarification rounds and visible escalation

**Files:** Modify `teams.rs`, `teams/store.rs`, `desktop/teams.rs`, `team_store.rs`, `desktop/team_tests.rs`.

**Consumes:** Authenticated actions/durable messages. **Produces:** task-scoped clarification threads and explicit assessment/escalation through `accept`/`control`.

- [ ] Write tests `third_unresolved_assessment_escalates_once`, `third_reply_waits_for_questioner_assessment`, `fourth_request_is_refused`, `duplicate_reply_does_not_advance_round`, `transport_retry_is_not_a_clarification_round`, `unrelated_questions_have_independent_counters`, `agent_cannot_forge_theos_resolution`.
- [ ] Run selected team store/desktop tests and record the failures.
- [ ] Implement requester-owned Assess with resolved/unresolved; after reply 3 await assessment without repeated wake-ups, then escalate on unresolved assessment or attempted fourth request. Waiting/missing assessment is visible but has no automatic reminder loop. Persist a unique escalation key and trusted host Resolve action with reason/provenance.
- [ ] Rerun tests after DB reopen at each round; require exact counters and one needs-you event. Commit: `feat(cockpit): bound team clarification rounds`.

### Task 6: Structured developer handoffs and one QA request

**Files:** Modify `teams.rs`, `teams/store.rs`, `team_store.rs`, `team_workflow.rs`.

**Consumes:** Approved Developer/QA memberships, durable message IDs and host/task policy. **Produces:** validated handoffs, unique automatic QA triggers and evidence-linked findings.

- [ ] Write tests `only_linked_developer_completes_handoff`, `handoff_requires_real_commit_reference`, `duplicate_handoff_creates_one_qa_request`, `approved_qa_addition_schedules_waiting_handoff_once`, `new_handoff_supersedes_unsent_old_review`, `running_old_review_cannot_pass_new_commit`, `revocation_cancels_qa_trigger`, `qa_findings_never_dispatch_product_changes`.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test team_store --test team_workflow`; record intended failures.
- [ ] Define `Handoff { submission_key: String, branch: String, commit: String, checkout: String, summary: String, tests: String, figcraft: Vec<FrameReference> }` and `FrameReference { file: String, frame_id: String }` with task/role/project derived from verified membership. Require nonempty branch/checkout/summary/test outcome and a full syntactically valid SHA-1 or SHA-256 object ID; native Git existence is attested inside the sender sandbox, not checked by host execution of project config. Missing evidence is incomplete, never fabricated.
- [ ] Atomically persist completed handoff and one trigger keyed by task/developer epoch/handoff/QA epoch. New explicit handoff cancels only not-yet-submitted older QA requests; started reviews retain old commit/frame labels and become superseded. Later QA addition creates a new request for the still-current unchanged Developer handoff, not resurrection of a cancelled trigger.
- [ ] Format role instructions in trusted envelope metadata: separate task-work authorization is required, unavailable board/commit access escalates, QA findings target Developer or Designer and never invoke a code-fix command. A pass cannot mutate todo/Git/release state.
- [ ] Rerun tests including each UI occupying each role. Commit: `feat(cockpit): request QA once per completed handoff`.

### Task 7: Host approval controls and the approved cockpit layout

**Files:** Create `teams/view.rs`, `team_ui.rs`; modify cockpit `app.rs`, `sessions.rs`, desktop `osd.rs`, `desktop/teams.rs`, `team_client.rs`.

**Consumes:** Typed immutable proposals, host-only actions, protected snapshots/history. **Produces:** Sessions team grouping, Link approval/Message history overlay, revoke/reconcile/resolution controls and actionable attention.

- [ ] Write host/UI tests `team_action_opens_selected_task`, `proposed_members_are_not_shown_as_linked`, `approval_displays_full_exact_mapping`, `edit_requires_fresh_confirmation`, `stale_approval_cannot_grant_edited_tuple`, `unlink_cancels_retries_immediately`, `history_escape_and_narrow_layout_preserve_identity`, `closing_overlay_restores_session_focus`, `uncertain_recovery_never_offers_blind_resend`.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test team_ui --test team_client`; record expected failures.
- [ ] Add host-only request/response control socket at `paths::runtime_dir().join("claude-team-host/control.sock")`, inside a 0700 directory not mounted into sandboxes. Match existing off-main-thread bounded readers; authenticate peer ancestry and reject sandbox descendants, even if they guess the path. No approvals on the sandbox report socket, legacy FIFO or agent CLI. Host snapshots/history are read through this endpoint, not a global readable JSON file.
- [ ] Implement `teams::client::host_request(action: &HostAction) -> Result<TeamReply, TeamError>` and `host_snapshot(task: Option<&TaskKey>, after: Option<i64>) -> Result<Value, TeamError>` with bounded async app requests. Control request IDs plus revision/mapping-hash prevent double/stale submit. Test missing/timeout server leaves the UI responsive and shows an error, never an optimistic grant.
- [ ] Add a palette-only `team` action in Sessions (no new global shortcut), matching approved overlay tabs and grouped role-labelled members. Stable navigation uses session IDs, not row offsets. Enter on a real member still opens its native session; proposed or unavailable members cannot receive messages. Mark pending links, failures and unresolved escalations for existing needs-you navigation without stealing focus or opening browser windows.
- [ ] Implement picker for task ID and eligible existing sessions; distinct roles/one-team constraints match backend validation. Full IDs remain accessible at narrow widths via wrapping/detail scroll, with explicit final exact-tuple confirmation. Reconcile permits inspect receipt, manually mark handled or cancel; dismissing an alert is not delivery success. Unlink revokes immediately and never redirects messages.
- [ ] Rerun UI/socket tests at 80x24 and a narrow sidebar, keyboard-only navigation, duplicate label fixtures and two concurrent clients. Verify human prompts, todo/PR selection and status navigation remain unchanged. Commit: `feat(cockpit): review team links and message history`.

### Task 8: Cross-UI/security regression, documentation and reviewed rollout

**Files:** Extend `team_workflow.rs`, `team_adapters.rs`, `desktop/team_tests.rs`, relevant existing agent tests; update README/agent docs and these plan checkboxes.

**Consumes:** All prior tasks and native Codex plan. **Produces:** verified complete feature, reviewed application, truthful final report and todo completion.

- [ ] Add parametrized full entrypoint workflow tests for all 9 ordered sender/receiver pairs across Claude CLI/OpenCode/Codex CLI and role assignments rotated across all three. Include exact approval, clarification/reply, busy queue, handoff→QA and findings, daemon/database reopen and unlink mid-retry. No test should depend on which CLI hosts the coding agent running the suite.
- [ ] Add adversarial tests for forged sender/role/approval, different same-basename project, guessed IDs, user-looking authorization text, terminal escapes, shell metacharacters, oversized traffic, spoofed native receipt, native runtime replacement and concurrent revocation. Verify no host subprocess receives body text as executable code and no extra filesystem/network access is granted.
- [ ] Run `cargo test --manifest-path apps/Cargo.toml -p cockpit --test team_protocol --test team_store --test team_client --test team_adapters --test team_ui --test team_workflow --test native_codex` and `cargo test --manifest-path apps/Cargo.toml -p desktop --lib`. Fix real failures; do not disable/baseline them.
- [ ] Load `oss-docs` and document Team action, agent proposal CLI schema/examples, exact approval scope, role ownership, separate work authorization, pending/uncertain recovery, three-round limit, bounded transport retries, native adapter prerequisites and unsupported-version errors. Call out every changed host launcher/helper/hook/profile/daemon file. Keep mockup/local tokens uncommitted.
- [ ] With explicit build/test approval at execution time, run the repository's existing `make test` suite in the clone. Record verbatim outcomes including pre-existing failures. Request any required host command via `ask-to-run`; do not install a missing tool or weaken sandbox rules to make tests pass.
- [ ] Request explicit readiness for real UI smoke checks, then use `ask-to-guide` only after yes. Cover Claude CLI, OpenCode and native Codex CLI in a mixed team: user-approved exact links, one idle wake, busy deferred delivery, draft preservation, handoff QA once, revoke before retry. Native plugin/hook trust, account policy and billed turns require their own explicit approvals; fake fixtures are not a substitute for this check.
- [ ] Commit only finished tested requested files on the feature branch. Show `git diff --stat origin/main...HEAD` and list any unrelated inherited commits separately; this clone initially reports `origin/main...HEAD` counts **1 left / 69 right**, so do not assume the export contains only this feature. Compare read-only history with the applied live base. If synchronization needs a rebase/reset or would export unrelated work, stop for explicit authorization; do not rewrite the branch automatically.
- [ ] Export the reviewed intended commits using `git format-patch origin/main -o .outgoing` only after base verification, then invoke `ask-to-run --cwd <clone-root> --why '<review explanation>' 'claude-sessions apply-dotfiles <clone-root>'`. Host review is mandatory; a decline leaves the feature unapplied and todo incomplete.
- [ ] Through `ask-to-run`, request `make -C ~/dotfiles cockpit test` followed by `claude-sessions reload` when apps changed. Report actual pass/fail and host smoke status. No bypassing failed checks.
- [ ] Only after the whole requested work is implemented, verified and reviewed/applied, run `claude-todo done 1d3fd318`. Final report: what changed, what stayed unchanged, tests and exact outcomes, diff stat and any remaining blocker.

## Self-review and execution handoff

Coverage: exact approval/UI Tasks 2/7; protected state/isolation Tasks 2/4; persistence/retry/revocation Tasks 3/4; clarification Task 5; handoff/QA Task 6; all native UIs Tasks 1/4/8 plus Codex prerequisite; existing work authorization unchanged throughout. The five Review Focus conditions each have owning tests. All new store/daemon/client types above are local interfaces; native APIs are deliberately gated by Task 1 evidence, not guessed.

The safe-intake gate is a real dependency, not permission to ship an incomplete or weaker adapter. In particular, Claude's custom-channel preview restrictions may require a user decision before implementation can progress. Resolve that first rather than building the whole feature around terminal typing.

Theo approved native execution. Manual progress ledgers are explicitly authorized because the original execution helpers were unavailable. No dependencies have been installed. The native Codex foundation is committed; persistence is under test; no feature changes have been applied live. See `.superpowers/sdd/2026-10-20-linked-cockpit-sessions/progress.md` and the native prerequisite's sibling ledger for exact evidence and rulings.

Approved exception to the general no-bypass-flags rule: Theo explicitly approved only `--dangerously-load-development-channels server:cockpit-team`. It bypasses Claude's channel allowlist for that named local server, not tool approvals, org policy or greywall. Permission relay remains forbidden. `store::propose` takes the verified live-session slice in addition to its original parameters so it can bind runtime identity at intake.
