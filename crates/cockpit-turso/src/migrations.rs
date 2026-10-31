use std::collections::{BTreeMap, HashSet};

use crate::Error;

/// One schema step of a capability. `sql` may hold several statements; it runs in one
/// transaction with the ledger row that records it.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub sql: &'static str,
}

/// A capability's migrations, versions counting up from 1. Applied migrations are never edited:
/// a schema change is a new version.
#[derive(Debug, Clone, Copy)]
pub struct MigrationSet {
    pub owner: &'static str,
    pub migrations: &'static [Migration],
}

/// Checks the binary's own migration list: each owner once, versions 1, 2, 3... in order.
pub fn validate_migrations(sets: &[MigrationSet]) -> Result<(), Error> {
    let mut owners = HashSet::new();
    for set in sets {
        let invalid = |reason: String| Error::InvalidMigrations {
            owner: set.owner.to_string(),
            reason,
        };
        if set.owner.is_empty() {
            return Err(invalid("the owner name is empty".to_string()));
        }
        if !owners.insert(set.owner) {
            return Err(invalid("the owner appears more than once".to_string()));
        }
        for (position, migration) in set.migrations.iter().enumerate() {
            let expected_version = position as u32 + 1;
            if migration.version != expected_version {
                return Err(invalid(format!(
                    "version {} where {expected_version} was expected",
                    migration.version
                )));
            }
        }
    }
    Ok(())
}

pub(crate) const LEDGER_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS cockpit_migrations (
    owner TEXT NOT NULL,
    version INTEGER NOT NULL,
    sql TEXT NOT NULL,
    PRIMARY KEY (owner, version)
)";

/// Applies every missing migration, set by set in the given order, each in its own write
/// transaction. Refuses before applying anything when the file holds a migration the binary
/// does not know or one whose SQL changed. `sets` has passed [`validate_migrations`].
pub(crate) async fn apply(
    connection: &turso::Connection,
    sets: &[MigrationSet],
) -> Result<(), Error> {
    let applied = applied_migrations(connection).await?;
    refuse_unknown_schema(&applied, sets)?;
    for set in sets {
        for migration in set.migrations {
            match applied.get(&(set.owner.to_string(), migration.version)) {
                Some(applied_sql) if applied_sql == migration.sql => {}
                Some(_) => {
                    return Err(Error::MigrationEdited {
                        owner: set.owner.to_string(),
                        version: migration.version,
                    });
                }
                None => apply_one(connection, set.owner, migration).await?,
            }
        }
    }
    Ok(())
}

async fn applied_migrations(
    connection: &turso::Connection,
) -> Result<BTreeMap<(String, u32), String>, Error> {
    let mut rows = connection
        .query("SELECT owner, version, sql FROM cockpit_migrations", ())
        .await?;
    let mut applied = BTreeMap::new();
    while let Some(row) = rows.next().await? {
        let owner: String = row.get(0)?;
        let version: i64 = row.get(1)?;
        let Ok(version) = u32::try_from(version) else {
            return Err(Error::InvalidMigrations {
                owner,
                reason: format!("the database records version {version}"),
            });
        };
        applied.insert((owner, version), row.get(2)?);
    }
    Ok(applied)
}

fn refuse_unknown_schema(
    applied: &BTreeMap<(String, u32), String>,
    sets: &[MigrationSet],
) -> Result<(), Error> {
    for (owner, applied_version) in applied.keys() {
        let known_version = sets
            .iter()
            .find(|set| set.owner == owner)
            .map_or(0, |set| set.migrations.len() as u32);
        if *applied_version > known_version {
            return Err(Error::SchemaNewerThanBinary {
                owner: owner.clone(),
                applied_version: *applied_version,
                known_version,
            });
        }
    }
    Ok(())
}

async fn apply_one(
    connection: &turso::Connection,
    owner: &str,
    migration: &Migration,
) -> Result<(), Error> {
    connection.execute("BEGIN IMMEDIATE", ()).await?;
    let applied = async {
        connection.execute_batch(migration.sql).await?;
        connection
            .execute(
                "INSERT INTO cockpit_migrations (owner, version, sql) VALUES (?1, ?2, ?3)",
                (owner, i64::from(migration.version), migration.sql),
            )
            .await?;
        connection.execute("COMMIT", ()).await
    }
    .await;
    if let Err(error) = applied {
        if !connection.is_autocommit()? {
            connection.execute("ROLLBACK", ()).await?;
        }
        return Err(error.into());
    }
    Ok(())
}
