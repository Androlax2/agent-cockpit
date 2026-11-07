//! The agent channel's wire types (report 17): what a sandboxed agent sends on the agent
//! socket, and the owner's view of what agents reported.

use serde::{Deserialize, Serialize};

/// The variable a launcher passes the launch token in. The agent sends it with its reports;
/// the daemon binds it only from the launch's own process tree.
pub const LAUNCH_TOKEN_VARIABLE: &str = "TORLEK_LAUNCH_TOKEN";

/// The variable a launcher passes the agent socket's path in, for sandboxes where the
/// default path is mounted elsewhere.
pub const AGENT_SOCKET_VARIABLE: &str = "TORLEK_AGENT_SOCKET";

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
    /// state or a turn's outcome), a new launch or a new conversation.
    Dialog,
}

/// What triggered a compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompactionTrigger {
    /// The harness compacted by itself, near the end of its window.
    Auto,
    /// The owner (or the agent) asked for it, as with `/compact`.
    Manual,
}

impl CompactionTrigger {
    /// The trigger's wire name, as serialized and as `--compaction-trigger` spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            CompactionTrigger::Auto => "auto",
            CompactionTrigger::Manual => "manual",
        }
    }
}

/// Which moment of a compaction a report announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompactionPhase {
    Started,
    Ended,
    Failed,
}

impl CompactionPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            CompactionPhase::Started => "started",
            CompactionPhase::Ended => "ended",
            CompactionPhase::Failed => "failed",
        }
    }
}

/// One moment of a context compaction, as the harness announced it (#782). It proves nothing
/// about what the agent does: a compaction can run in the middle of a turn. The daemon
/// measures the seconds between a start and its end by its own clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionReport {
    pub phase: CompactionPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<CompactionTrigger>,
    /// The context's size in tokens before the compaction, when the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_before: Option<u64>,
    /// The context's size in tokens after it, when the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_after: Option<u64>,
}

/// How a recorded compaction stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompactionStatus {
    /// Started, not yet ended.
    Running,
    Completed,
    /// The harness reported a failure, or started another compaction without ending this one.
    Failed,
}

/// A session's latest compaction in its current launch, as its card shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionView {
    pub status: CompactionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<CompactionTrigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_before: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_after: Option<u64>,
    /// Whole seconds from the start to the end, when both were seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
    /// Seconds since 1970 of the last report about this compaction.
    pub at: i64,
}

/// The harness's own context usage at the end of a turn (#479), in its own tokens: what its
/// last model call was given. Never converted from or into Cockpit's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsage {
    pub tokens: u64,
    /// The model's context window in tokens, when the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
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
    /// A moment of a context compaction. Comes alone (with `conversation`): it changes
    /// nothing else in the report (#782).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionReport>,
    /// The harness's context at the end of the turn `outcome` reports; it comes only with an
    /// outcome (#479).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUsage>,
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
    /// The coordinator session that holds the agent's own question before the owner (#710):
    /// it started this session, and the question is not yet escalated nor past its time.
    /// Absent while the question waits on the owner, who may answer it either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_by: Option<String>,
    /// Seconds since 1970.
    pub reported_at: i64,
    /// The latest compaction of the session's current launch. Absent from older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionView>,
}
