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
