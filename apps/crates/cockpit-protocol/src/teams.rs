//! Team wire contracts; deserialization does not grant authority.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskKey {
    pub project: String,
    pub task_id: String,
}

impl TaskKey {
    /// A task is a todo (8 lowercase hex digits) or a GitHub issue (`owner/repo#N`) in a named project.
    pub fn validate(&self) -> Result<(), TeamError> {
        let invalid = |message: &str| TeamError::new(ErrorCode::Invalid, message);
        if self.project.trim().is_empty()
            || self.project.chars().count() > 300
            || self
                .project
                .chars()
                .any(|character| character.is_control() && character != '\n')
        {
            return Err(invalid(
                "task project is empty, oversized or contains controls",
            ));
        }
        let is_todo = self.task_id.len() == 8
            && self
                .task_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        let issue = regex::Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+#[1-9][0-9]*$")
            .expect("constant issue pattern");
        if self.task_id.len() > 256 || (!is_todo && !issue.is_match(&self.task_id)) {
            return Err(invalid("task must name a todo or issue"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Designer,
    Developer,
    Qa,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub session_id: String,
    pub role: Role,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub submission_key: String,
    pub task: TaskKey,
    pub members: Vec<Member>,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkProposal {
    pub submission_key: String,
    pub task: TaskKey,
    pub peer_session_id: String,
    pub purpose: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkSubmission {
    pub link_id: String,
    pub submission_key: String,
    pub kind: MessageKind,
    pub body: String,
    pub thread_id: Option<String>,
}

/// A teammate an agent asks Theo to start, with what it will be told first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recruit {
    pub role: Role,
    pub brief: String,
}

/// An agent's request for missing teammates: it joins as `role`; the recruits start only after
/// Theo approves, in the global default agent recorded with the request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recruitment {
    pub submission_key: String,
    pub task: TaskKey,
    pub role: Role,
    pub recruits: Vec<Recruit>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedSession {
    pub session_id: String,
    pub agent: String,
    pub project: String,
    pub runtime: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostAction {
    LinkPropose {
        request_id: String,
        requester_session_id: String,
        proposal: LinkProposal,
    },
    LinkApprove {
        request_id: String,
        proposal_id: String,
        expected_revision: i64,
        mapping_hash: String,
    },
    LinkEdit {
        request_id: String,
        proposal_id: String,
        expected_revision: i64,
        replacement: LinkProposal,
    },
    LinkReject {
        request_id: String,
        proposal_id: String,
        expected_revision: i64,
    },
    LinkRevoke {
        request_id: String,
        link_id: String,
    },
    Approve {
        request_id: String,
        proposal_id: String,
        expected_revision: i64,
        mapping_hash: String,
    },
    Edit {
        request_id: String,
        proposal_id: String,
        expected_revision: i64,
        replacement: Proposal,
    },
    Reject {
        request_id: String,
        proposal_id: String,
        expected_revision: i64,
    },
    Unlink {
        request_id: String,
        task: TaskKey,
        session_id: String,
    },
    /// Theo closes an escalated clarification; no agent action can.
    Resolve {
        request_id: String,
        thread_id: String,
        note: String,
    },
    /// Theo's own team request: pending until he approves its exact mapping like any other.
    Propose {
        request_id: String,
        proposal: Proposal,
    },
    /// An uncertain delivery: "handled" (Theo confirmed it arrived) or "cancel". Never a resend.
    Reconcile {
        request_id: String,
        message_id: String,
        decision: String,
    },
    /// Theo has seen a team start that failed or could not join: it leaves the view and the count.
    Dismiss {
        request_id: String,
        token: String,
    },
}

/// What only programs outside the sandbox send, on the host socket no sandbox can reach.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostRequest {
    Control {
        action: HostAction,
    },
    Snapshot,
    PrepareSleep {
        session_id: String,
    },
    SleepRecorded {
        session_id: String,
        token: String,
    },
    CancelSleep {
        session_id: String,
        token: String,
    },
    BeginWake {
        session_id: String,
        token: String,
    },
    SleepStatus {
        session_id: String,
        token: String,
    },
    /// `cockpit restart` keeps a member's team through its own exit and relaunch; cancel and
    /// status go through the sleep requests, which name the permit by its token.
    PrepareRestart {
        session_id: String,
    },
    Chief {
        action: crate::chief::ChiefHostAction,
    },
    ChiefSnapshot {
        after: i64,
        limit: usize,
        #[serde(default)]
        generation: Option<String>,
    },
    ChiefAudit {
        after: i64,
        limit: usize,
        #[serde(default)]
        generation: Option<String>,
    },
    ReserveManual {
        task_id: String,
        directory: String,
    },
    ReleaseManual {
        task_id: String,
        directory: String,
        token: String,
    },
    SettleReservation {
        token: String,
        expected_revision: i64,
        outcome: String,
    },
    ReserveTask {
        task_id: String,
        directory: String,
        worker: Option<String>,
        automatic: bool,
    },
    ReserveResume {
        session: String,
        directory: String,
    },
    ChiefFocus {
        viewed: String,
        workspace_active: bool,
        gaming: bool,
    },
    /// Published only by a host-native verifier, never the sandbox plugin or a session report.
    ChiefNativeObservation {
        session: VerifiedSession,
        idle: bool,
        pending_input: bool,
        observed_at_ms: i64,
    },
    /// An explicit host attestation after the required real draft/queue verification.
    VerifyChiefAdmission {
        session: VerifiedSession,
        adapter: String,
        evidence: String,
    },
    /// Runtime discovery only; this grants neither task execution nor team membership.
    SubscriptionRuntime {
        generation: String,
    },
    SubscriptionSource {
        session: String,
    },
    /// Retired-source replacement: bind all task ownership before opening its prompt gate.
    SubscriptionBind {
        origin: String,
        generation: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Invalid,
    Unauthorized,
    Conflict,
    Storage,
    NotFound,
    Unavailable,
    Limit,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeamError {
    pub code: ErrorCode,
    pub message: String,
}

impl TeamError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TeamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for TeamError {}
impl From<serde_json::Error> for TeamError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(ErrorCode::Storage, error.to_string())
    }
}
impl From<std::io::Error> for TeamError {
    fn from(error: std::io::Error) -> Self {
        Self::new(ErrorCode::Storage, error.to_string())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeamReply {
    pub request_id: String,
    pub result: Result<Value, TeamError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Clarification,
    Reply,
    Handoff,
    QaRequest,
    Findings,
    Notice,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Submission {
    pub task: TaskKey,
    pub submission_key: String,
    pub recipient: String,
    pub kind: MessageKind,
    pub body: String,
    pub thread_id: Option<String>,
    pub handoff_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeCounts {
    pub running_sessions: u32,
    pub pending_inbox: u32,
    pub pending_permissions: u32,
    pub pending_forms: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentAction {
    LinkPropose {
        proposal: LinkProposal,
    },
    LinkSend {
        message: LinkSubmission,
    },
    LinkAssess {
        link_id: String,
        thread_id: String,
        resolved: bool,
    },
    Context,
    /// Runtime-reported native facts, not readiness, identity, approval or host verification.
    NativeTelemetry {
        sequence: i64,
        counts: Option<NativeCounts>,
    },
    Chief {
        request: crate::chief::ChiefAgentAction,
    },
    Propose {
        proposal: Proposal,
    },
    Send {
        message: Submission,
    },
    /// The recipient's own adapter asks for its next message; the daemon decides whether it may wake.
    Receive,
    Receipt {
        dispatch_id: String,
        outcome: TransportOutcome,
    },
    /// The asker's verdict on the latest reply in its clarification thread.
    Assess {
        task: TaskKey,
        thread_id: String,
        resolved: bool,
    },
    /// The Developer's completed work: it supersedes earlier handoffs and requests QA once.
    Handoff {
        handoff: Handoff,
    },
    /// Missing teammates, started only after Theo approves.
    Recruit {
        recruitment: Recruitment,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameReference {
    pub file: String,
    pub frame_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handoff {
    pub task: TaskKey,
    pub submission_key: String,
    pub branch: String,
    /// The full commit id QA reviews (SHA-1 or SHA-256); never a short or symbolic ref.
    pub commit: String,
    pub checkout: String,
    pub summary: String,
    pub tests: String,
    pub figcraft: Vec<FrameReference>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dispatch {
    pub id: String,
    pub message_id: String,
    pub envelope: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransportOutcome {
    Received { native_id: String },
    NotSubmitted { reason: String },
    Uncertain { reason: String },
}
