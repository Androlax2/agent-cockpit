//! Durable producer intent. This is the receipt-identity seam for future retention, not a
//! separate capacity ledger: reservations never deny ordinary verification by a lifetime cap.
use crate::receipts::{optional_text, ran_from};
use crate::{Artifact, CheckDefinition, Error, NewReceipt, Outcome, Receipt, Source};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Value, params_from_iter};

pub(crate) const SCHEMA: &str = "
CREATE TABLE verification_run_identities (
 project TEXT NOT NULL, run_id TEXT NOT NULL, origin_epoch TEXT NOT NULL,
 execution INTEGER NOT NULL, revision INTEGER NOT NULL DEFAULT 1,
 sealed INTEGER NOT NULL DEFAULT 0 CHECK (sealed IN (0,1)),
 recovered INTEGER NOT NULL DEFAULT 0 CHECK (recovered IN (0,1)),
 check_name TEXT NOT NULL, command TEXT NOT NULL, working_directory TEXT NOT NULL,
 environment TEXT NOT NULL, head TEXT NOT NULL, base TEXT, dirty TEXT,
 started_at INTEGER NOT NULL, PRIMARY KEY(project,run_id), UNIQUE(origin_epoch,execution)
);
CREATE TABLE verification_pending_artifacts (
 project TEXT NOT NULL, run_id TEXT NOT NULL, path TEXT NOT NULL, digest TEXT NOT NULL,
 PRIMARY KEY(project,run_id,path),
 FOREIGN KEY(project,run_id) REFERENCES verification_run_identities(project,run_id)
);";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingResult {
    pub origin_epoch: String,
    pub execution: i64,
    pub revision: i64,
    pub sealed: bool,
    pub recovered: bool,
    pub receipt: NewReceipt,
}

pub async fn reserve_result(
    write: &WriteTransaction<'_>,
    epoch: &str,
    execution: i64,
    receipt: NewReceipt,
) -> Result<PendingResult, Error> {
    if epoch.is_empty()
        || execution <= 0
        || receipt.run_id != format!("execution-{epoch}-{execution}")
    {
        return Err(changed());
    }
    let receipt = crate::receipts::validated(receipt.clone(), receipt.started_at)?;
    if receipt.outcome != Outcome::Unknown
        || receipt.exit_code.is_some()
        || !receipt.output.is_empty()
        || !receipt.output_references.is_empty()
    {
        return Err(changed());
    }
    if crate::receipt(write, &receipt.project, &receipt.run_id)
        .await?
        .is_some()
    {
        return Err(changed());
    }
    write.execute("INSERT INTO verification_run_identities
        (project,run_id,origin_epoch,execution,check_name,command,working_directory,environment,head,base,dirty,started_at,
         ran_environment,ran_isolation)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        params_from_iter([Value::Text(receipt.project.clone()), Value::Text(receipt.run_id.clone()), Value::Text(epoch.to_owned()),
         Value::Integer(execution), Value::Text(receipt.check.name.clone()), Value::Text(receipt.check.command.clone()),
         Value::Text(receipt.check.working_directory.clone()), Value::Text(receipt.check.environment.clone()),
         Value::Text(receipt.source.head.clone()), optional_text(receipt.source.base.as_deref()),
         optional_text(receipt.source.dirty.as_deref()), Value::Integer(receipt.started_at),
         optional_text(receipt.ran.as_ref().map(|ran| ran.environment.as_str())),
         optional_text(receipt.ran.as_ref().map(|ran| ran.isolation.as_str()))])).await?;
    for artifact in &receipt.artifacts {
        write
            .execute(
                "INSERT INTO verification_pending_artifacts VALUES (?1,?2,?3,?4)",
                (
                    receipt.project.as_str(),
                    receipt.run_id.as_str(),
                    artifact.path.as_str(),
                    artifact.digest.as_str(),
                ),
            )
            .await?;
    }
    Ok(PendingResult {
        origin_epoch: epoch.to_owned(),
        execution,
        revision: 1,
        sealed: false,
        recovered: false,
        receipt,
    })
}

