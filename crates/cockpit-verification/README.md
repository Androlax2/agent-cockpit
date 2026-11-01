# cockpit-verification

Owns what was checked: receipts that bind a check's exact definition, source, environment
and artifacts to its outcome and bounded output, prose claims kept as unverified, and the
outcome a gate of required checks has on an exact result. Design: [report 06](../../docs/design/greenfield/06-tasks-workflows-verification.md)
(evidence-bound completion) and [report 09](../../docs/design/greenfield/09-project-setup-readiness.md)
(verification receipts).

It evaluates evidence; it never launches agents or programs (`cockpit-processes` runs the
check, the daemon reports the run) and never advances a workflow (`cockpit-workflows` reads
the gate outcome).

## Receipts

`record_receipt` keeps one run of a check:

| Part | Holds |
| --- | --- |
| Check | name, command, working directory (relative to the checkout), environment |
| Source | head, base, digest of the uncommitted changes (`None` when clean) |
| Artifacts | path and SHA-256 of each file it covered, taken by `fingerprint` before the run |
| Outcome | `passed` (exit 0), `failed` (not exit 0), `unavailable`, `blocked`, `cancelled`, `unknown` |
| Output | the last `OUTPUT_MAX_BYTES` (16 KiB), the full size, references to the full log |

The runner's `run_id` makes delivery idempotent: the same run again returns its receipt,
the same run id with another result is refused (`RunRecordedDifferently`), so evidence is
never rewritten. Receipts and claims are append-only.

`record_claim` keeps an agent's statement that a check passed on a source, with no run
behind it. A claim is read back for the next agent and the human, and never counts as
evidence.

## Gates

`evidence` loads, for one exact source (head and uncommitted changes), each required
check's newest `RECEIPTS_LOADED_PER_CHECK` receipts and `CLAIMS_LOADED_PER_CHECK` claims on
that source, and its newest receipt on any other source. Parallel clones recording the same
check on other heads therefore never push this source's receipts out. `Evidence::evaluate`
answers, outside any transaction because it reads the checkout, where each check stands:

| Status | When |
| --- | --- |
| `checked(outcome)` | the newest receipt of this exact check definition on this source, every artifact unchanged |
| `stale(artifact-changed / -missing / -unreadable)` | that receipt's artifacts changed; an older receipt does not stand in for it |
| `stale(check-changed)` | receipts on this source ran another definition of the check |
| `stale(source-changed)` | receipts only on other heads or uncommitted changes |
| `unverified` | no receipt, only claims about this source |
| `missing` | nothing |

Freshness is computed at evaluation, never stored, so an edited file makes its receipt
stale the moment it changes; each file is hashed at most once per evaluation. The gate is
`passed` when every required check passed, `failed` when one failed, and `open`
otherwise; each check that did not pass names its next action. An empty gate, a check
required twice, one whose evidence was not loaded, or a checkout that does not resolve is
refused. Paths are `/`-separated and normalized, so one file has one spelling.

## Dependency edges

- Depends on: `cockpit-turso` (transactions, migrations) and `sha2` (artifact digests, the
  pin `cockpit-approvals` carries).
- Never on `cockpit-processes`, `cockpit-sessions`, `cockpit-workflows`, `cockpit-tmux`, the
  scheduler or the wire types (`cockpit-workspace-rules/tests/crate_graph.rs`, rules
  `verification -> processes` and `verification -> sessions/workflows/wire types`).
- Used by: the daemon (not wired yet).

## Tests

- `tests/gate.rs`: applicability (edited, removed artifacts; changed check definition; other
  head or uncommitted changes), claims, gate outcomes and next actions.
- `tests/receipts.rs`: what a receipt keeps, bounded output, duplicate delivery, restart,
  and the requests refused at the boundary.
