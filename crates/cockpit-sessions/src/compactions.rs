//! Context compactions the harness announced (#782): one row per compaction, kept beside the
//! agent's latest report. A compaction proves nothing about what the agent does, so recording
//! one never changes the report; the seconds it took are the daemon's own clock between the
//! start and the end, not the agent's claim.

use cockpit_authorization::SessionId;
use cockpit_provider_api::{CompactionMoment, CompactionPhase, CompactionTrigger};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::Error;
use crate::store::now;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionStatus {
    Running,
    Completed,
    /// The harness reported a failure, or started another compaction without ending this one.
    Failed,
}

impl CompactionStatus {
    fn as_str(self) -> &'static str {
        match self {
            CompactionStatus::Running => "running",
            CompactionStatus::Completed => "completed",
            CompactionStatus::Failed => "failed",
        }
    }

    fn parse(text: &str) -> Option<CompactionStatus> {
        [
            CompactionStatus::Running,
            CompactionStatus::Completed,
            CompactionStatus::Failed,
        ]
        .into_iter()
        .find(|status| status.as_str() == text)
    }
}

/// A recorded compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    pub status: CompactionStatus,
    pub trigger: Option<CompactionTrigger>,
    pub tokens_before: Option<u64>,
    pub tokens_after: Option<u64>,
    /// Whole seconds from the start to the end, when both were seen.
    pub seconds: Option<u64>,
    /// Seconds since 1970 of the last report about it.
    pub at: i64,
}

const SELECT_COMPACTION: &str = "SELECT id, status, trigger, tokens_before, tokens_after,
     started_at, ended_at, reported_at FROM session_compactions";

/// Records `report` for `session`'s `launch`, in the caller's write transaction.
///
/// A start closes a compaction of the same launch that never ended as failed: the harness
/// has moved on. An end or a failure closes the launch's running compaction; with none
/// running (its start report was lost) it records a compaction of unknown start.
pub(crate) async fn record(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    launch: i64,
    report: &CompactionMoment,
) -> Result<(), Error> {
    let at = now();
    let running = running_id(write, session, launch).await?;
    match (report.phase, running) {
        (CompactionPhase::Started, abandoned) => {
            if let Some(abandoned) = abandoned {
                write
                    .execute(
                        "UPDATE session_compactions SET status = 'failed', ended_at = ?2,
                         reported_at = ?2 WHERE id = ?1",
                        (abandoned, at),
                    )
                    .await?;
            }
            write
                .execute(
                    "INSERT INTO session_compactions
                        (session_id, launch_id, status, trigger, tokens_before, tokens_after,
                         started_at, reported_at)
                     VALUES (?1, ?2, 'running', ?3, ?4, ?5, ?6, ?6)",
                    (
                        session.as_str(),
                        launch,
                        report.trigger.map(CompactionTrigger::name),
                        token_column(report.tokens_before)?,
                        token_column(report.tokens_after)?,
                        at,
                    ),
                )
                .await?;
        }
        (phase, Some(running)) => {
            write
                .execute(
                    "UPDATE session_compactions SET status = ?2, ended_at = ?3, reported_at = ?3,
                     trigger = coalesce(?4, trigger),
                     tokens_before = coalesce(?5, tokens_before),
                     tokens_after = coalesce(?6, tokens_after)
                     WHERE id = ?1",
                    (
                        running,
                        closed_status(phase).as_str(),
                        at,
                        report.trigger.map(CompactionTrigger::name),
                        token_column(report.tokens_before)?,
                        token_column(report.tokens_after)?,
                    ),
                )
                .await?;
        }
        (phase, None) => {
            write
                .execute(
                    "INSERT INTO session_compactions
                        (session_id, launch_id, status, trigger, tokens_before, tokens_after,
                         ended_at, reported_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                    (
                        session.as_str(),
                        launch,
                        closed_status(phase).as_str(),
                        report.trigger.map(CompactionTrigger::name),
                        token_column(report.tokens_before)?,
                        token_column(report.tokens_after)?,
                        at,
                    ),
                )
                .await?;
        }
    }
    Ok(())
}

fn closed_status(phase: CompactionPhase) -> CompactionStatus {
    match phase {
        CompactionPhase::Failed => CompactionStatus::Failed,
        CompactionPhase::Started | CompactionPhase::Ended => CompactionStatus::Completed,
    }
}

fn token_column(tokens: Option<u64>) -> Result<Option<i64>, Error> {
    tokens
        .map(|tokens| {
            i64::try_from(tokens).map_err(|_| Error::InvalidStoredRow {
                reason: format!("session_compactions: {tokens} tokens do not fit a column"),
            })
        })
        .transpose()
}

async fn running_id(
    connection: &Connection,
    session: &SessionId,
    launch: i64,
) -> Result<Option<i64>, Error> {
    let mut rows = connection
        .query(
            "SELECT max(id) FROM session_compactions
             WHERE session_id = ?1 AND launch_id = ?2 AND status = 'running'",
            (session.as_str(), launch),
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(row.get(0)?),
        None => Ok(None),
    }
}

/// `session`'s latest compaction in its latest launch: a compaction of an earlier launch
/// says nothing of the agent that runs now.
pub async fn latest_compaction(
    connection: &Connection,
    session: &SessionId,
) -> Result<Option<Compaction>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "{SELECT_COMPACTION} WHERE session_id = ?1 AND launch_id =
                    (SELECT max(id) FROM session_launches WHERE session_id = ?1)
                 ORDER BY id DESC LIMIT 1"
            ),
            [session.as_str()],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(compaction_from(&row)?)),
        None => Ok(None),
    }
}

fn compaction_from(row: &Row) -> Result<Compaction, Error> {
    let invalid_row = |reason: String| Error::InvalidStoredRow {
        reason: format!("session_compactions: {reason}"),
    };
    let status = row.get::<String>(1)?;
    let status = CompactionStatus::parse(&status)
        .ok_or_else(|| invalid_row(format!("status {status:?}")))?;
    let trigger = row
        .get::<Option<String>>(2)?
        .map(|name| {
            CompactionTrigger::from_name(&name)
                .ok_or_else(|| invalid_row(format!("trigger {name:?}")))
        })
        .transpose()?;
    let tokens = |index: usize| -> Result<Option<u64>, Error> {
        row.get::<Option<i64>>(index)?
            .map(|tokens| {
                u64::try_from(tokens).map_err(|_| invalid_row(format!("tokens {tokens}")))
            })
            .transpose()
    };
    let started_at: Option<i64> = row.get(5)?;
    let ended_at: Option<i64> = row.get(6)?;
    let seconds = match (started_at, ended_at) {
        (Some(started), Some(ended)) => Some(u64::try_from(ended - started).unwrap_or(0)),
        _ => None,
    };
    Ok(Compaction {
        status,
        trigger,
        tokens_before: tokens(3)?,
        tokens_after: tokens(4)?,
        seconds,
        at: row.get(7)?,
    })
}
