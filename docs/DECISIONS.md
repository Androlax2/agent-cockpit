# Engineering decisions

## 2026-10-31 — cockpit-turso: one opener, one writer, full change capture

Issue #24. `cockpit-turso` pins `turso = "=0.8.3"` (the newest non-pre-release) without its
default features, which add FTS and mimalloc; report 04 wants SQLite-compatible mode only.
Its tests measured report 04's adoption checks on that version: online backup, crash
recovery and single opener pass, and the closed file passes plain `sqlite3`'s integrity check.

- **Single opener.** Turso's own lock on the database file is a process-scoped `fcntl`
  lock, which a second opener in the same process does not see. `Database::open` also takes
  an exclusive `File::try_lock` on `<db>.lock`, per open file, so both cases are refused.
- **One writer.** Writes go through one connection behind an async mutex; writers queue there
  instead of on SQLite's busy lock. A dropped `WriteTransaction` rolls back when the next
  write starts (the approach `turso::Transaction` itself takes), since `Drop` cannot await.
- **Change capture `full`.** The writer runs `PRAGMA capture_data_changes_conn('full')`
  (CDC v2: a `change_txn_id` per row and a COMMIT row per transaction). Before and after
  images can be dropped later; images never captured cannot be recovered. A commit's
  revision is its COMMIT row's `change_id`. In WAL mode Turso allocates `change_id` as
  max + 1, so the change feed's retention must keep the newest COMMIT row, or revisions repeat.
- **Migrations.** A ledger row per (owner, version) stores the applied SQL. Startup refuses
  an unknown owner, a version newer than the binary knows, or an applied migration whose SQL
  changed, before applying anything. Each migration commits with its ledger row.
- **Backup and restore.** Backup is `VACUUM INTO`, which Turso does not gate behind an
  experimental flag, into a private file then hard-linked into place: a partial backup never
  appears and an existing destination is never overwritten. Restore is offline: it takes the
  single-opener lock, checks the copy's integrity, gives it a new database epoch so earlier
  change-feed cursors are recognisably stale (report 04), and folds its WAL. The target's old
  WAL moves aside before the rename and is deleted after it: a crash in between leaves its
  frames on disk, and rerunning the restore finishes the job.
- **Edges.** `cockpit-turso` depends on no `cockpit-*` crate, and no other member depends on
  `turso`: capabilities run SQL through `cockpit_turso::sql`, which leaves out `turso::Builder`
  and `turso::Database` so only this crate opens the file. Both rules are
  tests in `crates/cockpit-turso/tests/crate_graph.rs`, beside the crate they protect.

## 2026-10-31 — Bootstrap the greenfield workspace with one rules crate

Issue #18 asks for `cargo build` and `cargo test` to succeed from the root with no
capability crate. Cargo refuses a virtual workspace without members, so the root
workspace starts with `cockpit-workspace-rules`: a dev-only member whose tests fail when
a member does not inherit the root's edition, `rust-version` or lints. It is not a
capability and nothing depends on it; issue #19's crate-graph test can live there too.
MSRV is the pinned toolchain, 1.99.1, the only version CI builds with. `unsafe_code` is denied workspace-wide; an exception
is an `#[allow(unsafe_code, reason = "…")]` on the smallest item. `apps/` keeps its own
workspace and target until the M5 cutover; report 02 supersedes the 2026-10-28 entry
that kept `crates/` under `apps/`.

CI picks its jobs with `tools/ci-jobs.py` in a `changes` job instead of a third-party
paths-filter action or per-workflow `paths:` filters. A workflow skipped by `paths:` leaves
a required check pending forever, while a job skipped by `if:` reports as skipped. Plain
`git diff` plus a tested script needs no new pinned action. A path no job claims, and an
empty diff, run every job: a gap in the path lists shows up as extra work, not as a skipped
gate.

## 2026-10-29 — Keep database error conversion at the store boundary

Team and Chief wire contracts share `cockpit-protocol`, which has no SQLite or UI
dependency. Moving `TeamError` prevents `cockpit` from implementing the foreign
`From<rusqlite::Error>` trait for it. SQL callers therefore explicitly map errors with
`teams::store::storage_error`, retaining `ErrorCode::Storage` and `error.to_string()`.
The existing stores retain their queries, connections and transaction ownership.
A public approval-operation regression checks the serialized SQL error and rollback
of membership when the final proposal write fails.

## 2026-10-28 — Reuse Make as the verification entry point

Cockpit already owns build/test resource controls in Make and its helper scripts.
Local checks and future CI will share those recipes rather than introducing an `xtask`
crate solely to wrap commands. OpenKeychron demonstrates the smaller Make-based approach.

## 2026-10-28 — Extract responsibilities without relocating the workspace

Superseded for new code on 2026-10-31: greenfield crates go in the root `crates/`
(report 02); this entry still describes the code under `apps/`.

Keep `apps/Cargo.toml` and `apps/target/release`. Installed wrappers and integration
tests depend on that layout. New libraries may live under `apps/crates/`; binary owners
stay in their existing packages initially. A root-level `crates/` move has no demonstrated
benefit worth its operational churn.

## 2026-10-28 — Start from the current applied source

The initial parallel clone inherited broker work later reverted on main. Theo approved
a fresh `refactor/5779dac9-engineering` branch at verified `origin/main` (`652d6e2`),
preserving the old branch and uncommitted work. Only reviewed audit/planning documents
carry forward; reverted broker changes are not part of engineering alignment.
