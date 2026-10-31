# 02. Crate architecture

Status: decided (crate list is the current proposal; names may change, rules may not)

## Decision

Root Cargo workspace with `crates/`. Every crate owns **one named capability**: its types,
behavior, SQL queries and tests. No `core`, `domain`, `common`, `host`, `store` or
`platform` buckets.

### Product capabilities

| Crate | Owns |
| --- | --- |
| `cockpit-projects` | Project identity, checkouts (worktrees/clones), path validation, checkout occupancy |
| `cockpit-sessions` | Provider conversations: identity, lifecycle, resume, sleep/wake |
| `cockpit-tasks` | Requested outcome, scope, dependencies, completion requirements, attempts |
| `cockpit-workflows` | Preset definitions, step state, checkpoints, handoffs, continuation |
| `cockpit-verification` | Check receipts, applicability to an exact result, gate outcomes |
| `cockpit-scheduling` | Eligibility, placement, fairness, queue explanations |
| `cockpit-resource-admission` | Capacity reservations, enforced vs requested limits, profiles (gaming) |
| `cockpit-approvals` | Human decisions bound to exact action digest, revision, expiry |
| `cockpit-authorization` | Effective grants; the only place that answers "may this happen" |
| `cockpit-messaging` | Durable delivery envelopes, receipts, retries, change-feed cursors |
| `cockpit-teams` | Roles as workflow steps over tasks and messaging (report 07) |
| `cockpit-chief` | Judgment: decomposition, priorities, blockers; proposes, never places |
| `cockpit-processes` | Spawn, execution identity, stream draining, whole-tree termination, reattach |
| `cockpit-sandbox` | Requested restrictions, enforcement evidence, platform capability profiles |
| `cockpit-containers` | Docker engine identity, container/volume/network lifecycle |
| `cockpit-credentials` | Secret references, scoped injection, redaction |
| `cockpit-agent-context` | Launch brief and instruction discovery assembled from owners' facts |
| `cockpit-diagnostics` | Logging, levels, support bundle (report 12) |

### Integration and surfaces

| Crate | Owns |
| --- | --- |
| `cockpit-provider-api` | Common provider contract: capabilities with provenance, submit, events, cancel, outcome |
| `cockpit-claude`, `cockpit-codex`, `cockpit-opencode` | One adapter each; harness-specific mechanics stay inside |
| `cockpit-tmux` | Private tmux server, pane lifecycle, control-mode observation |
| `cockpit-turso` | Connections, transactions, migrations, backup; no feature queries |
| `cockpit-protocol` | Versioned public commands, events, errors; capability negotiation |
| `cockpit-client` | Typed client over the protocol, used by every frontend |
| `cockpit-daemon` | Startup, wiring, request routing, supervised worker lifetimes |
| `cockpit-cli`, `cockpit-tui`, `cockpit-web` | Frontends; render state, submit commands |

## Rules

- Dependency direction follows responsibility: `scheduling → tasks/sessions/admission/approvals`;
  lower capabilities never call scheduling. `workflows` never calls the scheduler;
  `verification` never launches agents; `authorization` depends on nothing product-level.
- Cross-capability atomicity: one caller opens a `cockpit-turso` transaction and calls each
  capability's transaction-taking API. No launch or provider call inside a transaction.
- Serializable protocol types are not internal models (rust-analyzer rule); capabilities
  convert explicitly at the boundary.
- A new crate must own a cohesive lifecycle, a concrete integration or a compatibility
  boundary. Helpers stay with their owner.
- Frontends import `cockpit-client` and `cockpit-protocol` only.

## Mechanical enforcement (CI)

- Forbidden-edge check on the crate graph (clients → persistence/scheduling, providers →
  UI, authorization → chief/workflows).
- Real-database tests for every multi-capability transaction.
- Restart and stale-result fixtures that assert ownership and reservations survive.

## Rejected

- `cockpit-domain` / `cockpit-core`: recreates the catch-all the audit identified.
- Keeping `apps/` with five crates (audit option A): superseded by this greenfield list.
- Separate databases per capability: loses atomic cross-capability changes.

## Sources

- rust-analyzer architecture (invariants, API boundaries, serialization rule):
  <https://rust-analyzer.github.io/book/contributing/architecture.html>
- Kamu domain/adapter/infra layering (reference only, DI too heavy for Cockpit):
  <https://github.com/kamu-data/kamu-cli/blob/master/DEVELOPER.md>
