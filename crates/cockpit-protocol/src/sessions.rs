//! Wire views of agent sessions (#31). Paths travel as UTF-8 text.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::ProtocolError;

/// The owner's desktop session variables, as `desktop-session-import` carries them and the
/// daemon passes them to agents while `agents.desktop_session` shares them (#303, #306, #307).
pub const DESKTOP_SESSION_VARIABLES: [&str; 4] = [
    "DISPLAY",
    "XAUTHORITY",
    "WAYLAND_DISPLAY",
    "DBUS_SESSION_BUS_ADDRESS",
];

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
    /// Start files its agent read at launch that changed since: `restart --all` restarts
    /// it (#146).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_start_files: Vec<String>,
    /// `restart --all` deferred its restart until its agent waits at its prompt.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restart_pending: bool,
    /// The task list says its work is finished: it is doing no item and finished one since
    /// its latest launch (#157, decided in #201; #226). Never read from the agent's own state.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub finished: bool,
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

/// The wire name, `stopping:<target>` while stopping; what the frontends print.
impl fmt::Display for SessionStateView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let word = match self {
            SessionStateView::Launching => "launching",
            SessionStateView::Running => "running",
            SessionStateView::Stopping { to } => {
                return write!(formatter, "stopping:{}", to.as_str());
            }
            SessionStateView::Asleep => "asleep",
            SessionStateView::Ended => "ended",
            SessionStateView::Failed => "failed",
            SessionStateView::Unknown => "unknown",
        };
        formatter.write_str(word)
    }
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

/// A session `session-restart-all` could neither restart nor defer, or
/// `session-end-finished` could not end, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFailure {
    pub session: String,
    pub error: ProtocolError,
}

/// One owner setting a running daemon applies, after a `config-reload` (#354): its key in
/// the owner's file and its value as that file writes it (TOML), before and after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedSettingView {
    pub key: String,
    pub value: String,
    pub before: String,
}

/// A provider a session may be started with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderView {
    /// As `session-new` takes it in `agent`.
    pub name: String,
    /// It can reopen a conversation it started: restart, sleep and resume work.
    pub resume: bool,
    /// It reports its agents' activity (state, questions, outcomes) to Cockpit: for Claude
    /// Code, the daemon found a hook bridge that runs (#203).
    pub events: bool,
}
