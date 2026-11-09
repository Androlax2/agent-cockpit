//! `usage-digest`: the daily digest's data for the owner (report 18, #700): each of the last N
//! UTC days' sessions, turns, approvals, time with the agent, cost, projects and incidents,
//! read from what `cockpit-usage` recorded. [`crate::turns`] records the turns and
//! [`crate::approvals`] the approvals.

use std::sync::Arc;

use cockpit_protocol::{
    DayDigestView, DigestView, ErrorCode, IncidentView, Outcome, ProjectDayView, ProtocolError,
    Reply,
};
use cockpit_turso::Database;
use cockpit_usage::{Day, DayDigest, ProjectDay};

/// The most days `usage-digest` answers, and the most the protocol accepts.
pub(crate) const MOST_DAYS: u32 = cockpit_usage::MAX_RANGE_DAYS as u32;

pub(crate) async fn report(database: Arc<Database>, days: u32) -> Outcome {
    if !(1..=MOST_DAYS).contains(&days) {
        return Outcome::Error(ProtocolError {
            code: ErrorCode::InvalidArgument,
            message: format!("days: {days} is not 1 to {MOST_DAYS}"),
            unblock: None,
        });
    }
    let last = Day::containing(crate::approvals::now());
    let first = last.minus_days(days - 1);
    let digests = async {
        let read = database.read().await.map_err(|error| error.to_string())?;
        cockpit_usage::digests(&read, first, last)
            .await
            .map_err(|error| error.to_string())
    }
    .await;
    match digests {
        Ok(digests) => Outcome::Ok(Reply::UsageDigest {
            digest: DigestView {
                first_day: first.to_string(),
                last_day: last.to_string(),
                days: digests.into_iter().map(day_view).collect(),
            },
        }),
        Err(error) => {
            tracing::error!(%error, "reading the digest failed");
            Outcome::Error(ProtocolError {
                code: ErrorCode::Internal,
                message: "the daemon failed; no digest was read".to_owned(),
                unblock: None,
            })
        }
    }
}

fn day_view(digest: DayDigest) -> DayDigestView {
    DayDigestView {
        day: digest.day.to_string(),
        sessions: digest.sessions,
        turns: digest.turns,
        approvals: digest.approvals,
        seconds_with_agent: digest.seconds_with_agent,
        replies: digest.replies,
        unpriced_replies: digest.unpriced_replies,
        cost_nanos: digest.cost_nanos,
        projects: digest.projects.into_iter().map(project_view).collect(),
        incidents: digest
            .incidents
            .into_iter()
            .map(|incident| IncidentView {
                provider: incident.provider().to_string(),
                title: incident.title().to_owned(),
                started_at: incident.started_at(),
                resolved_at: incident.resolved_at(),
            })
            .collect(),
    }
}

fn project_view(project: ProjectDay) -> ProjectDayView {
    ProjectDayView {
        project: project.project,
        sessions: project.sessions,
        turns: project.turns,
        approvals: project.approvals,
        seconds_with_agent: project.seconds_with_agent,
        replies: project.replies,
        unpriced_replies: project.unpriced_replies,
        cost_nanos: project.cost_nanos,
    }
}
