use cockpit_authorization::SessionId;
use cockpit_tasks::ItemId;

use crate::{Doer, Preset, StepDefinition};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// A step is still to be done.
    Running,
    /// Every step is done.
    Delivered,
    /// The owner stopped it; its steps stay as they were.
    Cancelled,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Delivered => "delivered",
            RunStatus::Cancelled => "cancelled",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<RunStatus> {
        [
            RunStatus::Running,
            RunStatus::Delivered,
            RunStatus::Cancelled,
        ]
        .into_iter()
        .find(|status| status.as_str() == text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    /// An earlier step is not done.
    Pending,
    /// The next required step: an agent step waits for a session to begin it, an owner step
    /// for the owner to complete it.
    Ready,
    /// A session is doing it.
    Active,
    Done,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::Ready => "ready",
            StepStatus::Active => "active",
            StepStatus::Done => "done",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<StepStatus> {
        [
            StepStatus::Pending,
            StepStatus::Ready,
            StepStatus::Active,
            StepStatus::Done,
        ]
        .into_iter()
        .find(|status| status.as_str() == text)
    }
}

/// The latest progress a session recorded in a step. It survives the session, a release and
/// a daemon restart: the next session resumes from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub note: String,
    /// Counts the checkpoints recorded in the step, across attempts.
    pub revision: i64,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Counted from 1, in the preset's order.
    pub position: u32,
    pub definition: &'static StepDefinition,
    pub status: StepStatus,
    /// The session doing it while it is active.
    pub session: Option<SessionId>,
    pub checkpoint: Option<Checkpoint>,
    /// How many times a session began it.
    pub attempts: u32,
    pub finished_at: Option<i64>,
}

/// The next action a blocked run waits on (report 06, "Blockers").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedAction {
    AnswerQuestion,
    ApproveOperation,
    RestoreCapability,
    WaitForCapacity,
    ReconcileResult,
}

impl BlockedAction {
    pub const ALL: [BlockedAction; 5] = [
        BlockedAction::AnswerQuestion,
        BlockedAction::ApproveOperation,
        BlockedAction::RestoreCapability,
        BlockedAction::WaitForCapacity,
        BlockedAction::ReconcileResult,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            BlockedAction::AnswerQuestion => "answer-question",
            BlockedAction::ApproveOperation => "approve-operation",
            BlockedAction::RestoreCapability => "restore-capability",
            BlockedAction::WaitForCapacity => "wait-for-capacity",
            BlockedAction::ReconcileResult => "reconcile-result",
        }
    }

    /// Only the exact name parses.
    pub fn parse(name: &str) -> Option<BlockedAction> {
        BlockedAction::ALL
            .into_iter()
            .find(|action| action.as_str() == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    pub action: BlockedAction,
    /// What exactly is needed, in the words of whoever blocked it.
    pub detail: String,
    pub since: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub id: RunId,
    pub item: ItemId,
    /// The item's project when the run started: the scope its sessions work in.
    pub project: String,
    pub preset: &'static Preset,
    pub status: RunStatus,
    /// The item's intent revision when the run started. A later one means the goal or scope
    /// changed under the run (report 06, "intent revision").
    pub intent_revision: i64,
    pub blocked: Option<Blocked>,
    pub steps: Vec<Step>,
    pub created_at: i64,
    pub changed_at: i64,
}

impl Run {
    /// The next required step: the first one not done, while the run is running.
    pub fn current_step(&self) -> Option<&Step> {
        if self.status != RunStatus::Running {
            return None;
        }
        self.steps
            .iter()
            .find(|step| step.status != StepStatus::Done)
    }

    /// What has to happen for the run to move, and who does it.
    pub fn next_action(&self) -> NextAction {
        let Some(step) = self.current_step() else {
            return NextAction::Nothing;
        };
        if let Some(blocked) = &self.blocked {
            return NextAction::Unblock {
                action: blocked.action,
            };
        }
        match (step.status, step.definition.doer) {
            (StepStatus::Active, _) => NextAction::InProgress {
                position: step.position,
                session: step.session.clone(),
            },
            (_, Doer::Owner) => NextAction::OwnerCompletes {
                position: step.position,
            },
            (_, Doer::Agent) => NextAction::AwaitPlacement {
                position: step.position,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextAction {
    /// The blocker's action comes first.
    Unblock { action: BlockedAction },
    /// An agent step is ready: scheduling gives it to a session, which begins it.
    AwaitPlacement { position: u32 },
    /// The owner completes this step.
    OwnerCompletes { position: u32 },
    /// A session is doing it.
    InProgress {
        position: u32,
        session: Option<SessionId>,
    },
    /// Delivered or cancelled.
    Nothing,
}

/// What a session (or the owner) hands on when a step ends or changes hands (report 06,
/// "Completion and handoff"). Evidence is referenced, not pasted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Handoff {
    /// What changed.
    pub changed: String,
    /// The exact result that was checked: a commit range, a file, a URL.
    pub checked: String,
    /// What passed and what failed.
    pub outcome: String,
    /// What still needs the human; empty when nothing does.
    pub needs_human: String,
    /// References to the evidence: receipts, logs, pull requests. One line each.
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffKind {
    /// The step is done; the next one is ready.
    Completed,
    /// The session stopped mid-step; the step is ready again from its checkpoint.
    Continued,
}

impl HandoffKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HandoffKind::Completed => "completed",
            HandoffKind::Continued => "continued",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<HandoffKind> {
        [HandoffKind::Completed, HandoffKind::Continued]
            .into_iter()
            .find(|kind| kind.as_str() == text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffRecord {
    pub position: u32,
    pub step: &'static str,
    pub kind: HandoffKind,
    /// The session that handed off; `None` when the owner did.
    pub session: Option<SessionId>,
    pub handoff: Handoff,
    /// The step's requirements that passed, as the caller's verification reported them.
    pub satisfied: Vec<String>,
    /// The step's requirements the owner waived instead.
    pub waived: Vec<String>,
    pub at: i64,
}

/// A ready agent step, as scheduling consumes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyStep {
    pub run: RunId,
    pub item: ItemId,
    pub project: String,
    pub preset: &'static Preset,
    pub position: u32,
    pub definition: &'static StepDefinition,
}

/// What `runs` returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunFilter {
    /// One project's runs; every visible run without it.
    pub project: Option<String>,
    /// Delivered and cancelled runs too.
    pub include_closed: bool,
}
