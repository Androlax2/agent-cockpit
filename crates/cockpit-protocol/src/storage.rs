//! Registered-clone size observations and explicit owner target-pruning decisions.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneStorageView {
    pub checkout: i64,
    pub project: String,
    pub incarnation: i64,
    pub target: String,
    pub holder: Option<String>,
    pub measurement: TargetSizeView,
    /// Absent from a daemon older than the verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prune: Option<PruneVerdictView>,
}

/// What the sweep did to a clone once the session that occupied it ended (#767).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndSweepView {
    pub checkout: i64,
    pub incarnation: i64,
    /// The ended session that occupied the clone.
    pub session: String,
    pub project: String,
    pub path: String,
    /// Why the clone was kept; `None` when it was deleted.
    pub kept: Option<String>,
    /// What became of a kept clone's `target/`; `None` when its pruning was not tried.
    pub target: Option<String>,
    /// Seconds since the Unix epoch.
    pub swept_at: i64,
}

/// What `storage prune` would decide now for this clone's measured `target/`; it
/// persists nothing and prune decides again from its own measurement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PruneVerdictView {
    /// `confirmation_required` is false only where automatic policy would prune it.
    Eligible {
        confirmation_required: bool,
    },
    Kept {
        reason: String,
    },
}

/// One session's temporary folder, its agent's `TMPDIR` (#766). Named by the folder, which
/// carries the session's id: a folder whose session is gone is listed too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTempStorageView {
    pub session: String,
    pub path: String,
    pub measurement: TargetSizeView,
}

/// One process's log folder, `<state dir>/logs/<process>`; its files rotate by size and age.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFolderStorageView {
    pub process: String,
    pub path: String,
    pub measurement: TargetSizeView,
}

/// The report's areas beyond clones and session temp folders (#406): what the check-output and
/// database dry runs select now, and the log folders. An area that could not be read says why
/// in its `_unavailable` field instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageAreasView {
    pub outputs: Option<Vec<OutputRetentionView>>,
    /// The output pass's bound left outputs out; they are not counted.
    pub outputs_truncated: bool,
    pub outputs_unavailable: Option<String>,
    pub database: Option<DatabaseStorageView>,
    pub database_unavailable: Option<String>,
    pub logs: Vec<LogFolderStorageView>,
    pub logs_truncated: bool,
    pub logs_unavailable: Option<String>,
}

/// Apparent bytes sum regular-file lengths per directory entry, including hardlinks.
/// They do not measure allocated or reclaimable space. Concurrent writes may change them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TargetSizeView {
    Complete {
        apparent_bytes: u64,
    },
    Missing,
    /// Only the entries visited successfully contribute to this partial measurement.
    Incomplete {
        apparent_bytes: u64,
        reason: String,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoragePrunePlanView {
    pub operation: i64,
    pub revision: i64,
    pub epoch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoragePrunePreviewView {
    pub checkout: i64,
    pub incarnation: i64,
    pub target: String,
    pub measurement: TargetSizeView,
    pub eligibility: Option<String>,
    pub kept_reason: Option<String>,
    pub confirmation_required: bool,
    pub plan: Option<StoragePrunePlanView>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoragePruneStatusView {
    pub target: String,
    pub quarantine: Option<String>,
    pub epoch: String,
    /// Current database identity, distinct from the historical plan epoch after restore.
    pub database_epoch: String,
    pub operation: i64,
    pub revision: i64,
    pub checkout: i64,
    pub phase: String,
    pub apparent_bytes: u64,
    pub removed_apparent_bytes: u64,
    pub absent_unconfirmed_bytes: u64,
    pub eligibility: String,
    pub reason: Option<String>,
}

/// One project's ended check outputs that still have their files, under the owner's retention
/// policy. Apparent bytes of `stdout` and `stderr`; unmeasured outputs are counted, never
/// selected, and are not zero bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRetentionView {
    pub project: String,
    pub outputs: u64,
    pub apparent_bytes: u64,
    pub unmeasured: u64,
    /// Kept whatever their age: each check's newest run, a live session's latest, a read one.
    pub protected: u64,
    pub selected: u64,
    pub selected_bytes: u64,
    /// Retired by this pass; zero when it only reports.
    pub retired: u64,
    pub retired_bytes: u64,
    /// Selected outputs this pass kept, each with why.
    pub failures: Vec<String>,
}

/// The state database as the owner's retention policy sees it (#406). Bytes are apparent
/// sizes; identity and audit rows are never selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseStorageView {
    pub file_bytes: u64,
    pub wal_bytes: u64,
    /// Sessions and runs that ended this many days ago lose their payloads.
    pub retention_days: u32,
    /// Ended or failed sessions whose launches' delivery context and start-file snapshots go.
    pub sessions: PayloadRetentionView,
    /// Finished command runs whose output text goes.
    pub run_outputs: PayloadRetentionView,
    /// Captured change rows kept now for clients that resume, and the number the hourly trim
    /// keeps. Rows written since the last trim can put the first above the second.
    pub feed_changes: u64,
    pub feed_changes_kept: u64,
    /// Newest first.
    pub backups: Vec<BackupView>,
    pub backups_kept: u32,
    /// The pass stopped at its bound: further sessions were neither counted nor retired, or
    /// run outputs were left for the next pass.
    pub truncated: bool,
    /// The pass's WAL checkpoint found a reader still on the WAL, so the WAL did not shrink;
    /// the hourly checkpoint finishes it.
    pub wal_checkpoint_busy: bool,
    /// What this report or pass could not do, each with why.
    pub failures: Vec<String>,
}

/// Records one retention rule selects, and with `apply` retired.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadRetentionView {
    pub selected: u64,
    pub selected_bytes: u64,
    /// Zero when the pass only reports.
    pub retired: u64,
    pub retired_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupView {
    pub path: String,
    /// Seconds since the Unix epoch.
    pub taken_at: u64,
    pub bytes: u64,
}
