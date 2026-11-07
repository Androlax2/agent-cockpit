//! Who an agent's own question waits on (#710): the coordinator session that started its
//! session, for [`QUESTION_HOLD_SECONDS`], before the owner. The owner sees every question
//! and may answer it at any time; this decides only whose list it is on.

use cockpit_authorization::{Actor, Outcome, Permission, Scope};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::Connection;

use crate::reports::{QuestionSource, SessionReport, latest_launch, latest_report};
use crate::{Error, Session, SessionId, SessionState};

/// How long a coordinator holds a question it has not answered or escalated before it
/// reaches the owner: fixed, decided in #710.
pub const QUESTION_HOLD_SECONDS: i64 = 15 * 60;

/// The scope a coordinator grant must cover to act on `session` (#1131): the project its
/// checkout belongs to. A grant over that project or over everything covers it.
pub async fn coordinator_scope(connection: &Connection, session: &Session) -> Result<Scope, Error> {
    let project = cockpit_projects::checkout_project(connection, session.checkout).await?;
    Ok(Scope::Project(cockpit_authorization::ProjectName::new(
        project.as_str(),
    )?))
}

/// The coordinator holding `report`'s open question at `now`, or `None` when the question
/// waits on the owner. A coordinator holds it while all of these hold:
///
/// - it is the agent's own question: a dialog waits on the owner, since a message would
///   answer it;
/// - it was asked since #710, in the session's current launch, less than
///   [`QUESTION_HOLD_SECONDS`] ago, and not escalated;
/// - a coordinator started the session, runs (or is launching), and still holds
///   `message-sessions` over its project ([`coordinator_scope`]), the grant its answer needs: an asleep, stopping, ended or
///   failed coordinator reads nothing, so its question goes to the owner.
pub async fn question_holder(
    connection: &Connection,
    report: &SessionReport,
    now: i64,
) -> Result<Option<SessionId>, Error> {
    if report.needs_input.is_none() || report.question_source != Some(QuestionSource::Agent) {
        return Ok(None);
    }
    let Some(asked_at) = report.question_asked_at else {
        return Ok(None);
    };
    if report.question_escalated_at.is_some() || now >= asked_at + QUESTION_HOLD_SECONDS {
        return Ok(None);
    }
    // Checked first: a reporter Torlek never launched has no session row to read.
    let current_launch = latest_launch(connection, &report.session).await?;
    if current_launch.is_none() || report.launch != current_launch {
        return Ok(None);
    }
    let asking =
        crate::store::session(connection, &SessionId::parse(report.session.as_str())?).await?;
    let Some(coordinator) = asking.started_by.clone() else {
        return Ok(None);
    };
    let coordinator_session = crate::store::session(connection, &coordinator).await?;
    if !matches!(
        coordinator_session.state,
        SessionState::Running | SessionState::Launching
    ) {
        return Ok(None);
    }
    let decision = cockpit_authorization::evaluate(
        connection,
        &Actor::Session(cockpit_authorization::SessionId::new(coordinator.as_str())?),
        Permission::MessageSessions,
        &coordinator_scope(connection, &asking).await?,
    )
    .await?;
    Ok(match decision.outcome {
        Outcome::Allowed(_) => Some(coordinator),
        Outcome::Refused(_) => None,
    })
}

/// Hands `session`'s own question to the owner at `at` (`agent escalate`, #710). Returns
/// whether the agent has a question of its own open, under a dialog or not; escalating one
/// twice keeps the first time.
pub async fn escalate_question(
    write: &WriteTransaction<'_>,
    session: &cockpit_authorization::SessionId,
    at: i64,
) -> Result<bool, Error> {
    let Some(report) = latest_report(write, session).await? else {
        return Ok(false);
    };
    let has_agent_question = match report.question_source {
        Some(QuestionSource::Agent) => true,
        Some(QuestionSource::Dialog) => report.covered_question.is_some(),
        None => false,
    };
    if !has_agent_question {
        return Ok(false);
    }
    write
        .execute(
            "UPDATE session_reports SET question_escalated_at = ?2
             WHERE session = ?1 AND question_asked_at IS NOT NULL
                AND question_escalated_at IS NULL",
            (session.as_str(), at),
        )
        .await?;
    Ok(true)
}

/// Clears `session`'s own question once its agent's hook collected a message from `from`, the
/// coordinator that started it (#710): the message is its answer, and a collected one starts
/// no prompt to clear it (#703); a typed one does, as any prompt. A dialog covering the
/// question stays open: the owner answers it. Returns whether a question was cleared; a
/// message from any other session clears nothing.
pub async fn answer_question(
    write: &WriteTransaction<'_>,
    session: &cockpit_authorization::SessionId,
    from: &cockpit_authorization::SessionId,
) -> Result<bool, Error> {
    let asking = crate::store::session(write, &SessionId::parse(session.as_str())?).await?;
    if asking.started_by.as_ref().map(SessionId::as_str) != Some(from.as_str()) {
        return Ok(false);
    }
    let cleared = write
        .execute(
            "UPDATE session_reports SET
                needs_input = CASE question_source WHEN 'agent' THEN NULL ELSE needs_input END,
                question_source = CASE question_source WHEN 'agent' THEN NULL
                    ELSE question_source END,
                covered_question = NULL, question_asked_at = NULL, question_escalated_at = NULL
             WHERE session = ?1
                AND (question_source = 'agent' OR covered_question IS NOT NULL)",
            [session.as_str()],
        )
        .await?;
    Ok(cleared > 0)
}
