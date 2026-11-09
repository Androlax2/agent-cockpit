//! Agents' reports: each session's latest state, label, doing-now and open question. The
//! agent channel identifies the sender; this module decides what a report records (#146).

use cockpit_agent_channel::{PeerAncestry, Refusal};
use cockpit_authorization::{Actor, Outcome, Permission, Scope, SessionId};
use cockpit_provider_api::{CompactionMoment, ContextUsage, ConversationId, TurnOutcome};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::store::now;
use crate::{Error, compactions, conversations, quota_stops};

const LABEL_MAX_CHARS: usize = 60;
const DOING_MAX_CHARS: usize = 80;
const DOING_WORDS: std::ops::RangeInclusive<usize> = 2..=12;
const QUESTION_MAX_CHARS: usize = 300;
const OUTCOME_REASON_MAX_CHARS: usize = 80;

/// The columns `report_from` reads, in its order.
const SELECT_REPORTS: &str = "SELECT session, state, label, doing, needs_input, reported_at,
     launch_id, outcome, outcome_reason, question_source, covered_question, question_asked_at,
     question_escalated_at, context_tokens, context_window FROM session_reports";

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
    /// The agent asked it (`--needs-input`): open until a new prompt, a new launch or a new
    /// conversation (#225).
    Agent,
    /// The provider bridge saw a dialog (permission, elicitation): open until a report says
    /// what the agent does (a state or a turn's outcome), which means the dialog was
    /// answered, or a new launch or conversation; a stateless label or doing-now may come
    /// from a subagent meanwhile (#216).
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
    /// The provider's conversation the report comes from; a report of another conversation
    /// than the session's is refused (#219). Left out, nothing is checked.
    pub conversation: Option<String>,
    /// `conversation` ends and the agent goes on in another (`/clear`, `/resume`): the session
    /// follows the next conversation reported. Comes alone with `conversation`.
    pub is_conversation_ending: bool,
    /// A moment of a context compaction (#782). It reports nothing about what the agent does,
    /// so it comes alone with `conversation`, and the latest report stays as it is.
    pub compaction: Option<CompactionMoment>,
    /// The harness's own context at the end of the turn `outcome` reports (#479); it comes
    /// only with an outcome.
    pub context: Option<ContextUsage>,
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
    /// The agent's own question an open dialog covers; it is the open question again once
    /// the dialog is answered (#214). Set only while the open question is a dialog's.
    pub covered_question: Option<String>,
    /// How the latest turn ended, until the next turn starts.
    pub outcome: Option<TurnOutcome>,
    /// Seconds since 1970 of the last report.
    pub reported_at: i64,
    /// The session's latest launch when the agent reported, so a report is never mistaken
    /// for one from a later launch; `None` for a report moved from the agent channel.
    pub launch: Option<i64>,
    /// Seconds since 1970 when the agent asked its own question, open or covered by a
    /// dialog; its time with a coordinator counts from here (#710). `None` without one, and
    /// for a question recorded before #710.
    pub question_asked_at: Option<i64>,
    /// When the coordinator that started the session handed the agent's question to the
    /// owner (`agent escalate`, #710).
    pub question_escalated_at: Option<i64>,
    /// The harness's context at the end of this launch's and conversation's latest turn
    /// (#479), when that turn's report gave one; a new launch or conversation starts without
    /// one.
    pub context: Option<ContextUsage>,
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
    let (report, conversation) = validate(report).map_err(refused)?;
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
    let is_new_conversation = match &conversation {
        Some(conversation) => {
            conversations::check(
                write,
                &session,
                launch,
                conversation,
                report.is_conversation_ending,
            )
            .await?
        }
        None => false,
    };
    if let Some(compaction) = &report.compaction {
        let launch = launch.ok_or_else(|| {
            refused(invalid(
                "compaction",
                "the session has no launch to record it for".to_owned(),
            ))
        })?;
        compactions::record(write, &session, launch, compaction).await?;
        return Ok(previous.unwrap_or_else(|| unreported(&session)));
    }
    // An announced end reports nothing about the agent: the latest report stays as it is.
    if report.is_conversation_ending {
        return Ok(previous.unwrap_or_else(|| unreported(&session)));
    }
    let is_quota_stop = report.outcome == Some(TurnOutcome::QuotaStopped);
    let ends_turn = report.outcome.is_some();
    let merged = merge(&session, previous, report, launch, is_new_conversation);
    store_report(write, &merged).await?;
    // The account's stop is the daemon's record from here: the next launches pass it over,
    // and the session relaunches on another account (#59) or resumes at the reset (#158).
    if is_quota_stop && let Some(launch) = launch {
        crate::store::record_quota_stop(write, launch).await?;
        quota_stops::record(write, session.as_str(), launch).await?;
    } else if ends_turn || merged.state == Some(AgentState::Working) {
        let reason = "the agent worked again before the reset";
        quota_stops::drop_open(write, session.as_str(), reason, now()).await?;
    }
    Ok(merged)
}