pub async fn pending_result(
    read: &Connection,
    project: &str,
    run: &str,
) -> Result<Option<PendingResult>, Error> {
    let mut rows = read.query("SELECT origin_epoch,execution,revision,sealed,recovered,check_name,command,working_directory,environment,head,base,dirty,started_at,
        ran_environment,ran_isolation FROM verification_run_identities WHERE project=?1 AND run_id=?2", (project,run)).await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let started_at = row.get(12)?;
    let mut pending = PendingResult {
        origin_epoch: row.get(0)?,
        execution: row.get(1)?,
        revision: row.get(2)?,
        sealed: row.get::<i64>(3)? != 0,
        recovered: row.get::<i64>(4)? != 0,
        receipt: NewReceipt {
            project: project.to_owned(),
            run_id: run.to_owned(),
            check: CheckDefinition {
                name: row.get(5)?,
                command: row.get(6)?,
                working_directory: row.get(7)?,
                environment: row.get(8)?,
            },
            ran: ran_from(row.get(13)?, row.get(14)?).map_err(|reason| {
                Error::InvalidStoredRow {
                    table: "verification_run_identities",
                    reason: format!("run {run} {reason}"),
                }
            })?,
            source: Source {
                head: row.get(9)?,
                base: row.get(10)?,
                dirty: row.get(11)?,
            },
            artifacts: Vec::new(),
            outcome: Outcome::Unknown,
            exit_code: None,
            output: String::new(),
            output_references: Vec::new(),
            started_at,
            finished_at: started_at,
        },
    };
    drop(rows);
    let mut rows = read.query("SELECT path,digest FROM verification_pending_artifacts WHERE project=?1 AND run_id=?2 ORDER BY path", (project,run)).await?;
    while let Some(row) = rows.next().await? {
        pending.receipt.artifacts.push(Artifact {
            path: row.get(0)?,
            digest: row.get(1)?,
        });
    }
    Ok(Some(pending))
}

/// Seals only the original producer revision. Caller commits output-hold release with this.
/// Existing record_receipt's equality/conflict rule remains the sole receipt recording rule.
pub async fn seal_result(
    write: &WriteTransaction<'_>,
    pending: &PendingResult,
    result: NewReceipt,
    bytes: u64,
    now: i64,
    recovered: bool,
) -> Result<Receipt, Error> {
    let current = pending_result(write, &pending.receipt.project, &pending.receipt.run_id)
        .await?
        .ok_or_else(changed)?;
    if current.origin_epoch != pending.origin_epoch
        || current.execution != pending.execution
        || current.receipt != pending.receipt
    {
        return Err(changed());
    }
    let never_ran = result.ran.is_none() && pending.receipt.ran.is_some();
    if never_ran && result.outcome != Outcome::Unavailable {
        return Err(changed());
    }
    let mut input = result.clone();
    // A run whose environment did not start says it ran nowhere, never somewhere else.
    if never_ran {
        input.ran.clone_from(&pending.receipt.ran);
    }
    input.outcome = Outcome::Unknown;
    input.exit_code = None;
    input.output.clear();
    input.output_references.clear();
    input.finished_at = input.started_at;
    if input != pending.receipt
        || (recovered && (result.outcome != Outcome::Unknown || result.exit_code.is_some()))
    {
        return Err(changed());
    }
    if current.sealed {
        // The immutable receipt still decides duplicate equality/conflict after sealing.
        return crate::receipts::record_tail_unchecked_pending(write, result, bytes, now).await;
    }
    if current.revision != pending.revision {
        return Err(changed());
    }
    let receipt = crate::receipts::record_tail_unchecked_pending(write, result, bytes, now).await?;
    write.execute("UPDATE verification_run_identities SET sealed=1,recovered=?1,revision=revision+1 WHERE project=?2 AND run_id=?3",
        (i64::from(recovered), pending.receipt.project.as_str(), pending.receipt.run_id.as_str())).await?;
    Ok(receipt)
}

fn changed() -> Error {
    crate::Refusal::PendingResultChanged.into()
}

/// Only identity/state is needed for owner discovery; do not load receipt artifact payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingIdentity {
    pub run_id: String,
    pub origin_epoch: String,
    pub execution: i64,
    pub revision: i64,
}

pub async fn pending_results(
    read: &Connection,
    project: &str,
    after: i64,
    limit: u32,
) -> Result<Vec<PendingIdentity>, Error> {
    let mut rows = read.query("SELECT run_id,origin_epoch,execution,revision FROM verification_run_identities WHERE project=?1 AND execution>?2 AND sealed=0 ORDER BY execution LIMIT ?3", (project,after,i64::from(limit.min(101)))).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(PendingIdentity {
            run_id: row.get(0)?,
            origin_epoch: row.get(1)?,
            execution: row.get(2)?,
            revision: row.get(3)?,
        });
    }
    Ok(result)
}

/// The executions of `epoch` whose output a retention pass keeps whole (#406): each check's
/// newest run, and its newest finished run while a newer one is still pending.
pub async fn newest_runs(read: &Connection, epoch: &str) -> Result<Vec<i64>, Error> {
    let mut rows = read
        .query(
            "SELECT max(execution) FROM verification_run_identities WHERE origin_epoch = ?1
            GROUP BY project, check_name
            UNION SELECT max(execution) FROM verification_run_identities
            WHERE origin_epoch = ?1 AND sealed = 1 GROUP BY project, check_name",
            [epoch],
        )
        .await?;
    let mut newest = Vec::new();
    while let Some(row) = rows.next().await? {
        newest.push(row.get(0)?);
    }
    Ok(newest)
}

/// Whether `execution` is among [`newest_runs`], looking only at its own check's runs: the
/// recheck a retirement makes under the writer.
pub async fn is_newest_run(read: &Connection, epoch: &str, execution: i64) -> Result<bool, Error> {
    let row = read
        .prepare(
            "SELECT EXISTS (SELECT 1 FROM verification_run_identities r
            WHERE r.origin_epoch = ?1 AND r.execution = ?2 AND (
                r.execution = (SELECT max(execution) FROM verification_run_identities
                    WHERE origin_epoch = ?1 AND project = r.project AND check_name = r.check_name)
                OR r.execution = (SELECT max(execution) FROM verification_run_identities
                    WHERE origin_epoch = ?1 AND project = r.project AND check_name = r.check_name
                    AND sealed = 1)))",
        )
        .await?
        .query_row((epoch, execution))
        .await?;
    Ok(row.get::<i64>(0)? != 0)
}
