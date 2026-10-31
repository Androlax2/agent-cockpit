# 06. Tasks, workflows and verification

Status: proposed

## Decision

Cockpit owns workflow bookkeeping so agents concentrate on the work. Three separate owners:

| Capability | Owns | Must not |
| --- | --- | --- |
| `cockpit-tasks` | Outcome, scope, dependencies, completion requirements, attempts | Sequence steps |
| `cockpit-workflows` | Next required step, checkpoints, handoffs, continuation | Call the scheduler |
| `cockpit-verification` | Evaluate evidence against declared requirements | Launch agents, advance workflows |

Scheduling consumes ready workflow steps; workflows never schedule.

## Workflow presets (versioned, configurable per project)

- `research`: investigate → synthesize → deliver.
- `code-change`: implement → required checks/review → deliver.
- `design-led`: approved design → implementation → required verification.

Each preset declares required provider capabilities (for example a native review command);
dispatch checks them before launch. Start with one worker; recruit roles only when the
preset needs them. Theo's personal pipeline (`/code-review`, `/simplify`) becomes one
preset, not a product requirement.

## Three kinds of change (prevents stale and needless approvals)

| Change | Effect |
| --- | --- |
| Intent revision (goal, scope, required workflow) | Re-evaluate authorization and approvals |
| State revision (progress, delivery, availability) | Preserve existing authorization |
| Artifact identity (code, document changed) | Affected review/verification evidence becomes stale |

## Task packet (what an agent receives)

Goal, authorized scope, relevant decisions, workflow checkpoint, instruction-discovery entry
point, identities (task, attempt, checkout, role) bound automatically at launch. Evidence and
procedures load on demand. On restart or handoff the packet is rebuilt from durable records;
conversation summaries supplement, current grants are rechecked separately.

## Completion and handoff

A completion names: what changed, the exact result that was checked, what passed or failed,
what still needs the human. **Accepted, merged, installed and live-verified** are separately
evidenced; a task requires only what its definition asks.
A handoff target is about 2 KiB plus referenced evidence; it is incomplete if it only says
"done", cites the last commit while omitting earlier task commits, or claims a native review
from prose.

## Blockers

Every blocked or waiting task states the next action: answer a question, approve an
operation, restore a capability, wait for capacity, or reconcile an uncertain result.

## Validate

- Restart, duplicate-delivery and stale-result fixtures per preset.
- Measure tokens per verified result and useful events per wake; message volume is not progress.

## Sources

- Evidence/handoff contract baseline: `docs/audits/2026-10-30-cockpit-autonomy-performance.md` §4
