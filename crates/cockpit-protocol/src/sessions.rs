//! Wire views of agent sessions (#31). Paths travel as UTF-8 text.

use serde::{Deserialize, Serialize};

use crate::error::ProtocolError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionView {
    pub id: String,
    /// The provider's name, as `--agent` takes it.
    pub agent: String,
    pub checkout: i64,
    /// Whether the session occupies its checkout (a clone).
    pub holds_checkout: bool,
    pub working_dir: String,
    /// The provider's conversation id, once known.
    pub conversation: Option<String>,
    pub design_link: Option<String>,
    pub state: SessionStateView,
    /// Why the session is in its state: a failure, an exit, an expiry.
    pub reason: Option<String>,
    /// Counts every change of the session.
    pub revision: i64,
    /// Seconds since the Unix epoch.
    pub created_at: i64,
    pub changed_at: i64,
    pub launch: Option<LaunchView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SessionStateView {
    Launching,
    Running,
    Stopping {
        to: StopTargetView,
    },
    Asleep,
    Ended,
    Failed,
    /// A state this version does not know.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopTargetView {
    Asleep,
    Ended,
    Restart,
    #[serde(other)]
    Unknown,
}

impl StopTargetView {
    /// The wire name; `unknown` for a value this version cannot read.
    pub fn as_str(self) -> &'static str {
        match self {
            StopTargetView::Asleep => "asleep",
            StopTargetView::Ended => "ended",
            StopTargetView::Restart => "restart",
            StopTargetView::Unknown => "unknown",
        }
    }
}

/// The latest launch of a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchView {
    /// Counts up across every session's launches; never reused.
    pub number: i64,
    pub kind: LaunchKindView,
    /// Seconds since the Unix epoch.
    pub launched_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchKindView {
    Start,
    Restart,
    Wake,
    Resume,
    /// Reopened at daemon start, after the agent was gone.
    Restore,
    #[serde(other)]
    Unknown,
}

impl LaunchKindView {
    /// The wire name; `unknown` for a value this version cannot read.
    pub fn as_str(self) -> &'static str {
        match self {
            LaunchKindView::Start => "start",
            LaunchKindView::Restart => "restart",
            LaunchKindView::Wake => "wake",
            LaunchKindView::Resume => "resume",
            LaunchKindView::Restore => "restore",
            LaunchKindView::Unknown => "unknown",
        }
    }
}

/// A session `session-restart-all` could not restart, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFailure {
    pub session: String,
    pub error: ProtocolError,
}
