//! Retiring an ended run's captured output files (#406). The catalog row and the receipt
//! referring to it stay; only `stdout` and `stderr` go. A retirement is durable before any file
//! goes, readers are fenced from that point, and recovery finishes one interrupted midway.
pub(crate) const SCHEMA: &str = "
ALTER TABLE process_output_sets ADD COLUMN retirement TEXT
    CHECK (retirement IN ('retiring', 'retired'));
";

use crate::output_catalog::{invalid, output_facts};
use crate::{Error, ExecutionId, OutputFacts, OutputKind, Supervisor, output_users};
use cockpit_turso::{Database, WriteTransaction, sql::Connection};
use std::path::Path;

/// Where an output's files are in their retirement. `None` on [`OutputFacts`] means kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputRetirement {
    /// Committed, readers fenced; its files may still exist until recovery removes them.
    Retiring,
    Retired,
}

impl OutputRetirement {
    pub(crate) fn parse(id: ExecutionId, stored: Option<&str>) -> Result<Option<Self>, Error> {
        match stored {
            None => Ok(None),
            Some("retiring") => Ok(Some(Self::Retiring)),
            Some("retired") => Ok(Some(Self::Retired)),
            Some(_) => Err(invalid(id, "invalid stored output retirement")),
        }
    }
}

/// An ended, attributed, file-captured output with no pending result and no retirement yet:
/// the only kind a retention policy may consider. Session and newest-run protections are the
/// caller's to apply on top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetirableOutput {
    pub facts: OutputFacts,
    /// When its execution ended, in milliseconds since the Unix epoch.
    pub ended_at: i64,
}

/// A committed retirement whose files may still exist. Only this crate makes one, from the
/// catalog, so removal never trusts a caller-built path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetiringOutput {
    execution: ExecutionId,
    origin_epoch: String,
}

impl RetiringOutput {
    pub fn execution(&self) -> ExecutionId {
        self.execution
    }
}

/// Up to `limit` retirable outputs of this database epoch, oldest execution first, after
/// `after`. Outputs of an earlier epoch are never listed: their files are not this database's.
pub async fn retirable_outputs(
    read: &Connection,
    epoch: &str,
    after: ExecutionId,
    limit: u32,
) -> Result<Vec<RetirableOutput>, Error> {
    let mut rows = read
        .query(
            "SELECT o.execution, p.ended_at FROM process_output_sets o
            JOIN process_executions p ON p.id = o.execution
            WHERE o.origin_epoch = ?1 AND o.mode = 'files' AND o.pending_result = 0
            AND o.project IS NOT NULL AND o.retirement IS NULL AND p.phase = 'ended'
            AND o.execution > ?2 ORDER BY o.execution LIMIT ?3",
            (epoch, after.0, i64::from(limit)),
        )
        .await?;
    let mut listed = Vec::new();
    while let Some(row) = rows.next().await? {
        listed.push((ExecutionId(row.get(0)?), row.get::<i64>(1)?));
    }
    drop(rows);
    let mut outputs = Vec::with_capacity(listed.len());
    for (id, ended_at) in listed {
        outputs.push(RetirableOutput {
            facts: output_facts(read, id).await?,
            ended_at,
        });
    }
    Ok(outputs)
}

/// Marks `id` retiring when it is still what the caller judged at `revision` and nobody reads
/// it. The output-user check and the guard fence both run under the writer, so no reader can
/// start between this commit and the removal. Commit before removing any file.
pub async fn begin_output_retirement(
    write: &WriteTransaction<'_>,
    id: ExecutionId,
    epoch: &str,
    revision: i64,
) -> Result<RetiringOutput, Error> {
    if output_users(epoch, id) != 0 {
        return Err(invalid(id, "output is being read"));
    }
    let changed = write
        .execute(
            "UPDATE process_output_sets SET retirement = 'retiring', revision = revision + 1
            WHERE execution = ?1 AND origin_epoch = ?2 AND revision = ?3 AND mode = 'files'
            AND pending_result = 0 AND project IS NOT NULL AND retirement IS NULL
            AND EXISTS (SELECT 1 FROM process_executions WHERE id = ?1 AND phase = 'ended')",
            (id.0, epoch, revision),
        )
        .await?;
    if changed != 1 {
        return Err(invalid(id, "output changed since it was judged retirable"));
    }
    Ok(RetiringOutput {
        execution: id,
        origin_epoch: epoch.to_owned(),
    })
}

