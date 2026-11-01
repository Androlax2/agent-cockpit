//! Wire views of workflows (#39): runs and their steps, handoffs, presets and the task packet.

use serde::{Deserialize, Serialize};

use crate::tasks::{TaskKindView, TaskStatusView, TaskWaitView};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowView {
    pub id: i64,
    pub item: i64,
    /// The item's project when the run started: the scope its sessions work in.
    pub project: String,
    pub preset: String,
    pub preset_version: u32,
    pub status: WorkflowStatusView,
    /// The item's intent revision when the run started.
    pub intent_revision: i64,
    pub blocked: Option<WorkflowBlockedView>,
    pub steps: Vec<WorkflowStepView>,
    pub next: WorkflowNextView,
    /// Seconds since the Unix epoch.
    pub created_at: i64,
    pub changed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowStatusView {
    Running,
    Delivered,
    Cancelled,
    #[serde(other)]
    Unknown,
}

impl WorkflowStatusView {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowStatusView::Running => "running",
            WorkflowStatusView::Delivered => "delivered",
            WorkflowStatusView::Cancelled => "cancelled",
            WorkflowStatusView::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowStepView {
    /// Counted from 1.
    pub position: u32,
    pub name: String,
    pub purpose: String,
    /// `agent` or `owner`.
    pub doer: String,
    pub status: WorkflowStepStatusView,
    pub session: Option<String>,
    pub checkpoint: Option<WorkflowCheckpointView>,
    /// How many times a session began it.
    pub attempts: u32,
    /// Evidence it needs before it completes, by name.
    pub requirements: Vec<String>,
    /// What the provider of the session beginning it must report.
    pub required_capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowStepStatusView {
    Pending,
    Ready,
    Active,
    Done,
    #[serde(other)]
    Unknown,
}

impl WorkflowStepStatusView {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowStepStatusView::Pending => "pending",
            WorkflowStepStatusView::Ready => "ready",
            WorkflowStepStatusView::Active => "active",
            WorkflowStepStatusView::Done => "done",
            WorkflowStepStatusView::Unknown => "unknown",
        }
    }
}

/// Where the step's previous holder left off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowCheckpointView {
    pub note: String,
    pub revision: i64,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowBlockedView {
    pub action: WorkflowBlockedActionView,
    pub detail: String,
    pub since: i64,
}

/// The next action a blocked run waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowBlockedActionView {
    AnswerQuestion,
    ApproveOperation,
    RestoreCapability,
    WaitForCapacity,
    ReconcileResult,
    /// An action this version does not know. Never sent: the daemon refuses it.
    #[serde(other)]
    Unknown,
}

impl WorkflowBlockedActionView {
    pub const KNOWN: [WorkflowBlockedActionView; 5] = [
        WorkflowBlockedActionView::AnswerQuestion,
        WorkflowBlockedActionView::ApproveOperation,
        WorkflowBlockedActionView::RestoreCapability,
        WorkflowBlockedActionView::WaitForCapacity,
        WorkflowBlockedActionView::ReconcileResult,
    ];

    pub fn parse(text: &str) -> Option<WorkflowBlockedActionView> {
        WorkflowBlockedActionView::KNOWN
            .into_iter()
            .find(|action| action.as_str() == text)
    }

    /// The known actions' names, for a usage or refusal message.
    pub fn known_names() -> String {
        let names: Vec<&str> = WorkflowBlockedActionView::KNOWN
            .iter()
            .map(|action| action.as_str())
            .collect();
        names.join(", ")
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowBlockedActionView::AnswerQuestion => "answer-question",
            WorkflowBlockedActionView::ApproveOperation => "approve-operation",
            WorkflowBlockedActionView::RestoreCapability => "restore-capability",
            WorkflowBlockedActionView::WaitForCapacity => "wait-for-capacity",
            WorkflowBlockedActionView::ReconcileResult => "reconcile-result",
            WorkflowBlockedActionView::Unknown => "unknown",
        }
    }
}

/// What has to happen for the run to move, and so who acts next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum WorkflowNextView {
    /// The blocker's action comes first.
    Unblock {
        action: WorkflowBlockedActionView,
    },
    /// An agent step is ready for a session to begin.
    AwaitPlacement {
        position: u32,
    },
    /// The owner completes this step.
    OwnerCompletes {
        position: u32,
    },
    InProgress {
        position: u32,
        session: Option<String>,
    },
    Nothing,
    #[serde(other)]
    Unknown,
}

/// What a step's holder hands on: what changed, the exact result checked, what passed or
/// failed, what still needs the human, and references to the evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowHandoffView {
    pub changed: String,
    pub checked: String,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub needs_human: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowHandoffRecordView {
    pub position: u32,
    pub step: String,
    /// `completed` or `continued`.
    pub kind: String,
    /// None when the owner handed off.
    pub session: Option<String>,
    pub handoff: WorkflowHandoffView,
    /// Requirements that passed, as verification reported them.
    pub satisfied: Vec<String>,
    /// Requirements the owner waived.
    pub waived: Vec<String>,
    pub at: i64,
}

/// What a session receives for its step, rebuilt from durable records on every read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowPacketView {
    pub workflow: i64,
    pub status: WorkflowStatusView,
    pub preset: String,
    pub preset_version: u32,
    /// The item the workflow follows.
    pub item: i64,
    /// None once the item was deleted: the workflow still answers, for the owner to cancel.
    pub goal: Option<WorkflowGoalView>,
    /// The authorized scope of its steps.
    pub project: String,
    /// None once the run is delivered or cancelled.
    pub step: Option<WorkflowStepView>,
    pub next: WorkflowNextView,
    pub blocked: Option<WorkflowBlockedView>,
    /// What the items it waited on passed on when they finished.
    pub decisions: Vec<TaskWaitView>,
    pub handoffs: Vec<WorkflowHandoffRecordView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowGoalView {
    pub title: String,
    pub notes: String,
    pub kind: TaskKindView,
    pub item_status: TaskStatusView,
    pub started_on_intent: i64,
    pub intent_revision: i64,
    /// The goal or scope changed since the run started.
    pub intent_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowPresetView {
    pub name: String,
    pub version: u32,
    pub summary: String,
    pub steps: Vec<WorkflowPresetStepView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowPresetStepView {
    pub name: String,
    pub purpose: String,
    pub doer: String,
    pub requirements: Vec<String>,
    pub required_capabilities: Vec<String>,
}
