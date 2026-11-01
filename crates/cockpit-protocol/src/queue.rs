//! Wire views of scheduling (#43): why each open item waits and what a pass placed. Reasons
//! and who acts travel as their stable names, so a newer daemon's names reach an older
//! client unchanged.

use serde::{Deserialize, Serialize};

use crate::resources::DemandView;

/// The step of a workflow run that waits, or was placed, instead of the item itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueStepView {
    pub run: i64,
    /// Counted from 1.
    pub position: u32,
    pub name: String,
}

/// Why one open item waits, as the last scheduling pass explained it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueExplanationView {
    pub item: i64,
    pub project: String,
    pub title: String,
    /// `None` when the item itself waits; the step when a workflow run's step does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<QueueStepView>,
    /// `not-queued`, `waits-on-items`, `run-blocked`, `owner-step`, `queue-paused`,
    /// `control-unsupported`, `infeasible`, `no-room`, `behind`, `no-session` or
    /// `no-session-free`.
    pub reason: String,
    pub why: String,
    /// The resource and capacity, or the control, that holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limited_by: Option<String>,
    /// Each session of the project that could not take it, with why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<QueueSessionNoteView>,
    /// The item it waits behind while that one gathers capacity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<i64>,
    pub action: String,
    /// `owner`, or `cockpit` when time or released capacity moves it.
    pub action_by: String,
    pub priority: i64,
    /// Levels added for the time it waited.
    pub aging_boost: i64,
    /// What its agent turn would charge, startup headroom included.
    pub demand: DemandView,
    pub headroom_percent: u64,
    /// When it entered the queue; `None` for an item waiting outside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_since: Option<i64>,
    pub evaluated_at: i64,
    pub next_evaluation_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueSessionNoteView {
    pub session: String,
    pub why: String,
}

/// Work a pass placed on a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuePlacementView {
    pub id: i64,
    pub item: i64,
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<QueueStepView>,
    pub session: String,
    pub demand: DemandView,
    pub placed_at: i64,
}
