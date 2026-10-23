//! Protected global-dispatcher contract. Wire types do not confer authorization or readiness.

pub mod store;
pub mod evidence;
pub mod view;
pub use store::reconcile_todos;

use crate::teams::{ErrorCode, Role, TaskKey, TeamError, VerifiedSession};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_ACTIVE_WORK: usize = 256;
pub const MAX_OPEN_DECISIONS: usize = 100;
pub const MAX_CONTEXT_RECORDS: usize = 50;
// Reserve space for the TeamReply wrapper and cursors beneath the agent transport's 65,536 bytes.
const CONTEXT_BYTES: usize = 60_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkRef {
    pub task: TaskKey,
    pub revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkBrief {
    pub work: WorkRef,
    pub brief: String,
    pub role: Role,
    pub prerequisites: Vec<WorkRef>,
    pub gates: Vec<GateRequirement>,
}

/// Untrusted scope proposal. The host supplies revision, identity, provenance and hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkProposal {
    pub task: TaskKey,
    pub brief: String,
    pub role: Role,
    pub prerequisites: Vec<WorkRef>,
    pub gates: Vec<GateRequirement>,
}

impl WorkProposal {
    fn at_revision(&self, revision: i64) -> WorkBrief {
        WorkBrief { work: WorkRef { task:self.task.clone(), revision }, brief:self.brief.clone(), role:self.role,
            prerequisites:self.prerequisites.clone(), gates:self.gates.clone() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateKind { Tests, Qa, Apply }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateRequirement {
    pub kind: GateKind,
    pub source: String,
    /// Kind-specific expected fields are checked by the evidence boundary, not interpreted as authority.
    pub expected: Value,
}

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
    ProposeWork { submission_key: String, proposal: WorkProposal },
    Context { after: i64, limit: usize },
    ContextSnapshot { after: i64, limit: usize, #[serde(default)] generation: Option<String> },
    SummaryResponse { request_id: String, sections: SummarySections, evidence_ids: Vec<String> },
    Dispatch { submission_key: String, work: WorkRef, worker: String },
    Reprioritize { submission_key: String, ordered: Vec<WorkRef> },
    Accept { assignment: String },
    Status { assignment: Option<String>, summary: String },
    Decision { work: Option<WorkRef>, key: String, question: String, recommendation: String },
    Coordinate { submission_key: String, work: WorkRef, recipient: String, body: String },
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
    ApproveWork { request_id: String, brief: WorkBrief, approval_hash: String, expected_revision: i64 },
    RevokeWork { request_id: String, work: WorkRef },
    BindWorker { request_id: String, session: VerifiedSession, role: Role, expected_epoch: Option<String> },
    Reconcile { request_id: String, assignment: String, expected_revision: i64, evidence: ReconciliationEvidence },
    ReconcileDelivery { request_id: String, message: String, expected_revision: i64, evidence: ReconciliationEvidence },
    ReconcileLaunch { request_id: String, token: String, expected_revision: i64, evidence: ReconciliationEvidence },
    Resolve { request_id: String, decision: String, expected_revision: i64, resolution: String },
    RequestSummary { request_id: String },
    AttestEvidence { request_id: String, evidence: Evidence },
    ImportEvidence { request_id: String, work: WorkRef, gate: GateKind, source_id: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub id: String,
    pub work: WorkRef,
    pub kind: GateKind,
    pub commit: String,
    pub source: String,
    pub result: Value,
}

fn invalid(message: &str) -> TeamError { TeamError::new(ErrorCode::Invalid, message) }

fn review_payload(value: &impl Serialize) -> Result<(), TeamError> {
    // Reserve room for full host identity/provenance metadata, not just the payload itself.
    if serde_json::to_vec(value)?.len() > 56_000 { return Err(TeamError::new(ErrorCode::Limit,"chief payload exceeds host review bound")); }
    Ok(())
}

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

fn source(value: &str) -> Result<(), TeamError> {
    if !matches!(value, "host_run" | "qa" | "reviewed_apply" | "host_attestation") {
        return Err(invalid("unsupported chief evidence source"));
    }
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

impl WorkRef {
    pub fn validate(&self) -> Result<(), TeamError> {
        positive(self.revision)?;
        text(&self.task.project, 300)?;
        let todo = self.task.task_id.len() == 8 && self.task.task_id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        let issue = regex::Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+#[1-9][0-9]*$").expect("constant issue pattern");
        if self.task.task_id.len() > 256 || (!todo && !issue.is_match(&self.task.task_id)) {
            return Err(invalid("chief work must name a todo or issue"));
        }
        Ok(())
    }
}

impl WorkBrief {
    pub fn validate(&self) -> Result<(), TeamError> {
        self.work.validate()?;
        text(&self.brief, 4000)?;
        if self.prerequisites.len() > MAX_ACTIVE_WORK || self.gates.len() > 3 {
            return Err(invalid("too many chief prerequisites or gates"));
        }
        for (index, prerequisite) in self.prerequisites.iter().enumerate() {
            prerequisite.validate()?;
            if prerequisite.task == self.work.task || self.prerequisites[..index].contains(prerequisite) {
                return Err(invalid("chief prerequisites cannot repeat or depend on their own work"));
            }
        }
        for (index, gate) in self.gates.iter().enumerate() {
            source(&gate.source)?;
            evidence::validate_requirement(gate)?;
            if !gate.expected.is_object() || self.gates[..index].iter().any(|previous| previous.kind == gate.kind) {
                return Err(invalid("chief gates need distinct kinds and structured expectations"));
            }
        }
        review_payload(self)?;
        Ok(())
    }

    pub fn approval_hash(&self) -> Result<String, TeamError> {
        self.validate()?;
        Ok(crate::audit::sha256_hex(&serde_json::to_vec(self)?))
    }
}

impl ChiefAgentAction {
    pub fn validate(&self) -> Result<(), TeamError> {
        match self {
            Self::ProposeWork { submission_key, proposal } => { key(submission_key)?; proposal.at_revision(1).validate() },
            Self::Context { after, limit } => validate_cursor(*after, *limit),
            Self::ContextSnapshot {after,limit,generation} => {validate_cursor(*after,*limit)?; if let Some(generation)=generation {text(generation,64)?;} Ok(())},
            Self::SummaryResponse {request_id,sections,evidence_ids} => {
                key(request_id)?;
                for section in [&sections.completed,&sections.active,&sections.decisions,&sections.idle] {text(section,1000)?;}
                if evidence_ids.len()>20 {return Err(invalid("summary has too many evidence references"))}
                for reference in evidence_ids {key(reference)?;}
                Ok(())
            },
            Self::Dispatch { submission_key, work, worker } => { key(submission_key)?; work.validate()?; text(worker, 256) },
            Self::Reprioritize { submission_key, ordered } => {
                key(submission_key)?;
                if ordered.is_empty() || ordered.len() > MAX_ACTIVE_WORK { return Err(invalid("invalid chief priority list size")); }
                for (index, work) in ordered.iter().enumerate() {
                    work.validate()?;
                    if ordered[..index].iter().any(|previous| previous.task == work.task) { return Err(invalid("duplicate chief work identity")); }
                }
                Ok(())
            },
            Self::Accept { assignment } => key(assignment),
            Self::Status { assignment, summary } => {
                if let Some(assignment) = assignment { key(assignment)?; }
                text(summary, 4000)
            },
            Self::Decision { work, key: decision_key, question, recommendation } => {
                if let Some(work) = work { work.validate()?; }
                key(decision_key)?; text(question, 1000)?; text(recommendation, 1000)
            },
            Self::Coordinate { submission_key, work, recipient, body } => { key(submission_key)?; work.validate()?; text(recipient, 256)?; text(body, 4000) },
        }
    }
}

impl ChiefHostAction {
    pub fn validate(&self) -> Result<(), TeamError> {
        let request_id = match self {
            Self::Enable { request_id, .. } | Self::Pause { request_id, .. } | Self::Resume { request_id, .. }
            | Self::Disable { request_id, .. } | Self::ApproveWork { request_id, .. } | Self::RevokeWork { request_id, .. }
            | Self::BindWorker { request_id, .. } | Self::Reconcile { request_id, .. } | Self::Resolve { request_id, .. }
            | Self::RequestSummary { request_id } | Self::AttestEvidence { request_id, .. } | Self::ImportEvidence { request_id, .. } | Self::ReconcileDelivery {request_id,..} | Self::ReconcileLaunch {request_id,..} => request_id,
        };
        key(request_id)?;
        match self {
            Self::Enable { session: verified, .. } => session(verified),
            Self::Pause { expected_epoch, .. } | Self::Resume { expected_epoch, .. } | Self::Disable { expected_epoch, .. } => key(expected_epoch),
            Self::ApproveWork { brief, approval_hash, expected_revision, .. } => {
                if *expected_revision < 0 || brief.work.revision != expected_revision.checked_add(1).ok_or_else(|| invalid("revision overflow"))?
                    || brief.approval_hash()? != *approval_hash {
                    return Err(invalid("chief approval must bind the exact next work revision and brief hash"));
                }
                Ok(())
            },
            Self::RevokeWork { work, .. } => work.validate(),
            Self::BindWorker { session: verified, expected_epoch, .. } => {
                session(verified)?;
                if let Some(epoch) = expected_epoch { key(epoch)?; }
                Ok(())
            },
            Self::Reconcile { assignment, expected_revision, evidence, .. } | Self::ReconcileDelivery {message:assignment,expected_revision,evidence,..} | Self::ReconcileLaunch {token:assignment,expected_revision,evidence,..} => {
                key(assignment)?; positive(*expected_revision)?;
                match evidence { ReconciliationEvidence::Handled { note } | ReconciliationEvidence::SafeStop { note, .. } => text(note, 1000) }
            },
            Self::Resolve { decision, expected_revision, resolution, .. } => { key(decision)?; positive(*expected_revision)?; text(resolution, 1000) },
            Self::RequestSummary { .. } => Ok(()),
            Self::AttestEvidence { evidence, .. } => {
                evidence.validate()?;
                if evidence.source != "host_attestation" { return Err(invalid("attestation must identify its host approval provenance")); }
                Ok(())
            },
            Self::ImportEvidence { work, source_id, .. } => { work.validate()?; key(source_id) },
        }
    }
}

impl Evidence {
    pub fn validate(&self) -> Result<(), TeamError> {
        key(&self.id)?;
        self.work.validate()?;
        source(&self.source)?;
        if !matches!(self.commit.len(), 40 | 64) || !self.commit.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || !self.result.is_object() {
            return Err(invalid("chief evidence requires a full commit and structured result"));
        }
        review_payload(self)?;
        Ok(())
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
