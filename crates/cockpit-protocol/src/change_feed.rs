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
    /// "This part of the owner's view changed" (#706). The id is a [`ViewArea::id`] and the
    /// state a [`ViewStamp`]. A client re-asks for the area when the
    /// stamp moves instead of polling it.
    ViewStamp,
    /// Free disk space below the owner's floor (#1143): one entity, in the state of a
    /// [`DiskView`], on the feed while the low-disk guard finds the space low and retracted
    /// when it is not (or the guard is off).
    Disk,
    /// The last alert threshold a plan window passed (#58): one entity per provider, account
    /// and limit, id `provider/account/limit`, in the state of a [`crate::QuotaAlertView`].
    /// Published when a threshold fires and kept after, so a new state is a new alert.
    QuotaAlert,
    /// A kind this version does not know, kept as sent so a client can skip or show it.
    /// Never build one with a known name: it would read back as the known variant.
    #[serde(untagged)]
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityRevision(pub u64);

/// A part of the owner's view that the daemon answers by request rather than as entities:
/// the change feed carries only a [`ViewStamp`] for it, and the client asks again when the
/// stamp moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ViewArea {
    /// `project-list`.
    Projects,
    /// `agent-reports`.
    AgentReports,
    /// `approval-queue`.
    Approvals,
    /// The task list and the queue explanation.
    Tasks,
    /// The workflow list, a run and its packet.
    Workflows,
}

impl ViewArea {
    pub const ALL: [ViewArea; 5] = [
        ViewArea::Projects,
        ViewArea::AgentReports,
        ViewArea::Approvals,
        ViewArea::Tasks,
        ViewArea::Workflows,
    ];

    /// The id of this area's [`EntityKind::ViewStamp`] entity.
    pub fn id(self) -> &'static str {
        match self {
            ViewArea::Projects => "projects",
            ViewArea::AgentReports => "agent-reports",
            ViewArea::Approvals => "approvals",
            ViewArea::Tasks => "tasks",
            ViewArea::Workflows => "workflows",
        }
    }

    /// The area a stamp entity id names; `None` for an area this version does not know.
    pub fn from_id(id: &str) -> Option<ViewArea> {
        ViewArea::ALL.into_iter().find(|area| area.id() == id)
    }
}

/// The state of a [`EntityKind::ViewStamp`] entity: it differs whenever the area may have
/// changed, and says nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewStamp {
    /// The commit revision of the last commit that touched what the area shows.
    pub changed_at: u64,
}

/// The state of the [`EntityKind::Disk`] entity. Published once on entering low disk, so a
/// reading that only changes `free_mib` while the space stays low is not published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskView {
    /// Free space when the guard found it low, in MiB.
    pub free_mib: u64,
    pub floor_mib: u64,
}

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
