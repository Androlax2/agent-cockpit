use std::fs::{File, OpenOptions, TryLockError};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{Mutex, MutexGuard};

use crate::Error;
use crate::migrations::{self, MigrationSet};

const DATABASE_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS cockpit_database (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
)";

/// The open state file. One per file across every process: a second `open` fails with
/// [`Error::AlreadyOpen`] until this value is dropped.
pub struct Database {
    epoch: String,
    /// Every write goes through this one connection, which has change capture on. The daemon
    /// is the only writer (report 04), so writers queue here instead of on SQLite's busy lock.
    writer: Mutex<turso::Connection>,
    turso: turso::Database,
    /// Declared last so it is released after the Turso handles above are closed.
    _opener_lock: File,
}

impl Database {
    /// Opens (or creates) the state file at `path` and runs the capabilities' migrations in the
    /// given order. Refuses a file another `Database` holds or one with a newer schema.
    pub async fn open(path: &Path, migration_sets: &[MigrationSet]) -> Result<Database, Error> {
        migrations::validate_migrations(migration_sets)?;
        let opener_lock = lock_for_single_opener(path)?;
        let turso = open_turso(path).await?;
        let writer = turso.connect()?;
        // Full mode keeps before and after images: the change feed can drop them later, but
        // images that were never captured cannot be recovered (docs/DECISIONS.md).
        writer
            .execute("PRAGMA capture_data_changes_conn('full')", ())
            .await?;
        writer.execute(migrations::LEDGER_SCHEMA, ()).await?;
        writer.execute(DATABASE_SCHEMA, ()).await?;
        migrations::apply(&writer, migration_sets).await?;
        let epoch = ensure_epoch(&writer).await?;
        Ok(Database {
            epoch,
            writer: Mutex::new(writer),
            turso,
            _opener_lock: opener_lock,
        })
    }

    /// Identifies this file's history. A restore starts a new epoch, so change-feed cursors
    /// from before it are recognisably stale (report 04).
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// Starts the write transaction a multi-capability operation passes to each capability.
    /// Waits for the previous writer to finish. Dropping it without [`WriteTransaction::commit`]
    /// rolls it back before the next write starts.
    pub async fn write(&self) -> Result<WriteTransaction<'_>, Error> {
        let connection = self.writer.lock().await;
        if !connection.is_autocommit()? {
            connection.execute("ROLLBACK", ()).await?;
        }
        connection.execute("BEGIN IMMEDIATE", ()).await?;
        Ok(WriteTransaction { connection })
    }

    /// Starts a read-only transaction on its own connection. Its snapshot is taken here, so
    /// every query and [`ReadTransaction::revision`] in it see the same commit.
    pub async fn read(&self) -> Result<ReadTransaction, Error> {
        let connection = self.turso.connect()?;
        connection.execute("PRAGMA query_only = 1", ()).await?;
        connection.execute("BEGIN", ()).await?;
        connection
            .query("SELECT count(*) FROM cockpit_database", ())
            .await?
            .next()
            .await?;
        Ok(ReadTransaction { connection })
    }

    /// A connection for engine-level statements (backup) that need neither the writer nor a
    /// read-only snapshot.
    pub(crate) fn connect(&self) -> Result<turso::Connection, Error> {
        Ok(self.turso.connect()?)
    }
}

impl std::fmt::Debug for Database {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Database")
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

pub struct WriteTransaction<'database> {
    connection: MutexGuard<'database, turso::Connection>,
}

impl WriteTransaction<'_> {
    pub async fn commit(self) -> Result<(), Error> {
        self.connection.execute("COMMIT", ()).await?;
        Ok(())
    }

    pub async fn rollback(self) -> Result<(), Error> {
        self.connection.execute("ROLLBACK", ()).await?;
        Ok(())
    }
}

impl Deref for WriteTransaction<'_> {
    type Target = turso::Connection;

    fn deref(&self) -> &turso::Connection {
        &self.connection
    }
}

pub struct ReadTransaction {
    connection: turso::Connection,
}

impl Deref for ReadTransaction {
    type Target = turso::Connection;

    fn deref(&self) -> &turso::Connection {
        &self.connection
    }
}

/// Holds an exclusive lock on `<path>.lock` for as long as the returned file lives.
///
/// Turso's own lock on the database file is a process-scoped `fcntl` lock, which a second
/// opener in the same process does not see; this lock is per open file, so it refuses both.
pub(crate) fn lock_for_single_opener(path: &Path) -> Result<File, Error> {
    let lock_path = sibling_path(path, ".lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|source| Error::Io {
            path: lock_path.clone(),
            source,
        })?;
    match lock_file.try_lock() {
        Ok(()) => Ok(lock_file),
        Err(TryLockError::WouldBlock) => Err(Error::AlreadyOpen {
            path: path.to_path_buf(),
        }),
        Err(TryLockError::Error(source)) => Err(Error::Io {
            path: lock_path,
            source,
        }),
    }
}

/// Opens a file in SQLite-compatible mode: default journal mode, no MVCC, no experimental
/// feature flags (report 04).
pub(crate) async fn open_turso(path: &Path) -> Result<turso::Database, Error> {
    let path_text = path.to_str().ok_or_else(|| Error::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the database path is not valid UTF-8",
        ),
    })?;
    Ok(turso::Builder::new_local(path_text).build().await?)
}

/// `path` with `suffix` appended to its file name, as SQLite names `-wal` files.
pub(crate) fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// A value unique to this creation or restore: the time in nanoseconds and the process id.
pub(crate) fn new_epoch() -> String {
    let nanoseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_nanos();
    format!("{nanoseconds:x}-{:x}", std::process::id())
}

async fn ensure_epoch(writer: &turso::Connection) -> Result<String, Error> {
    writer
        .execute(
            "INSERT OR IGNORE INTO cockpit_database (key, value) VALUES ('epoch', ?1)",
            [new_epoch()],
        )
        .await?;
    let mut rows = writer
        .query("SELECT value FROM cockpit_database WHERE key = 'epoch'", ())
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or(turso::Error::QueryReturnedNoRows)?;
    Ok(row.get(0)?)
}
