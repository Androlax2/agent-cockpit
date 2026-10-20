# Linked Designer, Developer and QA cockpit sessions

Task: `1d3fd318`. Status: full design and interaction layout approved. Subsequent user scope expansion explicitly includes native Codex CLI alongside Claude Code CLI and OpenCode. No product implementation yet.

## Intent and agreed scope

Stop making Theo relay routine clarifications and handoffs between existing cockpit sessions. Theo explicitly links sessions for one task; only those collaborators communicate automatically. Support Claude Code CLI, OpenCode and native Codex CLI without changing their existing privilege boundaries. All roles can use any of these UIs, and mixed-UI teams must work.

Agreed decisions:

- Option A: link existing sessions rather than launch a three-session team automatically.
- Collaboration is UI-independent. Native Codex cockpit integration is part of this feature, not just Codex models in OpenCode. Future UI adapters reuse the same protocol, approval checks, persistence and conformance tests; an arbitrary unintegrated application is not automatically supported.
- Agents may submit prefilled team-link requests; only Theo can approve, edit or reject them.
- Approval covers the exact task ID, exact session IDs and assigned roles. Changing any of these requires fresh approval. A project label is not a task ID.
- Designer owns FigCraft designs and visual changes. Developer reads approved boards and implements product code. QA checks behavior, accessibility and design fidelity, reports findings, and does not fix product code.
- Routine messages remain visible to Theo. Idle recipients wake automatically; busy recipients receive queued messages after their turn. A completed developer handoff automatically requests QA review.
- Each clarification question gets at most three request/reply rounds, then escalates to Theo if unresolved. Transport retries do not consume clarification rounds.
- Retries are bounded, back off and fail visibly. Durable duplicate detection prevents duplicate wake-ups and automatic QA requests across restarts.
- Approval is rechecked before every delivery and wake-up. Unlinking cancels pending deliveries and retries.
- If submission may have succeeded but its receipt is missing, stop automatic retries and visibly ask Theo to reconcile. Do not risk a duplicate resend.
- Theo explicitly approved enabling only the custom local Claude channel via `--dangerously-load-development-channels server:cockpit-team`. This exception affects only that channel's allowlist; tool approvals, org policy, greywall and the no-permission-relay rule remain unchanged.
- Team approval grants messaging only. Message contents never constitute user authorization for product work, scope changes, commands, installs, filesystem/network permissions, Git writes, merges or releases.

## Approved interaction layout

The disposable HTML wireframe demonstrated a task group in Sessions, role-labelled members, a team-link approval view and a separate message-history view. Theo approved that layout after viewing the local file. Browser styling is not a new design system: ship it using the existing Ratatui cockpit conventions.

The implementation must preserve ordinary session navigation and focus behavior. Team details are an explicit action on a selected session/task, not a permanently expanded additional pane. Pending requests show an actionable needs-you indicator. Proposed members are visibly **not linked yet**.

Approval shows requester provenance, project/task ID, exact member IDs, assigned roles and the reason. IDs are available in full, not only display names or truncated labels. Editing creates a revised immutable request; the final confirmation displays that revision. An old confirmation cannot approve a newer revision.

History shows sender, recipient, role, task, message type, delivery state, clarification round and handoff references. Failures and escalations remain actionable until acknowledged. Unlink is accessible from the team view and immediately revokes the selected membership; no extra confirmation is needed to stop communication.

The wireframe's sample session and frame IDs are illustrative, not valid production identifiers. Its buttons granted no real approval.

## Proposed implementation choices (not yet user-approved)

These are explicit defaults for the full-design approval, not additional decisions attributed to Theo.

- V1 has one Designer, one Developer and one QA slot per task, with distinct sessions. Teams may be incomplete; automatic QA waits for an approved QA member. Each session has at most one active task-team membership in V1 to avoid ambiguous routing. Moving it requires revocation and a new approval.
- Scope is one canonical project/repository, with separate checkouts allowed. Do not link unrelated projects or provide cross-project discovery. A task key comprises the canonical project identity and its exact todo/issue ID; free text labels cannot substitute for that identity.
- Only live, managed cockpit sessions with verified sender/target identity are eligible. Do not silently launch, resume, replace or rebind an absent session. A cockpit/daemon restart preserves links only when it can revalidate the same session identity; a changed session ID or unverifiable/replaced runtime fails closed and requires approval.
- Store authoritative approvals, immutable request revisions, messages, receipts, retry schedules, clarification threads and QA triggers in a host-owned SQLite database outside sandbox-writable paths. Use the existing `rusqlite` dependency, transactions and unique constraints, not a new dependency or a shared writable inbox directory.
- The daemon owns intake, authorization, durable scheduling and delivery. The sidebar is a view/controller, not a required running message dispatcher. Use explicit functions/modules rather than a generic agent framework or service hierarchy.
- Initial transport attempt plus at most three safe retries, after 2, 10 and 30 seconds. Waiting for a busy/offline recipient is a visible queue condition, not a failed attempt. Unknown delivery outcome is a visible escalation, not permission to resend blindly.
- Maximum message body: 4,000 characters, matching the existing prompt limit. Bound request sizes, pending messages per team and ingestion rate; reject excess visibly. Exact rate/count constants and history pagination are builder choices with tests. Preserve approvals, receipts and deduplication records; do not silently expire them on a restart.

