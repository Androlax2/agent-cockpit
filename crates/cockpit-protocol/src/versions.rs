//! What `version --all` asks the running daemon for (report 13, #64): the release it runs,
//! the schema it holds the database at, and the bundle each live session started with.

use serde::{Deserialize, Serialize};

use crate::sessions::SessionStateView;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionReportView {
    /// The release the daemon was built from.
    pub release: String,
    pub source_revision: Option<String>,
    /// The id of the instruction manifest the daemon serves.
    pub instruction_manifest: String,
    /// The protocol versions the daemon speaks, oldest first.
    pub protocol_versions: Vec<u32>,
    /// One entry per capability that owns migrations, in the order the daemon applies them.
    pub schema: Vec<SchemaOwnerView>,
    /// Sessions that are not ended or failed, by creation.
    pub sessions: Vec<SessionBundleView>,
}

/// A capability's schema as the database holds it and as the daemon's binary knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaOwnerView {
    pub owner: String,
    /// The newest migration the database records; 0 when it records none.
    pub applied: u32,
    pub known: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBundleView {
    pub session: String,
    pub state: SessionStateView,
    /// What the session's latest launch started with; absent for a launch recorded before
    /// launches kept it.
    pub bundle: Option<LaunchBundleView>,
}

/// The release whose helpers and instruction bundle a launch ran with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchBundleView {
    pub release: String,
    pub instruction_manifest: String,
}
