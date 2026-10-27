//! Protected Chief contract. Wire types do not confer authorization or readiness.

pub mod bundle;
pub mod store;
pub mod view;
pub use store::reconcile_todos;

use crate::teams::{ErrorCode, Role, TeamError, VerifiedSession};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A complete host review, never a mixture of pagination generations.
pub fn host_snapshot() -> Result<Value, TeamError> {
    use crate::teams::{HostRequest, client::host_request};
    for _ in 0..3 {
        let read_at_ms = chrono::Utc::now().timestamp_millis();
        let mut records = Vec::new();
        let (mut after, mut generation, mut envelope) = (0, None, None);
        let mut session_names = serde_json::Map::new();
        for _ in 0..512 {
            let page = host_request(&HostRequest::ChiefSnapshot {after, limit:50, generation:generation.clone()})?.result?;
            if page["invalidated"] == true {break;}
            if page["projection"] != "host_review" || page["cursor_kind"] != "snapshot_offset" {
                return Err(TeamError::new(ErrorCode::Unavailable, "Chief host review projection is missing"));
            }
            let current = page["generation"].as_str().ok_or_else(||TeamError::new(ErrorCode::Unavailable,"Chief snapshot generation is missing"))?.to_string();
            if generation.as_ref().is_some_and(|generation|generation != &current) {break;}
            generation = Some(current);
            let batch = page["records"].as_array().ok_or_else(||TeamError::new(ErrorCode::Unavailable,"Chief snapshot records are missing"))?;
            records.extend(batch.iter().cloned());
            if envelope.is_none() {envelope = Some(page.clone());}
            if let Some(names) = page["session_names"].as_object() {session_names.extend(names.clone());}
            if page["has_more"] == false {
                let mut complete = envelope.expect("first page is present");
                complete["records"] = serde_json::json!(records);
                complete["has_more"] = serde_json::json!(false);
                complete["session_names"] = serde_json::json!(session_names);
                complete["read_at_ms"] = serde_json::json!(read_at_ms);
                return Ok(complete);
            }
            after = page["next"].as_i64().filter(|next|*next > after)
                .ok_or_else(||TeamError::new(ErrorCode::Unavailable,"Chief snapshot cursor did not advance"))?;
        }
    }
    Err(TeamError::new(ErrorCode::Unavailable,"Chief host snapshot changed repeatedly or exceeded its bounded read; try opening again"))
}

pub const MAX_OPEN_DECISIONS: usize = 100;
pub const MAX_CONTEXT_RECORDS: usize = 50;
/// How many habits one answer may name as the ones it relied on.
pub const MAX_CITED_HABITS: usize = 5;
/// The limit of a Chief answer, in the words Theo approved. Told to the Chief in its wake text and
/// appended by the host to every answer the agent receives.
pub const CHIEF_ANSWER_BOUNDARY: &str = "Chief answer on Theo's behalf. It authorises edits, builds, tests and commits inside your current task only. Installs, dependency changes, pushes, merges, default-branch or other Git writes your project does not already allow, anything outside the task, and anything security-related still need Theo. Permission prompts, ask-to-run and cockpit apply are always Theo's.";
/// What the host appends to Theo's own answer to an escalation, in place of the Chief's boundary.
pub const THEO_ANSWER_LABEL: &str = "Answer from Theo, sent through the cockpit Chief view.";
/// What the host appends to Theo's correction of an earlier Chief answer.
pub const CORRECTION_LABEL: &str = "Correction from Theo, sent through the cockpit Chief view. Treat it as a new instruction inside your current task. If following it means undoing work or changing the task's scope, stop and ask Theo.";
/// Why a started Chief waits: its exact session has no recorded native admission verification yet.
pub const ADMISSION_REQUIRED: &str = "host live OpenCode draft/queue verification is required before chief activation";
/// The adapter Theo's confirmed delivery check verifies.
pub const ADMISSION_ADAPTER: &str = "opencode-queued-v1";
/// How long a stopped agent keeps its stop to itself (or to Theo) before a Chief answer may reach it.
pub const CHIEF_ANSWER_GRACE_S: i64 = 30;
// Reserve space for the TeamReply wrapper and cursors beneath the agent transport's 65,536 bytes.
const CONTEXT_BYTES: usize = 60_000;

