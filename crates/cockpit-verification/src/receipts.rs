//! Recording receipts and claims, and loading them as [`Evidence`].
//!
//! The daemon records a receipt for a run it executed and reported; an agent's own statement
//! that a check passed is recorded only as a claim. Nothing here grants itself authority: the
//! caller decides who may reach which function.

use std::collections::{BTreeMap, BTreeSet};

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::artifacts::check_relative;
use crate::error::invalid;
use crate::{
    Artifact, CheckDefinition, Claim, ClaimId, Error, Evidence, NewClaim, NewReceipt, Outcome,
    Receipt, ReceiptId, Refusal, Source,
};

/// Recording keeps the last this many bytes of a run's output: failures print last, and the
/// full output stays where `output_references` point.
pub const OUTPUT_MAX_BYTES: usize = 16 * 1024;
/// [`evidence`] loads this many of a check's newest receipts. An older one that would still
/// apply shows as stale: a rerun, never a hidden pass.
pub const RECEIPTS_LOADED_PER_CHECK: u32 = 20;
pub const NAME_MAX_CHARS: usize = 200;
pub const COMMAND_MAX_CHARS: usize = 4_000;
pub const STATEMENT_MAX_CHARS: usize = 2_000;
pub const ARTIFACTS_MAX: usize = 1_000;
pub const OUTPUT_REFERENCES_MAX: usize = 20;

/// Records one run of a check. The same run delivered again returns its receipt; with
/// another result it is refused, so evidence is never rewritten.
pub async fn record_receipt(
    write: &WriteTransaction<'_>,
    new: NewReceipt,
    now: i64,
) -> Result<Receipt, Error> {
    let output_bytes = new.output.len() as u64;
    let new = validated(new)?;
    if let Some(existing) = receipt_of_run(write, &new.project, &new.run_id).await? {
        if existing.recorded == new && existing.output_bytes == output_bytes {
            return Ok(existing);
        }
        return Err(Refusal::RunRecordedDifferently {
            run_id: new.run_id,
            receipt: existing.id,
        }
        .into());
    }
    let id = write
        .prepare(
            "INSERT INTO verification_receipts (project, run_id, check_name, command,
                working_directory, environment, head, base, dirty, outcome, exit_code, output,
                output_bytes, started_at, finished_at, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             RETURNING id",
        )
        .await?
        .query_row((
            new.project.as_str(),
            new.run_id.as_str(),
            new.check.name.as_str(),
            new.check.command.as_str(),
            new.check.working_directory.as_str(),
            new.check.environment.as_str(),
            new.source.head.as_str(),
            new.source.base.as_deref(),
            new.source.dirty.as_deref(),
            new.outcome.as_str(),
            new.exit_code,
            new.output.as_str(),
            i64::try_from(output_bytes).unwrap_or(i64::MAX),
            new.started_at,
            new.finished_at,
            now,
        ))
        .await?
        .get::<i64>(0)?;
    for artifact in &new.artifacts {
        write
            .execute(
                "INSERT INTO verification_artifacts (receipt, path, digest) VALUES (?1, ?2, ?3)",
                (id, artifact.path.as_str(), artifact.digest.as_str()),
            )
            .await?;
    }
    for (position, reference) in (0_i64..).zip(&new.output_references) {
        write
            .execute(
                "INSERT INTO verification_output_references (receipt, position, reference)
                 VALUES (?1, ?2, ?3)",
                (id, position, reference.as_str()),
            )
            .await?;
    }
    Ok(Receipt {
        id: ReceiptId(id),
        recorded: new,
        output_bytes,
        recorded_at: now,
    })
}

/// Records a statement that a check passed, made without a run behind it.
pub async fn record_claim(
    write: &WriteTransaction<'_>,
    new: NewClaim,
    now: i64,
) -> Result<Claim, Error> {
    required("project", &new.project, NAME_MAX_CHARS)?;
    required("check_name", &new.check_name, NAME_MAX_CHARS)?;
    required("claimant", &new.claimant, NAME_MAX_CHARS)?;
    required("statement", &new.statement, STATEMENT_MAX_CHARS)?;
    let id = write
        .prepare(
            "INSERT INTO verification_claims (project, check_name, claimant, statement,
                claimed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             RETURNING id",
        )
        .await?
        .query_row((
            new.project.as_str(),
            new.check_name.as_str(),
            new.claimant.as_str(),
            new.statement.as_str(),
            now,
        ))
        .await?
        .get::<i64>(0)?;
    Ok(Claim {
        id: ClaimId(id),
        check_name: new.check_name,
        claimant: new.claimant,
        statement: new.statement,
        claimed_at: now,
    })
}