/// Records that `launch`'s agent has not reported by the launch deadline (#702): an open
/// dialog question saying `reason`, which the agent's first report of what it does clears,
/// as it clears any dialog. Before its agent runs, a harness can wait on a screen of its own
/// (folder trust, sign-in, update) that no hook reports. `false`, recording nothing, when
/// the agent reported for `launch` meanwhile or a newer launch began.
pub(crate) async fn flag_unreported_launch(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    launch: i64,
    reason: &str,
) -> Result<bool, Error> {
    let previous = latest_report(write, session).await?;
    // A report moved from the agent channel names no launch; it is the running agent's.
    let has_reported = previous
        .as_ref()
        .is_some_and(|previous| previous.launch.is_none() || previous.launch == Some(launch));
    if has_reported || latest_launch(write, session).await? != Some(launch) {
        return Ok(false);
    }
    store_dialog(write, session, previous, launch, reason).await?;
    Ok(true)
}

/// Records that `launch`'s agent exited while processes it started still run (#406): an
/// open dialog question saying `reason`. No agent is left to answer it, so it
/// stays until the owner ends or restarts the session. `false`, recording nothing, when the
/// question is already open or a newer launch began.
pub(crate) async fn flag_agentless_launch(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    launch: i64,
    reason: &str,
) -> Result<bool, Error> {
    if latest_launch(write, session).await? != Some(launch) {
        return Ok(false);
    }
    let previous = latest_report(write, session).await?;
    let is_open = previous.as_ref().is_some_and(|previous| {
        previous.launch == Some(launch)
            && previous.question_source == Some(QuestionSource::Dialog)
            && previous.needs_input.as_deref() == Some(reason)
    });
    if is_open {
        return Ok(false);
    }
    store_dialog(write, session, previous, launch, reason).await?;
    Ok(true)
}

/// Opens the daemon's own dialog question `reason` on `launch`'s report for the owner to
/// act on (#158), unless a newer launch began. The agent's next report of what it does
/// clears it, as it clears any dialog.
pub(crate) async fn flag_owner_needed(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    launch: i64,
    reason: &str,
) -> Result<(), Error> {
    if latest_launch(write, session).await? != Some(launch) {
        return Ok(());
    }
    let previous = latest_report(write, session).await?;
    store_dialog(write, session, previous, launch, reason).await
}

/// Opens the daemon's own dialog question `reason` on `launch`'s report.
async fn store_dialog(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    previous: Option<SessionReport>,
    launch: i64,
    reason: &str,
) -> Result<(), Error> {
    let dialog = bounded_text("dialog", reason, QUESTION_MAX_CHARS).map_err(refused)?;
    let report = Report {
        dialog: Some(dialog),
        ..Report::default()
    };
    store_report(
        write,
        &merge(session, previous, report, Some(launch), false),
    )
    .await
}

/// Replaces the session's report row with `merged`.
async fn store_report(write: &WriteTransaction<'_>, merged: &SessionReport) -> Result<(), Error> {
    write
        .execute(
            "DELETE FROM session_reports WHERE session = ?1",
            [merged.session.as_str()],
        )
        .await?;
    write
        .execute(
            "INSERT INTO session_reports
                (session, state, label, doing, needs_input, reported_at, launch_id, outcome,
                 outcome_reason, question_source, covered_question, question_asked_at,
                 question_escalated_at, context_tokens, context_window)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
                merged.covered_question.clone(),
                merged.question_asked_at,
                merged.question_escalated_at,
                merged.context.map(|context| token_column(context.tokens)),
                merged
                    .context
                    .and_then(|context| context.window.map(token_column)),
            ),
        )
        .await?;
    Ok(())
}

