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

/// Where an open question came from, which decides what clears it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuestionSource {
    /// The agent's own question: open until a new prompt, a new launch or a new conversation.
    Agent,
    /// A dialog the provider bridge saw: open until a report says what the agent does (a
    /// state or a turn's outcome) or a new conversation starts.
    Dialog,
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
    /// The agent's own question for the owner; a new prompt, launch or conversation clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_input: Option<String>,
    /// How the turn that just ended went; it means waiting, and reporting `working` clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<TurnOutcome>,
    /// A dialog's question the provider bridge saw; the next report with a state or a turn's
    /// outcome clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<String>,
    /// The owner sent a new prompt: the agent works and its own question is answered.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_new_prompt: bool,
    /// The provider's conversation the report comes from (Claude Code's `session_id`). The
    /// daemon refuses a report whose conversation is not the session's (#219).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    /// `conversation` ends and the same agent goes on in another (`/clear`, an in-session
    /// `/resume`): the session follows the next conversation a report names. Needs
    /// `conversation`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_conversation_ending: bool,
}

/// A session's latest report as the daemon recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionReport {
    pub session: String,
    pub state: Option<AgentState>,
    pub label: Option<String>,
    pub doing: Option<String>,
    /// The open question, the agent's own or a dialog's.
    pub needs_input: Option<String>,
    /// How the latest turn ended, until the next one starts. Absent from older daemons.
    #[serde(default)]
    pub outcome: Option<TurnOutcome>,
    /// Set exactly when `needs_input` is; absent from a daemon before #182.
    #[serde(default)]
    pub question_source: Option<QuestionSource>,
    /// The agent's own question an open dialog covers; it is open again once the dialog is
    /// answered. Set only while the open question is a dialog's; absent from a daemon before
    /// #250.
    #[serde(default)]
    pub covered_question: Option<String>,
    /// Seconds since 1970.
    pub reported_at: i64,
}
