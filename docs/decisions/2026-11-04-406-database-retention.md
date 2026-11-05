# Old payloads in the state database are trimmed, never its identity or audit rows; the WAL is checkpointed and the database backed up on a schedule

Issue #406. Decided by Theo on 2026-11-04 (option A for PR 3, the Database line of the
[split plan](https://github.com/Androlax2/agent-cockpit/issues/406#issuecomment-6632573806)).

- **Rows that name something are never deleted.** Sessions, their launches, transitions,
  reports and exits, approval requests, executions, messaging envelopes and entities stay:
  an ended or failed session can be resumed, and report 04 keeps the approval and
  execution records as the audit trail. Retention clears payloads only.
- **Two payloads go after `database_retention_days` (30).** A session ended or failed and
  unchanged since the cutoff loses its launches' delivery context (`session_delivery*`) and
  start-file snapshots: only a running or launching session reads them, and `resume`
  records a fresh delivery. A finished command run ended before the cutoff loses its output
  text and gets `output_retired_at`; its command, reason, decision, digest, exit code and
  times stay, and the schema refuses the mark on a run that kept its output. A start
  request's output names the session it started and is kept.
- **Rechecked under the writer, in bounded batches.** A session is retired only if it is
  still retirable inside the write transaction, so a resume since the listing keeps
  everything. Sessions go one page of 64 per transaction, run outputs 256 per transaction,
  so Control requests keep the writer. One pass considers at most 16 384 sessions, and says
  when it left some out, and clears at most 16 384 run outputs; the report shows what it
  selected beside what it retired, and the next pass takes the rest.
- **Retention writes capture ids, not images.** The writer captures changes in `full` mode,
  so a delete or an update would copy the payload it removes into `turso_cdc` as a before
  image. Retention's transactions use `Database::write_without_images` (`id` mode): each
  commit and changed row still reach the feed, no payload is copied, and the next write
  captures in full again. The payload's earlier images (its insert) leave with the feed
  history and the backups as they age out. A first pass over a large history writes one
  change row per removed row, so it can push older history past `feed_history_changes`; a
  client resuming from before then takes a fresh snapshot, nothing is skipped.
- **Change-feed history is a setting.** `feed_history_changes` (100 000) replaces the
  hard-coded cap; the hourly discard keeps the newest commits as before (#41).
- **Compaction is a WAL checkpoint, not `VACUUM`.** In-place `VACUUM` is experimental in
  Turso and report 04 forbids it. A supervised task runs `PRAGMA wal_checkpoint(TRUNCATE)`
  every `database_maintenance_interval_minutes` (60) and after each applied pass. Freed
  pages are reused, not returned to the filesystem; apparent bytes are not reclaimed space.
- **Backups are `VACUUM INTO` copies, kept N.** The same task writes
  `state/<profile>/backups/state-<secs>.db` when none exists, when the newest is older than
  `database_backup_interval_hours` (24), or when the newest is dated in the future, then
  keeps the newest `database_backups_kept` (7) and removes partial copies. 0 takes none.
  A backup copies the whole database, so it waits while the low-disk guard (#818) reads
  free space below `low_disk_floor_mib`.
  A copy is restorable with the existing offline restore.
- **The owner runs it; automation is opt-in.** `torlek storage database` reports sizes,
  what the policy selects, the feed history and the backups (a dry run);
  `torlek storage database prune` retires it. Both are owner-only on the Bulk lane.
  `database_prune_automatic` (off) runs the same pass each maintenance period. A reader
  shown a retired run says so (`output_retired` on the run view) instead of an empty output.
- **Rejected:** deleting ended sessions or old approval rows (resume and the audit trail
  need them), in-place `VACUUM` (experimental, forbidden), and a task per schedule (one
  supervised task runs the checkpoint, the backup and the automatic pass in order, so they
  never compete for the writer), and admitting the checkpoint and backup to the Bulk lane
  (a backup at daemon start would hold a Bulk place and refuse the owner's first feed
  subscription; the feed's history discard takes no lane place either).

Evidence: `crates/cockpit-sessions/tests/launch_payload_retention.rs`,
`crates/cockpit-approvals/tests/run_output_retention.rs` and
`crates/cockpit-approvals/tests/starts.rs` (what goes and what stays),
`crates/cockpit-turso/tests/maintenance.rs` (checkpoint, backups and rotation),
`crates/cockpit-turso/tests/change_capture.rs` (retained count),
`crates/cockpit-daemon/tests/database_retention.rs` (dry run, prune, owner-only, settings,
backup at start), `crates/cockpit-cli/tests/storage.rs` and `crates/cockpit-cli/tests/ask.rs`
(`storage database`, retired output), `crates/cockpit-config/tests/storage_settings.rs`
(settings and bounds).
