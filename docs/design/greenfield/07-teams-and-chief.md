# 07. Teams and Chief

Status: proposed; keep/drop list final after the 2026-10-31 read-only review, awaiting
Theo's confirmation.

## Why revisit

Audit evidence (2026-10-30): whole-roster availability records on every change, no-change
LLM wakes, messages expiring before delivery, native receipt treated as completion, and Chief
shipped off by default pending live verification. Porting the current model into new crates
would preserve those costs.

## Keep

- Roles Designer / Developer / QA; any provider can hold any role.
- A team message is collaborator content, never authority.
- Theo's approval to link existing sessions; automatic recruitment within size and rate limits.
- Handoffs bound to branch, full commit id, checkout and test outcome; QA on a fresh copy at
  exactly that commit; a newer handoff cancels the unsent review of the older one.
- Never resend a delivery whose outcome is uncertain; surface it for a human decision.
- Chief cannot also be a product worker; Chief cannot waive gates or widen scope.

## Change

| Today | Target |
| --- | --- |
| Separate linking subsystem with its own inbox, retries, clarification threads | A team is a **task-scoped workflow instance**: roles are steps, messages are task events, clarifications are task questions |
| Chief observes rosters and dispatches | Chief **decides**: decomposition, priorities, blockers needing judgment. Scheduling and admission place work deterministically |
| Per-feature delivery mechanics | One durable messaging path (receipts, cursors, idempotent retries) shared by teams, Chief and clients |
| Agents woken for availability | Woken only for actionable change or due retry; capacity changes dispatch without a model turn |
| Proposals, gates, attestations modelled inside Chief | Gates live in verification; approvals bind exact revisions like everywhere else |
| Full snapshots to Chief | Per-entity revision deltas plus bounded recovery snapshot (report 04) |

Result: `cockpit-teams` and `cockpit-chief` are thin behaviors over tasks, workflows,
messaging and scheduling, not parallel systems.

## Rules

- Chief proposes work; `cockpit-scheduling` places it; `cockpit-authorization` permits it.
  Scheduling must not depend on Chief.
- A Chief wake records why it happened and whether it produced a useful action
  (delivered decision, verified handoff, resolved blocker). Availability-only reports count
  for nothing.
- Measures: tokens per verified result, useful events per wake, stalled-task age, zero wakes
  for identical observation revisions in deterministic replay.

## Keep/drop after the implementation review (2026-10-31, read-only)

Paths: `S`=`apps/crates/cockpit-core/src/teams/store.rs`, `C`=`…/chief/store.rs`,
`D`=`apps/crates/cockpit-daemon/src/{teams,chief}.rs`, `B`=`…/chief/bundle.rs`.

### Keep as product behavior (re-implemented on the new owners)

- Roles, ≤1 session per role, one team per session; approval bound to the exact
  (task, members, agent, runtime) hash and revision, drift = conflict (`S:787-906`).
- Recruit with host-chosen agent and directory, never from the request (`D teams:607-617`);
  QA on a fresh clone at the handoff commit with the last findings, hand-linked QA never
  replaced (`S:1685-1773`); ≤3 QA spawns per task per 10 min.
- Message kinds with role rules (only Developer hands off, QA findings only to Dev/Designer,
  only the asker assesses) and structured actions that cannot be smuggled through `send`
  (`S:1361-1418`); submission-key idempotency with content fingerprint everywhere.
- Clarification threads ≤3 rounds, then one escalation Theo resolves (`S:1891-2099`).
- Uncertain delivery is never resent; only a confirmed safe stop releases (`S:750-759`,
  `C:666-673`); a session waiting for Theo is never woken (`D teams:625`).
- Chief: pick/pause/resume/recover bound to the exact runtime; Chief is never a worker and
  cannot wake itself (`C:234-242`, `C:2797-2802`); answers and forms within grace periods;
  host-appended boundary text forbidden in agent text (`protocol chief.rs:360-371`);
  escalations by category; `start_session`/`give_todo`/`message`/`inspect` with their rate
  caps; habits with provenance, delete/undo/rescope (`C:1764-1991`); the masked stop bundle
  with its byte caps and secret redaction (`B:18-59`); digest and answer counts.
- Every agent text tagged as data, never instructions (`C:4893`, `C:3065`).

### Replace by the shared owners

| Today | New owner |
| --- | --- |
| `teams`, `members`, `team_qa`, `link_work`, `link_participants` | task workflow instance + participants (06) |
| `proposals`, `host_actions`, `chief_actions`, `chief_grants`, `chief_startup`, `chief_admission_verifications`, link proposals | approvals bound to revisions (04, 17) |
| `messages`, `attempts`, `threads`, `handoffs`, `chief_inbox`, `chief_delivery_attempts`, `inbox_turns`, `delivery_recipients`, `chief_summary_requests` | one messaging outbox with receipts and cursors (04) |
| `spawns`, `qa_exits`, `chief_session_starts`, `scheduling_reservations`, `legacy_launches`, `chief_supervised_tasks` | scheduling + admission + task state (10) |
| `sleep_permits`, spawn/resume tokens read from `/proc` | session identity that survives process exit (04) + launch tokens (17) |
| `chief_attention` | decision notification state (19) |

### Delete outright

`chief_coordination`, `chief_turns`, `chief_context_snapshots` (+generation and 21 triggers),
`chief_observations`, native telemetry and `chief_focus` side channels, the 10 dead dispatcher
tables created-then-dropped on every fresh database, `_v13` rebuilds, 2/10/30 s adapter
retry schedule, three overlapping idle-turn gates, the 1 s live-session cache, the
`ResolvedTodos` mtime cache, polling adapters (OpenCode 3 s, Claude watch 3 s, Codex 1 s).
Session links as a separate subsystem: a link is a task message with scheduling limits.

### Findings that change other reports

- README contradictions: "Chief does not hand out todos" vs `start_session`/`give_todo`;
  Codex delivery is a 1 s RPC poll, not a Stop hook; enable admission accepts verification of
  *any* OpenCode session (`D chief:525-532`). None of these carry forward.
- The team socket is already the universal inbox (Chief wakes, answers, gifts, subscription
  prompts ride on `cockpit-team receive`): confirms report 17's single channel.
- Tests exercise the stores directly far more than the socket; the new tests drive the CLI
  and protocol (02).

Status after review: list final; `decided` once Theo confirms in chat.

## Sources

- `docs/audits/2026-10-30-cockpit-autonomy-performance.md` §4 (context contract) and
  `docs/audits/audit-2026-10-30.md` F5/F6
- Current operator docs: `README.md` "Linked sessions" and "Global dispatcher Chief"
