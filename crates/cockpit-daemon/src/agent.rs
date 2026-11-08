//! The agent socket's commands: wire types converted to and from `cockpit-sessions`' reports.
//! `cockpit-agent-channel` decides who the sender is; sessions decide what a report records.

use std::sync::Arc;

use cockpit_agent_channel::{ProcessError, ProcessIdentity, Refusal};
use cockpit_protocol::{ErrorCode, Outcome, ProtocolError, Reply, Unblock, UnblockedBy};
use cockpit_provider_api::{
    CompactionMoment, CompactionPhase, CompactionTrigger, ContextUsage, TurnOutcome,
};
use cockpit_sessions::{
    AgentState, Compaction, CompactionStatus, Error, QuestionSource, Report, SessionId,
    SessionReport,
};
use cockpit_turso::Database;
use cockpit_turso::sql::Connection;

use crate::approvals::Sender;
use crate::routes::refusal;

/// Identifies the sender from its process tree, then records its report in one write
/// transaction. A refusal changes nothing, so a token is spent only by a recorded report.
/// A recorded quota stop relaunches the session on its next account once answered (#59).
pub(crate) async fn report(
    database: Arc<Database>,
    sessions: Arc<cockpit_sessions::Sessions>,
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
    let recorded = match recorded {
        Ok(recorded) => wire_report(&write, recorded, crate::approvals::now()).await,
        Err(error) => Err(error),
    };
    match recorded {
        Ok(recorded) => match write.commit().await {
            Ok(()) => {
                if recorded.outcome == Some(cockpit_protocol::TurnOutcome::QuotaStopped) {
                    relaunch_quota_stopped(sessions, &recorded.session);
                }
                Outcome::Ok(Reply::AgentReported {
                    report: Box::new(recorded),
                })
            }
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
    let reports = match cockpit_sessions::latest_reports(&read).await {
        Ok(reports) => wire_reports(&read, reports).await,
        Err(error) => Err(error),
    };
    match reports {
        Ok(reports) => Outcome::Ok(Reply::AgentReports { reports }),
        Err(error) => internal(&error),
    }
}

/// `reports` on the wire, each with the coordinator holding its question now (#710).
pub(crate) async fn wire_reports(
    connection: &Connection,
    reports: Vec<SessionReport>,
) -> Result<Vec<cockpit_protocol::SessionReport>, Error> {
    let at = crate::approvals::now();
    let mut wired = Vec::with_capacity(reports.len());
    for report in reports {
        wired.push(wire_report(connection, report, at).await?);
    }
    Ok(wired)
}

async fn wire_report(
    connection: &Connection,
    report: SessionReport,
    at: i64,
) -> Result<cockpit_protocol::SessionReport, Error> {
    let held_by = cockpit_sessions::question_holder(connection, &report, at).await?;
    let compaction = cockpit_sessions::latest_compaction(connection, &report.session).await?;
    Ok(to_wire(report, held_by.as_ref(), compaction))
}

/// Relaunches the quota-stopped session on its next account in the background: the relaunch
/// stops the agent that sent the report, so the reply cannot wait for it. The sessions'
/// periodic pass tries again if this one fails.
fn relaunch_quota_stopped(sessions: Arc<cockpit_sessions::Sessions>, session: &str) {
    let Ok(id) = cockpit_sessions::SessionId::parse(session) else {
        tracing::warn!(
            session,
            "a quota-stopped session's id is not valid; not relaunched"
        );
        return;
    };
    tokio::spawn(async move {
        match sessions.relaunch_quota_stopped(&id).await {
            Ok(Some(_)) => {
                tracing::info!(session = %id, "a quota-stopped session relaunched on its next account");
            }
            Ok(None) => {
                tracing::info!(session = %id, "a quota-stopped session has no other account with capacity");
            }
            Err(error) => {
                tracing::warn!(session = %id, %error, "a quota-stopped session's relaunch failed");
            }
        }
    });
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
        compaction: report.compaction.map(compaction_from_wire),
        context: report.context.map(|context| ContextUsage {
            tokens: context.tokens,
            window: context.window,
        }),
    }
}

fn compaction_from_wire(report: cockpit_protocol::CompactionReport) -> CompactionMoment {
    CompactionMoment {
        phase: match report.phase {
            cockpit_protocol::CompactionPhase::Started => CompactionPhase::Started,
            cockpit_protocol::CompactionPhase::Ended => CompactionPhase::Ended,
            cockpit_protocol::CompactionPhase::Failed => CompactionPhase::Failed,
        },
        trigger: report.trigger.map(trigger_from_wire),
        tokens_before: report.tokens_before,
        tokens_after: report.tokens_after,
    }
}

fn trigger_from_wire(trigger: cockpit_protocol::CompactionTrigger) -> CompactionTrigger {
    match trigger {
        cockpit_protocol::CompactionTrigger::Auto => CompactionTrigger::Auto,
        cockpit_protocol::CompactionTrigger::Manual => CompactionTrigger::Manual,
    }
}

fn compaction_to_wire(compaction: Compaction) -> cockpit_protocol::CompactionView {
    cockpit_protocol::CompactionView {
        status: match compaction.status {
            CompactionStatus::Running => cockpit_protocol::CompactionStatus::Running,
            CompactionStatus::Completed => cockpit_protocol::CompactionStatus::Completed,
            CompactionStatus::Failed => cockpit_protocol::CompactionStatus::Failed,
        },
        trigger: compaction.trigger.map(|trigger| match trigger {
            CompactionTrigger::Auto => cockpit_protocol::CompactionTrigger::Auto,
            CompactionTrigger::Manual => cockpit_protocol::CompactionTrigger::Manual,
        }),
        tokens_before: compaction.tokens_before,
        tokens_after: compaction.tokens_after,
        seconds: compaction.seconds,
        at: compaction.at,
    }
}

fn to_wire(
    report: SessionReport,
    held_by: Option<&SessionId>,
    compaction: Option<Compaction>,
) -> cockpit_protocol::SessionReport {
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
        held_by: held_by.map(|coordinator| coordinator.as_str().to_owned()),
        reported_at: report.reported_at,
        compaction: compaction.map(compaction_to_wire),
    }
}
