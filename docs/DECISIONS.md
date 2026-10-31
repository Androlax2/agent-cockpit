# Engineering decisions

## 2026-10-31 — Greenfield protocol: version lists, open enums, closed change kinds

`cockpit-protocol` (issue #20) negotiates with a list, not a single number: the client's
`Hello` names every version it speaks and the daemon picks the highest shared one, or
refuses with `version-mismatch` and its own list. A client can then span an upgrade without
a second round trip. Additive features are capabilities, granted only when both sides offer
them; a version bump is reserved for changes an older peer would misread.

Enums that grow (capabilities, refusal reasons, error codes, entity kinds) keep unknown
values through a `#[serde(untagged)] Unknown(String)` variant. Commands cannot: an untagged
fallback also swallows a malformed known command as "unknown". `Command` uses a unit
`#[serde(other)]` variant instead, which catches only unknown tags, so `decode_request`
answers `unknown-command` naming the kind, or `invalid-request` with serde's precise message.
The change kind (`upsert`, `tombstone`) is closed: a client that skipped a change it could
not apply would advance its cursor over data it never applied, so a new kind needs a new
version. Feed positions (a batch's `follows`, `subscribe`'s `from`) are a required
`snapshot`-or-cursor enum, not an optional cursor: serde reads a missing `Option` as `None`,
which would turn a dropped or misspelled field into "replace all state".

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

## 2026-10-31 — Check forbidden crate edges with `cargo tree`, transitively

Issue #19's rule test reads the graph from `cargo tree --target all --edges
normal,build,dev` instead of parsing manifests: renamed, workspace-inherited and
target-specific dependencies resolve exactly as Cargo builds them, with no new dependency.
An edge counts when it is direct or goes through other crates, since a client that reaches
`cockpit-turso` through an intermediate still links it. Dev-dependencies count only for the
crate that declares them, matching what Cargo builds. The rules name crates from report 02's
current table; a crate that does not exist yet is skipped, and a test fails when the
table names a crate report 02 no longer lists, so a rename cannot disable a rule silently. Issue #19's acceptance says `make ci`; since #18 split the gates, the rule runs in
`make greenfield-ci` (`crates-test`), which CI's `greenfield` job runs.

## 2026-10-31 — cockpit-diagnostics: own queue, own tar, field-name guard

Issue #23. The non-blocking writer is a `sync_channel` and one thread instead of
`tracing-appender`: the writer thread needs the drop counter to write its own
`dropped log records` warning, and a bounded shutdown that gives up on a stalled disk.
The support bundle is a plain ustar archive written by 30 lines in the crate rather than
the `tar` crate; tests read it back with the system `tar`.

`COCKPIT_LOG_LEVEL` accepts only levels and `component=level`. `EnvFilter` reads a bare word
as a target, so `loud` would silently enable everything for a target named `loud`; it is
refused instead. A component names a Cockpit crate without its prefix (`tmux` →
`cockpit_tmux`), matching report 12's examples to the targets the crates emit.

Overrides longer than 15 minutes are refused, not shortened, so the host sees the limit.
Records and spans whose field names mark hosted content (`prompt`, `shell_command`, `env`, …)
are refused at the callsite and counted. The check reads names, not values: content
formatted into a message string still depends on review.

Each process logs to `logs/<process>/cockpit.log`, keeping report 12's file name per
process; a locked `cockpit.log.lock` (a writable file, so it also works on NFS) makes a second process with the same name fail at start instead of
breaking the other's rotation. The crate is a library only: the `cockpit logs level` and `cockpit support-bundle`
verbs need the daemon's host-only channel (#26) and the CLI.

## 2026-10-31 — cockpit-config: declared start files, snapshots, and a temporary binary

Issue #22. Of today's 17 start files (`apps/crates/cockpit-core/src/sessions.rs`), the
product declares only the five a provider itself owns (`CLAUDE.md`, Claude `settings` and
`plugins`, `opencode config`, `codex hooks`). Sandbox launchers, learned profiles, the session
hook and the ADHD flag are installation paths, so the owner declares them in
`config.toml` under `[[start_files]]`; report 01 forbids shipping one machine's paths.
Two of today's pairings are installation wiring too: codex reading `CLAUDE.md`, and
opencode's `cockpit.ts` plugin, whose install path the opencode adapter will own; Theo's
config re-declares `CLAUDE.md` with `codex` added and lists the plugin.
Change detection compares a launch-time snapshot (path, modification time, ctime, inode,
size) for inequality instead of today's "newer than the session start": a file restored from
an older copy (even `cp -p`), deleted, created or repointed now counts, and a file declared
after launch is reported because the session cannot have it.

The project file is `<project>/.cockpit/config.toml`, parsed with the owner file's schema;
setting an owner key there fails instead of being ignored. `XDG_RUNTIME_DIR` unset leaves
the runtime directory unset rather than inventing one: the daemon (#26) owns that fallback.

`config show` ships as a `cockpit-config show` binary inside the crate: no M0 issue creates
a CLI crate, and report 02 lets frontends import only `cockpit-client` and
`cockpit-protocol`, while report 09 needs config inspection without the daemon. Where
`cockpit config show` finally lives is a decision for Theo (decision-needed issue).

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
