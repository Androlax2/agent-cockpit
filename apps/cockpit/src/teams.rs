//! Task-scoped collaboration. Only host controls can grant or revoke membership.

pub mod store;
pub mod client;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskKey {
    pub project: String,
    pub task_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role { Designer, Developer, Qa }

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
    Approve { request_id: String, proposal_id: String, expected_revision: i64, mapping_hash: String },
    Edit { request_id: String, proposal_id: String, expected_revision: i64, replacement: Proposal },
    Reject { request_id: String, proposal_id: String, expected_revision: i64 },
    Unlink { request_id: String, task: TaskKey, session_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode { Invalid, Unauthorized, Conflict, Storage, NotFound, Unavailable, Limit }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeamError {
    pub code: ErrorCode,
    pub message: String,
}

impl TeamError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self { Self { code, message: message.into() } }
}

impl std::fmt::Display for TeamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { formatter.write_str(&self.message) }
}
impl std::error::Error for TeamError {}
impl From<rusqlite::Error> for TeamError {
    fn from(error: rusqlite::Error) -> Self { Self::new(ErrorCode::Storage, error.to_string()) }
}
impl From<serde_json::Error> for TeamError {
    fn from(error: serde_json::Error) -> Self { Self::new(ErrorCode::Storage, error.to_string()) }
}
impl From<std::io::Error> for TeamError {
    fn from(error: std::io::Error) -> Self { Self::new(ErrorCode::Storage, error.to_string()) }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeamReply {
    pub request_id: String,
    pub result: Result<Value, TeamError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind { Clarification, Reply, Handoff, QaRequest, Findings, Notice }

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
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentAction {
    Context,
    Propose { proposal: Proposal },
    Send { message: Submission },
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
