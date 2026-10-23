# One global dispatcher chief

Todo: `5fa54c26`. Status: full design approved by Theo in this conversation;
implementation waits for plan review, execution-method selection and the UI design gate.

## Intent

Theo has many agents running across projects and cannot follow every session. One chief
coordinates the existing agents, keeps approved work moving, and brings Theo only decisions
that need him. Keeping agents busy means useful authorized work, not generating busywork.

## Decisions Theo made

- Dispatcher, not merely an advisor and not an autonomous manager.
- One chief across all cockpit projects, not one per project.
- It may assign and reprioritize already-approved tasks without asking each time.
- New scope and recruitment require Theo's approval. Sensitive actions retain their approvals.
- Notifications only for decisions Theo needs to make; progress summaries on demand.
- Advance automatically once required test, QA and apply evidence is recorded, not merely
  because an agent says "done". Missing or uncertain evidence comes to Theo rather than
  asking him to confirm every successfully finished task.
- Admission adjustment approved during implementation: start automatic assignments with
  OpenCode's native queued inbox and live-test draft preservation before activation. Monitor
  all supported managed agents, but keep Claude/Codex non-dispatching until their native
  admission paths are verified. OpenCode need not prove its composer is empty: it must leave
  drafts untouched and queue without interrupting a running turn or displacing human input.
- Availability telemetry adjustment approved during implementation: the OpenCode native plugin
  may report raw activity/inbox facts over existing runtime-authenticated IPC. The daemon
  derives eligibility; reports do not grant authority. Runtime authentication establishes the
  source, not independent truth of the reports. False availability could queue work while
  busy, not interrupt it. Scope, single ownership, permissions and live-verification gates
  remain host-enforced; no new host API exposure, sandbox grants or readiness override.
- UI handoff approved in the Designer session: use the separate Chief view and exact interaction
  contract in `2026-10-23-chief-controls-interaction-design.md` (F1–F7 plus follow-up choices).
  Agents prepare immutable proposed exact briefs for host review; proposals grant nothing.
  Host attestation is restricted to gates already approved with source `host_attestation`, not
  a substitute for host-run, QA or reviewed-apply evidence. Unsupported sources require revised
  explicit work approval rather than an evidence-source waiver.

Everything below is a proposed implementation choice unless identified above.

## Responsibilities and limits

The chief observes managed sessions across projects, identifies idle capacity, coordinates
dependencies and handoffs, avoids duplicate assignments, and consolidates blockers. It does
not edit product code, design interfaces, perform QA, merge, deploy, or declare another
agent's work complete. Existing Designer/Developer/QA ownership stays intact.

The host enforces authority; a chief prompt is not a security boundary. Chief messages remain
collaborator content. They cannot authorize installs, commands, Git writes, filesystem or
network grants, or a new task. A worker retains its own instructions and tool approvals.

## Explicit approved-work queue

A todo's existence, status, priority, or agent-authored text does not prove authorization.
Introduce a host-owned dispatch approval record: canonical project and task identity, exact
versioned brief, prerequisites, permitted worker role, and any existing authorization reference.
Theo explicitly approves the bounded work described by that record. He may approve a batch of
exact briefs once; there is no approval of an unspecified future backlog.

This approval authorizes beginning the described work, not bypassing the worker's existing
permission rules. The chief may choose the ordering and eligible worker within these bounds.
Any changed brief, expanded scope, new project, or additional privilege needs fresh approval.
Agent suggestions remain proposals until Theo approves them. Revocation cancels pending
assignments; running work receives a stop-at-safe-boundary notice, not a false claim it was
rolled back. Already completed effects cannot be recalled.

If there is no approved next task, the agent stays idle. The chief asks once for a specific
queue approval or recruitment decision when useful, rather than starting a reminder loop.

## Chief lifecycle

Theo explicitly enables the chief and selects one verified managed session. This is a separate
global coordination grant, not a fourth task-team role. There is at most one active chief.
The selected session cannot simultaneously be a product worker in a team.

Enabling shows that this chief will receive cross-project operational status and bounded task
briefs. It does not mount other projects or grant access to their files, credentials, full
transcripts, command output, or raw approval payloads. Global visibility is intentionally an
exception to ordinary agents' task-scoped discovery, restricted to the approved chief.

Pause stops new chief actions and automatic wake-ups; accepted worker assignments continue.
Disable revokes the chief's observation and dispatch grant and cancels its unsent messages.
Workers and existing teams continue independently. Replacement requires explicit approval;
the same session name, pane, or project never proves identity. A missing or unverifiable chief
is shown as unavailable, without silently starting another agent. Restart revalidates its
verified runtime and grant before resuming.

## Observation, scheduling, and delivery

Reuse the daemon's verified session identities and status sources. Give the chief bounded,
paginated structured snapshots and event deltas: project/task identity, role, operational
state, authorized brief, assignment ownership, dependency state, handoff/QA status, and a
bounded worker-supplied blocker summary. Mark missing and stale reports explicitly. Agent
claims such as "done" or "approved" are not trusted host facts.

