use cockpit_authorization::SessionId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub i64);

/// What sort of work an item asks for. Every item has one; `task` when nothing fits better.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Task,
    Feature,
    Bug,
    Improvement,
    Chore,
    Research,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Task,
        Kind::Feature,
        Kind::Bug,
        Kind::Improvement,
        Kind::Chore,
        Kind::Research,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Task => "task",
            Kind::Feature => "feature",
            Kind::Bug => "bug",
            Kind::Improvement => "improvement",
            Kind::Chore => "chore",
            Kind::Research => "research",
        }
    }

    /// Only the exact name parses.
    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

/// How urgent an item is: 1 (low) to 6; 2 is normal. Ordering a list puts the highest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Priority(u8);

impl Priority {
    pub const LOW: Priority = Priority(1);
    pub const NORMAL: Priority = Priority(2);
    pub const HIGH: Priority = Priority(3);
    pub const HIGHEST: Priority = Priority(6);

    pub fn new(level: i64) -> Option<Priority> {
        u8::try_from(level)
            .ok()
            .filter(|level| (Priority::LOW.0..=Priority::HIGHEST.0).contains(level))
            .map(Priority)
    }

    pub fn level(self) -> i64 {
        i64::from(self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// A session suggested it; it becomes work only once the owner approves it.
    Proposed,
    Todo,
    Doing,
    Done,
    /// The owner declined the proposal. Kept, so the same title is not proposed again.
    Declined,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Proposed => "proposed",
            Status::Todo => "todo",
            Status::Doing => "doing",
            Status::Done => "done",
            Status::Declined => "declined",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Status> {
        [
            Status::Proposed,
            Status::Todo,
            Status::Doing,
            Status::Done,
            Status::Declined,
        ]
        .into_iter()
        .find(|status| status.as_str() == text)
    }
}

/// Who acts on the tasks, as the daemon authenticated them; never taken from a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    Owner,
    /// A session, identified by the agent channel. `project` is the registered project it
    /// works in, when it works in one.
    Session {
        session: SessionId,
        project: Option<String>,
    },
}

/// The owner's settings for what sessions file (owner configuration `[tasks]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The project blocker reports go to; without one, sessions cannot report a blocker.
    pub blocker_project: Option<String>,
    /// Open items one project's sessions may have filed in another project's list.
    pub cross_project_open_max: u32,
}

/// An item it waits on, with what its session said when it finished it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wait {
    pub item: ItemId,
    pub title: String,
    pub status: Status,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    /// `[Image N]` in the notes names image N.
    pub number: i64,
    pub media_type: &'static str,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,
    pub project: String,
    pub title: String,
    pub notes: String,
    pub kind: Kind,
    pub priority: Priority,
    pub status: Status,
    /// A session reported something it could not do because of the setup.
    pub is_blocker_report: bool,
    /// Why a session proposed it or reported the blocker, in its words.
    pub why: Option<String>,
    /// The session that filed it; `None` when the owner did.
    pub filed_by: Option<SessionId>,
    /// The project the filing session worked in.
    pub filed_from: Option<String>,
    /// The session doing it; `None` while nobody is, or when the owner started it.
    pub doing_session: Option<SessionId>,
    /// What its session said it did when it finished it.
    pub summary: Option<String>,
    /// The owner made it available for a session to take.
    pub is_queued: bool,
    pub waits: Vec<Wait>,
    pub images: Vec<ImageInfo>,
    /// Counts changes to what is asked: title, notes, kind, waits, images (report 06). A
    /// decision made on an older intent revision was made on different work.
    pub intent_revision: i64,
    /// Counts changes to progress and availability: status, priority, queue, summary. They
    /// keep the intent, and whatever was decided on it.
    pub state_revision: i64,
    pub created_at: i64,
    pub changed_at: i64,
}

impl Item {
    /// The items it still waits on: those not done.
    pub fn waiting_on(&self) -> Vec<ItemId> {
        self.waits
            .iter()
            .filter(|wait| wait.status != Status::Done)
            .map(|wait| wait.item)
            .collect()
    }

    /// What has to happen next for it to move, and who does it (report 06, "Blockers").
    pub fn next_action(&self) -> NextAction {
        match self.status {
            Status::Proposed => NextAction::DecideProposal,
            Status::Done | Status::Declined => NextAction::Nothing,
            Status::Doing => NextAction::InProgress {
                session: self.doing_session.clone(),
            },
            Status::Todo => {
                let waiting_on = self.waiting_on();
                if !waiting_on.is_empty() {
                    NextAction::FinishDependencies { items: waiting_on }
                } else if self.is_queued {
                    NextAction::AwaitPlacement
                } else {
                    NextAction::Queue
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextAction {
    /// The owner approves or declines the proposal.
    DecideProposal,
    /// These items are finished first, or the owner removes the wait.
    FinishDependencies {
        items: Vec<ItemId>,
    },
    /// The owner queues it or hands it to a session.
    Queue,
    /// Queued: it waits for scheduling to give it to a session.
    AwaitPlacement,
    /// A session (or the owner, when `None`) is doing it.
    InProgress {
        session: Option<SessionId>,
    },
    Nothing,
}

/// What a new item asks for, as the filer wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewItem {
    /// The project whose list it goes to. A session that names none files in its own.
    pub project: Option<String>,
    pub title: String,
    pub notes: String,
    pub kind: Kind,
    pub priority: Priority,
    /// Items it waits on before it can start.
    pub after: Vec<ItemId>,
}

/// The fields an edit replaces; `None` keeps the current value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edit {
    pub title: Option<String>,
    pub notes: Option<String>,
    pub kind: Option<Kind>,
}

/// What a draft is for: a new item in a project, or an edit of an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftTarget {
    New { project: String },
    Item(ItemId),
}

/// Unsaved form text the owner's client keeps across reloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub target: DraftTarget,
    pub title: String,
    pub notes: String,
    /// For an edit: the item's intent revision the draft started from.
    pub base_intent_revision: Option<i64>,
    /// For an edit: the item's intent changed since the draft started, so saving it would
    /// overwrite a change the draft never showed.
    pub is_stale: bool,
    pub saved_at: i64,
}
