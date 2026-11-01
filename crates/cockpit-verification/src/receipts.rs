//! Recording receipts and claims, and loading them as [`Evidence`].
//!
//! The daemon records a receipt for a run it executed and reported; an agent's own statement
//! that a check passed is recorded only as a claim. Nothing here grants itself authority: the
//! caller decides who may reach which function.

use std::collections::{BTreeSet, HashMap};

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row, Value, params_from_iter};

use crate::artifacts::{check_artifact_paths, check_relative};
use crate::error::invalid;
use crate::gate::CheckEvidence;
use crate::{
    Artifact, CheckDefinition, Claim, ClaimId, Error, Evidence, NewClaim, NewReceipt, Outcome,
    Receipt, ReceiptId, Refusal, Source,
};

/// Recording keeps the last this many bytes of a run's output: failures print last, and the
/// full output stays where `output_references` point.
pub const OUTPUT_MAX_BYTES: usize = 16 * 1024;
/// [`evidence`] loads this many of a check's newest claims on the source asked about.
pub const CLAIMS_LOADED_PER_CHECK: u32 = 20;
pub const NAME_MAX_CHARS: usize = 200;
pub const PATH_MAX_CHARS: usize = 1_000;
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
    let stored_output_bytes =
        i64::try_from(output_bytes).map_err(|_| invalid("output", "is too large"))?;
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
            stored_output_bytes,
            new.started_at,
            new.finished_at,
            now,
        ))
        .await?
        .get::<i64>(0)?;
    let mut insert_artifact = write
        .prepare("INSERT INTO verification_artifacts (receipt, path, digest) VALUES (?1, ?2, ?3)")
        .await?;
    for artifact in &new.artifacts {
        insert_artifact
            .execute((id, artifact.path.as_str(), artifact.digest.as_str()))
            .await?;
    }
    let mut insert_reference = write
        .prepare(
            "INSERT INTO verification_output_references (receipt, position, reference)
             VALUES (?1, ?2, ?3)",
        )
        .await?;
    for (position, reference) in (0_i64..).zip(&new.output_references) {
        insert_reference
            .execute((id, position, reference.as_str()))
            .await?;
    }
    Ok(Receipt {
        id: ReceiptId(id),
        recorded: new,
        output_bytes,
        recorded_at: now,
    })
}

/// Records a statement that a check passed on `source`, made without a run behind it.
pub async fn record_claim(
    write: &WriteTransaction<'_>,
    new: NewClaim,
    now: i64,
) -> Result<Claim, Error> {
    required("project", &new.project, NAME_MAX_CHARS)?;
    required("check_name", &new.check_name, NAME_MAX_CHARS)?;
    check_source(&new.source)?;
    required("claimant", &new.claimant, NAME_MAX_CHARS)?;
    required("statement", &new.statement, STATEMENT_MAX_CHARS)?;
    let id = write
        .prepare(
            "INSERT INTO verification_claims (project, check_name, head, base, dirty, claimant,
                statement, claimed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             RETURNING id",
        )
        .await?
        .query_row((
            new.project.as_str(),
            new.check_name.as_str(),
            new.source.head.as_str(),
            new.source.base.as_deref(),
            new.source.dirty.as_deref(),
            new.claimant.as_str(),
            new.statement.as_str(),
            now,
        ))
        .await?
        .get::<i64>(0)?;
    Ok(Claim {
        id: ClaimId(id),
        check_name: new.check_name,
        source: new.source,
        claimant: new.claimant,
        statement: new.statement,
        claimed_at: now,
    })
}

/// What `project` holds about a gate requiring `checks` on the exact `source`: each check's
/// newest receipt of exactly its definition there, the source of its newest other receipt,
/// and its claims about the source. Refused for an empty gate or a check required twice.
pub async fn evidence(
    connection: &Connection,
    project: &str,
    source: &Source,
    checks: &[CheckDefinition],
) -> Result<Evidence, Error> {
    if checks.is_empty() {
        return Err(invalid("checks", "a gate requires at least one check").into());
    }
    let mut seen = BTreeSet::new();
    if let Some(twice) = checks
        .iter()
        .find(|check| !seen.insert(check.name.as_str()))
    {
        return Err(invalid("checks", format!("`{}` is required twice", twice.name)).into());
    }
    let mut loaded = Vec::with_capacity(checks.len());
    for check in checks {
        let on_source = vec![
            Value::Text(project.to_owned()),
            Value::Text(check.name.clone()),
            Value::Text(source.head.clone()),
            source.dirty.clone().map_or(Value::Null, Value::Text),
        ];
        let exact: Vec<Value> = on_source
            .iter()
            .cloned()
            .chain([
                Value::Text(check.command.clone()),
                Value::Text(check.working_directory.clone()),
                Value::Text(check.environment.clone()),
            ])
            .collect();
        let receipt = load_receipts(
            connection,
            &format!("{EXACT} ORDER BY id DESC LIMIT 1"),
            exact.clone(),
        )
        .await?
        .pop();
        loaded.push(CheckEvidence {
            check: check.clone(),
            receipt,
            other_source: other_source(connection, exact).await?,
            claims: claims_on_source(connection, on_source).await?,
        });
    }
    Ok(Evidence {
        source: source.clone(),
        checks: loaded,
    })
}

/// The receipts of check `?2` in project `?1` on head `?3` with uncommitted changes `?4`
/// (`IS`: equal when both are clean), of command `?5` in `?6` on environment `?7`.
const EXACT: &str = "project = ?1 AND check_name = ?2 AND head = ?3 AND dirty IS ?4
    AND command = ?5 AND working_directory = ?6 AND environment = ?7";