Use event-driven, coalesced wake-ups for material state changes, not a periodic LLM polling
loop. The daemon may use bounded timers for delivery deadlines and stale-state detection;
these are not invitations for the chief to repeatedly wake workers. Chief turns are bounded
by an event budget and cooldown. A failed or exhausted chief action remains visible without
an automatic reasoning/retry loop. Progress summaries are generated only when requested.

Existing work is observed, not silently reassigned or cancelled. A busy worker receives no
new task. Reprioritization changes only the queued order, not a task already executing.
Safe idle means a verified, available session at a task boundary, with no open assignment,
pending human question/approval, unresolved delivery, or unknown state. For the approved
OpenCode queued-inbox path, a preserved composer draft does not itself prohibit admission;
an idle-state race must queue the input safely, not interrupt execution or type into the
composer. This replaces the earlier strict requirement to prove an empty composer before
OpenCode admission. Pending human inbox messages and user focus still take precedence.
Sleeping sessions are unavailable in V1; do not bypass their current wake/team limitations.

Assignments are project-bound: an existing worker must already have an eligible sandbox and
role for that canonical project. Never change its checkout, project, role, or sandbox access
to fill spare capacity. A team member is eligible only where the approved task/team mapping
allows the work. Reusing it for another team's task requires the existing host membership
approval, not an implicit move by the chief. A standalone eligible worker can accept an
approved standalone task without joining a team.

The host atomically claims each dispatchable task and worker. Duplicate proposals, concurrent
chief turns, restarts, and an already-owned todo cannot create a second owner. Recheck the
chief grant, approval revision, dependencies, task ownership, verified worker identity, project,
and safe-idle state immediately before delivery. The envelope identifies the exact host-approved
brief and its revision separately from chief-authored commentary. The worker acknowledges
acceptance; transport receipt alone is not work acceptance or completion.

Use the established safe-inbox adapter behavior for supported Claude, OpenCode, and Codex
sessions, with durable deduplication, bounded known-safe retries, and a visible uncertain
state when acceptance cannot be established. Never blindly resend or reassign uncertain
work. Delivery to busy sessions waits; it does not interrupt turns or displace human input.
Use the current authenticated daemon request channel, not simulated keystrokes or a shared
writable queue. Do not expose the ordinary host-control socket to the chief.

Automatic completion rule (Theo approved): each approved brief revision records its required
completion gates, including applicable tests, QA and apply milestones. The host records the
evidence source, result and exact task revision/commit for each gate; the chief cannot invent
evidence or waive a gate. Test evidence needs a structured result from an approved execution
or verification path, not text extracted from the worker's completion message. QA evidence
must reference the current reviewed handoff/commit; apply evidence must reference that same
approved work. A gate can be not applicable only as part of the host-approved work record,
never by the chief's choice after a failure. The builder must verify the available evidence
paths and surface any missing structured source rather than treating prose as proof.

Once all required gates pass, the host automatically records completion for that revision,
releases its task claim and makes its exact dependent tasks eligible. No extra confirmation
from Theo is needed. The worker separately needs verified safe-idle state before its next
assignment. Missing, failed, stale or uncertain evidence leaves completion blocked and creates
one actionable decision for Theo. A receipt, worker claim or independently marked-done todo
cannot satisfy these gates. Later invalidation of evidence blocks unsent dependent work and
raises a decision about already-started dependents, without claiming to undo their effects.

Uncertain delivery uses host-only reconciliation, following existing team recovery semantics.
A durable receipt may establish transport acceptance, but not task completion. Theo may
confirm handling and keep the claim, or cancel after establishing that the old worker will
not execute it. Cancellation after possible acceptance requires a confirmed safe stop; an
unresponsive or unknown worker keeps ownership blocked. A newly approved dispatch is allowed
only after that resolution, with a new identity/key; it is not a blind retry. The chief can
assemble the decision evidence but cannot resolve uncertainty or free an uncertain claim.

## Coordination and decisions

The chief may request bounded status/clarification from workers and route routine in-scope
coordination across projects. The daemon checks approved task relevance and recipient
identity. This does not grant arbitrary worker-to-worker cross-project discovery or history.
Dependency links identify exact project/task revisions; they do not grant access to another
project's files. A worker reports inability to proceed rather than acquiring missing access.

Keep one durable decision per underlying blocked action/revision. A decision includes the
project/task, concise blocker, chief's recommendation, exact choice requested, and source
references. New evidence updates that decision instead of creating repeated notifications.
The chief cannot resolve one on Theo's behalf or forge his reply. Human waiting states are
not answered, suppressed, or overwritten automatically.

Existing security/tool approval prompts remain directly actionable and unchanged. The chief
can point Theo to them, never approve them or consume private payloads to summarize them.
Recruitment and scope requests use the existing approval path wherever applicable. Only a
specific need for Theo triggers a notification; ordinary progress, idle states, and successful
handoffs do not. Finishing a task still requires the existing evidence, QA, todo, and apply rules.

