//! Agents' reports: each session's latest state, label, doing-now and open question. The
//! agent channel identifies the sender; this module decides what a report records (#146).

use cockpit_agent_channel::{PeerAncestry, Refusal};
use cockpit_authorization::{Actor, Outcome, Permission, Scope, SessionId};
use cockpit_provider_api::TurnOutcome;
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::Error;
use crate::store::now;

const LABEL_MAX_CHARS: usize = 60;
const DOING_MAX_CHARS: usize = 80;
const DOING_WORDS: std::ops::RangeInclusive<usize> = 2..=12;
const QUESTION_MAX_CHARS: usize = 300;
const OUTCOME_REASON_MAX_CHARS: usize = 80;

/// The columns `report_from` reads, in its order.
const SELECT_REPORTS: &str = "SELECT session, state, label, doing, needs_input, reported_at,
     launch_id, outcome, outcome_reason, question_source FROM session_reports";

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

/// Where an open question came from, which decides what clears it (#200).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionSource {
    /// The agent asked it (`--needs-input`): open until a new prompt or a new launch.
    Agent,
    /// The provider bridge saw a dialog (permission, elicitation): open until the next
    /// report, since any later event means the dialog was answered.
    Dialog,
}

impl QuestionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            QuestionSource::Agent => "agent",
            QuestionSource::Dialog => "dialog",
        }
    }

    fn parse(text: &str) -> Option<QuestionSource> {
        [QuestionSource::Agent, QuestionSource::Dialog]
            .into_iter()
            .find(|source| source.as_str() == text)
    }
}

/// What an agent reports; each field it leaves out keeps its previous value, except the
/// open question, which [`QuestionSource`] clears.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub state: Option<AgentState>,
    /// The session's overall task, 1 to 60 characters.
    pub label: Option<String>,
    /// The current step, 2 to 12 words and at most 80 characters.
    pub doing: Option<String>,
    /// The agent's own question for the owner, at most 300 characters.
    pub needs_input: Option<String>,
    /// A dialog's question the provider bridge saw, at most 300 characters.
    pub dialog: Option<String>,
    /// The owner sent a new prompt: the agent is working and its own question is answered.
    pub is_new_prompt: bool,
    /// How the agent's turn ended; it means waiting. Reporting `working` clears it: a new
    /// turn started. A failure's reason is at most 80 characters.
    pub outcome: Option<TurnOutcome>,
}

/// A session's report as the channel last recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionReport {
    pub session: SessionId,
    pub state: Option<AgentState>,
    pub label: Option<String>,
    pub doing: Option<String>,
    /// The open question, the agent's own or a dialog's.
    pub needs_input: Option<String>,
    /// Set exactly when `needs_input` is.
    pub question_source: Option<QuestionSource>,
    /// How the latest turn ended, until the next turn starts.
    pub outcome: Option<TurnOutcome>,
    /// Seconds since 1970 of the last report.
    pub reported_at: i64,
    /// The session's latest launch when the agent reported, so a report is never mistaken
    /// for one from a later launch; `None` for a report moved from the agent channel.
    pub launch: Option<i64>,
}

/// Records `report` for the session the sender is identified as (see
/// [`cockpit_agent_channel::identify`]), after
/// `cockpit-authorization` allows that session `change-state` over itself. Nested agents
/// therefore report with their parent session's identity and grants.
pub async fn report(
    write: &WriteTransaction<'_>,
    peer: &PeerAncestry,
    token: Option<&str>,
    report: Report,
) -> Result<SessionReport, Error> {
    let report = validate(report).map_err(refused)?;
    let session = cockpit_agent_channel::identify(write, peer, token).await?;
    let decision = cockpit_authorization::evaluate(
        write,
        &Actor::Session(session.clone()),
        Permission::ChangeState,
        &Scope::Session(session.clone()),
    )
    .await?;
    if let Outcome::Refused(refusal) = decision.outcome {
        return Err(refused(Refusal::NotAuthorized(refusal)));
    }
    let previous = latest_report(write, &session).await?;
    let launch = latest_launch(write, &session).await?;
    let merged = merge(&session, previous, report, launch);
    write
        .execute(
            "DELETE FROM session_reports WHERE session = ?1",
            [session.as_str()],
        )
        .await?;
    write
        .execute(
            "INSERT INTO session_reports
                (session, state, label, doing, needs_input, reported_at, launch_id, outcome,
                 outcome_reason, question_source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            (
                merged.session.as_str(),
                merged.state.map(AgentState::as_str),
                merged.label.clone(),
                merged.doing.clone(),
                merged.needs_input.clone(),
                merged.reported_at,
                merged.launch,
                merged.outcome.as_ref().map(TurnOutcome::name),
                merged
                    .outcome
                    .as_ref()
                    .and_then(TurnOutcome::failure_reason)
                    .map(str::to_owned),
                merged.question_source.map(QuestionSource::as_str),
            ),
        )
        .await?;
    Ok(merged)
}

/// Every session's latest report, by session id.
pub async fn latest_reports(connection: &Connection) -> Result<Vec<SessionReport>, Error> {
    let mut rows = connection
        .query(&format!("{SELECT_REPORTS} ORDER BY session"), ())
        .await?;
    let mut reports = Vec::new();
    while let Some(row) = rows.next().await? {
        reports.push(report_from(&row)?);
    }
    Ok(reports)
}

pub(crate) async fn latest_report(
    connection: &Connection,
    session: &SessionId,
) -> Result<Option<SessionReport>, Error> {
    let mut rows = connection
        .query(
            &format!("{SELECT_REPORTS} WHERE session = ?1"),
            [session.as_str()],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(report_from(&row)?)),
        None => Ok(None),
    }
}

