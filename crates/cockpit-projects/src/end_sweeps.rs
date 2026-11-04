//! What the sweep did to a clone once the session that occupied it ended (#767), kept for
//! `torlek storage`. One record per clone incarnation and session end (the number of the
//! session's latest launch), so a clone is swept once per end and a daemon restart does not
//! sweep it again.

use std::path::PathBuf;

use cockpit_turso::sql::{Connection, params};
use cockpit_turso::{Migration, WriteTransaction};

use crate::store::{now, path_text};
use crate::{Checkout, CheckoutId, Error};

pub(crate) const MIGRATION: Migration = Migration {
    version: 4,
    sql: "CREATE TABLE project_end_sweeps (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        checkout_id INTEGER NOT NULL,
        incarnation INTEGER NOT NULL,
        session TEXT NOT NULL CHECK (session <> ''),
        session_launch INTEGER NOT NULL,
        project TEXT NOT NULL,
        path TEXT NOT NULL,
        kept TEXT,
        target TEXT,
        swept_at INTEGER NOT NULL,
        UNIQUE (checkout_id, incarnation, session, session_launch)
    );",
};

/// One clone swept at its session's end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndSweep {
    pub checkout: CheckoutId,
    pub incarnation: i64,
    /// The ended session that occupied the clone.
    pub session: String,
    pub project: String,
    pub path: PathBuf,
    /// Why the clone was kept; `None` when it was deleted.
    pub kept: Option<String>,
    /// What became of a kept clone's `target/`; `None` when its pruning was not tried.
    pub target: Option<String>,
    /// Seconds since the Unix epoch.
    pub swept_at: i64,
}

/// Whether the end of `session` after its launch `session_launch` already swept this
/// incarnation of `checkout`.
pub async fn end_sweep_recorded(
    connection: &Connection,
    checkout: CheckoutId,
    incarnation: i64,
    session: &str,
    session_launch: i64,
) -> Result<bool, Error> {
    let mut rows = connection
        .query(
            "SELECT 1 FROM project_end_sweeps
             WHERE checkout_id = ?1 AND incarnation = ?2 AND session = ?3
               AND session_launch = ?4",
            params![checkout.0, incarnation, session, session_launch],
        )
        .await?;
    Ok(rows.next().await?.is_some())
}

/// Records, stamped now, what the end of `session` after its launch `session_launch` did to
/// this incarnation of `checkout`: `kept` says why it was kept (`None`: deleted), `target`
/// what became of its `target/`. A second record of the same end is refused by the table.
pub async fn record_end_sweep(
    write: &WriteTransaction<'_>,
    checkout: &Checkout,
    session: &str,
    session_launch: i64,
    kept: Option<&str>,
    target: Option<&str>,
) -> Result<(), Error> {
    write
        .execute(
            "INSERT INTO project_end_sweeps
                 (checkout_id, incarnation, session, session_launch, project, path, kept,
                  target, swept_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                checkout.id.0,
                checkout.incarnation,
                session,
                session_launch,
                checkout.project.as_str(),
                path_text(&checkout.path)?,
                kept,
                target,
                now()
            ],
        )
        .await?;
    Ok(())
}

/// The `limit` latest session-end sweeps, newest first.
pub async fn end_sweeps(connection: &Connection, limit: i64) -> Result<Vec<EndSweep>, Error> {
    let mut rows = connection
        .query(
            "SELECT checkout_id, incarnation, session, project, path, kept, target, swept_at
             FROM project_end_sweeps ORDER BY id DESC LIMIT ?1",
            [limit],
        )
        .await?;
    let mut sweeps = Vec::new();
    while let Some(row) = rows.next().await? {
        sweeps.push(EndSweep {
            checkout: CheckoutId(row.get(0)?),
            incarnation: row.get(1)?,
            session: row.get(2)?,
            project: row.get(3)?,
            path: PathBuf::from(row.get::<String>(4)?),
            kept: row.get(5)?,
            target: row.get(6)?,
            swept_at: row.get(7)?,
        });
    }
    Ok(sweeps)
}
