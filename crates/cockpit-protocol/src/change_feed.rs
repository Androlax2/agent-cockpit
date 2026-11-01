use serde::{Deserialize, Serialize};

/// Identifies one database lifetime; a cursor from another epoch cannot be resumed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DatabaseEpoch(pub String);

/// The global commit revision, increasing with every committed transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommitRevision(pub u64);

/// A position in the change feed: everything up to `revision` in `epoch` is applied.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cursor {
    pub epoch: DatabaseEpoch,
    pub revision: CommitRevision,
}

/// Changes ending at `cursor`. A client applies the whole batch, deletions included, before
/// it stores `cursor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeBatch {
    /// What the batch builds on: a `snapshot` replaces all of the client's state; a cursor
    /// other than the client's own means it missed a batch and must subscribe again.
    pub follows: FeedPosition,
    pub cursor: Cursor,
    pub changes: Vec<ChangeEvent>,
}

/// A point in the change feed. Always explicit on the wire: a missing value must not read as
/// a snapshot, which would make a client drop its state or re-read everything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeedPosition {
    /// Before everything: the full current state.
    Snapshot,
    /// Just after this cursor.
    Cursor(Cursor),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEvent {
    pub entity: EntityRef,
    /// The entity's own revision after this change; commands quote it to guard against
    /// stale writes.
    pub entity_revision: EntityRevision,
    pub change: Change,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityRef {
    pub kind: EntityKind,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntityKind {
    Task,
    Attempt,
    Session,
    Execution,
    Checkout,
    /// A bug report draft waiting for the owner (#471); sent and discarded ones are retracted.
    BugReport,
    /// A kind this version does not know, kept as sent so a client can skip or show it.
    /// Never build one with a known name: it would read back as the known variant.
    #[serde(untagged)]
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityRevision(pub u64);

/// Deliberately closed: a client cannot apply a change it does not understand, and skipping
/// one would advance its cursor over data it never applied. A new kind needs a new protocol
/// version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Change {
    /// The entity's complete current published state.
    Upsert {
        state: serde_json::Map<String, serde_json::Value>,
    },
    /// The entity no longer exists.
    Tombstone,
}
