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
    pub prune: PruneVerdictView,
}

/// What `storage prune` would decide now for this clone's measured `target/`; it
/// persists nothing and prune decides again from its own measurement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PruneVerdictView {
    Eligible {
        proof: String,
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
