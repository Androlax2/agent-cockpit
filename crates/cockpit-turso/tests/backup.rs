//! Online backup, offline restore, and the file staying readable by plain SQLite.
mod common;

use std::process::Command;

use cockpit_turso::{Database, Error, restore};
use common::{TASK_MIGRATIONS, insert_task, scratch_dir, task_titles};

#[tokio::test]
async fn a_backup_restores_to_the_state_it_was_taken_at() {
    let dir = scratch_dir("backup-round-trip");
    let path = dir.join("state.db");
    let backup = dir.join("backup.db");
    let database = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    insert_task(&database, "in the backup").await;
    database.backup_to(&backup).await.unwrap();
    insert_task(&database, "after the backup").await;
    drop(database);

    restore(&backup, &path).await.unwrap();

    let restored = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    assert_eq!(task_titles(&restored).await, ["in the backup"]);
}

#[tokio::test]
async fn a_backup_runs_while_the_database_is_open_for_writes() {
    let dir = scratch_dir("backup-online");
    let database = Database::open(&dir.join("state.db"), &[TASK_MIGRATIONS])
        .await
        .unwrap();
    insert_task(&database, "before").await;

    database.backup_to(&dir.join("backup.db")).await.unwrap();

    insert_task(&database, "after").await;
    assert_eq!(task_titles(&database).await, ["before", "after"]);
}

#[tokio::test]
async fn a_backup_is_one_self_contained_file() {
    let dir = scratch_dir("backup-self-contained");
    let database = Database::open(&dir.join("state.db"), &[TASK_MIGRATIONS])
        .await
        .unwrap();
    insert_task(&database, "only in the main file").await;
    let backup = dir.join("backup.db");
    database.backup_to(&backup).await.unwrap();

    let moved = scratch_dir("backup-self-contained-moved").join("backup.db");
    std::fs::copy(&backup, &moved).unwrap();

    let restored_path = moved.with_file_name("restored.db");
    restore(&moved, &restored_path).await.unwrap();
    let restored = Database::open(&restored_path, &[TASK_MIGRATIONS])
        .await
        .unwrap();
    assert_eq!(task_titles(&restored).await, ["only in the main file"]);
}

#[tokio::test]
async fn a_backup_never_overwrites_an_existing_file() {
    let dir = scratch_dir("backup-existing");
    let database = Database::open(&dir.join("state.db"), &[TASK_MIGRATIONS])
        .await
        .unwrap();
    let existing = dir.join("precious.db");
    std::fs::write(&existing, "keep me").unwrap();

    let backup = database.backup_to(&existing).await;

    assert!(
        matches!(backup, Err(Error::DestinationExists { .. })),
        "{backup:?}"
    );
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "keep me");
}

#[tokio::test]
async fn restoring_over_an_open_database_is_refused() {
    let dir = scratch_dir("restore-open");
    let path = dir.join("state.db");
    let database = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    let backup = dir.join("backup.db");
    database.backup_to(&backup).await.unwrap();

    let restored = restore(&backup, &path).await;

    assert!(
        matches!(restored, Err(Error::AlreadyOpen { .. })),
        "{restored:?}"
    );
}

#[tokio::test]
async fn a_file_that_is_not_a_database_is_never_restored() {
    let dir = scratch_dir("restore-garbage");
    let path = dir.join("state.db");
    let database = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    insert_task(&database, "untouched").await;
    drop(database);
    let garbage = dir.join("garbage.db");
    std::fs::write(&garbage, vec![0x5a; 8192]).unwrap();

    assert!(restore(&garbage, &path).await.is_err());

    let reopened = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    assert_eq!(task_titles(&reopened).await, ["untouched"]);
}

/// Report 04: change-feed cursors are scoped by database epoch, so a client holding a cursor
/// from before the restore gets `resnapshot-required` instead of a silent jump.
#[tokio::test]
async fn a_restore_starts_a_new_database_epoch() {
    let dir = scratch_dir("restore-epoch");
    let path = dir.join("state.db");
    let backup = dir.join("backup.db");
    let database = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    let epoch_before = database.epoch().to_string();
    database.backup_to(&backup).await.unwrap();
    drop(database);

    restore(&backup, &path).await.unwrap();

    let restored = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    assert_ne!(restored.epoch(), epoch_before);
}

/// Report 04: the state file stays readable by ordinary SQLite tools.
#[tokio::test]
async fn a_closed_state_file_opens_with_plain_sqlite3() {
    let path = scratch_dir("sqlite3-compat").join("state.db");
    let database = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    insert_task(&database, "readable by sqlite3").await;
    drop(database);

    let output = Command::new("sqlite3")
        .arg(&path)
        .arg("PRAGMA integrity_check; SELECT title FROM tasks;")
        .output()
        .expect("sqlite3 is installed (Arch: sqlite, Debian/Ubuntu: sqlite3)");

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "ok\nreadable by sqlite3\n"
    );
}
