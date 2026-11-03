//! Read-only registered-clone target sizes. These are observations, never prune advice.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneStorageView {
    pub checkout: i64,
    pub project: String,
    pub incarnation: i64,
    pub target: String,
    pub holder: Option<String>,
    pub measurement: TargetSizeView,
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
