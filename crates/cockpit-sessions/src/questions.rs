//! Who an agent's own question waits on (#710): the coordinator session that started its
//! session, for [`QUESTION_HOLD_SECONDS`], before the owner. The owner sees every question
//! and may answer it at any time; this decides only whose list it is on.

use cockpit_authorization::{Actor, Outcome, Permission, Scope};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::Connection;

use crate::reports::{QuestionSource, SessionReport, latest_report};
use crate::{Error, SessionId, SessionState};

/// How long a coordinator holds a question it has not answered or escalated before it
/// reaches the owner: fixed, decided in #710.
pub const QUESTION_HOLD_SECONDS: i64 = 15 * 60;

/// The coordinator holding `report`'s open question at `now`, or `None` when the question
/// waits on the owner. A coordinator holds it while all of these hold:
///
/// - it is the agent's own question: a dialog waits on the owner, since a message would
///   answer it;
/// - it was asked since #710, less than [`QUESTION_HOLD_SECONDS`] ago, and not escalated;
/// - a coordinator started the session, has neither ended nor failed, and still holds
///   `message-sessions` over it, the grant its answer needs.
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
    let asking = SessionId::parse(report.session.as_str())?;
    let Some(coordinator) = crate::store::session(connection, &asking).await?.started_by else {
        return Ok(None);
    };
    let coordinator_session = crate::store::session(connection, &coordinator).await?;
    if matches!(
        coordinator_session.state,
        SessionState::Ended | SessionState::Failed
    ) {
        return Ok(None);
    }
    let decision = cockpit_authorization::evaluate(
        connection,
        &Actor::Session(cockpit_authorization::SessionId::new(coordinator.as_str())?),
        Permission::MessageSessions,
        &Scope::Session(report.session.clone()),
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
