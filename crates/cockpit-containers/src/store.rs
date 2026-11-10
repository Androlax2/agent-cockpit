//! The engine the owner recorded for each project (`torlek containers record`, #390). A
//! launch uses only that engine; recording again replaces it, which is how the owner accepts
//! a reinstalled engine.

use cockpit_turso::sql::{Connection, Row};
use cockpit_turso::{Migration, MigrationSet, WriteTransaction};

use crate::RecordedEngine;

pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "containers",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE container_engines (
            project TEXT PRIMARY KEY CHECK (project <> ''),
            engine_user TEXT NOT NULL CHECK (engine_user <> ''),
            engine_id TEXT NOT NULL CHECK (engine_id <> ''),
            recorded_at INTEGER NOT NULL,
            recorded_through TEXT CHECK (recorded_through IN ('tui', 'cli'))
        );",
    }],
};

const COLUMNS: &str = "project, engine_user, engine_id, recorded_at, recorded_through";

/// The engine recorded for `project`, if any.
pub async fn recorded_engine(
    connection: &Connection,
    project: &str,
) -> Result<Option<RecordedEngine>, cockpit_turso::sql::Error> {
    let mut rows = connection
        .query(
            &format!("SELECT {COLUMNS} FROM container_engines WHERE project = ?1"),
            [project],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(engine_from(&row)?)),
        None => Ok(None),
    }
}

/// Records `engine` for its project, replacing the one recorded before, which it returns.
/// The caller verified it and owns the transaction.
pub async fn record_engine(
    write: &WriteTransaction<'_>,
    engine: &RecordedEngine,
) -> Result<Option<RecordedEngine>, cockpit_turso::sql::Error> {
    let replaced = recorded_engine(write, &engine.project).await?;
    write
        .execute(
            "INSERT INTO container_engines
                (project, engine_user, engine_id, recorded_at, recorded_through)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (project) DO UPDATE SET
                engine_user = excluded.engine_user,
                engine_id = excluded.engine_id,
                recorded_at = excluded.recorded_at,
                recorded_through = excluded.recorded_through",
            (
                engine.project.as_str(),
                engine.user.as_str(),
                engine.engine_id.as_str(),
                engine.recorded_at,
                engine.recorded_through.as_deref(),
            ),
        )
        .await?;
    Ok(replaced)
}

fn engine_from(row: &Row) -> Result<RecordedEngine, cockpit_turso::sql::Error> {
    Ok(RecordedEngine {
        project: row.get(0)?,
        user: row.get(1)?,
        engine_id: row.get(2)?,
        recorded_at: row.get(3)?,
        recorded_through: row.get(4)?,
    })
}
