//! The agent socket's commands: wire types converted to and from `cockpit-sessions`' reports.
//! `cockpit-agent-channel` decides who the sender is; sessions decide what a report records.

use std::sync::Arc;

use cockpit_agent_channel::{ProcessError, ProcessIdentity, Refusal};
use cockpit_protocol::{ErrorCode, Outcome, ProtocolError, Reply, Unblock, UnblockedBy};
use cockpit_provider_api::TurnOutcome;
use cockpit_sessions::{AgentState, Error, QuestionSource, Report, SessionReport};
use cockpit_turso::Database;

use crate::approvals::Sender;
use crate::routes::refusal;

/// Identifies the sender from its process tree, then records its report in one write
/// transaction. A refusal changes nothing, so a token is spent only by a recorded report.
pub(crate) async fn report(
    database: Arc<Database>,
    connector: Result<ProcessIdentity, ProcessError>,
    launch_token: Option<String>,
    report: cockpit_protocol::AgentReport,
) -> Outcome {
    let peer_pid = connector.as_ref().ok().map(|process| process.pid);
    let sender = Sender::read(connector).await;
    let write = match database.write().await {
        Ok(write) => write,
        Err(error) => return internal(&error),
    };
    let recorded = cockpit_sessions::report(
        &write,
        sender.ancestry(),
        launch_token.as_deref(),
        from_wire(report),
    )
    .await;
    match recorded {
        Ok(recorded) => match write.commit().await {
            Ok(()) => Outcome::Ok(Reply::AgentReported {
                report: Box::new(to_wire(recorded)),
            }),
            Err(error) => internal(&error),
        },
        Err(Error::AgentChannel(cockpit_agent_channel::Error::Refused(refused))) => {
            tracing::info!(?peer_pid, reason = %refused, "refused an agent report");
            Outcome::Error(refusal_to_wire(&refused))
        }
        Err(error) => internal(&error),
    }
}

/// Every session's latest report, for the owner.
pub(crate) async fn reports(database: Arc<Database>) -> Outcome {
    let read = match database.read().await {
        Ok(read) => read,
        Err(error) => return internal(&error),
    };
    match cockpit_sessions::latest_reports(&read).await {
        Ok(reports) => Outcome::Ok(Reply::AgentReports {
            reports: reports.into_iter().map(to_wire).collect(),
        }),
        Err(error) => internal(&error),
    }
}

fn internal(error: &dyn std::fmt::Display) -> Outcome {
    tracing::error!(%error, "an agent channel request failed");
    refusal(
        ErrorCode::Internal,
        "the daemon failed; the request did not take effect",
    )
}

pub(crate) fn refusal_to_wire(refused: &Refusal) -> ProtocolError {
    let code = match refused {
        Refusal::UnknownLaunchToken
        | Refusal::LaunchNotStarted { .. }
        | Refusal::OutsideLaunchTree
        | Refusal::Unidentified
        | Refusal::PeerUnverifiable(_) => ErrorCode::UnidentifiedPeer,
        Refusal::NotAuthorized(_) => ErrorCode::NotAuthorized,
        Refusal::InvalidRequest { .. } => ErrorCode::InvalidArgument,
    };
    let unblock = refused.unblock();
    ProtocolError {
        code,
        message: refused.to_string(),
        unblock: Some(Unblock {
            action: unblock.action,
            by: match unblock.by {
                cockpit_agent_channel::UnblockedBy::Owner => UnblockedBy::Owner,
                cockpit_agent_channel::UnblockedBy::Agent => UnblockedBy::Agent,
                cockpit_agent_channel::UnblockedBy::Cockpit => UnblockedBy::Cockpit,
            },
        }),
    }
}

fn from_wire(report: cockpit_protocol::AgentReport) -> Report {
    Report {
        state: report.state.map(|state| match state {
            cockpit_protocol::AgentState::Working => AgentState::Working,
            cockpit_protocol::AgentState::Waiting => AgentState::Waiting,
        }),
        label: report.label,
        doing: report.doing,
        needs_input: report.needs_input,
        outcome: report.outcome.map(|outcome| match outcome {
            cockpit_protocol::TurnOutcome::Completed => TurnOutcome::Completed,
            cockpit_protocol::TurnOutcome::Failed { reason } => TurnOutcome::Failed { reason },
            cockpit_protocol::TurnOutcome::QuotaStopped => TurnOutcome::QuotaStopped,
        }),
        dialog: report.dialog,
        is_new_prompt: report.is_new_prompt,
        conversation: report.conversation,
        is_conversation_ending: report.is_conversation_ending,
    }
}

fn to_wire(report: SessionReport) -> cockpit_protocol::SessionReport {
    cockpit_protocol::SessionReport {
        session: report.session.as_str().to_owned(),
        state: report.state.map(|state| match state {
            AgentState::Working => cockpit_protocol::AgentState::Working,
            AgentState::Waiting => cockpit_protocol::AgentState::Waiting,
        }),
        label: report.label,
        doing: report.doing,
        needs_input: report.needs_input,
        outcome: report.outcome.map(|outcome| match outcome {
            TurnOutcome::Completed => cockpit_protocol::TurnOutcome::Completed,
            TurnOutcome::Failed { reason } => cockpit_protocol::TurnOutcome::Failed { reason },
            TurnOutcome::QuotaStopped => cockpit_protocol::TurnOutcome::QuotaStopped,
        }),
        question_source: report.question_source.map(|source| match source {
            QuestionSource::Agent => cockpit_protocol::QuestionSource::Agent,
            QuestionSource::Dialog => cockpit_protocol::QuestionSource::Dialog,
        }),
        covered_question: report.covered_question,
        reported_at: report.reported_at,
    }
}
