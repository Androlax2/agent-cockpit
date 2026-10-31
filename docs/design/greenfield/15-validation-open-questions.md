# 15. Validation and open questions

Status: validate

Nothing in reports 01–14 was executed. These are the claims that must be proven, the
acceptance scenarios, and the questions still open.

## Must be proven before "done"

| Claim | How to prove |
| --- | --- |
| Turso compatible mode: backup, WAL crash recovery, single-process opening, CDC retention | Fixtures in `cockpit-turso`; crash-injection test; backup/restore round trip |
| Executions survive daemon restart and are reattached, orphans contained | Restart fixtures per walkthrough in report 04 |
| Control path stays responsive under load | 30-minute run on named hardware with build + log flood + stalled provider + database contention; p95/p99 recorded |
| Resource limits are enforced, not only requested | Controller/effective-limit inspection on Linux, WSL2, macOS; report the three values |
| Greywall and Docker profiles per platform | Capability profile tests on each platform; macOS proxy-by-env and `.env` denial documented |
| Provider contracts (Claude, Codex, OpenCode) | Conformance tests per adapter: receipts, resume, native command discovery, cancellation |
| Clean-home onboarding | Scenario in report 09 on Linux, macOS, WSL2, with and without Docker |
| Context budgets | Measured bytes/tokens of launch brief and Cockpit-added context, separate from total |
| Teams/Chief efficiency | Zero wakes on identical revisions in replay; tokens per verified result baseline |
| Release bundle integrity | Install/upgrade tests check helpers and instruction bundle presence |

## Acceptance scenarios (end to end)

1. Two OS users, clean homes, no Docker: isolated state; precise missing-capability reasons;
   permitted host work runs; restart keeps durable state.
2. Three projects under a foreground/gaming profile: fair progress, headroom kept, fan-out
   does not raise entitlement, infeasible work explained.
3. Log flood, stalled viewer, stalled provider, then restart mid-operation: bounded memory,
   stop requests served, whole managed tree ends on cancel, no duplicate external effects.
4. Task from assignment to evidence-bound completion through a preset, with a restart and a
   stale approval in the middle.

## Open questions (owner: Theo unless noted)

- Teams/Chief: final keep/drop list after the read-only implementation review (report 07).
- Which workflow presets ship by default and which gates each requires.
- Exact rotation budgets for logs and change-feed retention window (defaults proposed in 12 and 04).
- When to turn on Turso encryption at rest (after it leaves experimental).
- Whether the phone/web client stays in the first release or follows the designed UI.
- Hardware and workload definition for the latency targets.

## Out of scope for this design

- Shared multi-human control; native Windows execution; a self-written terminal runtime;
  distributed deployment; the plugin runtime itself.