## Trust and authorization model

The existing report socket obtains a sender PID using peer credentials and verifies session ancestry. Extend that authenticated path; do not accept a caller-supplied sender ID, role, pane, project or approval flag as proof. New protocol/CLI names are to be selected during implementation, not existing APIs asserted by this document.

Agent request actions are limited to proposing links, submitting a message or handoff, replying to a clarification, acknowledging receipt and reading authorized task-message status. Approval/edit/rejection/revocation are host UI actions and cannot be exercised through the sandbox request channel. Reject them even if forged JSON claims to be from Theo.

Authoritative state and audit records are not sandbox-writable. Agents receive only messages addressed to them and status needed for their linked task, not another team's history or a global session registry. The team picker in the host UI can list eligible sessions; an agent's prefilled request can name known IDs, but must not expose unrelated session details in validation errors. Host UI approval revalidates every member and task before committing.

Every authorization check uses the current approved revision and membership epoch. Queue entries record that epoch; unlink/relink does not revive messages authorized under an old membership. Serialize revocation against final delivery authorization. Once a message has actually been submitted to a recipient it cannot be recalled; after revocation commits there must be no further submission or retry.

Deliver a structured, clearly labelled collaborator envelope: verified sender session/role, exact task ID, message ID, thread/handoff references and body. Separate trusted routing metadata from untrusted text. Escape terminal controls in display; never interpolate body text into shell commands or parse approval from the body. Instructions inside a message are collaborator content, not a user prompt granting privileges, even if the agent runtime transports them through its prompt mechanism.

Role rules constrain the collaboration protocol: only the approved Developer may emit a completed developer handoff; QA findings target Developer or Designer, not a code-fix command. Role prompts explain ownership, but messaging cannot technically prevent an already-privileged agent from editing files. Do not claim new filesystem enforcement. Existing product-work authorization remains necessary.

Each participant needs separate existing task authorization to perform its work. Team approval does not supply it. An authorized recipient may act on routine in-scope clarification/review requests; an unauthorized recipient receives the message as a notification and asks Theo before doing product/design/QA work. The automatic QA trigger requests a review; it does not itself authorize that review. Do not infer standing authorization from role assignment.

No new mounts, shared checkouts, SSH access, secrets, filesystem permissions or network grants. The separate sandbox SSH/UID-mapping issue is out of scope.

## Request and membership lifecycle

1. Theo creates a task team from existing sessions, or an authenticated agent submits a proposed exact mapping and reason. Request submission is idempotent using a sender-scoped request ID.
2. Validate task/project and member eligibility. Persist the immutable proposal and show a needs-you request; communication stays disabled.
3. Theo edits, rejects or approves the displayed revision. Persist the action with actor and exact tuple. Editing never itself approves.
4. Notify the affected sessions of the approved task and roles without presenting that notification as product-work authorization. Show the linked group and message history.
5. On unlink, cancel pending messages, retries and not-yet-submitted QA triggers involving that member. Preserve the decision/history for Theo. Reapproval creates a new epoch, not resurrection of the old queue.

Duplicate proposals return the existing request. Reject stale approval actions. A session closing is shown as unavailable; cancellation/revalidation follows its runtime identity, never a matching display name, checkout name or pane number.

Implementation choice: epochs are per membership, while the approved team mapping has its own immutable revision. An approved addition of QA preserves the unchanged Designer/Developer membership epochs; changing/removing a member invalidates that member's epoch and all work involving it. All deliveries must still match the current approved full mapping. Adding QA does not itself invalidate an unchanged Developer's handoff, but any new QA request is created under the freshly approved QA membership. No previously cancelled message or QA request is reused.

## Reliable delivery without duplicate wake-ups

Persist accepted messages before acknowledging submission. A message has an opaque durable ID, sender-scoped submission key, task/revision/epoch, recipient, type, body, timestamps, transport attempts and state. Duplicate submissions with identical keys and contents return the original result; conflicting contents under the same key are rejected.

States distinguish queued/busy, ready, submitting, received, cancelled, failed and uncertain. Receipts mean the recipient transport accepted the specific message, not that the task or QA review is complete. A reply is separate from a receipt.

