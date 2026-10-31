//! Agents' reports: each session's latest state, label, doing-now and open question. The
//! agent channel identifies the sender; this module decides what a report records (#146).

use cockpit_agent_channel::{PeerAncestry, Refusal};
use cockpit_authorization::{Actor, Outcome, Permission, Scope, SessionId};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::Error;
use crate::store::now;

const LABEL_MAX_CHARS: usize = 60;
const DOING_MAX_CHARS: usize = 80;
const DOING_WORDS: std::ops::RangeInclusive<usize> = 2..=12;
const QUESTION_MAX_CHARS: usize = 300;

/// The columns `report_from` reads, in its order.
const SELECT_REPORTS: &str = "SELECT session, state, label, doing, needs_input, reported_at,
     launch_id FROM session_reports";

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
    let mut merged = merge(&session, previous, report);
    merged.launch = latest_launch(write, &session).await?;
    write
        .execute(
            "DELETE FROM session_reports WHERE session = ?1",
            [session.as_str()],
        )
        .await?;
    write
        .execute(
            "INSERT INTO session_reports
                (session, state, label, doing, needs_input, reported_at, launch_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                merged.session.as_str(),
                merged.state.map(AgentState::as_str),
                merged.label.clone(),
                merged.doing.clone(),
                merged.needs_input.clone(),
                merged.reported_at,
                merged.launch,
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

fn merge(session: &SessionId, previous: Option<SessionReport>, report: Report) -> SessionReport {
    let previous = previous.unwrap_or(SessionReport {
        session: session.clone(),
        state: None,
        label: None,
        doing: None,
        needs_input: None,
        reported_at: 0,
        launch: None,
    });
    // A question is only open while the agent waits on it.
    let state = match &report.needs_input {
        Some(_) => Some(AgentState::Waiting),
        None => report.state.or(previous.state),
    };
    let needs_input = match (report.needs_input, report.state) {
        (Some(question), _) => Some(question),
        (None, Some(AgentState::Working)) => None,
        (None, _) => previous.needs_input,
    };
    SessionReport {
        session: session.clone(),
        state,
        label: report.label.or(previous.label),
        doing: report.doing.or(previous.doing),
        needs_input,
        reported_at: now(),
        launch: None,
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
    if report.state == Some(AgentState::Working) && report.needs_input.is_some() {
        return Err(invalid(
            "state",
            "is working, but a needs-input question means waiting".to_owned(),
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
    Ok(Report {
        state: report.state,
        label,
        doing,
        needs_input,
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
    Ok(SessionReport {
        session,
        state,
        label: row.get(2)?,
        doing: row.get(3)?,
        needs_input: row.get(4)?,
        reported_at: row.get(5)?,
        launch: row.get(6)?,
    })
}