/// The session's latest launch: the one its reporting agent runs as, since a launch is
/// recorded before its agent starts and the previous agent is stopped before that.
async fn latest_launch(connection: &Connection, session: &SessionId) -> Result<Option<i64>, Error> {
    let mut rows = connection
        .query(
            "SELECT max(id) FROM session_launches WHERE session_id = ?1",
            [session.as_str()],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(row.get(0)?),
        None => Ok(None),
    }
}

fn merge(
    session: &SessionId,
    previous: Option<SessionReport>,
    report: Report,
    launch: Option<i64>,
) -> SessionReport {
    let previous = previous.unwrap_or(SessionReport {
        session: session.clone(),
        state: None,
        label: None,
        doing: None,
        needs_input: None,
        question_source: None,
        outcome: None,
        reported_at: 0,
        launch: None,
    });
    let is_kept = previous.question_source == Some(QuestionSource::Agent)
        && !report.is_new_prompt
        && previous.launch == launch;
    let asked = report
        .needs_input
        .map(|question| (question, QuestionSource::Agent))
        .or(report
            .dialog
            .map(|question| (question, QuestionSource::Dialog)));
    // A report after a dialog means the dialog was answered and the agent goes on, so a
    // stateless one leaves it working rather than in the waiting the dialog forced.
    let is_answering_dialog = previous.question_source == Some(QuestionSource::Dialog);
    // A question is only asked, and a turn only ended, while the agent waits.
    let state = if asked.is_some() || report.outcome.is_some() {
        Some(AgentState::Waiting)
    } else if is_answering_dialog {
        report.state.or(Some(AgentState::Working))
    } else {
        report.state.or(previous.state)
    };
    let outcome = match (report.outcome, report.state) {
        (Some(outcome), _) => Some(outcome),
        (None, Some(AgentState::Working)) => None,
        // A turn of an earlier launch says nothing about this launch's agent.
        (None, _) if previous.launch != launch => None,
        (None, _) => previous.outcome,
    };
    let kept = previous
        .needs_input
        .filter(|_| is_kept)
        .map(|question| (question, QuestionSource::Agent));
    let (needs_input, question_source) = asked.or(kept).unzip();
    SessionReport {
        session: session.clone(),
        state,
        label: report.label.or(previous.label),
        doing: report.doing.or(previous.doing),
        needs_input,
        question_source,
        outcome,
        reported_at: now(),
        launch,
    }
}

