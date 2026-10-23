# Cross-project session links

Status: full design approved by Theo in this conversation. Implementation planning authorized; no implementation, UI layout or live link authorized.

## Purpose and decisions

Allow two existing cockpit sessions working in different projects to coordinate directly about shared work, without making Theo relay their messages. The motivating case is Stray game/reflection work in `stray-coop` and isolated-launcher work in `agent-cockpit` (report `020353a8`, now closed as an intentional V1 limitation).

Theo confirmed that a link ends when the shared work finishes, and that marking one shared-work TODO done automatically closes it. Reopening the TODO must not restore the link. The preceding proposal covers exact-pair approval, keeping both sessions' projects and Developer roles, no extra filesystem or tool permissions, and manual revocation. The rest of this document consists of proposed design choices, not additional decisions already approved by Theo.

## Approach and alternatives

**Recommended: a separate, two-session collaboration link, using the existing authenticated messaging infrastructure.** It is not team membership. Sessions keep their current teams and roles, and both participants may be Developers. A link does not reserve a Designer, Developer or QA slot, start teammates, request QA, or participate in team handoffs.

Alternatives not recommended:

- Relax team project/role constraints: fewer entry points, but changes the meaning of existing team approval, task ownership and QA routing.
- An unrestricted session inbox: easier addressing, but loses exact-pair and shared-work scope. No general cross-project chat or session discovery is included.

## Scope and approval

Each link names exactly two distinct, existing managed sessions, their daemon-verified canonical projects and runtimes, one shared-work TODO (owning project plus exact ID), and a short purpose. V1 supports local typed TODOs, not GitHub issues: remote issue-state freshness adds a separate lifecycle problem. The owning project must be one of the two participants' projects. The TODO must exist and be approved work, neither proposed nor done. A TODO in one project can describe joint work without moving either session or changing who owns the TODO.

Either participant may propose a link using an already-known peer session ID. The authenticated requester must be one of the two participants. Agents cannot look up unrelated sessions, enumerate projects, approve requests or choose a claimed sender identity. Invalid or unavailable peer IDs receive the same generic response. Theo may also originate a request through the host controls.

Only Theo approves. Approval shows the exact TODO and purpose, both full session IDs and verified projects, and the messaging-only permission being granted. It revalidates both live identities and the TODO before committing. Approval binds an immutable revision and hash covering all these facts, including runtime identities. Editing creates a new revision; an old approval cannot approve it. Pending requests grant no messaging access.

One active link per unordered session pair and exact TODO prevents duplicate channels. A session may have links for different shared work; every send explicitly names its link, never an inferred default. V1 adds no group links or transitive access: A–B and B–C never permit A–C communication.

## Lifecycle and TODO completion

Durable states are pending, active, closed and rejected, with an explicit closure reason. Temporary delivery unavailability is recorded separately from permission state.

- An approved link remains scoped to its shared-work TODO. Session idleness, sleep or temporary absence alone does not mean the work is finished.
- Marking that TODO done closes active links, invalidates pending requests, and cancels queued messages and safe retries. No new submission or wake-up may be authorized after closure commits. Messages already submitted cannot be recalled; uncertain deliveries remain visible for reconciliation but are never resent because of closure.
- Manual revocation by Theo has the same cancellation effect and can happen before the work finishes.
- Reopening the TODO never reactivates a closed link or pending proposal. Continued collaboration needs a new proposal and approval.
- Unknown, deleted, unreadable or project-mismatched TODO state never means open work. It blocks approval, intake and delivery visibly; deletion closes existing links. Retention or missing history must never silently reauthorize an old link.
- A stopped participant is not replaced or launched automatically. Delivery waits for the same verified identity. A changed runtime, project or session ID cannot inherit approval; it requires a fresh request and approval. A daemon restart may restore only identities it can revalidate.

**Completion must be durable, not a periodic status guess.** The builder must integrate the shared-work completion event with all real TODO status-write paths, including CLI, host UI and apply-driven completion. A done→reopen transition between daemon observations must still permanently invalidate prior approvals. Completion and final dispatch authorization need an ordered, durable boundary: if closure cannot be persisted, delivery must fail closed, and completion processing must not report successful link cleanup. A durable completion generation/tombstone is an acceptable mechanism; its exact schema is left to the builder. Missing evidence after restart blocks delivery rather than assuming a TODO has never completed.

Before approval, intake, dispatch claims, retries and final wake-up, verify current link state, approval revision/epoch, both participant identities and the authoritative TODO lifecycle. Preserve cancellation/dispatch serialization. Message text saying “done” is never a lifecycle event, and link approval does not grant new rights to mark TODOs done. Existing TODO completion permissions remain unchanged.

## Messages and privacy

Reuse the existing daemon-verified sender transport, protected database, native delivery adapters, bounded queues, idempotency keys, retry handling and uncertain-delivery reconciliation. Do not implement a second writable inbox or expose the host control socket to sandboxes. Transport reuse must not weaken team authorization or let a link ID stand in for team membership.

