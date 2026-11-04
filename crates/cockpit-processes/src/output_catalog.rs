//! Output identity belongs to the execution reservation, never to a guessed path.
pub(crate) const SCHEMA: &str = "
CREATE TABLE process_output_sets (
    execution INTEGER PRIMARY KEY REFERENCES process_executions(id),
    origin_epoch TEXT,
    mode TEXT NOT NULL CHECK (mode IN ('unknown', 'files', 'pty', 'hosted')),
    launch_started INTEGER NOT NULL DEFAULT 0 CHECK (launch_started IN (0,1)),
    project TEXT,
    session TEXT,
    pending_result INTEGER NOT NULL DEFAULT 0 CHECK (pending_result IN (0, 1)),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0)
);
INSERT INTO process_output_sets (execution, mode, launch_started) SELECT id, 'unknown', 1 FROM process_executions;
";

use crate::{Error, ExecutionId};
use cockpit_turso::{WriteTransaction, sql::Connection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Unknown,
    Files,
    Pty,
    Hosted,
}
impl OutputMode {
    fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Files => "files",
            Self::Pty => "pty",
            Self::Hosted => "hosted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFacts {
    pub execution: ExecutionId,
    pub origin_epoch: Option<String>,
    pub mode: OutputMode,
    pub project: Option<String>,
    pub session: Option<String>,
    pub pending_result: bool,
    pub active: bool,
    pub revision: i64,
    /// Set once a retention pass committed to removing its files (#406).
    pub retirement: Option<crate::OutputRetirement>,
}
impl OutputFacts {
    /// Session/latest protections are supplied by sessions, in the same read/write snapshot.
    /// These facts alone are never a permission to retire output.
    pub fn protected(&self) -> bool {
        self.active
            || self.pending_result
            || self.mode == OutputMode::Unknown
            || self.project.is_none()
            || self.origin_epoch.is_none()
    }
}

/// Binds caller-owned attribution once, while the execution is still reserved.
/// The caller must commit it in the SAME transaction as reserve and its launch record.
pub async fn attribute_output(
    write: &WriteTransaction<'_>,
    id: ExecutionId,
    epoch: &str,
    project: Option<&str>,
    session: Option<&str>,
    mode: OutputMode,
    pending_result: bool,
) -> Result<(), Error> {
    if mode == OutputMode::Unknown
        || project.is_some_and(|s| s.is_empty() || s.len() > 800)
        || session.is_some_and(|s| s.is_empty() || s.len() > 800)
    {
        return Err(invalid(id, "invalid output attribution"));
    }
    let changed = write.execute("UPDATE process_output_sets SET project = ?1, session = ?2, mode = ?3, pending_result = ?4, revision = revision + 1
        WHERE execution = ?5 AND origin_epoch = ?6 AND mode = 'unknown' AND launch_started = 0
        AND EXISTS (SELECT 1 FROM process_executions WHERE id = ?5 AND phase = 'launching')",
        (project, session, mode.name(), i64::from(pending_result), id.0, epoch)).await?;
    if changed != 1 {
        return Err(invalid(
            id,
            "output attribution changed or its epoch is stale",
        ));
    }
    Ok(())
}

pub async fn output_facts(read: &Connection, id: ExecutionId) -> Result<OutputFacts, Error> {
    let mut rows = read.query("SELECT o.origin_epoch, o.mode, o.project, o.session, o.pending_result, p.phase, o.revision, o.retirement
        FROM process_output_sets o JOIN process_executions p ON p.id = o.execution WHERE o.execution = ?1", [id.0]).await?;
    let row = rows
        .next()
        .await?
        .ok_or(Error::UnknownExecution { execution: id })?;
    let mode: String = row.get(1)?;
    Ok(OutputFacts {
        execution: id,
        origin_epoch: row.get(0)?,
        mode: match mode.as_str() {
            "unknown" => OutputMode::Unknown,
            "files" => OutputMode::Files,
            "pty" => OutputMode::Pty,
            "hosted" => OutputMode::Hosted,
            _ => return Err(invalid(id, "invalid stored output mode")),
        },
        project: row.get(2)?,
        session: row.get(3)?,
        pending_result: row.get::<i64>(4)? != 0,
        active: row.get::<String>(5)? != "ended",
        revision: row.get(6)?,
        retirement: crate::OutputRetirement::parse(id, row.get::<Option<String>>(7)?.as_deref())?,
    })
}

/// Only the caller committing the durable result may release this hold.
pub async fn seal_output_result(
    write: &WriteTransaction<'_>,
    id: ExecutionId,
    origin_epoch: &str,
) -> Result<(), Error> {
    let changed = write
        .execute(
            "UPDATE process_output_sets SET pending_result = 0, revision = revision + 1
        WHERE execution = ?1 AND origin_epoch = ?2 AND pending_result = 1",
            (id.0, origin_epoch),
        )
        .await?;
    if changed != 1 {
        let current = output_facts(write, id).await?;
        if current.origin_epoch.as_deref() != Some(origin_epoch) || current.pending_result {
            return Err(invalid(id, "pending output hold is absent or stale"));
        }
    }
    Ok(())
}

pub(crate) fn invalid(id: ExecutionId, reason: &str) -> Error {
    Error::InvalidStoredExecution {
        execution: id,
        reason: reason.to_owned(),
    }
}

pub async fn latest_file_output(
    read: &Connection,
    session: &str,
) -> Result<Option<ExecutionId>, Error> {
    let row = read
        .prepare("SELECT max(execution) FROM process_output_sets WHERE session=?1 AND mode='files'")
        .await?
        .query_row([session])
        .await?;
    Ok(row.get::<Option<i64>>(0)?.map(ExecutionId))
}

/// Final pre-effect launch fence. Recovery and this transition serialize on the writer.
pub(crate) async fn begin_output_launch(
    database: &cockpit_turso::Database,
    id: ExecutionId,
    mode: OutputMode,
    guarded: bool,
) -> Result<(), Error> {
    let write = database.write().await?;
    let changed = write.execute("UPDATE process_output_sets SET launch_started=1,revision=revision+1
        WHERE execution=?1 AND origin_epoch=?2 AND launch_started=0
        AND (mode='unknown' OR mode=?3) AND (mode<>'files' OR ?4=1)
        AND EXISTS (SELECT 1 FROM process_executions WHERE id=?1 AND phase='launching' AND process_group IS NULL)", (id.0,database.epoch(),mode.name(),i64::from(guarded))).await?;
    if changed != 1 {
        return Err(Error::NotReserved { execution: id });
    }
    write.commit().await?;
    Ok(())
}

/// Positive durable proof that no launcher ever acquired permission to perform an effect.
/// Caller must first exclude all active collectors under this same writer. Does not infer
/// termination from elapsed time, a missing PID, or the absence of output files.
pub async fn end_unstarted_output(
    write: &WriteTransaction<'_>,
    id: ExecutionId,
    origin_epoch: &str,
) -> Result<bool, Error> {
    let changed=write.execute("UPDATE process_executions SET phase='ended', ended_reason='never-started', ended_at=?1
        WHERE id=?2 AND phase='launching' AND process_group IS NULL
        AND EXISTS (SELECT 1 FROM process_output_sets WHERE execution=?2 AND origin_epoch=?3 AND launch_started=0 AND mode='files' AND pending_result=1)",
        (crate::store::now_ms(),id.0,origin_epoch)).await?;
    Ok(changed == 1)
}
