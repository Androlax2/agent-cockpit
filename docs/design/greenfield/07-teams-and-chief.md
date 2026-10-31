# 07. Teams and Chief

Status: proposed; a read-only review of the current implementation is pending before the
keep/drop list is final.

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

## Next step

Read the current `teams`/`chief` stores, delivery and wake code read-only; produce the final
keep/drop list with file references, then fold it into this report.

## Sources

- `docs/audits/2026-10-30-cockpit-autonomy-performance.md` §4 (context contract) and
  `docs/audits/audit-2026-10-30.md` F5/F6
- Current operator docs: `README.md` "Linked sessions" and "Global dispatcher Chief"
