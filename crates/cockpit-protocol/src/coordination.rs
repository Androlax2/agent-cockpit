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
    /// The latest message a coordinator queued to it, and how its delivery stands (#703).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbound_message: Option<InboundMessageView>,
    /// Its latest launch's note about the harness's folder trust (#1279).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_trust: Option<String>,
}

/// What one `agent-inbox` request found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum InboxView {
    /// The next message, now recorded as received; `message` is its audit entry and `line`
    /// what the agent reads, its sender named.
    Message { message: i64, line: String },
    /// Nothing for the agent now: none is queued, or its agent works.
    Empty,
    /// A newer waiter took over; this one stops.
    Superseded,
}

/// A coordinator grant a session holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorGrantView {
    /// `observe-sessions`, `message-sessions`, `start-sessions` or `end-sessions`.
    pub permission: String,
    pub grant: i64,
    /// What it covers (#1131): `project NAME`, or `everything`.
    pub scope: String,
    /// `start-sessions` only: starts asked or made per hour, at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_hour: Option<u32>,
    /// `start-sessions` only: sessions it started that are not ended, with its starts
    /// waiting for the owner, at most at once (#739).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_active: Option<u32>,
    /// `start-sessions` only: each start waits for the owner's approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_confirmation: Option<bool>,
}

/// One use of a coordinator grant, as the audit recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorAuditView {
    pub id: i64,
    /// The session that acted; for `approved` and `declined`, the session whose start the
    /// owner decided.
    pub actor: String,
    pub permission: String,
    /// `message`, `start` (asked or made, counted by the cap; detail `without confirmation`
    /// when nobody was asked), `started`, the owner's `approved` / `declined` on an asked
    /// start (#657), `ended` (target: the session the actor started and ended, detail: the
    /// item it finished; #713), or `edited` (no target; detail: the task item the actor filed
    /// and edited, with each changed field's old and new value; #714).
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
    /// Why a `queued` message is not typed yet, as its last attempt found, or why it
    /// failed, expired or is uncertain (#656).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A coordinator message still waiting for its target's turn to end when the owner's later
/// message reached the target (#897).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldMessageView {
    /// Its audit entry, as `agent message` printed it.
    pub message: i64,
    /// The session that sent it.
    pub from: String,
}
