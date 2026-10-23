# Cross-project Session Links Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let two approved existing sessions in different projects message about one shared TODO, automatically and permanently closing that approval when the TODO finishes.

**Architecture:** Add explicit link records alongside—not inside—team membership in the protected collaboration database. Reuse sender authentication, the durable message/attempt queue and native adapters, with distinct team/link authorization branches. Persist TODO invalidation before completing the TODO so a done→reopen transition or daemon outage cannot revive a link.

**Tech Stack:** Existing Rust workspace, rusqlite/SQLite, serde, Ratatui, existing Claude/OpenCode/Codex delivery adapters; no new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-23-cross-project-session-links-design.md` (approved; read it in full).

## Execution ledger (Task 7)

Theo authorized continuous implementation and explicitly waived FigCraft:
“Don't do a figcraft, just continue till this feature is DONE, don't ask me
everytime”. This supersedes the original execution/design gates below, not host
rollout or live-test consent. No FigCraft approval is claimed.

- [x] Task 1: protected schema and strict lookup — `26884e9`, independently reviewed.
- [x] Task 2: durable completion/deletion/retention and mutation ordering — `58dea27`, independently reviewed after the reopen-race fix.
- [x] Task 3: exact-pair host approval — `a908526`, independently reviewed.
- [x] Task 4: shared delivery queue and migration/recovery — `2ce6056`, independently reviewed.
- [x] Task 5: CLI/context/native envelopes — `16739ec`, independently reviewed after escaping and framed-context fixes.
- [x] Task 6: existing Ratatui host controls — `cc8a9b7`, independently reviewed; FigCraft waived by Theo.
- [x] Task 7 local implementation, regression and independent review complete. Final stale-snapshot persistence fix and deterministic migration test: `b3159ae`, independently re-reviewed. Deployment and live acceptance remain separate.

The original detailed checklists below are retained as the historical procedure,
not assertions that every proposed RED command was executed exactly as written.
Actual commands, failures, fixes and deviations are in
`.superpowers/sdd/2026-10-23-cross-project-session-links/task-1-report.md` through
`task-6-report.md`, with reviews alongside. Task 7 updates its own verified
checkboxes below. Whole-branch independent final review belongs to the controller;
rollout and live acceptance remain unchecked and separately gated.

## Global Constraints

- “It is not team membership.” Keep existing cross-project team rejection, unique roles, task ownership, recruit and QA behavior.
- “Only Theo approves.” A proposal, collaborator message or TODO report is never authorization.
- “Reopening the TODO must not restore the link.” Missing lifecycle evidence blocks delivery.
- “V1 supports local typed TODOs, not GitHub issues”. The owning project must be one participant's verified project.
- “No interaction layout is approved by this document.” Obtain FigCraft approval before UI implementation; do not open FigCraft or run host commands without the required consent.
- “No dependency or sandbox/network-policy change is intended.” No manifest edits, installs, broad discovery or host-control socket publication.
- No product implementation is authorized by approving this plan's creation. Wait for Theo's plan review and execution choice. Ask in chat before anything needing `ask-to-run`, per this task's explicit override of the usual automatic handoff.

## Review Focus

- Done→reopen while the daemon is down: the old approval stays closed (Task 2).
- A stale cockpit instance saves an old TODO snapshot: it cannot overwrite completion evidence or reopen messaging (Tasks 1–2).
- A recipient has a team and several links: they still get at most one message per idle turn and no parallel uncertain deliveries (Task 4).
- A guessed peer, link or thread ID: failure reveals no unrelated session, task or history (Tasks 3 and 5).
- A claimed delivery races closure: distinguish already-authorized in-flight delivery from a new dispatch after closure, without promising recall (Tasks 2 and 4).

## File map and execution boundaries

New `apps/cockpit/src/teams/links.rs` owns link state, exact-pair approval and durable work-lifecycle records. Keep queue/attempt mechanics in existing `teams/store.rs`; do not duplicate the transport or pretend links are teams. New integration tests: `apps/cockpit/tests/session_links.rs`. Add to existing `team_protocol.rs`, `team_store.rs`, `team_client.rs`, `team_watch.rs`, `team_ui.rs`, `native_codex.rs`, `claude_todo.rs`, `apps/cockpit/src/tests.rs`, and `apps/osd/src/tests.rs` as their entry points change.

Production wiring reaches `teams.rs`, `teams/store.rs`, `teams/client.rs` only if needed, `todo.rs`, `app.rs`, `bin/cockpit-team.rs`, `bin/cockpit-codex-hook.rs`, and `apps/osd/src/teams.rs`. OpenCode presentation lives in `config/opencode/.config/opencode/plugins/cockpit.ts`; it runs with host rights, so call out any change explicitly. No Makefile or launcher/script change is planned. `config/bin/.local/bin/cockpit:2244` already queues apply-driven completion through the TODO processor; test that route rather than adding a bypass.

All new names/signatures below are **proposed implementation interfaces**, not existing APIs. Existing paths/functions were inspected. Private SQL details may vary if the specified transaction, privacy and migration tests hold; do not vary the wire contracts between tasks.

The UI gate in Task 6 also applies to any earlier task's `app.rs` presentation changes: obtain the required design approval before changing those controls or states. Backend and regression work may proceed independently, but an unfinished UI dependency is not a shippable feature.

### Task 1: Protected schema and strict shared-work lookup

**Files:** Create `teams/links.rs` and `tests/session_links.rs`; modify `teams.rs`, `teams/store.rs::open`, `todo.rs` (paths relative to `apps/cockpit/src` or `apps/cockpit`).

**Interfaces:** Reuse `TaskKey`, `VerifiedSession`, `TeamError`, `TeamReply`. Add `links::SharedWork { task: TaskKey, title: String, status: String }`; `todo::read_shared_work(task: &TaskKey) -> Result<SharedWork, TeamError>`; `links::migrate(connection: &Connection) -> Result<(), TeamError>`. `read_shared_work` reads the actual host TODO file strictly, with no `TodoList::load` or `status_of` unknown→open default. Resolve the typed item's recorded host checkout to its canonical project using the same verified host path rules as daemon sessions; the basename alone is not identity.

The daemon currently owns `canonical_project` in the dependent `osd` crate. Move its existing host-path logic into `cockpit::teams::canonical_project(cwd: &str) -> Result<String, TeamError>` and have both daemon verification and strict TODO lookup call that shared primitive. Move the small `registry::parallel_clone_repository` path primitive into `cockpit::util::parallel_clone_repository(cwd: &str) -> String`, updating its existing callers explicitly. Preserve the host-managed parallel-clone mapping and its tests; never infer canonical identity from a writable remote URL or introduce a reverse crate dependency. Task 1 therefore also modifies `apps/cockpit/src/util.rs`, `apps/osd/src/registry.rs` and `apps/osd/src/teams.rs`.

- [ ] Write `shared_work_requires_an_existing_approved_typed_todo` with table cases unknown ID, issue ID, proposed, done, duplicate ID, owner/canonical-path mismatch, malformed/unreadable file: `assert!(todo::read_shared_work(&task).is_err())`. For a doing typed item with the exact owner assert `work.task == task` and `work.status == "doing"`. Add fixtures with valid `cwd`, including parallel clones and projects sharing a basename.
- [ ] Run `cargo test --offline --manifest-path apps/Cargo.toml -p cockpit --test session_links`; expect a compile failure for the absent interface, then a behavior failure if lookup wrongly uses the current fallback.
- [ ] Add link proposals, approved links, exact participant/runtime records, and `link_work` lifecycle records in one versioned migration. Preserve existing tables; add `scope_kind` (`team` default) and nullable `link_id` to messages, not a fake team. `link_work` is keyed by full `TaskKey`, stores durable generation/state (`open`, `blocked`, `done`, `deleted`), and is never removed by TODO retention. Add unique active unordered-pair/TODO protection and immutable approval revisions/hashes. A new DB initializes the current schema; an old DB upgrades atomically.
- [ ] Add `link_schema_upgrade_preserves_team_state_and_receipts`: create a version-2 fixture with approved team/message/attempt data, upgrade and reopen; `assert_eq!(team_before, team_after)` and assert the historical message defaults to team scope. Unsupported future versions and partial migration errors must remain errors. Run the new tests plus `--test team_store`; expect all pass.
- [ ] Commit only this task's reviewed files on an authorized feature branch. Never commit on the default branch; do not push or apply.

### Task 2: Durable TODO invalidation across every completion path

**Files:** Modify `todo.rs::{set_status,remove,try_save,apply_update}`, `teams/links.rs`, `app.rs` TODO mutation result handling; tests in `src/tests.rs`, `tests/session_links.rs`, `tests/claude_todo.rs`, `apps/osd/src/tests.rs`.

**Interfaces:** Add `links::invalidate_work(connection: &mut Connection, task: &TaskKey, reason: &str) -> Result<(), TeamError>` and `links::reopen_work(connection: &mut Connection, task: &TaskKey) -> Result<(), TeamError>`. These are host-library calls, never sandbox protocol actions. Invalidation increments generation, closes links, invalidates pending proposals and cancels safe queued retries in one immediate transaction. Reopen marks work open without decreasing generation or reviving proposals/messages. Keep `TodoList::set_status(...) -> bool`, returning false with a visible logged error on persistence failure; make `TodoList::remove(&mut self, id: &str) -> Result<(), String>` and update its real callers to propagate failure.

- [ ] Write `todo_completion_closes_links_before_daemon_observation`: approve a fixture link, complete via `set_status`, reopen before polling, reopen the database; `assert_eq!(link_status, "closed")`, `assert_eq!(queued_state, "cancelled")`, `assert!(approve_old_proposal().is_err())`. Add parameterized cases through the real queued CLI update processor, apply trailer's `{action:"done", id, started_in}` record, and host UI done action, retaining existing ownership checks.
- [ ] Write fault tests: failure to persist invalidation means `assert!(!todos.set_status(id, "done", session))` and no success banner; crash after invalidation but before JSON save leaves work blocked/done and old links unusable; a stale TODO save cannot erase the database generation. Deletion and retention cannot revive links; closing one TODO leaves unrelated links/teams unchanged.
- [ ] Run focused tests before implementation; expect failures because TODO changes currently have no link side effect and `save` only logs errors.
- [ ] Implement **invalidation first**: for a tracked typed item, commit a durable closed/blocked lifecycle record before JSON completion/deletion. Then use `try_save`, rolling back the in-memory TODO mutation on failure without undoing invalidation. Publish completion notifications/release dependent work only after successful persistence. Reopening may publish `open` only after its JSON write succeeds; it never removes closure evidence. Unknown/missing/truncated storage must fail closed for link authorization. If no collaboration DB exists and no link could have been approved, unrelated TODO operations need not create it; an existing but unreadable DB is not equivalent to absence.
- [ ] Serialize initial lifecycle enrollment/approval, invalidation, reopening and final link claim on SQLite immediate transactions. Read the authoritative TODO **inside** this boundary; never publish an open lifecycle solely from a snapshot read before acquiring it. Existing link approvals must never be seeded from scratch after loss of lifecycle rows. Automatic retention may drop TODO JSON only after done has durably invalidated tracked work; stale saves cannot modify `link_work`.
- [ ] Update UI mutation branches to check return values and show failure rather than “done”/“removed”. Run focused TODO/link tests and existing TODO ownership/retention tests; expect all pass. Commit the task only after review.

### Task 3: Exact-pair proposals and host-only approval

**Files:** Modify `teams.rs`, `teams/links.rs`, `apps/osd/src/teams.rs`; tests in `team_protocol.rs`, `session_links.rs`, `apps/osd/src/tests.rs`.

**Interfaces:** New deny-unknown-fields types: `LinkProposal { submission_key: String, task: TaskKey, peer_session_id: String, purpose: String }`, `LinkSubmission { link_id: String, submission_key: String, kind: MessageKind, body: String, thread_id: Option<String> }`. New `AgentAction::LinkPropose { proposal: LinkProposal }`, `LinkSend { message: LinkSubmission }`, `LinkAssess { link_id: String, thread_id: String, resolved: bool }`. Host variants `LinkApprove { request_id, proposal_id, expected_revision, mapping_hash }`, `LinkEdit { request_id, proposal_id, expected_revision, replacement: LinkProposal }`, `LinkReject { request_id, proposal_id, expected_revision }`, `LinkRevoke { request_id, link_id }` (IDs/hashes String, revisions i64). Host-originated proposals use `LinkPropose { request_id: String, requester_session_id: String, proposal: LinkProposal }`; requester identity is chosen only by the host, never an agent field.

**Store interfaces:** `links::propose(connection: &mut Connection, sender: &VerifiedSession, proposal: &LinkProposal, live: &[VerifiedSession], now_ms: i64) -> Result<TeamReply, TeamError>`; `links::control(connection: &mut Connection, action: &HostAction, live: &[VerifiedSession], now_ms: i64) -> Result<TeamReply, TeamError>`. Existing `store::control_with` dispatches only the link host variants to the latter, retaining host action idempotency.

- [ ] Add `cross_project_developers_require_exact_host_approval`: fixture two projects with each participant already Developer in its own team; assert pending link has no messaging rights, exact approval activates it, and member/task/QA snapshots are unchanged. Parameterize changed runtime/project/peer/TODO/purpose/revision/hash: `assert!(stale_approval.is_err())`. Swapping the pair cannot create a second active channel for the same TODO.
- [ ] Add protocol and authenticated daemon tests: caller cannot supply sender, approved, runtime or project claims; sandbox `link_approve` is rejected; unavailable, unknown and unrelated peer IDs produce indistinguishable errors. Owner must be one of the exact two projects. Reject self-links, non-managed sessions and third-party tasks. Pending/rejected receipts expose no peer details.
- [ ] Run protocol/link/daemon focused tests; expect absent variants/functions to fail first.
- [ ] Implement immutable proposals and exact approval revalidation, including lifecycle generation from Task 2. Reuse 128-byte alphanumeric/`-`/`_` submission keys and 1,000-character one-line purpose limit. Cap pending requests at 10 per requester, active links at 10 per participant, and proposal/send traffic at the existing combined 10/minute sender budget. Idempotent replays do not consume budget; changed contents conflict. Persist the exact verified bindings; no role assignment or team mutation.
- [ ] Run `cargo test --offline --manifest-path apps/Cargo.toml -p cockpit --test session_links --test team_protocol --test team_store` and `cargo test --offline --manifest-path apps/Cargo.toml -p osd team`; expect pass, including `unrelated_projects_or_duplicate_role_assignments_cannot_link`. Commit after review.

### Task 4: One durable delivery queue with explicit scope authorization

**Files:** Modify `teams/store.rs::{accept,claim,finish,recover,expire}`, its thread helpers, `teams/links.rs`, `apps/osd/src/teams.rs`; test `session_links.rs`, `team_store.rs`, `apps/osd/src/tests.rs`.

**Interfaces:** `links::accept(connection: &mut Connection, sender: &VerifiedSession, message: &LinkSubmission, live: &[VerifiedSession], now_ms: i64) -> Result<TeamReply, TeamError>` and `links::assess(connection: &mut Connection, sender: &VerifiedSession, link_id: &str, thread_id: &str, resolved: bool) -> Result<TeamReply, TeamError>`. Change the existing claim signature to `store::claim(connection: &mut Connection, recipient: &VerifiedSession, idle_turn: &str, live: &[VerifiedSession], now_ms: i64) -> Result<Option<Dispatch>, TeamError>`; update every actual caller/test explicitly. No default live-session bypass.

- [ ] Add `approved_link_delivers_without_team_membership`: A sends to link ID; recipient is derived as B; assert B receives verified sender project, link and TODO identity, no user authorization, and no fabricated team role. Only Notice/Clarification/Reply are accepted; reject Handoff/QaRequest/Findings and foreign link/thread IDs. Three clarification rounds escalate, replies/assessments retain existing participant restrictions and do not count retries as rounds.
- [ ] Add `team_and_links_share_recipient_idle_and_uncertain_gates`: queue one team message and messages from two links, assert only one claim per recipient runtime/idle-turn; a submitting/uncertain attempt from any scope blocks subsequent claims from all scopes. A new verified turn admits the next oldest eligible message after receipt. Link-only recipients need no member row.
- [ ] Add tests for combined 10/minute sender proposal/message budget, 100 pending messages per exact shared TODO across its links, combined queue accounting, duplicate keys across team/link scopes, unchanged four-attempt delays (2/10/30 seconds), missing receipt timeout, safe failed submission, late receipts after closure, recovery and runtime changes. Send/enqueue/claim cannot succeed for third-party or transitive peers. Pause on absence; never auto-start or bind replacement runtimes.
- [ ] Run focused new tests; expect failure before extending queue scope.
- [ ] Implement authorization by explicit scope, sharing message IDs, attempts, idempotency namespace and ordering. A link epoch covers its exact mapping and work generation. Recheck both participant identities and strict work/lifecycle state before accepting/claiming; fail closed and visibly block on unknown state. `finish` reauthorizes safe retries by scope, not member joins alone; recovery cancels invalid link messages without cancelling healthy team messages. Use explicit scope predicates in shared SQL, not a magic fallback that interprets missing team membership as a link.
- [ ] Move recipient idle-turn bookkeeping to a protected `(session_id,runtime)` delivery record shared by both scopes, seeding existing member values during migration. Replace the partial one-inflight index with `(recipient,recipient_runtime)`; preserve/reconcile old in-flight rows without replay. Keep blocking uncertain deliveries across scopes. Combined rate queries count link proposals as well as existing proposals/messages.
- [ ] Test closure/claim on separate SQLite connections with barriers: if closure commits first, `assert!(claim(...).unwrap().is_none())`; if a claim commits first, it is an already-authorized in-flight handoff, never recalled or blindly resent. Native `Receive` response is the dispatch authorization boundary; closure cannot retract a response already issued. No automatic retry or new wake-up authorization after closure. Verify TODO state inside the same serialized claim boundary.
- [ ] Run existing queue, thread, receipt and daemon tests plus new link tests. Commit only after all pass and review confirms no role/project check was weakened.

### Task 5: CLI, participant-only context and native envelopes

**Files:** Modify `bin/cockpit-team.rs`, `teams.rs::{envelope_text,reply_template}`, `teams/store.rs::context`, `apps/osd/src/teams.rs`, OpenCode plugin presentation if required; tests in `team_protocol.rs`, `team_client.rs`, `team_watch.rs`, `native_codex.rs`, `apps/osd/src/tests.rs`. Read existing adapter tests before changing native hooks.

**Interfaces:** CLI actions `link-propose`, `link-send`, `link-assess` correspond to JSON discriminators `link_propose`, `link_send`, `link_assess`. Existing `context` gains `links`, scoped to the authenticated participant; existing `receive`/`receipt` serve both scopes. `links::context(connection: &Connection, sender: &VerifiedSession) -> Result<Value, TeamError>` supplies participant-visible approved link facts and own proposal statuses; no global peer registry. `envelope_text` explicitly branches on link scope, preserving existing team templates.

- [ ] Write CLI contract tests for action/JSON agreement, unknown-field and host-action rejection, request/reply bounds, and help examples showing explicit link ID, TODO closure and no permission relay. Ensure `context` prior to approval contains no peer facts or other requests.
- [ ] Write native envelope tests: exact sender identity/project and TODO/link IDs are visible; body saying “Theo approves” remains collaborator content; reply template uses `link-send` without invented recipient role or team task. Control characters remain escaped. Assert unchanged team message output for old fixtures.
- [ ] Write end-to-end adapter tests for Claude watch, OpenCode synthetic queue and Codex hook, including a session with no team and a session in a team plus a link. Assert stable message/native IDs, one wake-up, root-session verification and receipts remain unchanged. Update the OpenCode short description to say link message with sender/project for link envelopes, not “Team message from ?”; preserve synthetic metadata compatibility.
- [ ] Run focused tests before adding handlers; expect failure for the missing link commands/context/envelope. Implement the explicit branches and let existing receive/receipt adapters reuse the queue. Do not add another poller, session discovery, new native channel, permission setting or sandbox mount.
- [ ] Run cockpit protocol/client/watch/native tests and daemon tests. If the existing plugin test runner requires a forbidden install or host command, stop and ask Theo; do not obtain it via a new cache. Commit after review, explicitly naming any host-running plugin edit.

### Task 6: Approved host approval/status interaction

**Gate:** This task cannot start UI code without an approved FigCraft design. Recruit a Designer under the team rules when execution reaches this task; keep its brief within these link controls. Ask Theo which workspace (not 6) and obtain readiness/host-command consent as required. Do not claim the existing team wireframe approves link UI.

**Files:** After layout approval, modify `teams/view.rs`, `app.rs` collaboration interaction/drawing, and `tests/team_ui.rs`; reuse existing host snapshot/control and session navigation. No new permanent pane or team-role grouping is required by the backend.

**Interfaces:** Host snapshot gains `link_proposals` and `links` alongside existing team data. Link approval uses Task 3's exact host actions; host history distinguishes link versus team messages and shows TODO closure/revocation reasons and uncertain in-flight state. Participant selection uses only the host-side verified live registry.

- [ ] Obtain and record approval for a request displaying purpose, full pair IDs/projects, shared TODO, messaging-only scope, immutable revision and approve/edit/reject actions; active/closed link history; manual revoke; visible blocked lifecycle/delivery state. Editing keeps exact approval semantics and never grants a team role.
- [ ] Add failing UI tests through existing screen/key entry points: exact mapping confirmation, stale revision refusal, edits require new confirmation, revoke cancels queued messages, done closes link, closed links remain history, unavailable TODO shows no successful approval. Preserve ordinary session focus/navigation, team controls and unknown-service indicators.
- [ ] Implement from approved frames using existing Ratatui patterns. Show a backend lifecycle failure rather than implying a link or TODO operation succeeded. Reconcile uncertain deliveries using existing host control; no resend button for ambiguous submissions.
- [ ] Run `cargo test --offline --manifest-path apps/Cargo.toml -p cockpit --test team_ui` and existing relevant app unit tests. Commit after review; do not start a real link to test the UI without separate approval.

### Task 7: Full regression, review and separately approved rollout

**Files:** Tests touched above; approved spec/plan checkboxes and only narrowly necessary operating documentation.

**Interfaces:** No new APIs; this task proves all earlier contracts together.

- [x] Run `cargo test --offline --manifest-path apps/Cargo.toml` and `python3 -m unittest discover -s tools/tests -p 'test_*.py'`, record exact outcomes. Rust: 2,095 passed across 59 suite summaries, zero failed/ignored; Python: 77 passed. Full verbatim logs: `task-7-cargo.log`, `task-7-python.log` beside the Task 7 report. `make test` was not run; no installs or manifest changes.
- [x] Run `git diff --check` and inspect the complete six-commit feature diff and full branch stat; verify actual API callers/tests, strict unknown-TODO rejection and all five Review Focus test contracts. See Task 7 report for scope and evidence.
- [x] Controller: independent whole-branch review completed; its stale-snapshot lifecycle finding and migration-test minor were fixed in `b3159ae` and independently re-reviewed with no remaining findings. Final `cargo test --offline --manifest-path apps/Cargo.toml -- --test-threads=1`: 2,099 passed, zero failed; Python: `Ran 77 tests in 8.910s`, `OK`. The preceding parallel invocation failed one mocked FigCraft startup test after its 15-second timeout; no test was disabled or changed to bypass it. Final logs: `final-controller-rust-serial.log`, `final-controller-python.log`.
- [ ] Present what changed, what stayed unchanged, exact test failures/successes and every host-executed file change. Keep the original blocked report closed as investigated; do not falsely mark the new feature live merely because its design is approved.
- [ ] Only after Theo agrees to host steps: export reviewed feature commits with `git format-patch origin/main -o .outgoing`, then request host `cockpit apply <clone-root>` and subsequent `make -C ~/agent-cockpit test` through `ask-to-run`. No push, default-branch commit or destructive git action. Check clone baseline/other-session work before execution; never blindly rebase or overwrite it.
- [ ] Separately ask approval for the live acceptance test's exact current sessions, exact shared TODO and benign message content. Verify both session IDs afresh, not from report text. Exchange one note in each direction, complete a dedicated agreed test TODO to observe closure, and demonstrate a subsequent send is refused. Never finish the game-launcher TODO just to test cleanup, launch the game, or infer host/game permissions from link approval.

## Self-review and handoff

Spec coverage: exact-pair approvals and isolation (Tasks 1/3); durable closure and work ownership (1/2); bounded queue, retries, epochs and clarification (4); CLI/privacy/native behavior (5); host interaction and design gate (6); migrations, regressions and live consent (1/7). Every Review Focus item has a named test step above.

Execution selected: **subagent-driven**, sequential implementation with independent per-task reviews and final whole-feature review. Review gates and local verification are complete; host rollout and live acceptance remain unperformed. Any future export must isolate the feature range from baseline `814253c514a5996e3ad683304cb264d8a97bcb83`: `origin/main` is older and includes unrelated baseline work, so the historical export command above must not be used blindly.