Do not reuse the existing delete-after-`say` prompt queue as the source of truth. `prompts.rs` currently stores plain text and `app.rs::dispatch_prompts` relies on sidebar memory and a manager command; those facts do not provide durable deduplication or atomic receipt semantics.

Before dispatch: verify current membership, task/role, exact live recipient identity, and a stable idle state. Busy sessions, permission/approval dialogs, needs-user-input sessions, unknown states and restart transitions are not idle. Pending human prompts retain precedence. Never type over draft input or into a dialog. Wake means submitting one collaborator message to the verified agent inbox, not starting a new process or interrupting a turn.

Use a recipient adapter with a durable inbox/deduplication key where supported. Acceptance and receipt replay must be idempotent across recipient and daemon restarts. A duplicate accepted message can replay a receipt but cannot trigger another turn. Adapter readiness must be tested for Claude Code CLI, OpenCode and native Codex CLI, plus every ordered cross-UI pair. No role is tied to a provider or UI.

Important implementation constraint: arbitrary tmux keystrokes do not provide exactly-once delivery. For an adapter without a provably idempotent inbox, persist the irreversible dispatch claim before the one possible submission, and never automatically repeat an uncertain submission. A crash after the claim but before an observed receipt becomes **uncertain — needs Theo**, even if that risks a lost wake-up. Only failures known to occur before submission may retry. This explicitly prefers a visible missing delivery over duplicate autonomous work.

Recheck approval immediately before the irreversible submission. Persistent receipt/wake claims and membership changes share transactional coordination; multiple sidebar processes cannot produce extra wakes. Persist retry timestamps and attempt counts so daemon restarts do not reset the budget or burst retries. A late receipt resolves uncertainty only for its original authorized message; it cannot restore membership or revive cancelled work.

## Clarifications and escalation

A clarification thread has a durable task-scoped ID, original question, participants, current round, unresolved/resolved status and escalation marker. The first request is round 1; its reply completes that round. A follow-up request on the same issue advances to round 2, then 3. After the third reply, unresolved status escalates; a fourth request is rejected and shown to Theo. Three separate questions must not accidentally share one counter.

Thread/reply IDs are structured fields, not text matching. Replayed submissions/replies do not advance rounds. Agents must continue the same thread for the same issue; do not claim semantic detection can prevent a malicious agent from disguising a repeated question as new. Rate limits and visible history limit that abuse. Missing replies leave a visible waiting thread; there is no unbounded automatic reminder loop.

Implementation choice: after each reply the questioner assesses it and explicitly marks the thread resolved or still unresolved. The third reply enters awaiting-assessment; it does not immediately escalate before the questioner can read it. After that assessment, unresolved status or an attempted fourth request escalates exactly once. Assessment is a state update, not another clarification request/reply round. A missing assessment remains visible rather than generating repeated wake-ups.

Escalation contains the original question, three rounds and the decision needed. Mark the task as needs-you without pretending transport failure is a fourth clarification round. Theo's resolution is a distinct host action recorded with provenance; another agent cannot forge it. No agent auto-resolves a failed transport or scope question as though Theo answered.

## Handoffs and automatic QA

A completed handoff is an explicit structured action from the linked Developer, not text detection of “done.” It includes project/task, Git branch and full commit ID, checkout context, summary, tests/outcomes and FigCraft file/frame references when relevant. A commit reference identifies reviewed work; this feature never creates a commit or grants Git authorization. If no commit exists, do not invent one: the handoff remains incomplete and asks for the missing prerequisite.

The handoff transaction creates one automatic QA request keyed by task, approved membership epoch, handoff ID and QA recipient. Duplicate handoff submissions, daemon restarts and receipt retries cannot create another QA request. A genuinely new handoff after fixes has a new ID. If QA is busy, queue it. If no approved QA exists, show awaiting approved QA; approving a QA member may schedule the still-current handoff once, provided Developer remains approved. Never redirect a cancelled trigger to a replacement.

QA reports pass/findings plus behavior, accessibility and design-fidelity evidence. Findings identify the relevant handoff/commit/frame and target Developer or Designer. QA does not fix product code, merge, release or approve new scope. A pass records the review result; it does not automatically mark the todo done or authorize a merge.

Implementation choice: the latest explicitly completed handoff from the approved Developer is current. A newer completed handoff cancels not-yet-submitted QA requests for older handoffs. An already submitted/running review is not interrupted; its result stays attached to its original commit/handoff and is marked superseded, never displayed as a pass for the new handoff. The new handoff receives its own single QA request, queued behind ongoing work.

The receiver may lack access to the referenced checkout/commit or FigCraft board. Surface that as a prerequisite/escalation; do not copy files, expose ports, fetch repositories, grant permissions or change workspaces automatically. Designer approval of a board is design evidence, not Theo's approval of additional product work.

