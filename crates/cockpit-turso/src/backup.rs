use std::fs::File;
use std::path::{Path, PathBuf};

use crate::database::{lock_for_single_opener, new_epoch, open_turso, path_text, sibling_path};
use crate::{Database, Error};

impl Database {
    /// Writes a consistent, self-contained copy of the database to `destination` while it
    /// stays open for writes (`VACUUM INTO`, SQLite's online-backup statement). Never
    /// overwrites: an existing destination fails with [`Error::DestinationExists`].
    pub async fn backup_to(&self, destination: &Path) -> Result<(), Error> {
        refuse_existing(destination)?;
        // The copy is written under a private name and linked into place, so a half-written
        // backup never appears at `destination` and the link, not a check, refuses a file
        // created there meanwhile.
        let partial = sibling_path(destination, &format!(".partial-{}", new_epoch()));
        let quoted_partial = path_text(&partial)?.replace('\'', "''");
        let written = self
            .connect()?
            .execute(format!("VACUUM INTO '{quoted_partial}'"), ())
            .await;
        let linked = written.map_err(Error::from).and_then(|_| {
            refuse_unfolded_wal(&partial)?;
            sync_file(&partial)?;
            link_new(&partial, destination)
        });
        remove_if_present(&partial)?;
        remove_if_present(&sibling_path(&partial, "-wal"))?;
        linked?;
        sync_file(&parent_directory(destination))
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
    std::fs::copy(backup, &staging).map_err(Error::io(backup))?;
    prepare_staged_copy(&staging).await?;
    refuse_unfolded_wal(&staging)?;
    remove_if_present(&sibling_path(&staging, "-wal"))?;
    sync_file(&staging)?;
    // The old WAL must not meet the new file. It moves aside rather than being deleted, so a
    // crash before the rename leaves its frames on disk; rerunning the restore finishes it.
    let target_wal = sibling_path(target, "-wal");
    let replaced_wal = sibling_path(target, "-wal.replaced");
    rename_if_present(&target_wal, &replaced_wal)?;
    std::fs::rename(&staging, target).map_err(Error::io(target))?;
    sync_file(&parent_directory(target))?;
    remove_if_present(&replaced_wal)
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

/// Fails when `database`'s WAL still holds frames: the file alone would miss them, and the
/// caller deletes the WAL next.
fn refuse_unfolded_wal(database: &Path) -> Result<(), Error> {
    let wal = sibling_path(database, "-wal");
    if std::fs::metadata(&wal).is_ok_and(|metadata| metadata.len() > 0) {
        return Err(Error::CheckpointIncomplete { path: wal });
    }
    Ok(())
}

fn refuse_existing(path: &Path) -> Result<(), Error> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(Error::DestinationExists {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::io(path)(source)),
    }
}

/// Links `from` at `to`, which must not exist: the link is the atomic no-overwrite step.
fn link_new(from: &Path, to: &Path) -> Result<(), Error> {
    match std::fs::hard_link(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(Error::DestinationExists {
                path: to.to_path_buf(),
            })
        }
        Err(source) => Err(Error::io(to)(source)),
    }
}

/// The directory holding `path`; `.` for a bare file name, whose parent is the empty path.
fn parent_directory(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn rename_if_present(from: &Path, to: &Path) -> Result<(), Error> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::io(from)(source)),
    }
}

fn remove_if_present(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::io(path)(source)),
    }
}

fn sync_file(path: &Path) -> Result<(), Error> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(Error::io(path))
}
