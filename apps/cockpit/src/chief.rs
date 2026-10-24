//! Protected Chief contract. Wire types do not confer authorization or readiness.

pub mod store;
pub mod view;
pub use store::reconcile_todos;

use crate::teams::{ErrorCode, Role, TeamError, VerifiedSession};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_OPEN_DECISIONS: usize = 100;
pub const MAX_CONTEXT_RECORDS: usize = 50;
// Reserve space for the TeamReply wrapper and cursors beneath the agent transport's 65,536 bytes.
const CONTEXT_BYTES: usize = 60_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerState {
    pub session: VerifiedSession,
    pub role: Option<Role>,
    pub ready: bool,
    pub blocked_by: Vec<String>,
    pub observed_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChiefAgentAction {
    Context { after: i64, limit: usize },
    ContextSnapshot { after: i64, limit: usize, #[serde(default)] generation: Option<String> },
    SummaryResponse { request_id: String, sections: SummarySections },
    Status { summary: String },
    Decision { key: String, question: String, recommendation: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummarySections {
    pub completed: String,
    pub active: String,
    pub decisions: String,
    pub idle: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReconciliationEvidence {
    Handled { note: String },
    SafeStop { note: String, confirmed_stopped: bool },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChiefHostAction {
    Enable { request_id: String, session: VerifiedSession },
    Pause { request_id: String, expected_epoch: String },
    Resume { request_id: String, expected_epoch: String },
    Disable { request_id: String, expected_epoch: String },
    ReconcileDelivery { request_id: String, message: String, expected_revision: i64, evidence: ReconciliationEvidence },
    ReconcileLaunch { request_id: String, token: String, expected_revision: i64, evidence: ReconciliationEvidence },
    Resolve { request_id: String, decision: String, expected_revision: i64, resolution: String },
    RequestSummary { request_id: String },
}

fn invalid(message: &str) -> TeamError { TeamError::new(ErrorCode::Invalid, message) }

fn text(value: &str, maximum: usize) -> Result<(), TeamError> {
    if value.trim().is_empty() || value.chars().count() > maximum
        || value.chars().any(|character| character.is_control() && character != '\n') {
        return Err(invalid("chief text is empty, oversized or contains controls"));
    }
    Ok(())
}

fn key(value: &str) -> Result<(), TeamError> {
    if value.is_empty() || value.len() > 128 || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) {
        return Err(invalid("chief key must be 1–128 letters, digits, - or _"));
    }
    Ok(())
}

fn positive(revision: i64) -> Result<(), TeamError> {
    if revision < 1 { return Err(invalid("chief revision must be positive")); }
    Ok(())
}

fn session(value: &VerifiedSession) -> Result<(), TeamError> {
    text(&value.session_id, 256)?;
    text(&value.project, 300)?;
    text(&value.runtime, 1000)?;
    if crate::agents::of_session_id(&value.session_id).map(|agent| agent.name) != Some(value.agent.as_str()) {
        return Err(invalid("chief session id and agent disagree"));
    }
    Ok(())
}

impl ChiefAgentAction {
    pub fn validate(&self) -> Result<(), TeamError> {
        match self {
            Self::Context { after, limit } => validate_cursor(*after, *limit),
            Self::ContextSnapshot {after,limit,generation} => {validate_cursor(*after,*limit)?; if let Some(generation)=generation {text(generation,64)?;} Ok(())},
            Self::SummaryResponse {request_id,sections} => {
                key(request_id)?;
                for section in [&sections.completed,&sections.active,&sections.decisions,&sections.idle] {text(section,1000)?;}
                Ok(())
            },
            Self::Status { summary } => text(summary, 4000),
            Self::Decision { key: decision_key, question, recommendation } => {
                key(decision_key)?; text(question, 1000)?; text(recommendation, 1000)
            },
        }
    }
}

impl ChiefHostAction {
    pub fn validate(&self) -> Result<(), TeamError> {
        let request_id = match self {
            Self::Enable { request_id, .. } | Self::Pause { request_id, .. } | Self::Resume { request_id, .. }
            | Self::Disable { request_id, .. } | Self::Resolve { request_id, .. } | Self::RequestSummary { request_id }
            | Self::ReconcileDelivery {request_id,..} | Self::ReconcileLaunch {request_id,..} => request_id,
        };
        key(request_id)?;
        match self {
            Self::Enable { session: verified, .. } => session(verified),
            Self::Pause { expected_epoch, .. } | Self::Resume { expected_epoch, .. } | Self::Disable { expected_epoch, .. } => key(expected_epoch),
            Self::ReconcileDelivery {message:target,expected_revision,evidence,..} | Self::ReconcileLaunch {token:target,expected_revision,evidence,..} => {
                key(target)?; positive(*expected_revision)?;
                match evidence { ReconciliationEvidence::Handled { note } | ReconciliationEvidence::SafeStop { note, .. } => text(note, 1000) }
            },
            Self::Resolve { decision, expected_revision, resolution, .. } => { key(decision)?; positive(*expected_revision)?; text(resolution, 1000) },
            Self::RequestSummary { .. } => Ok(()),
        }
    }
}

pub fn validate_cursor(after: i64, limit: usize) -> Result<(), TeamError> {
    if after < 0 || !(1..=MAX_CONTEXT_RECORDS).contains(&limit) { return Err(invalid("chief cursor needs after >= 0 and limit 1–50")); }
    Ok(())
}

/// Return only whole records; the caller advances its cursor through the returned records alone.
pub fn context_page(records: &[Value], limit: usize) -> Result<Value, TeamError> {
    validate_cursor(0, limit)?;
    let mut page = Vec::new();
    let mut bytes = 2;
    for record in records.iter().take(limit) {
        let size = serde_json::to_vec(record)?.len();
        if size + 2 >= CONTEXT_BYTES { return Err(TeamError::new(ErrorCode::Limit, "chief context record exceeds reply bound")); }
        if bytes + size + 1 >= CONTEXT_BYTES { break; }
        bytes += size + 1;
        page.push(record.clone());
    }
    Ok(Value::Array(page))
}