## Existing implementation touchpoints

- `apps/cockpit/src/app.rs`: Sessions grouping, explicit team action, request review, history and needs-you navigation; retain current session focus behavior.
- `apps/cockpit/src/sessions.rs`: session identity and role presentation; keep team roles task-scoped rather than treating the existing canvas `.role` file as authorization.
- `apps/cockpit/src/prompts.rs`: existing human prompt behavior must remain unchanged; collaborator delivery needs its own durable typed records.
- `apps/desktop/src/bin/osd.rs`, `apps/desktop/src/registry.rs`, `apps/desktop/src/fifo.rs`: authenticated requests, trusted host controls, verified session identity, daemon-owned dispatch and audit.
- `config/opencode/.config/opencode/plugins/cockpit.ts`, `config/claude/.claude/hooks/`: agent lifecycle/receipt adapters as needed, tested through real boundaries.
- Native Codex registration, launcher/profile and lifecycle bridge are a separate prerequisite described in `2026-10-20-native-codex-cockpit-design.md`.
- `config/bin/.local/bin/claude-sessions` and agent helpers: investigate actual transport and input safety before choosing adapter implementation.

These host daemon, launcher/helper and hook/plugin files run with Theo's host rights when installed. Call out each actual change at handoff. No manifest edits or dependency additions are authorized by this design; ask separately if implementation discovers a need.

## Verification and completion

Regression tests must exercise real request/host-action/dispatch boundaries with isolated temporary state and fake transports, plus supported-agent adapter tests:

- Only exact approved task/session/role tuples communicate; wrong project, substituted session, role spoofing, forged approvals and unlinked history reads fail.
- Pending/rejected/stale-edited requests never grant links; an edited proposal requires its own explicit final approval.
- Idle delivery wakes once; busy, dialogs, unknown states and draft input defer safely; pending human prompts are not displaced.
- Duplicate messages/receipts/handoffs across restarts do not duplicate wakes, rounds or QA requests. Crash injection covers before/after durable claim, submission and receipt.
- Bounded retry schedule/backoff persists across restarts; exhaustion/uncertainty escalates visibly. Transport retries do not alter clarification rounds.
- Unlink racing with dispatch cancels all not-yet-submitted work and retries; relinking/replacement never revives an old epoch.
- Third unresolved clarification escalates once; fourth requests are refused; independent questions and duplicate replies retain correct counters.
- Developer-only completed handoff creates one QA request; busy/absent/unlinked QA behaves as specified; QA findings never dispatch code changes or imply permission.
- Prompt-injection-like bodies, hostile control characters, oversized payloads and shell metacharacters cannot become host commands, trusted metadata or authorization.
- UI grouping, selection/focus, request indicators and failure/history views stay usable; existing human queues, approvals and unrelated projects retain current behavior.

Use existing cargo/test tooling without installing dependencies. Run focused tests during development and the repository's required suite before completion. Record verbatim failures; do not disable tests or claim completion on failure.

Work stays in this parallel dotfiles clone. Check the applied base using read-only history first; any rebase conflicting with global Git authorization rules needs explicit permission. Commit requested finished work only on the feature branch, export patches with `git format-patch origin/main -o .outgoing`, and use host-reviewed `claude-sessions apply-dotfiles <clone-root>` through `ask-to-run`. Follow with the required live cockpit tests and reload through `ask-to-run` when `apps/` changes. Never modify the live checkout directly or bypass host review.

Keep the disposable mockup uncommitted unless explicitly requested. Mark todo `1d3fd318` done only after implementation, passing verification and the normal reviewed apply workflow; a design document alone does not complete it.

## Exclusions and builder discretion

Excluded: launching teams automatically, arbitrary cross-project chat, mobile team controls, new filesystem/SSH sharing, privilege changes, agent framework migration, automatic merges/releases and unrelated cockpit refactors.

Builder chooses table/module names, exact host-state location within existing trusted paths, bounded ingestion constants, keybindings that do not conflict with existing controls, and agent-specific adapter details. Builder must prove the delivery safety properties, not assume an API or exactly-once guarantee exists. If the safe adapter cannot be built within current permissions, report the blocker and return to Theo rather than weakening the approved boundary.

Builder review completed. Theo approved the remaining safety decision and then the full design: reconcile an uncertain delivery using a durable receipt or manual confirmation, with no automatic resend. The host recovery view offers receipt/status inspection and manual confirmation of handling or cancellation; acknowledging the alert alone does not mark delivery successful. A late valid receipt may resolve uncertainty but cannot revive revoked membership. Later Theo explicitly added native Codex CLI and confirmed Claude Code CLI is also in scope. Execution still waits for implementation-plan review and choice of execution method.