/// The newest receipts and every claim of `check_names` in `project`.
pub async fn evidence(
    connection: &Connection,
    project: &str,
    check_names: &[&str],
) -> Result<Evidence, Error> {
    let mut receipts = BTreeMap::new();
    let mut claims = BTreeMap::new();
    for check_name in check_names.iter().collect::<BTreeSet<_>>() {
        receipts.insert(
            (*check_name).to_owned(),
            receipts_of_check(connection, project, check_name).await?,
        );
        claims.insert(
            (*check_name).to_owned(),
            claims_of_check(connection, project, check_name).await?,
        );
    }
    Ok(Evidence { receipts, claims })
}

/// The receipt fields `receipt_from` reads, in its order.
const SELECT_RECEIPTS: &str = "SELECT id, project, run_id, check_name, command,
        working_directory, environment, head, base, dirty, outcome, exit_code, output,
        output_bytes, started_at, finished_at, recorded_at
     FROM verification_receipts";

async fn receipt_of_run(
    connection: &Connection,
    project: &str,
    run_id: &str,
) -> Result<Option<Receipt>, Error> {
    let mut rows = connection
        .query(
            &format!("{SELECT_RECEIPTS} WHERE project = ?1 AND run_id = ?2"),
            (project, run_id),
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(receipt_from(connection, &row).await?)),
        None => Ok(None),
    }
}

async fn receipts_of_check(
    connection: &Connection,
    project: &str,
    check_name: &str,
) -> Result<Vec<Receipt>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "{SELECT_RECEIPTS} WHERE project = ?1 AND check_name = ?2
                 ORDER BY id DESC LIMIT ?3"
            ),
            (project, check_name, RECEIPTS_LOADED_PER_CHECK),
        )
        .await?;
    let mut receipts = Vec::new();
    while let Some(row) = rows.next().await? {
        receipts.push(receipt_from(connection, &row).await?);
    }
    Ok(receipts)
}

async fn receipt_from(connection: &Connection, row: &Row) -> Result<Receipt, Error> {
    let id = row.get::<i64>(0)?;
    let outcome_name = row.get::<String>(10)?;
    let outcome = Outcome::parse(&outcome_name).ok_or_else(|| Error::InvalidStoredRow {
        table: "verification_receipts",
        reason: format!("receipt {id} has outcome {outcome_name:?}"),
    })?;
    let output_bytes = u64::try_from(row.get::<i64>(13)?).map_err(|_| Error::InvalidStoredRow {
        table: "verification_receipts",
        reason: format!("receipt {id} has a negative output size"),
    })?;
    let recorded = NewReceipt {
        project: row.get(1)?,
        run_id: row.get(2)?,
        check: CheckDefinition {
            name: row.get(3)?,
            command: row.get(4)?,
            working_directory: row.get(5)?,
            environment: row.get(6)?,
        },
        source: Source {
            head: row.get(7)?,
            base: row.get(8)?,
            dirty: row.get(9)?,
        },
        artifacts: artifacts_of(connection, id).await?,
        outcome,
        exit_code: row.get(11)?,
        output: row.get(12)?,
        output_references: output_references_of(connection, id).await?,
        started_at: row.get(14)?,
        finished_at: row.get(15)?,
    };
    Ok(Receipt {
        id: ReceiptId(id),
        recorded,
        output_bytes,
        recorded_at: row.get(16)?,
    })
}

async fn artifacts_of(connection: &Connection, receipt: i64) -> Result<Vec<Artifact>, Error> {
    let mut rows = connection
        .query(
            "SELECT path, digest FROM verification_artifacts WHERE receipt = ?1 ORDER BY path",
            [receipt],
        )
        .await?;
    let mut artifacts = Vec::new();
    while let Some(row) = rows.next().await? {
        artifacts.push(Artifact {
            path: row.get(0)?,
            digest: row.get(1)?,
        });
    }
    Ok(artifacts)
}

async fn output_references_of(connection: &Connection, receipt: i64) -> Result<Vec<String>, Error> {
    let mut rows = connection
        .query(
            "SELECT reference FROM verification_output_references WHERE receipt = ?1
             ORDER BY position",
            [receipt],
        )
        .await?;
    let mut references = Vec::new();
    while let Some(row) = rows.next().await? {
        references.push(row.get(0)?);
    }
    Ok(references)
}

