# 04. State and durability

Status: decided (engine); recovery contracts `validate`

## Engine decision

**Turso Database** (Rust rewrite of SQLite, same file format) through the `turso` crate,
in **SQLite-compatible mode**: default journal mode, no MVCC, no experimental flags.
**CDC enabled** to back the change feed. The state file stays readable by ordinary SQLite tools.

Why Turso over rusqlite: native async API inside the Tokio daemon (no C compiler), and
committed, transaction-grouped change records for free. Why not MVCC/encryption/FTS: one
daemon is one writer; the rest is experimental and would lock the file into vendor formats.
Pre-1.0 caveat: keep independent backups; verify backup, WAL recovery and single-process
opening before adoption is final.

`cockpit-turso` owns connections, transactions, migrations and backup only. Each capability
owns its own queries and migrations; startup runs one ordered migration sequence.

## Identities (each has its own lifetime)

| Entity | Meaning |
| --- | --- |
| Task | Desired outcome and acceptance criteria |
| Attempt | One authorized pursuit of a task revision; redelivery keeps the id, replacement creates a new one |
| Session | Provider conversation; may span attempts and process restarts |
| Execution | One OS process group or container, identified by supervisor identity, never PID alone |
| Checkout | Working copy plus incarnation; path is a locator, not identity |

A **dispatch reservation** links attempt, destination, checkout and admission grant before launch.

## State dimensions (never one enum)

- Delivery: `queued → submitting → received | uncertain | failed | expired`. "Received" needs
  a durable receiver receipt.
- Outcome: `not-observed → running → outcome-reported → verified-complete | failed | blocked`.
  Completion is a verifier result, never an agent label.
- Cancellation: `cancel-requested` is separate from observed termination and from effect outcome.
- Authority: monotonically increasing grant generation per assignment; every heartbeat, receipt
  and completion must match the active attempt and generation (fencing). Lease expiry is
  suspicion, not proof the old process stopped.

## Durable effects

- Commit state change and **effect intent** in one transaction; workers deliver afterward;
  receivers deduplicate on a stable effect id (one idempotency key per logical effect).
- Retries cannot make external effects exactly-once. If an acknowledgement is lost, reconcile
  through provider lookup or verifiable artifacts; otherwise persist `outcome-unknown`. Never
  blindly repeat a non-idempotent action.
- No event sourcing and no replay engine: current state + transition history + pending intents.

## Change feed and client recovery

- Global commit revision from CDC; entity revisions guard commands against stale writes.
- Snapshot + revision from one read transaction, then replay retained deltas, then stream.
- Clients advance a cursor only after applying a complete batch (deletions included).
- Cursors are scoped by database epoch and subscription identity; after retention or epoch
  mismatch the daemon answers `resnapshot-required`, never a silent jump.
- Only replaceable availability observations may coalesce; decisions, approvals, outcomes and
  ownership transitions are never dropped.

## Three walkthroughs the fixtures must pass

1. Crash after reservation commit, before launch: recovery finds the intent, checks whether
   the launch identity exists, launches or reattaches; no second reservation.
2. Crash after launch, before receipt: execution marked uncertain, occupancy kept, process
   reattached or contained; late messages from a superseded generation cannot complete the
   replacement attempt.
3. Offline client approves obsolete work: approval fails its bound digest/revision; client
   replays or resnapshots; old completion cannot overwrite the new assignment.

## Sources

- Turso: <https://github.com/tursodatabase/turso> (README FAQ: production use, pre-1.0),
  compatibility <https://docs.turso.tech/sql-reference/compatibility.md>,
  CDC <https://docs.turso.tech/tursodb/cdc.md>,
  multi-process limits <https://docs.turso.tech/sql-reference/multiprocess-access.md>
- SQLite transactions/WAL: <https://sqlite.org/lang_transaction.html>, <https://sqlite.org/wal.html>,
  backup API <https://www.sqlite.org/backup.html>
- Restate (patterns, not architecture): <https://docs.restate.dev/guides/request-lifecycle>,
  <https://docs.restate.dev/services/invocation/managing-invocations>
