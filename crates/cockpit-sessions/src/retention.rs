//! Launch-payload retention (#406): a session ended or failed long ago loses what only its
//! live agent read, its launches' delivery context and start-file snapshots. The session row,
//! its launches, transitions, reports and exits stay, so an old reference still reads as the
//! session it named, and `resume` records a fresh delivery for its new launch. Decided in
//! docs/decisions/2026-11-04-406-database-retention.md.

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::Connection;

use crate::Error;

/// Rows and apparent payload bytes one session's launches keep.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaunchPayload {
    pub rows: u64,
    pub bytes: u64,
}

/// A session [`retirable_launch_payloads`] lists, with what its launches keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetirablePayload {
    pub session: String,
    pub payload: LaunchPayload,
}

/// Ended or failed, unchanged since `cutoff`, and still holding a launch payload. A resume
/// moves the session out of this state before its new launch records anything, so the
/// writer's recheck never removes a payload a launch still needs.
const IS_RETIRABLE: &str = "s.state IN ('ended', 'failed') AND s.changed_at <= ?1
    AND EXISTS (SELECT 1 FROM session_launches l WHERE l.session_id = s.id AND (
        EXISTS (SELECT 1 FROM session_delivery d WHERE d.execution_id = l.execution_id)
        OR EXISTS (SELECT 1 FROM session_start_files f WHERE f.launch_id = l.id)))";

/// Up to `limit` sessions whose launch payloads retention retires, by id after `after`, each
/// measured in the caller's snapshot. `cutoff` is in seconds since the Unix epoch, as
/// `changed_at` is.
pub async fn retirable_launch_payloads(
    connection: &Connection,
    cutoff: i64,
    after: &str,
    limit: u32,
) -> Result<Vec<RetirablePayload>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT s.id FROM sessions s WHERE {IS_RETIRABLE} AND s.id > ?2
                 ORDER BY s.id LIMIT ?3"
            ),
            (cutoff, after, i64::from(limit)),
        )
        .await?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next().await? {
        sessions.push(row.get::<String>(0)?);
    }
    drop(rows);
    let mut retirable = Vec::with_capacity(sessions.len());
    for session in sessions {
        let payload = measure(connection, &session).await?;
        retirable.push(RetirablePayload { session, payload });
    }
    Ok(retirable)
}

/// Retires `session`'s launch payloads when it is still retirable under the writer, and
/// returns what went; `None` when it no longer is (resumed, or already retired).
pub async fn retire_launch_payload(
    write: &WriteTransaction<'_>,
    session: &str,
    cutoff: i64,
) -> Result<Option<LaunchPayload>, Error> {
    let mut rows = write
        .query(
            &format!("SELECT 1 FROM sessions s WHERE s.id = ?2 AND {IS_RETIRABLE}"),
            (cutoff, session),
        )
        .await?;
    let is_retirable = rows.next().await?.is_some();
    drop(rows);
    if !is_retirable {
        return Ok(None);
    }
    let payload = measure(write, session).await?;
    for table in [
        "session_delivery_mounts",
        "session_delivery_arguments",
        "session_delivery_environment",
        "session_delivery",
    ] {
        write
            .execute(
                &format!(
                    "DELETE FROM {table} WHERE execution_id IN
                        (SELECT execution_id FROM session_launches WHERE session_id = ?1)"
                ),
                [session],
            )
            .await?;
    }
    write
        .execute(
            "DELETE FROM session_start_files WHERE launch_id IN
                (SELECT id FROM session_launches WHERE session_id = ?1)",
            [session],
        )
        .await?;
    Ok(Some(payload))
}

/// Each payload table's row count and apparent bytes for one session's launches.
const MEASURES: [&str; 5] = [
    "SELECT count(*), coalesce(sum(length(d.provider_program)
            + coalesce(length(CAST(d.provider_conversation AS BLOB)), 0)
            + coalesce(length(CAST(d.reason AS BLOB)), 0)), 0)
        FROM session_delivery d JOIN session_launches l ON l.execution_id = d.execution_id
        WHERE l.session_id = ?1",
    "SELECT count(*), coalesce(sum(length(CAST(m.host AS BLOB))
            + length(CAST(m.sandbox AS BLOB))), 0)
        FROM session_delivery_mounts m JOIN session_launches l ON l.execution_id = m.execution_id
        WHERE l.session_id = ?1",
    "SELECT count(*), coalesce(sum(length(a.value)), 0)
        FROM session_delivery_arguments a JOIN session_launches l ON l.execution_id = a.execution_id
        WHERE l.session_id = ?1",
    "SELECT count(*), coalesce(sum(length(e.name) + length(e.value)), 0)
        FROM session_delivery_environment e
        JOIN session_launches l ON l.execution_id = e.execution_id
        WHERE l.session_id = ?1",
    "SELECT count(*), coalesce(sum(length(CAST(f.label AS BLOB)) + length(CAST(f.path AS BLOB))
            + coalesce(length(CAST(f.error AS BLOB)), 0)), 0)
        FROM session_start_files f JOIN session_launches l ON l.id = f.launch_id
        WHERE l.session_id = ?1",
];

async fn measure(connection: &Connection, session: &str) -> Result<LaunchPayload, Error> {
    let mut payload = LaunchPayload::default();
    for sql in MEASURES {
        let mut rows = connection.query(sql, [session]).await?;
        let Some(row) = rows.next().await? else {
            continue;
        };
        payload.rows += count(row.get(0)?)?;
        payload.bytes += count(row.get(1)?)?;
    }
    Ok(payload)
}

fn count(stored: i64) -> Result<u64, Error> {
    u64::try_from(stored).map_err(|_| Error::InvalidStoredRow {
        reason: format!("a negative count {stored}"),
    })
}