/// Checks each field at the boundary and collapses runs of whitespace. Text reaches terminals
/// and tmux, so control characters, characters that reorder or hide text (bidirectional
/// overrides, zero-width marks) and the tmux sequences that run or restyle (`#(`, `#{`, `#[`)
/// are refused. A plain `#` stays ("issue #32"): whatever renders this text into a tmux
/// format escapes it as `##`.
fn validate(report: Report) -> Result<Report, Refusal> {
    if report == Report::default() {
        return Err(invalid("report", "names no field to report".to_owned()));
    }
    let is_asking = report.needs_input.is_some() || report.dialog.is_some();
    if report.needs_input.is_some() && report.dialog.is_some() {
        return Err(invalid(
            "dialog",
            "comes with a needs-input question; a report carries one question".to_owned(),
        ));
    }
    if report.is_new_prompt && (is_asking || report.outcome.is_some()) {
        return Err(invalid(
            "prompt",
            "comes with a question or a turn's outcome; a new prompt starts a turn".to_owned(),
        ));
    }
    if report.is_new_prompt && report.state == Some(AgentState::Waiting) {
        return Err(invalid(
            "state",
            "is waiting, but a new prompt means working".to_owned(),
        ));
    }
    if report.state == Some(AgentState::Working) && is_asking {
        return Err(invalid(
            "state",
            "is working, but a question means waiting".to_owned(),
        ));
    }
    if report.state == Some(AgentState::Working) && report.outcome.is_some() {
        return Err(invalid(
            "state",
            "is working, but a turn's outcome means waiting".to_owned(),
        ));
    }
    let label = report
        .label
        .map(|label| bounded_text("label", &label, LABEL_MAX_CHARS))
        .transpose()?;
    let doing = report
        .doing
        .map(|doing| {
            let doing = checked_text("doing", &doing)?;
            let words = doing.split(' ').filter(|word| !word.is_empty()).count();
            if !DOING_WORDS.contains(&words) || doing.chars().count() > DOING_MAX_CHARS {
                return Err(invalid(
                    "doing",
                    format!(
                        "must be {} to {} words and at most {DOING_MAX_CHARS} characters",
                        DOING_WORDS.start(),
                        DOING_WORDS.end()
                    ),
                ));
            }
            Ok(doing)
        })
        .transpose()?;
    let needs_input = report
        .needs_input
        .map(|question| bounded_text("needs-input", &question, QUESTION_MAX_CHARS))
        .transpose()?;
    let dialog = report
        .dialog
        .map(|question| bounded_text("dialog", &question, QUESTION_MAX_CHARS))
        .transpose()?;
    let outcome = match report.outcome {
        Some(TurnOutcome::Failed {
            reason: Some(reason),
        }) => Some(TurnOutcome::Failed {
            reason: Some(bounded_text(
                "outcome-reason",
                &reason,
                OUTCOME_REASON_MAX_CHARS,
            )?),
        }),
        outcome => outcome,
    };
    Ok(Report {
        // A new prompt means working, so its report clears what a new turn clears.
        state: if report.is_new_prompt {
            Some(AgentState::Working)
        } else {
            report.state
        },
        label,
        doing,
        needs_input,
        dialog,
        is_new_prompt: report.is_new_prompt,
        outcome,
    })
}

fn bounded_text(field: &'static str, text: &str, max_chars: usize) -> Result<String, Refusal> {
    let text = checked_text(field, text)?;
    match text.chars().count() {
        0 => Err(invalid(field, "is empty".to_owned())),
        count if count > max_chars => Err(invalid(
            field,
            format!("is {count} characters; at most {max_chars}"),
        )),
        _ => Ok(text),
    }
}

fn checked_text(field: &'static str, text: &str) -> Result<String, Refusal> {
    if text.chars().any(char::is_control) {
        return Err(invalid(field, "contains control characters".to_owned()));
    }
    if text.chars().any(is_invisible_format) {
        return Err(invalid(
            field,
            "contains a character that reorders or hides text".to_owned(),
        ));
    }
    if ["#(", "#{", "#["]
        .iter()
        .any(|opener| text.contains(opener))
    {
        return Err(invalid(field, "contains a tmux format sequence".to_owned()));
    }
    Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Unicode format characters (category Cf) that change how surrounding text displays: soft
/// hyphen, zero-width marks, bidirectional embeddings, overrides and isolates, word joiners
/// and the byte-order mark.
fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

/// A refusal keeps the agent channel's shape, so every agent request is refused alike.
fn refused(refusal: Refusal) -> Error {
    Error::AgentChannel(refusal.into())
}

fn invalid(field: &'static str, reason: String) -> Refusal {
    Refusal::InvalidRequest { field, reason }
}

fn report_from(row: &Row) -> Result<SessionReport, Error> {
    let invalid_row = |reason: String| Error::InvalidStoredRow {
        reason: format!("session_reports: {reason}"),
    };
    let session =
        SessionId::new(row.get::<String>(0)?).map_err(|error| invalid_row(error.to_string()))?;
    let state = row
        .get::<Option<String>>(1)?
        .map(|state| {
            AgentState::parse(&state).ok_or_else(|| invalid_row(format!("state {state:?}")))
        })
        .transpose()?;
    let reason = row.get::<Option<String>>(8)?;
    let outcome = row
        .get::<Option<String>>(7)?
        .map(|name| {
            TurnOutcome::from_name(&name, reason)
                .ok_or_else(|| invalid_row(format!("outcome {name:?}")))
        })
        .transpose()?;
    let question_source = row
        .get::<Option<String>>(9)?
        .map(|source| {
            QuestionSource::parse(&source)
                .ok_or_else(|| invalid_row(format!("question_source {source:?}")))
        })
        .transpose()?;
    Ok(SessionReport {
        session,
        state,
        label: row.get(2)?,
        doing: row.get(3)?,
        needs_input: row.get(4)?,
        question_source,
        outcome,
        reported_at: row.get(5)?,
        launch: row.get(6)?,
    })
}