On-demand summary: completed work with evidence references, active work, unresolved decisions,
and idle capacity. Keep it short and distinguish worker claims from verified test/QA/apply
results. "Idle: no approved work" is an acceptable outcome.

## Persistence and trust boundaries

Store chief grants/epochs, approved brief revisions, assignment claims and acknowledgements,
dependency references, decision deduplication, and dispatch history in host-owned durable
state inaccessible for sandbox writes. Reuse current SQLite/daemon patterns without a new
dependency or generic orchestration framework.

Revalidate grants at every observation and action boundary. Revocation races with dispatch
must prevent any not-yet-submitted action. Old epochs cannot resume after replacement.
Treat worker summaries, task bodies, and chief prose as untrusted content: bounded, escaped,
never interpreted as shell, routing metadata, approval, or authorization. Paginate global
data and cap queue sizes, events per turn, message sizes, and outstanding decisions.

## Existing code to extend

- `apps/cockpit/src/teams.rs`: existing roles, actions, identity and message envelopes; do not
  shoehorn the chief into task membership or weaken normal `Context` isolation.
- `apps/cockpit/src/teams/store.rs` and `client.rs`: durable task collaboration and agent requests.
- `apps/osd/src/teams.rs` and `registry.rs`: authenticated request handling, host controls,
  session identity and lifecycle. Keep privileged decisions host-side.
- `apps/cockpit/src/teams/view.rs`: current host approval/history conventions to reuse where useful.

Builder verifies exact adapter, todo ownership and host UI integration paths before planning;
these references are existing touchpoints, not claims that a chief API already exists.

## UI and implementation gates

This is a substantial feature: written-design approval, then an implementation plan. The
written design is approved; implementation has not started. Any new cockpit interaction needs an
approved design following the repository's design workflow; no invented UI from this prose.
Offer a visual companion when choosing those interactions. Recruit a Designer, with Theo's
approval, if the existing design evidence does not cover the new controls.

Changes to daemon, adapters, hooks, launcher/helpers or Makefiles run with host rights when
applied and must be individually called out. Do not modify dependency manifests without asking.
Requested completed code follows the project's feature-branch, reviewed apply and test workflow.
This design alone does not complete todo `5fa54c26`.

## Verification requirements

Test through real agent/host request and dispatch boundaries, isolated host state and fake
transports; do not claim a prompt enforces authorization.

- Only the selected verified chief can observe global status or propose dispatch; ordinary
  agents retain isolation. Disable/replacement/restart invalidates old identities and epochs.
- Unapproved todos, changed briefs, blocked dependencies, wrong projects/roles, already-owned
  tasks, stale status, sleeping sessions and busy/human-waiting workers never dispatch.
- Approved standalone assignment works without team membership; existing teams cannot be
  silently moved or bypassed. Human input and permission prompts retain precedence.
- Concurrency, duplicates, crash/restart and uncertain receipts cannot duplicate work or
  release ownership prematurely; revocation prevents unsent actions and preserves audit.
- Collaborator text cannot forge approvals, trusted metadata, host commands or scope. Global
  status never leaks transcripts, secrets, arbitrary files or raw approval payloads.
- Material events coalesce; event/cooldown limits prevent an LLM wake loop; idle/no-work does
  not produce busywork. Repeated blockers do not repeat notifications. Summaries are on demand.
- Existing mixed-agent delivery, team approvals, recruitment, handoffs, QA and ordinary
  non-chief sessions remain functional. Check all supported adapters plus live behavior after
  the reviewed apply, and run `make test` with failures reported verbatim.

## Left to the builder

Exact module/table names, protocol action names, conservative bounded limits, host-state
placement within existing trusted directories, and compatible test organization. Settle UI
details only through the required design approval, not as incidental implementation choices.

## Builder check

A fresh read-only reviewer identified completion/dependency milestones and uncertain-assignment
recovery as the remaining planning gaps. Theo subsequently approved automatic advancement
from required recorded test/QA/apply evidence, instead of manual confirmation of every task.
The revision-bound evidence rules above describe the proposed enforcement of that decision.
Uncertain assignments still require host-only reconciliation without automatic reassignment.
A second read-only builder check found nothing important left to resolve before planning.
Theo approved the full design after that check. No implementation or tests have run.

### Final-fix recovery approval

Theo subsequently approved using exact legacy launch tokens and exact coordination/stop/wake
message IDs in the existing host Recovery flow. Launches may have no known worker identity.
Explicit safe-stop confirmation concerns that exact launch; no worker identity is fabricated.
Handled coordination retains assignment ownership. Handled wakes are transport tombstones only,
never native-admission proof or a mutation allowance. See the interaction design's final-fix
extension for the retained note, unchecked assertion and frozen-review/confirmation safeguards.