/// The source of the newest receipt of check `?2` in project `?1` that is not [`EXACT`].
async fn other_source(connection: &Connection, exact: Vec<Value>) -> Result<Option<Source>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT head, base, dirty FROM verification_receipts
                 WHERE project = ?1 AND check_name = ?2 AND NOT ({EXACT})
                 ORDER BY id DESC LIMIT 1"
            ),
            params_from_iter(exact),
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(Source {
        head: row.get(0)?,
        base: row.get(1)?,
        dirty: row.get(2)?,
    }))
}

async fn receipt_of_run(
    connection: &Connection,
    project: &str,
    run_id: &str,
) -> Result<Option<Receipt>, Error> {
    let found = load_receipts(
        connection,
        "project = ?1 AND run_id = ?2",
        vec![
            Value::Text(project.to_owned()),
            Value::Text(run_id.to_owned()),
        ],
    )
    .await?;
    Ok(found.into_iter().next())
}

/// The receipts matching `condition`, in its order, read in three queries: the receipts,
/// then every artifact and every output reference of them.
async fn load_receipts(
    connection: &Connection,
    condition: &str,
    parameters: Vec<Value>,
) -> Result<Vec<Receipt>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT id, project, run_id, check_name, command, working_directory,
                    environment, head, base, dirty, outcome, exit_code, output, output_bytes,
                    started_at, finished_at, recorded_at
                 FROM verification_receipts WHERE {condition}"
            ),
            params_from_iter(parameters),
        )
        .await?;
    let mut receipts = Vec::new();
    while let Some(row) = rows.next().await? {
        receipts.push(receipt_from(&row)?);
    }
    if receipts.is_empty() {
        return Ok(receipts);
    }
    // Ids are integers this crate read, so listing them inline cannot inject SQL.
    let ids = receipts
        .iter()
        .map(|receipt| receipt.id.0.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let mut artifacts = artifacts_of(connection, &ids).await?;
    let mut references = output_references_of(connection, &ids).await?;
    for receipt in &mut receipts {
        receipt.recorded.artifacts = artifacts.remove(&receipt.id.0).unwrap_or_default();
        receipt.recorded.output_references = references.remove(&receipt.id.0).unwrap_or_default();
    }
    Ok(receipts)
}

/// A receipt without its artifacts and output references, which [`load_receipts`] adds.
fn receipt_from(row: &Row) -> Result<Receipt, Error> {
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
        artifacts: Vec::new(),
        outcome,
        exit_code: row.get(11)?,
        output: row.get(12)?,
        output_references: Vec::new(),
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

async fn artifacts_of(
    connection: &Connection,
    receipt_ids: &str,
) -> Result<HashMap<i64, Vec<Artifact>>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT receipt, path, digest FROM verification_artifacts
                 WHERE receipt IN ({receipt_ids}) ORDER BY receipt, path"
            ),
            (),
        )
        .await?;
    let mut artifacts: HashMap<i64, Vec<Artifact>> = HashMap::new();
    while let Some(row) = rows.next().await? {
        artifacts.entry(row.get(0)?).or_default().push(Artifact {
            path: row.get(1)?,
            digest: row.get(2)?,
        });
    }
    Ok(artifacts)
}

async fn output_references_of(
    connection: &Connection,
    receipt_ids: &str,
) -> Result<HashMap<i64, Vec<String>>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT receipt, reference FROM verification_output_references
                 WHERE receipt IN ({receipt_ids}) ORDER BY receipt, position"
            ),
            (),
        )
        .await?;
    let mut references: HashMap<i64, Vec<String>> = HashMap::new();
    while let Some(row) = rows.next().await? {
        references.entry(row.get(0)?).or_default().push(row.get(1)?);
    }
    Ok(references)
}

/// The claims of check `?2` in project `?1` about head `?3` with uncommitted changes `?4`.
async fn claims_on_source(
    connection: &Connection,
    parameters: Vec<Value>,
) -> Result<Vec<Claim>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT id, check_name, head, base, dirty, claimant, statement, claimed_at
                 FROM verification_claims
                 WHERE project = ?1 AND check_name = ?2 AND head = ?3 AND dirty IS ?4
                 ORDER BY id DESC LIMIT {CLAIMS_LOADED_PER_CHECK}"
            ),
            params_from_iter(parameters),
        )
        .await?;
    let mut claims = Vec::new();
    while let Some(row) = rows.next().await? {
        claims.push(Claim {
            id: ClaimId(row.get(0)?),
            check_name: row.get(1)?,
            source: Source {
                head: row.get(2)?,
                base: row.get(3)?,
                dirty: row.get(4)?,
            },
            claimant: row.get(5)?,
            statement: row.get(6)?,
            claimed_at: row.get(7)?,
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
    if new.check.working_directory != "." {
        check_relative(&new.check.working_directory)
            .map_err(|reason| invalid("check.working_directory", reason))?;
    }
    required("check.environment", &new.check.environment, NAME_MAX_CHARS)?;
    check_source(&new.source)?;
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
    new.artifacts
        .sort_by(|left, right| left.path.cmp(&right.path));
    let paths: Vec<&str> = new
        .artifacts
        .iter()
        .map(|artifact| artifact.path.as_str())
        .collect();
    check_artifact_paths(&paths)?;
    for artifact in &new.artifacts {
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
    new.output = output_tail(new.output);
    Ok(new)
}

fn check_source(source: &Source) -> Result<(), Refusal> {
    required("source.head", &source.head, NAME_MAX_CHARS)?;
    if let Some(base) = &source.base {
        required("source.base", base, NAME_MAX_CHARS)?;
    }
    if let Some(dirty) = &source.dirty {
        required("source.dirty", dirty, NAME_MAX_CHARS)?;
    }
    Ok(())
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
