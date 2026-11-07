# `torlek storage` composes every storage area into one read-only report, built by the daemon

Status: accepted

Issue #406, part 3 of the plan in its comments (the composed report). Decided by the worker:
the plan named the areas and the "what each prune would free" column; the shape below is the
smallest one that reuses the passes already merged.

- **The daemon composes, in the one `StorageReport` request.** `Reply::Storage` gains
  `areas`: the check-output and database passes run as dry runs (`apply = false`, which
  retires nothing) and the log folders under `<state dir>/logs` are measured with the same
  bounded, no-follow `folder_storage` as the session temp folders. One request keeps one Bulk
  admission and one answer for the CLI. A daemon older than this sends no `areas`; the CLI then
  shows those rows as unavailable and exits 1, as it does for a daemon without temp folders.
- **Each area is read on its own.** An output or database pass that fails leaves the other
  areas; its reason travels in `outputs_unavailable` or `database_unavailable`. Partial reads
  keep the bytes they saw and make the row and the report incomplete; a backup listing that
  fails marks the backups row, not the database row. A stray file directly under `logs/` is
  listed as an unavailable folder, as one under the session temp folders is.
- **What "retires" means per area.** Build folders: the complete `target/` sizes prune would
  accept now. Check outputs and database: the bytes their dry run selects. Session temp
  folders, backups and logs: zero, because the session end, backup rotation and log rotation
  free them, not a prune. Database pages retired are reused and the file does not shrink, so
  the total's retires leave the database out. The check-output row counts ended runs' outputs,
  the ones retention can select, and says so. All are apparent bytes, not reclaimable space.
- **No new deletion path, setting or retention rule.** `torlek storage prune`,
  `storage outputs prune` and `storage database prune` stay the only ways to delete.
- **Rejected:** the CLI sending `StorageReport`, `StorageOutputs` and `StorageDatabase` itself,
  because it would hold three Bulk admissions, and logs would still need a daemon-side
  measurement. A separate summary view with protected and unknown byte columns, because the
  output pass counts protected outputs, not their bytes; the status column and the per-area
  explanations say what is unknown instead.

The [daemon composition](../../crates/cockpit-daemon/src/storage.rs), the
[CLI summary](../../crates/cockpit-cli/src/storage.rs), its
[fixtures](../../crates/cockpit-cli/tests/storage.rs), and the daemon tests in
[database retention](../../crates/cockpit-daemon/tests/database_retention.rs) and
[output retention](../../crates/cockpit-daemon/tests/verification/output_retention.rs) cover it.
Theo's live before/after numbers remain the owner-run step of v0.1.0-alpha.7.
