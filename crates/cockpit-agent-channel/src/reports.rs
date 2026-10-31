//! Agents' reports: each session's latest state, label, doing-now and open question.
//!
//! The channel keeps them until `cockpit-sessions` (#31) owns session state; it then moves
//! these rows there and keeps only the identification step.

use cockpit_authorization::{Actor, Outcome, Permission, Scope, SessionId};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::launch::{PeerAncestry, identify, unix_seconds};
use crate::{Error, Refusal};

const LABEL_MAX_CHARS: usize = 60;
const DOING_MAX_CHARS: usize = 80;
const DOING_WORDS: std::ops::RangeInclusive<usize> = 2..=12;
const QUESTION_MAX_CHARS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Working,
    Waiting,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::Waiting => "waiting",
        }
    }

    fn parse(text: &str) -> Option<AgentState> {
        [AgentState::Working, AgentState::Waiting]
            .into_iter()
            .find(|state| state.as_str() == text)
    }
}

/// What an agent reports; each field it leaves out keeps its previous value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub state: Option<AgentState>,
    /// The session's overall task, 1 to 60 characters.
    pub label: Option<String>,
    /// The current step, 2 to 12 words and at most 80 characters.
    pub doing: Option<String>,
    /// A question the agent needs answered before it can go on, at most 300 characters.
    /// Reporting `working` clears it: the agent got its answer.
    pub needs_input: Option<String>,
}

/// A session's report as the channel last recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionReport {
    pub session: SessionId,
    pub state: Option<AgentState>,
    pub label: Option<String>,
    pub doing: Option<String>,
    pub needs_input: Option<String>,
    /// Seconds since 1970 of the last report.
    pub reported_at: i64,
}

/// Records `report` for the session the sender is identified as (see [`identify`]), after
/// `cockpit-authorization` allows that session `change-state` over itself. Nested agents
/// therefore report with their parent session's identity and grants.
pub async fn report(
    write: &WriteTransaction<'_>,
    peer: &PeerAncestry,
    token: Option<&str>,
    report: Report,
) -> Result<SessionReport, Error> {
    let report = validate(report)?;
    let session = identify(write, peer, token).await?;
    let decision = cockpit_authorization::evaluate(
        write,
        &Actor::Session(session.clone()),
        Permission::ChangeState,
        &Scope::Session(session.clone()),
    )
    .await?;
    if let Outcome::Refused(refusal) = decision.outcome {
        return Err(Refusal::NotAuthorized(refusal).into());
    }
    let previous = latest_report(write, &session).await?;
    let merged = merge(&session, previous, report);
    write
        .execute(
            "DELETE FROM agent_channel_reports WHERE session = ?1",
            [session.as_str()],
        )
        .await?;
    write
        .execute(
            "INSERT INTO agent_channel_reports
                (session, state, label, doing, needs_input, reported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                merged.session.as_str(),
                merged.state.map(AgentState::as_str),
                merged.label.clone(),
                merged.doing.clone(),
                merged.needs_input.clone(),
                merged.reported_at,
            ),
        )
        .await?;
    Ok(merged)
}

/// Every session's latest report, by session id.
pub async fn latest_reports(connection: &Connection) -> Result<Vec<SessionReport>, Error> {
    let mut rows = connection
        .query(
            "SELECT session, state, label, doing, needs_input, reported_at
             FROM agent_channel_reports ORDER BY session",
            (),
        )
        .await?;
    let mut reports = Vec::new();
    while let Some(row) = rows.next().await? {
        reports.push(report_from(&row)?);
    }
    Ok(reports)
}

async fn latest_report(
    connection: &Connection,
    session: &SessionId,
) -> Result<Option<SessionReport>, Error> {
    let mut rows = connection
        .query(
            "SELECT session, state, label, doing, needs_input, reported_at
             FROM agent_channel_reports WHERE session = ?1",
            [session.as_str()],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(report_from(&row)?)),
        None => Ok(None),
    }
}

fn merge(session: &SessionId, previous: Option<SessionReport>, report: Report) -> SessionReport {
    let previous = previous.unwrap_or(SessionReport {
        session: session.clone(),
        state: None,
        label: None,
        doing: None,
        needs_input: None,
        reported_at: 0,
    });
    let needs_input = match (report.needs_input, report.state) {
        (Some(question), _) => Some(question),
        (None, Some(AgentState::Working)) => None,
        (None, _) => previous.needs_input,
    };
    SessionReport {
        session: session.clone(),
        state: report.state.or(previous.state),
        label: report.label.or(previous.label),
        doing: report.doing.or(previous.doing),
        needs_input,
        reported_at: unix_seconds(),
    }
}

/// Checks each field at the boundary and collapses runs of whitespace. Text reaches terminals
/// and tmux, so control characters and tmux format openers (`#(`, `#{`) are refused.
fn validate(report: Report) -> Result<Report, Refusal> {
    if report == Report::default() {
        return Err(invalid("report", "names no field to report".to_owned()));
    }
    let label = report
        .label
        .map(|label| {
            let label = checked_text("label", &label)?;
            match label.chars().count() {
                0 => Err(invalid("label", "is empty".to_owned())),
                count if count > LABEL_MAX_CHARS => Err(invalid(
                    "label",
                    format!("is {count} characters; at most {LABEL_MAX_CHARS}"),
                )),
                _ => Ok(label),
            }
        })
        .transpose()?;
    let doing = report
        .doing
        .map(|doing| {
            let doing = checked_text("doing", &doing)?;
            let words = doing.split(' ').filter(|word| !word.is_empty()).count();
            if !DOING_WORDS.contains(&words) || doing.chars().count() > DOING_MAX_CHARS {
                return Err(invalid(
                    "doing",
                    format!("must be 2 to 12 words and at most {DOING_MAX_CHARS} characters"),
                ));
            }
            Ok(doing)
        })
        .transpose()?;
    let needs_input = report
        .needs_input
        .map(|question| {
            let question = checked_text("needs-input", &question)?;
            match question.chars().count() {
                0 => Err(invalid("needs-input", "is empty".to_owned())),
                count if count > QUESTION_MAX_CHARS => Err(invalid(
                    "needs-input",
                    format!("is {count} characters; at most {QUESTION_MAX_CHARS}"),
                )),
                _ => Ok(question),
            }
        })
        .transpose()?;
    Ok(Report {
        state: report.state,
        label,
        doing,
        needs_input,
    })
}

fn checked_text(field: &'static str, text: &str) -> Result<String, Refusal> {
    if text.chars().any(char::is_control) {
        return Err(invalid(field, "contains control characters".to_owned()));
    }
    if text.contains("#(") || text.contains("#{") {
        return Err(invalid(field, "contains a tmux format sequence".to_owned()));
    }
    Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn invalid(field: &'static str, reason: String) -> Refusal {
    Refusal::InvalidRequest { field, reason }
}

fn report_from(row: &Row) -> Result<SessionReport, Error> {
    let invalid_row = |reason: String| Error::InvalidStoredRow {
        table: "agent_channel_reports",
        reason,
    };
    let session =
        SessionId::new(row.get::<String>(0)?).map_err(|error| invalid_row(error.to_string()))?;
    let state = row
        .get::<Option<String>>(1)?
        .map(|state| {
            AgentState::parse(&state).ok_or_else(|| invalid_row(format!("state {state:?}")))
        })
        .transpose()?;
    Ok(SessionReport {
        session,
        state,
        label: row.get(2)?,
        doing: row.get(3)?,
        needs_input: row.get(4)?,
        reported_at: row.get(5)?,
    })
}