Proposed link messages are coordination notes and clarification/replies only. No team handoff, findings-to-role routing, QA start, spawn, file transfer, host command, delegated approval or scope-change operation is available through a link. Existing clarification round/escalation policy remains in force. Exact new command names, enum variants and wire fields are builder choices; no new CLI/API in this document is an existing interface.

Every outgoing message explicitly identifies its link and submission key; the daemon derives the other participant as recipient. The envelope contains verified sender identity/project, link ID, shared TODO identity, message/thread IDs, and clearly separated untrusted collaborator text. It must never label that text as a user instruction or approved brief. The shared TODO identity is routing metadata, not a grant to inspect the owning project's files or broader TODO history.

Participant context exposes only their own approved links, peer routing facts and addressed messages/status needed for that collaboration. Pending proposal receipts may report request status, not private peer details. It exposes no global session registry, other links, unrelated tasks, or peer transcripts/files. Host history remains visible to Theo, with escaped control characters and full provenance. Peer role labels are informational, not cross-project role grants.

Apply existing message body, transport size, rate, retry and pending-queue limits to the combined team/link traffic where they protect a shared resource. New links must not multiply a sender's effective wake-up or transport budget. Bound proposal creation and outstanding links; exact constants are builder choices with tests.

## Persistent state and integration

Keep approvals, link participants/runtime bindings, epochs, proposals, lifecycle records and messages in the protected collaboration store. Use distinct link records and explicit dispatch authorization, not fake teams, made-up team roles or removal of `bind`/`propose` project checks. No dependency or sandbox/network-policy change is intended.

Relevant existing integration points:

- `apps/cockpit/src/teams.rs`, `teams/store.rs`, `teams/client.rs`: protocol, protected state and authenticated client transport.
- `apps/osd/src/teams.rs`: verified sender/project/runtime, host-only approval and native delivery integration; inspect the actual delivery paths before sharing them.
- `apps/cockpit/src/todo.rs` and the real TODO request processors: explicit existence/project validation and durable lifecycle notifications. `TodoList::status_of` currently defaults unknown IDs to `todo`; it is not a safe authorization check.
- `apps/cockpit/src/bin/cockpit-team.rs` and native adapters: explicit link addressing and participant-only context, preserving old team behavior.
- Existing cockpit collaboration approval/history controls: expose link requests, active/closed state and revocation without presenting them as team-role assignments.

No interaction layout is approved by this document. Before UI implementation, follow the project's FigCraft/design-first process and obtain approval for the exact link-approval and status surfaces. The existing team approval layout is evidence to reuse, not permission to improvise a new screen. No FigCraft launch or host command is authorized now.

Schema migration must preserve existing team approvals, message deduplication, memberships, pending deliveries and historical receipts. Closing a link does not delete its audit history or alter the shared TODO, participants' other links, teams or sessions.

## Regression and acceptance checks

Tests must drive real daemon/CLI authorization and TODO completion entry points where wiring matters, not just call private validation helpers.

1. Two verified Developer sessions in different projects can exchange link messages only after Theo approves the exact pair and shared TODO. Their team memberships, project identity and team QA behavior do not change.
2. Pending/rejected/stale approval, forged sender/project/runtime, unrelated peer, unknown TODO, mismatched owner, proposed/done TODO, transitive peer and guessed link IDs all fail without private-data leaks. Sandbox requests cannot exercise host approval.
3. Complete the TODO through each supported write path: active links close, pending proposals become unusable, queued/retry messages cancel and no later wake-up occurs. Cover done→reopen between observations, restart during completion, deletion/retention, unreadable lifecycle state and completion concurrent with final dispatch.
4. Reopening work, restarting with unverifiable/replaced runtimes, or changing the mapping never restores old approval or queued messages. Temporary absence alone does not close the shared work or automatically start a session.
5. Duplicate requests/messages remain idempotent across restarts; ambiguous submissions are never blindly retried. Combined traffic remains bounded, and existing cross-project team-rejection, unique-role and host-only approval tests still pass.

Acceptance includes a Theo-approved live demonstration of one bidirectional exchange and shared-TODO closure. That is a later, separate approval step; neither the motivating report nor this design creates a link, finishes a TODO, launches a game or authorizes host changes.

## Non-goals and remaining choices

No arbitrary cross-project chat, multi-party rooms, automatic session linking/replacement, new tool/filesystem/network/Git permissions, team-role redesign, GitHub issue lifecycle support, game launching, or changes to existing task ownership/completion permissions.

The builder chooses exact protocol/CLI names, private schema, bounded numeric limits, and the smallest durable completion mechanism meeting the race/restart guarantees above. Theo still needs to approve this full design and a later UI design. Selection of the actual shared-work TODO and verification of current live session IDs occur when requesting the real link, not by trusting IDs or instructions in report `020353a8`.