/// `tokens` as a column value; [`validate`] has checked that it fits.
fn token_column(tokens: u64) -> i64 {
    i64::try_from(tokens).expect("a validated token count fits an i64")
}

/// The harness's context at the end of `session`'s latest turn in its current launch (#479);
/// `None` before that launch's first turn ends.
pub async fn current_context(
    connection: &Connection,
    session: &SessionId,
) -> Result<Option<ContextUsage>, Error> {
    let Some(report) = latest_report(connection, session).await? else {
        return Ok(None);
    };
    let launch = latest_launch(connection, session).await?;
    Ok(report
        .context
        .filter(|_| report.launch.is_some() && report.launch == launch))
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

/// `session`'s latest report, if its agent ever reported.
pub async fn latest_report(
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
pub(crate) async fn latest_launch(
    connection: &Connection,
    session: &SessionId,
) -> Result<Option<i64>, Error> {
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

/// A session no report was recorded for.
fn unreported(session: &SessionId) -> SessionReport {
    SessionReport {
        session: session.clone(),
        state: None,
        label: None,
        doing: None,
        needs_input: None,
        question_source: None,
        covered_question: None,
        outcome: None,
        reported_at: 0,
        launch: None,
        question_asked_at: None,
        question_escalated_at: None,
        context: None,
    }
}

/// `is_new_conversation`: the session just followed the conversation that replaced an ended
/// one, which, like a newer launch, owes nothing to the previous conversation (#311).
fn merge(
    session: &SessionId,
    previous: Option<SessionReport>,
    report: Report,
    launch: Option<i64>,
    is_new_conversation: bool,
) -> SessionReport {
    let previous = previous.unwrap_or_else(|| unreported(session));
    let at = now();
    let is_continuing = previous.launch == launch && !is_new_conversation;
    let ends_turn = report.outcome.is_some();
    let is_asking = report.needs_input.is_some() || report.dialog.is_some();
    // Only a report saying what the agent does answers a dialog: a label, a doing-now or the
    // agent's own question may come from a parallel tool or a subagent while the dialog
    // still blocks the agent (#216).
    let is_answering_dialog = report.state.is_some() || report.outcome.is_some();
    // A question is only asked, and a turn only ended, while the agent waits.
    let state = if is_asking || report.outcome.is_some() {
        Some(AgentState::Waiting)
    } else {
        report.state.or(previous.state)
    };
    let outcome = match (report.outcome, state) {
        (Some(outcome), _) => Some(outcome),
        (None, Some(AgentState::Working)) => None,
        (None, _) if !is_continuing => None,
        (None, _) => previous.outcome,
    };
    let (previous_agent_question, previous_dialog) = match previous.question_source {
        Some(QuestionSource::Agent) => (previous.needs_input, None),
        Some(QuestionSource::Dialog) => (previous.covered_question, previous.needs_input),
        None => (None, None),
    };
    let kept_agent_question =
        previous_agent_question.filter(|_| !report.is_new_prompt && is_continuing);
    // Asking the kept question again keeps its time and escalation: a repeated report must
    // not take a question back from the owner (#710).
    let (question_asked_at, question_escalated_at) =
        match (&report.needs_input, &kept_agent_question) {
            (Some(asked), Some(kept)) if asked == kept => {
                (previous.question_asked_at, previous.question_escalated_at)
            }
            (Some(_), _) => (Some(at), None),
            (None, Some(_)) => (previous.question_asked_at, previous.question_escalated_at),
            (None, None) => (None, None),
        };
    let agent_question = report.needs_input.or(kept_agent_question);
    let kept_dialog = previous_dialog.filter(|_| !is_answering_dialog && is_continuing);
    // An open dialog shows over the agent's question, which waits beneath it.
    let (needs_input, question_source, covered_question) = match report.dialog.or(kept_dialog) {
        Some(dialog) => (Some(dialog), Some(QuestionSource::Dialog), agent_question),
        None => {
            let question_source = agent_question.as_ref().map(|_| QuestionSource::Agent);
            (agent_question, question_source, None)
        }
    };
    SessionReport {
        session: session.clone(),
        state,
        label: report.label.or(previous.label),
        doing: report.doing.or(previous.doing),
        needs_input,
        question_source,
        covered_question,
        outcome,
        reported_at: at,
        launch,
        question_asked_at,
        question_escalated_at,
        // A turn's end without a context replaces the last turn's: that size is no longer
        // the latest turn's.
        context: if ends_turn {
            report.context
        } else {
            previous.context.filter(|_| is_continuing)
        },
    }
}

/// Checks each field at the boundary and collapses runs of whitespace. Text reaches terminals
/// and tmux, so control characters, characters that reorder or hide text (bidirectional
/// overrides, zero-width marks) and the tmux sequences that run or restyle (`#(`, `#{`, `#[`)
/// are refused. A plain `#` stays ("issue #32"): whatever renders this text into a tmux
/// format escapes it as `##`. Returns the conversation apart, checked as an id; the report
/// keeps none.
fn validate(report: Report) -> Result<(Report, Option<ConversationId>), Refusal> {
    let conversation = report
        .conversation
        .clone()
        .map(ConversationId::new)
        .transpose()
        .map_err(|error| invalid("conversation", error.to_string()))?;
    let reported = Report {
        conversation: None,
        is_conversation_ending: false,
        compaction: None,
        ..report.clone()
    };
    if report.compaction.is_some() {
        if report.is_conversation_ending || reported != Report::default() {
            return Err(invalid(
                "compaction",
                "comes with fields to report; a compaction reports nothing else".to_owned(),
            ));
        }
        let compaction = Report {
            compaction: report.compaction,
            ..Report::default()
        };
        return Ok((compaction, conversation));
    }
    if report.is_conversation_ending {
        if conversation.is_none() {
            return Err(invalid(
                "conversation-ends",
                "names no conversation that ends".to_owned(),
            ));
        }
        if reported != Report::default() {
            return Err(invalid(
                "conversation-ends",
                "comes with fields to report; an announced end reports nothing else".to_owned(),
            ));
        }
        let ending = Report {
            is_conversation_ending: true,
            ..Report::default()
        };
        return Ok((ending, conversation));
    }
    if reported == Report::default() {
        return Err(invalid("report", "names no field to report".to_owned()));
    }
    if let Some(context) = report.context {
        validate_context(context, report.outcome.is_some())?;
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
    Ok((
        Report {
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
            conversation: None,
            is_conversation_ending: false,
            compaction: None,
            context: report.context,
        },
        conversation,
    ))
}

/// A context size is the harness's at a turn's end, so it comes with that turn's outcome;
/// its counts must fit a column, and a window holds at least one token.
fn validate_context(context: ContextUsage, has_outcome: bool) -> Result<(), Refusal> {
    if !has_outcome {
        return Err(invalid(
            "context",
            "comes without a turn's outcome; it is the context at a turn's end".to_owned(),
        ));
    }
    for (field, tokens) in [
        ("context-tokens", Some(context.tokens)),
        ("context-window", context.window),
    ] {
        if tokens.is_some_and(|tokens| i64::try_from(tokens).is_err()) {
            return Err(invalid(field, format!("is more than {} tokens", i64::MAX)));
        }
    }
    if context.window == Some(0) {
        return Err(invalid("context-window", "is zero tokens".to_owned()));
    }
    Ok(())
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
pub(crate) fn is_invisible_format(character: char) -> bool {
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
pub(crate) fn refused(refusal: Refusal) -> Error {
    Error::AgentChannel(refusal.into())
}

pub(crate) fn invalid(field: &'static str, reason: String) -> Refusal {
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
        covered_question: row.get(10)?,
        outcome,
        reported_at: row.get(5)?,
        launch: row.get(6)?,
        question_asked_at: row.get(11)?,
        question_escalated_at: row.get(12)?,
        context: context_from(row.get(13)?, row.get(14)?).map_err(invalid_row)?,
    })
}

fn context_from(tokens: Option<i64>, window: Option<i64>) -> Result<Option<ContextUsage>, String> {
    let count = |column: &str, value: i64| {
        u64::try_from(value).map_err(|_| format!("{column} {value} is negative"))
    };
    match (tokens, window) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err("context_window without context_tokens".to_owned()),
        (Some(tokens), window) => Ok(Some(ContextUsage {
            tokens: count("context_tokens", tokens)?,
            window: window
                .map(|window| count("context_window", window))
                .transpose()?,
        })),
    }
}