/// Records that `retiring`'s files are gone. Call after [`Supervisor::remove_output_files`].
pub async fn finish_output_retirement(
    write: &WriteTransaction<'_>,
    retiring: RetiringOutput,
) -> Result<(), Error> {
    let changed = write
        .execute(
            "UPDATE process_output_sets SET retirement = 'retired', revision = revision + 1
            WHERE execution = ?1 AND origin_epoch = ?2 AND retirement = 'retiring'",
            (retiring.execution.0, retiring.origin_epoch.as_str()),
        )
        .await?;
    if changed != 1 {
        return Err(invalid(retiring.execution, "output is not retiring"));
    }
    Ok(())
}

/// Retirements of this epoch committed but not finished, oldest first.
pub async fn retiring_outputs(
    read: &Connection,
    epoch: &str,
) -> Result<Vec<RetiringOutput>, Error> {
    let mut rows = read
        .query(
            "SELECT execution FROM process_output_sets
            WHERE origin_epoch = ?1 AND retirement = 'retiring' ORDER BY execution",
            [epoch],
        )
        .await?;
    let mut retiring = Vec::new();
    while let Some(row) = rows.next().await? {
        retiring.push(RetiringOutput {
            execution: ExecutionId(row.get(0)?),
            origin_epoch: epoch.to_owned(),
        });
    }
    Ok(retiring)
}

/// What [`recover_output_retirements`] finished, and each retirement it could not, which
/// stays retiring with its files until a later recovery.
#[derive(Debug, Default)]
pub struct RecoveredRetirements {
    pub finished: usize,
    pub failed: Vec<(ExecutionId, Error)>,
}

/// Finishes every retirement an interruption or a failed removal left between its commit and
/// its last file. One that fails again does not stop the others.
pub async fn recover_output_retirements(
    database: &Database,
    supervisor: &Supervisor,
) -> Result<RecoveredRetirements, Error> {
    let retiring = {
        let read = database.read().await?;
        retiring_outputs(&read, database.epoch()).await?
    };
    let mut recovered = RecoveredRetirements::default();
    for output in retiring {
        let execution = output.execution;
        match finish_removal(database, supervisor, output).await {
            Ok(()) => recovered.finished += 1,
            Err(error) => recovered.failed.push((execution, error)),
        }
    }
    Ok(recovered)
}

async fn finish_removal(
    database: &Database,
    supervisor: &Supervisor,
    output: RetiringOutput,
) -> Result<(), Error> {
    let remover = supervisor.clone();
    let removing = output.clone();
    tokio::task::spawn_blocking(move || remover.remove_output_files(&removing))
        .await
        .map_err(|error| invalid(output.execution, &format!("removal stopped: {error}")))??;
    let write = database.write().await?;
    finish_output_retirement(&write, output).await?;
    write.commit().await?;
    Ok(())
}

impl Supervisor {
    /// Removes `retiring`'s `stdout` and `stderr`, and nothing else in its directory. A file
    /// already gone counts as removed; anything but a regular file there is refused. Returns
    /// the bytes removed. Blocking filesystem work: run it off the async workers.
    pub fn remove_output_files(&self, retiring: &RetiringOutput) -> Result<u64, Error> {
        let directory = self
            .executions_dir()
            .join(&retiring.origin_epoch)
            .join(retiring.execution.0.to_string());
        let mut removed = 0;
        for kind in [OutputKind::Stdout, OutputKind::Stderr] {
            removed += remove_regular_file(&directory.join(kind.file_name()))?;
        }
        Ok(removed)
    }

    /// The apparent bytes of `id`'s `stdout` and `stderr` in this database epoch; a missing
    /// file counts as zero. `None` when either cannot be measured or is not a regular file.
    pub fn output_bytes(&self, database: &Database, id: ExecutionId) -> Option<u64> {
        [OutputKind::Stdout, OutputKind::Stderr]
            .into_iter()
            .map(
                |kind| match self.output_path(database, id, kind).symlink_metadata() {
                    Ok(metadata) if metadata.file_type().is_file() => Some(metadata.len()),
                    Ok(_) => None,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(0),
                    Err(_) => None,
                },
            )
            .sum()
    }
}

fn remove_regular_file(path: &Path) -> Result<u64, Error> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(Error::io("inspect", path)(error)),
    };
    if !metadata.file_type().is_file() {
        return Err(Error::io("remove", path)(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        )));
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(Error::io("remove", path)(error)),
    }
}
