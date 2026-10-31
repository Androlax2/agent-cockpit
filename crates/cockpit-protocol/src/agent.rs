//! The agent channel's wire types (report 17): what a sandboxed agent sends on the agent
//! socket, and the owner's view of what agents reported.

use serde::{Deserialize, Serialize};

/// The variable a launcher passes the launch token in. The agent sends it with its reports;
/// the daemon binds it only from the launch's own process tree.
pub const LAUNCH_TOKEN_VARIABLE: &str = "COCKPIT_LAUNCH_TOKEN";

/// The variable a launcher passes the agent socket's path in, for sandboxes where the
/// default path is mounted elsewhere.
pub const AGENT_SOCKET_VARIABLE: &str = "COCKPIT_AGENT_SOCKET";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentState {
    Working,
    Waiting,
}

impl AgentState {
    pub const ALL: [AgentState; 2] = [AgentState::Working, AgentState::Waiting];

    /// The state's wire name, as serialized.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::Waiting => "waiting",
        }
    }
}

/// How an agent's turn ended (report 08's `outcome` capability).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TurnOutcome {
    Completed,
    /// The provider's request failed; `reason` is the provider's own word for why.
    Failed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The plan's usage limit stopped the turn.
    QuotaStopped,
}

/// What an agent reports about its session. Every field is optional; one left out keeps its
/// previous value. The session is never named: the daemon identifies the sender.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<AgentState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doing: Option<String>,
    /// A question the agent needs the owner to answer; reporting `working` clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_input: Option<String>,
    /// How the turn that just ended went; it means waiting, and reporting `working` clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<TurnOutcome>,
}

/// A session's latest report as the daemon recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReport {
    pub session: String,
    pub state: Option<AgentState>,
    pub label: Option<String>,
    pub doing: Option<String>,
    pub needs_input: Option<String>,
    /// How the latest turn ended, until the next one starts. Absent from older daemons.
    #[serde(default)]
    pub outcome: Option<TurnOutcome>,
    /// Seconds since 1970.
    pub reported_at: i64,
}
