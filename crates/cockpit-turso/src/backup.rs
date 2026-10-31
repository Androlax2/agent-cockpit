use std::fs::File;
use std::path::Path;

use crate::database::{lock_for_single_opener, new_epoch, open_turso, sibling_path};
use crate::{Database, Error};

impl Database {
    /// Writes a consistent, self-contained copy of the database to `destination` while it
    /// stays open for writes (`VACUUM INTO`, SQLite's online-backup statement). Refuses an
    /// existing destination.
    pub async fn backup_to(&self, destination: &Path) -> Result<(), Error> {
        refuse_existing(destination)?;
        let destination_text = destination.to_str().ok_or_else(|| Error::Io {
            path: destination.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the backup path is not valid UTF-8",
            ),
        })?;
        let quoted_destination = destination_text.replace('\'', "''");
        self.connect()?
            .execute(format!("VACUUM INTO '{quoted_destination}'"), ())
            .await?;
        Ok(())
    }
}

/// Replaces the closed database at `target` with `backup`, under a new epoch.
///
/// A restore is a reconciliation, not a resume (report 13): the new epoch makes every
/// change-feed cursor from before it stale. Refuses while a `Database` holds `target`, and
/// leaves `target` untouched when `backup` fails the integrity check.
pub async fn restore(backup: &Path, target: &Path) -> Result<(), Error> {
    let _opener_lock = lock_for_single_opener(target)?;
    let staging = sibling_path(target, ".restoring");
    remove_if_present(&staging)?;
    remove_if_present(&sibling_path(&staging, "-wal"))?;
    std::fs::copy(backup, &staging).map_err(|source| Error::Io {
        path: backup.to_path_buf(),
        source,
    })?;
    prepare_staged_copy(&staging).await?;
    sync_file(&staging)?;
    remove_if_present(&sibling_path(&staging, "-wal"))?;
    remove_if_present(&sibling_path(target, "-wal"))?;
    std::fs::rename(&staging, target).map_err(|source| Error::Io {
        path: target.to_path_buf(),
        source,
    })?;
    if let Some(directory) = target.parent() {
        sync_file(directory)?;
    }
    Ok(())
}

/// Checks the copy's integrity, gives it a new epoch and folds its WAL into the main file.
async fn prepare_staged_copy(staging: &Path) -> Result<(), Error> {
    let staged = open_turso(staging).await?;
    let connection = staged.connect()?;
    let mut rows = connection.query("PRAGMA integrity_check", ()).await?;
    let mut report = Vec::new();
    while let Some(row) = rows.next().await? {
        report.push(row.get::<String>(0)?);
    }
    drop(rows);
    if report != ["ok"] {
        return Err(Error::IntegrityCheckFailed {
            path: staging.to_path_buf(),
            report: report.join("\n"),
        });
    }
    let epochs_updated = connection
        .execute(
            "UPDATE cockpit_database SET value = ?1 WHERE key = 'epoch'",
            [new_epoch()],
        )
        .await?;
    if epochs_updated != 1 {
        return Err(Error::IntegrityCheckFailed {
            path: staging.to_path_buf(),
            report: "not a Cockpit database: it has no epoch".to_string(),
        });
    }
    connection
        .query("PRAGMA wal_checkpoint(TRUNCATE)", ())
        .await?
        .next()
        .await?;
    Ok(())
}

fn refuse_existing(path: &Path) -> Result<(), Error> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(Error::DestinationExists {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn remove_if_present(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn sync_file(path: &Path) -> Result<(), Error> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
}