async fn claims_of_check(
    connection: &Connection,
    project: &str,
    check_name: &str,
) -> Result<Vec<Claim>, Error> {
    let mut rows = connection
        .query(
            "SELECT id, check_name, claimant, statement, claimed_at FROM verification_claims
             WHERE project = ?1 AND check_name = ?2 ORDER BY id DESC",
            (project, check_name),
        )
        .await?;
    let mut claims = Vec::new();
    while let Some(row) = rows.next().await? {
        claims.push(Claim {
            id: ClaimId(row.get(0)?),
            check_name: row.get(1)?,
            claimant: row.get(2)?,
            statement: row.get(3)?,
            claimed_at: row.get(4)?,
        });
    }
    Ok(claims)
}

/// The receipt as it is stored: every field checked, artifacts ordered by path, the output
/// cut to its tail.
fn validated(mut new: NewReceipt) -> Result<NewReceipt, Refusal> {
    required("project", &new.project, NAME_MAX_CHARS)?;
    required("run_id", &new.run_id, NAME_MAX_CHARS)?;
    required("check.name", &new.check.name, NAME_MAX_CHARS)?;
    required("check.command", &new.check.command, COMMAND_MAX_CHARS)?;
    check_relative(&new.check.working_directory, true)
        .map_err(|reason| invalid("check.working_directory", reason))?;
    required("check.environment", &new.check.environment, NAME_MAX_CHARS)?;
    required("source.head", &new.source.head, NAME_MAX_CHARS)?;
    if let Some(base) = &new.source.base {
        required("source.base", base, NAME_MAX_CHARS)?;
    }
    if let Some(dirty) = &new.source.dirty {
        required("source.dirty", dirty, NAME_MAX_CHARS)?;
    }
    check_exit_code(new.outcome, new.exit_code)?;
    if new.finished_at < new.started_at {
        return Err(invalid("finished_at", "is before started_at"));
    }
    if new.output_references.len() > OUTPUT_REFERENCES_MAX {
        return Err(invalid(
            "output_references",
            format!("at most {OUTPUT_REFERENCES_MAX}"),
        ));
    }
    for reference in &new.output_references {
        required("output_references", reference, COMMAND_MAX_CHARS)?;
    }
    new.artifacts = validated_artifacts(new.artifacts)?;
    new.output = output_tail(new.output);
    Ok(new)
}

fn validated_artifacts(mut artifacts: Vec<Artifact>) -> Result<Vec<Artifact>, Refusal> {
    if artifacts.len() > ARTIFACTS_MAX {
        return Err(invalid("artifacts", format!("at most {ARTIFACTS_MAX}")));
    }
    for artifact in &artifacts {
        check_relative(&artifact.path, false)
            .map_err(|reason| invalid("artifacts", format!("{}: {reason}", artifact.path)))?;
        let is_sha256_hex = artifact.digest.len() == 64
            && artifact
                .digest
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if !is_sha256_hex {
            return Err(invalid(
                "artifacts",
                format!("{}: the digest is not a lower-case SHA-256", artifact.path),
            ));
        }
    }
    artifacts.sort_by(|left, right| left.path.cmp(&right.path));
    if let Some(pair) = artifacts
        .windows(2)
        .find(|pair| pair[0].path == pair[1].path)
    {
        return Err(invalid(
            "artifacts",
            format!("{} is named twice", pair[0].path),
        ));
    }
    Ok(artifacts)
}

/// A pass is a zero exit and a failure is not one; other outcomes may have no exit at all.
fn check_exit_code(outcome: Outcome, exit_code: Option<i64>) -> Result<(), Refusal> {
    match (outcome, exit_code) {
        (Outcome::Passed, Some(0)) => Ok(()),
        (Outcome::Passed, _) => Err(invalid("exit_code", "a passed run exits with 0")),
        (Outcome::Failed, Some(0)) => Err(invalid("exit_code", "a failed run did not exit with 0")),
        _ => Ok(()),
    }
}

fn output_tail(output: String) -> String {
    if output.len() <= OUTPUT_MAX_BYTES {
        return output;
    }
    let mut start = output.len() - OUTPUT_MAX_BYTES;
    while !output.is_char_boundary(start) {
        start += 1;
    }
    output[start..].to_owned()
}

/// Refuses an empty (or blank) value and one longer than `max_chars`.
fn required(field: &'static str, value: &str, max_chars: usize) -> Result<(), Refusal> {
    if value.trim().is_empty() {
        return Err(invalid(field, "is empty"));
    }
    if value.chars().count() > max_chars {
        return Err(invalid(
            field,
            format!("is longer than {max_chars} characters"),
        ));
    }
    Ok(())
}
