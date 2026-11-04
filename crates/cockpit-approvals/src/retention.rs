//! Run-output retention (#406): a finished command run ended long ago loses its output text.
//! The request stays whole otherwise: asked and edited command, reason, decision, who decided
//! and through which client, approved digest, exit code and times, so it remains the audit
//! record of what the owner approved. A start request's output names the session it started
//! and is kept. Decided in docs/decisions/2026-11-04-406-database-retention.md.

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::Connection;

use crate::Error;
use crate::rows::invalid_row;

/// How many run outputs, and their apparent bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunOutputs {
    pub runs: u64,
    pub bytes: u64,
}

/// A finished command run that ended at or before `?1` and still holds its output.
const IS_RETIRABLE: &str = "state = 'finished' AND kind = 'command' AND ended_at <= ?1
    AND output_retired_at IS NULL";

/// What [`retire_run_outputs`] would clear now, in the caller's snapshot. `cutoff` is in
/// seconds since the Unix epoch, as `ended_at` is.
pub async fn retirable_run_outputs(
    connection: &Connection,
    cutoff: i64,
) -> Result<RunOutputs, Error> {
    measure(
        connection,
        &format!("SELECT count(*), coalesce(sum(length(CAST(output AS BLOB))), 0) FROM approvals_runs WHERE {IS_RETIRABLE}"),
        (cutoff,),
    )
    .await
}

/// Clears the output of up to `limit` retirable runs, oldest first, and records `now` as when;
/// returns what went. Call again until it retires none.
pub async fn retire_run_outputs(
    write: &WriteTransaction<'_>,
    cutoff: i64,
    now: i64,
    limit: u32,
) -> Result<RunOutputs, Error> {
    let batch = format!("SELECT id FROM approvals_runs WHERE {IS_RETIRABLE} ORDER BY id LIMIT ?2");
    let retired = measure(
        write,
        &format!(
            "SELECT count(*), coalesce(sum(length(CAST(output AS BLOB))), 0) FROM approvals_runs
             WHERE id IN ({batch})"
        ),
        (cutoff, i64::from(limit)),
    )
    .await?;
    let updated = write
        .execute(
            &format!(
                "UPDATE approvals_runs SET output = '', output_retired_at = ?3
                 WHERE id IN ({batch})"
            ),
            (cutoff, i64::from(limit), now),
        )
        .await?;
    if updated != retired.runs {
        return Err(invalid_row(
            "approvals_runs",
            format!("cleared {updated} outputs of the {} measured", retired.runs),
        ));
    }
    Ok(retired)
}

async fn measure(
    connection: &Connection,
    sql: &str,
    params: impl cockpit_turso::sql::IntoParams,
) -> Result<RunOutputs, Error> {
    let mut rows = connection.query(sql, params).await?;
    let Some(row) = rows.next().await? else {
        return Ok(RunOutputs::default());
    };
    let count = |column: usize| -> Result<u64, Error> {
        let stored: i64 = row.get(column)?;
        u64::try_from(stored)
            .map_err(|_| invalid_row("approvals_runs", format!("a negative count {stored}")))
    };
    Ok(RunOutputs {
        runs: count(0)?,
        bytes: count(1)?,
    })
}