/// When Theo last saw his habits, on the Habits page or in a digest (epoch ms); a missing or unreadable note, or one from the future, counts as never.
pub fn habits_seen_ms() -> Option<i64> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    std::fs::read_to_string(crate::paths::chief_habits_seen_at()).ok().and_then(|text| text.trim().parse::<i64>().ok()).filter(|seen_ms| (1..=now_ms).contains(seen_ms))
}

/// Saves when Theo saw his habits, never earlier than the time already saved: the page and the digest each read them at their own time.
pub fn save_habits_seen(seen_ms: i64) -> std::io::Result<()> {
    if habits_seen_ms().is_some_and(|saved_ms| saved_ms >= seen_ms) { return Ok(()); }
    let note = crate::paths::chief_habits_seen_at();
    if let Some(folder) = note.parent() { std::fs::create_dir_all(folder)?; }
    std::fs::write(note, seen_ms.to_string())
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
    Context { after: i64, limit: usize },
    ContextSnapshot { after: i64, limit: usize, #[serde(default)] generation: Option<String> },
    SummaryResponse { request_id: String, sections: SummarySections },
    Status { summary: String },
    Decision { key: String, question: String, recommendation: String },
    /// Reply to an open stop; the host delivers it to the stopped agent with `CHIEF_ANSWER_BOUNDARY`.
    /// `habits` names the habits the answer relied on.
    Answer { stop: String, text: String, reason: String, #[serde(default)] habits: Vec<String> },
    /// Settle an open single-question form with one of its options, named by label or value; the
    /// agent's plugin replies through OpenCode's form API.
    AnswerForm { stop: String, choice: String, reason: String, #[serde(default)] habits: Vec<String> },
    /// Record a lasting preference Theo showed in his own answer (`source_id`: his answer or the stop
    /// whose bundle shows it), for one project or, with no project, everywhere.
    AddHabit { text: String, project: Option<String>, source_id: String, reason: String },
    /// Hand an open stop to Theo, who answers it from the Chief view.
    Escalate { stop: String, category: EscalationCategory, reason: String, #[serde(default)] suggested_answer: Option<String> },
}

/// Why the Chief did not answer a stop itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationCategory { Security, Scope, NotSure }

impl EscalationCategory {
    pub fn as_str(self) -> &'static str {
        match self { Self::Security => "security", Self::Scope => "scope", Self::NotSure => "not_sure" }
    }
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
    Start { request_id: String, previous_attempt: Option<String> },
    /// Theo's confirmation, after checking, that no session remains for an unresolved startup attempt.
    SettleStartup { request_id: String, attempt: String, expected_revision: i64, evidence: ReconciliationEvidence },
    /// Theo's test message to the exact live session of an unresolved startup attempt, which he then
    /// confirms he saw arrive (VerifyChiefAdmission). Never a launch.
    SendCheck { request_id: String, attempt: String },
    Enable { request_id: String, session: VerifiedSession },
    Pause { request_id: String, expected_epoch: String },
    Resume { request_id: String, expected_epoch: String },
    Disable { request_id: String, expected_epoch: String },
    ReconcileDelivery { request_id: String, message: String, expected_revision: i64, evidence: ReconciliationEvidence },
    ReconcileLaunch { request_id: String, token: String, expected_revision: i64, evidence: ReconciliationEvidence },
    Resolve { request_id: String, decision: String, expected_revision: i64, resolution: String },
    /// Theo's answer to an escalated stop: resolves the decision and queues the text for the stopped agent.
    AnswerDecision { request_id: String, decision: String, expected_revision: i64, answer: String },
    /// Theo's correction of a sent answer: queued for the session that got the answer, once per answer.
    Overrule { request_id: String, answer: String, correction: String },
    /// Theo deletes a habit; the Chief stops using it and never adds it back by itself.
    DeleteHabit { request_id: String, habit: String },
    RestoreHabit { request_id: String, habit: String },
    /// Theo widens a habit to every project, or narrows it back to the project it came from.
    RescopeHabit { request_id: String, habit: String, everywhere: bool },
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

/// The host adds these labels itself; text that already carries one could pass a Chief answer off as
/// Theo's, or hide where the host's boundary starts.
fn without_host_labels(value: &str) -> Result<(), TeamError> {
    if value.contains(CHIEF_ANSWER_BOUNDARY) || value.contains(THEO_ANSWER_LABEL) || value.contains(CORRECTION_LABEL) { return Err(invalid("the host adds the answer labels itself")); }
    Ok(())
}

fn cited_habits(habits: &[String]) -> Result<(), TeamError> {
    if habits.len() > MAX_CITED_HABITS { return Err(invalid("an answer names at most 5 habits")); }
    for (index, habit) in habits.iter().enumerate() {
        key(habit)?;
        if habits[..index].contains(habit) { return Err(invalid("an answer names each habit once")); }
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
                key(decision_key)?;
                // `escalate` files its decision under `stop-<stop>`; a plain decision must not rewrite or collide with it.
                if decision_key.starts_with("stop-") { return Err(invalid("decision keys starting with stop- are reserved for escalations")); }
                text(question, 1000)?; text(recommendation, 1000)
            },
            Self::Answer { stop, text: answer, reason, habits } => { key(stop)?; text(answer, 4000)?; without_host_labels(answer)?; text(reason, 300)?; cited_habits(habits) },
            Self::AnswerForm { stop, choice, reason, habits } => { key(stop)?; text(choice, 200)?; text(reason, 300)?; cited_habits(habits) },
            Self::AddHabit { text: habit, project, source_id, reason } => {
                text(habit, 500)?;
                if habit.contains('\n') { return Err(invalid("a habit is a single line")); }
                // The host stores the habit with its whitespace collapsed: check what it would store.
                without_host_labels(&store::habit_text(habit))?;
                if let Some(project) = project {
                    text(project, 300)?;
                    if project.contains('\n') { return Err(invalid("a project key is a single line")); }
                }
                key(source_id)?; text(reason, 300)
            },
            Self::Escalate { stop, reason, suggested_answer, .. } => {
                key(stop)?; text(reason, 300)?;
                if let Some(suggestion) = suggested_answer { text(suggestion, 2000)?; without_host_labels(suggestion)?; }
                Ok(())
            },
        }
    }
}

impl ChiefHostAction {
    pub fn validate(&self) -> Result<(), TeamError> {
        let request_id = match self {
            Self::Start { request_id, .. } | Self::SettleStartup { request_id, .. } | Self::SendCheck { request_id, .. } | Self::Enable { request_id, .. } | Self::Pause { request_id, .. } | Self::Resume { request_id, .. }
            | Self::Disable { request_id, .. } | Self::Resolve { request_id, .. } | Self::AnswerDecision { request_id, .. } | Self::RequestSummary { request_id }
            | Self::ReconcileDelivery {request_id,..} | Self::ReconcileLaunch {request_id,..} | Self::Overrule { request_id, .. }
            | Self::DeleteHabit { request_id, .. } | Self::RestoreHabit { request_id, .. } | Self::RescopeHabit { request_id, .. } => request_id,
        };
        key(request_id)?;
        match self {
            Self::Start { previous_attempt, .. } => previous_attempt.as_deref().map_or(Ok(()), key),
            Self::SendCheck { attempt, .. } => key(attempt),
            Self::Enable { session: verified, .. } => session(verified),
            Self::Pause { expected_epoch, .. } | Self::Resume { expected_epoch, .. } | Self::Disable { expected_epoch, .. } => key(expected_epoch),
            Self::ReconcileDelivery {message:target,expected_revision,evidence,..} | Self::ReconcileLaunch {token:target,expected_revision,evidence,..}
            | Self::SettleStartup {attempt:target,expected_revision,evidence,..} => {
                key(target)?; positive(*expected_revision)?;
                match evidence { ReconciliationEvidence::Handled { note } | ReconciliationEvidence::SafeStop { note, .. } => text(note, 1000) }
            },
            Self::Resolve { decision, expected_revision, resolution, .. } => { key(decision)?; positive(*expected_revision)?; text(resolution, 1000) },
            Self::AnswerDecision { decision, expected_revision, answer, .. } => { key(decision)?; positive(*expected_revision)?; text(answer, 4000) },
            Self::Overrule { answer, correction, .. } => {
                key(answer)?; text(correction, 2000)?;
                if correction.contains('\n') { return Err(invalid("a correction is a single line")); }
                without_host_labels(correction)
            },
            Self::DeleteHabit { habit, .. } | Self::RestoreHabit { habit, .. } | Self::RescopeHabit { habit, .. } => key(habit),
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
