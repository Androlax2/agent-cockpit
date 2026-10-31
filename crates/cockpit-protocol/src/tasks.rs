//! Wire views of tasks (#38): items, what each needs next, note images and drafts.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskView {
    pub id: i64,
    pub project: String,
    pub title: String,
    pub notes: String,
    pub kind: TaskKindView,
    /// 1 (low) to 6; 2 is normal.
    pub priority: i64,
    pub status: TaskStatusView,
    /// A session reported something it could not do because of the setup.
    pub is_blocker_report: bool,
    /// Why a session proposed it or reported the blocker, in its words.
    pub why: Option<String>,
    /// The session that filed it; none when the owner did.
    pub filed_by: Option<String>,
    /// The project the filing session worked in.
    pub filed_from: Option<String>,
    pub doing_session: Option<String>,
    /// What its session said when it finished it.
    pub summary: Option<String>,
    pub is_queued: bool,
    pub waits: Vec<TaskWaitView>,
    pub images: Vec<TaskImageView>,
    /// Counts changes to what is asked; an edit names the one it was made on.
    pub intent_revision: i64,
    /// Counts changes to progress and availability.
    pub state_revision: i64,
    /// Seconds since the Unix epoch.
    pub created_at: i64,
    pub changed_at: i64,
    pub next: TaskNextView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskKindView {
    Task,
    Feature,
    Bug,
    Improvement,
    Chore,
    Research,
    /// A kind this version does not know. Never sent: the daemon refuses it.
    #[serde(other)]
    Unknown,
}

impl TaskKindView {
    pub const KNOWN: [TaskKindView; 6] = [
        TaskKindView::Task,
        TaskKindView::Feature,
        TaskKindView::Bug,
        TaskKindView::Improvement,
        TaskKindView::Chore,
        TaskKindView::Research,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TaskKindView::Task => "task",
            TaskKindView::Feature => "feature",
            TaskKindView::Bug => "bug",
            TaskKindView::Improvement => "improvement",
            TaskKindView::Chore => "chore",
            TaskKindView::Research => "research",
            TaskKindView::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatusView {
    Proposed,
    Todo,
    Doing,
    Done,
    Declined,
    #[serde(other)]
    Unknown,
}

impl TaskStatusView {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatusView::Proposed => "proposed",
            TaskStatusView::Todo => "todo",
            TaskStatusView::Doing => "doing",
            TaskStatusView::Done => "done",
            TaskStatusView::Declined => "declined",
            TaskStatusView::Unknown => "unknown",
        }
    }
}

/// An item waited on, with the summary its session left when it finished it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskWaitView {
    pub item: i64,
    pub title: String,
    pub status: TaskStatusView,
    pub summary: Option<String>,
}

/// What has to happen for the item to move, and so who acts next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TaskNextView {
    /// The owner approves or declines the proposal.
    DecideProposal,
    /// These items are finished first, or the owner removes the wait.
    FinishDependencies {
        items: Vec<i64>,
    },
    /// The owner queues it or hands it to a session.
    Queue,
    /// Queued: it waits for a session to take it.
    AwaitPlacement,
    /// A session works on it; none when the owner started it.
    InProgress {
        session: Option<String>,
    },
    Nothing,
    #[serde(other)]
    Unknown,
}

/// `[Image N]` in the notes names image `number`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskImageView {
    pub number: i64,
    pub media_type: String,
    pub bytes: u64,
}

/// What a draft is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TaskDraftTargetView {
    /// The new-item form of a project.
    New {
        project: String,
    },
    Item {
        item: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDraftView {
    pub target: TaskDraftTargetView,
    pub title: String,
    pub notes: String,
    /// For an item: the intent revision the form was opened on.
    pub base_intent_revision: Option<i64>,
    /// The item's intent changed since the form was opened.
    pub is_stale: bool,
    pub saved_at: i64,
}

/// Bytes on the wire as lower-case hexadecimal text, so frames stay one JSON line.
#[derive(Clone, PartialEq, Eq)]
pub struct HexBytes(pub Vec<u8>);

impl std::fmt::Debug for HexBytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "HexBytes({} bytes)", self.0.len())
    }
}

impl Serialize for HexBytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut text = String::with_capacity(self.0.len() * 2);
        for byte in &self.0 {
            text.push(char::from(DIGITS[usize::from(byte >> 4)]));
            text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
        }
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for HexBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.len() % 2 != 0 {
            return Err(serde::de::Error::custom("hex text has an odd length"));
        }
        let digit = |byte: u8| match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(serde::de::Error::custom(
                "hex text holds a character other than 0-9 and a-f",
            )),
        };
        let (pairs, _) = text.as_bytes().as_chunks::<2>();
        pairs
            .iter()
            .map(|&[high, low]| Ok((digit(high)? << 4) | digit(low)?))
            .collect::<Result<_, _>>()
            .map(HexBytes)
    }
}
