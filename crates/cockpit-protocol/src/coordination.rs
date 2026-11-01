//! The coordinator kit's wire types (#410): what a session holding `observe-sessions` reads
//! about other sessions, the owner's view of coordinator grants, and their audit.

use serde::{Deserialize, Serialize};

use crate::agent::SessionReport;
use crate::sessions::SessionStateView;

/// A session as a coordinator observes it: its state and its agent's latest report, never
/// pane contents or transcripts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedSessionView {
    pub id: String,
    /// The provider's name.
    pub agent: String,
    pub state: SessionStateView,
    pub working_dir: String,
    /// The agent's latest report: state, label, doing-now, open question, outcome.
    pub report: Option<SessionReport>,
}

/// A coordinator grant a session holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorGrantView {
    /// `observe-sessions`, `message-sessions` or `start-sessions`.
    pub permission: String,
    pub grant: i64,
    /// `start-sessions` only: starts asked or made per hour, at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_hour: Option<u32>,
    /// `start-sessions` only: each start waits for the owner's approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_confirmation: Option<bool>,
}

/// One use of a coordinator grant, as the audit recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorAuditView {
    pub id: i64,
    /// The session that acted.
    pub actor: String,
    pub permission: String,
    /// `message`, `start` (asked or made, counted by the cap) or `started`.
    pub action: String,
    /// The session acted on; absent until a start's session exists.
    pub target: Option<String>,
    pub detail: Option<String>,
    /// Seconds since 1970.
    pub at: i64,
}

/// The latest coordinator message queued to a session, as its card shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundMessageView {
    /// The session that sent it.
    pub from: String,
    /// What the sender wrote.
    pub text: String,
    /// The delivery's state: `queued`, `submitting`, `received`, `uncertain`, `failed`,
    /// `expired`.
    pub delivery: String,
    /// Seconds since 1970.
    pub sent_at: i64,
}
