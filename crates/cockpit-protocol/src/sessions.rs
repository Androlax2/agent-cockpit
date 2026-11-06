//! Wire views of agent sessions (#31). Paths travel as UTF-8 text.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::coordination::InboundMessageView;
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
    /// How the latest launch's agent exited by itself, with the last lines it printed (#629).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<AgentExitView>,
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
    /// The coordinator grants it holds (#410): `observe-sessions`, `message-sessions`,
    /// `start-sessions`, `end-sessions`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coordinator_grants: Vec<String>,
    /// The latest message a coordinator queued to it, with the sender named (#410).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbound_message: Option<InboundMessageView>,
    /// In a `session new` reply alone, under `agents.sandbox = "required"`: the credential to
    /// set when its provider has none the session proxy can add, so its harness asks for a
    /// sign-in it cannot keep (#968).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in_needed: Option<String>,
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

impl SessionStateView {
    /// Ended or failed: the session has no agent nor terminal until it is resumed (#630).
    pub fn is_over(&self) -> bool {
        matches!(self, SessionStateView::Ended | SessionStateView::Failed)
    }
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

/// How a launch's agent exited by itself, as its terminal showed it before the session
/// closed it (#629).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentExitView {
    pub status: ExitStatusView,
    /// The last lines its terminal showed, oldest first, without control characters.
    /// Untrusted display data.
    pub output: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExitStatusView {
    Code {
        code: i32,
    },
    /// Its process was ended by a signal.
    Signal,
    /// Its terminal was gone, or still showed it running; also a status this version does
    /// not know.
    #[serde(other)]
    Unknown,
}

impl fmt::Display for ExitStatusView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExitStatusView::Code { code } => write!(formatter, "status {code}"),
            ExitStatusView::Signal => formatter.write_str("a signal"),
            ExitStatusView::Unknown => formatter.write_str("an unknown status"),
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

/// A pane of the manager session: the column the TUI runs in, or the viewer beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagerPaneView {
    Column,
    Viewer,
}

impl ManagerPaneView {
    /// The pane `manager-focus` meant before it named one (#345).
    pub fn column() -> ManagerPaneView {
        ManagerPaneView::Column
    }
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

/// How one launch ran (#61): enforced by a sandbox or not, on which platform, each rule with
/// what enforced it, engine-reported capabilities and labelled owner probe attestations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxEvidenceView {
    pub enforcement: SandboxEnforcementView,
    pub platform: String,
    pub rules: Vec<SandboxRuleView>,
    pub observed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SandboxEnforcementView {
    /// `client_identity`: this launch's own name, which owner observations attach to (#433).
    Sandboxed {
        backend: String,
        engine: String,
        client_identity: String,
    },
    /// Only by the owner's own act, which `reason` names: the setting `agents.sandbox =
    /// "off"`, or an adoption of an agent the owner started outside Torlek (#296, #861).
    /// A launch under `"required"` that cannot be enforced is refused, never recorded
    /// here.
    Unsandboxed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxRuleView {
    pub rule: String,
    pub enforced_by: String,
}

impl SandboxEvidenceView {
    /// One line per fact, in a fixed order: what `session show` and `sandbox show` print.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = match &self.enforcement {
            SandboxEnforcementView::Sandboxed {
                backend,
                engine,
                client_identity,
            } => vec![
                format!("sandbox: enforced by {backend} on {}", self.platform),
                format!("engine: {engine}"),
                format!("launch identity: {client_identity}"),
            ],
            SandboxEnforcementView::Unsandboxed { reason } => vec![format!(
                "sandbox: NONE on {}, unsandboxed: {reason}",
                self.platform
            )],
        };
        lines.extend(
            self.rules
                .iter()
                .map(|rule| format!("rule: {} <- {}", rule.rule, rule.enforced_by)),
        );
        lines.extend(
            self.observed
                .iter()
                .map(|observation| format!("observed: {observation}")),
        );
        lines
    }
}
