//! #406: the WAL checkpoint, and the daemon's backup set (named, listed, rotated).
mod common;

use cockpit_turso::{Database, backups, remove_backups_beyond, restore};
use common::{TASK_MIGRATIONS, insert_task, scratch_dir, task_titles};

#[tokio::test]
async fn a_checkpoint_truncates_the_wal_and_keeps_every_row() {
    let dir = scratch_dir("checkpoint-truncates");
    let database = Database::open(&dir.join("state.db"), &[TASK_MIGRATIONS])
        .await
        .unwrap();
    for title in ["one", "two", "three"] {
        insert_task(&database, title).await;
    }
    assert!(database.file_sizes().unwrap().wal_bytes > 0);

    let checkpoint = database.checkpoint().await.unwrap();

    assert!(!checkpoint.is_busy, "{checkpoint:?}");
    assert_eq!(database.file_sizes().unwrap().wal_bytes, 0);
    insert_task(&database, "after").await;
    assert_eq!(
        task_titles(&database).await,
        ["one", "two", "three", "after"]
    );
}

#[tokio::test]
async fn a_backup_into_a_directory_is_named_for_its_time_and_restores() {
    let dir = scratch_dir("backup-into");
    let path = dir.join("state.db");
    let database = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    insert_task(&database, "kept").await;

    let backup = database
        .backup_into(&dir.join("backups"), 1_700_000_000)
        .await
        .unwrap();

    assert_eq!(backup.path, dir.join("backups/state-1700000000.db"));
    assert_eq!(
        backups(&dir.join("backups")).unwrap(),
        std::slice::from_ref(&backup)
    );
    drop(database);
    restore(&backup.path, &path).await.unwrap();
    let restored = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    assert_eq!(task_titles(&restored).await, ["kept"]);
}

#[tokio::test]
async fn a_backup_of_the_same_second_is_refused() {
    let dir = scratch_dir("backup-same-second");
    let database = Database::open(&dir.join("state.db"), &[TASK_MIGRATIONS])
        .await
        .unwrap();
    let backups_dir = dir.join("backups");
    database.backup_into(&backups_dir, 5).await.unwrap();

    let again = database.backup_into(&backups_dir, 5).await;

    assert!(
        matches!(again, Err(cockpit_turso::Error::DestinationExists { .. })),
        "{again:?}"
    );
}

#[test]
fn a_missing_backup_directory_holds_no_backups() {
    let dir = scratch_dir("backups-missing");
    assert_eq!(backups(&dir.join("absent")).unwrap(), []);
    assert_eq!(remove_backups_beyond(&dir.join("absent"), 1).unwrap(), []);
}

#[test]
fn rotation_keeps_the_newest_and_touches_no_other_file() {
    let dir = scratch_dir("backups-rotate");
    for name in [
        "state-100.db",
        "state-300.db",
        "state-20.db",
        "state-0100.db",
        "notes.txt",
        "state-abc.db",
        "state-300.db.bak",
    ] {
        std::fs::write(dir.join(name), name).unwrap();
    }

    let removed = remove_backups_beyond(&dir, 2).unwrap();

    let removed: Vec<_> = removed.iter().map(|backup| backup.taken_at).collect();
    assert_eq!(removed, [20]);
    let mut left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            "notes.txt",
            "state-0100.db",
            "state-100.db",
            "state-300.db",
            "state-300.db.bak",
            "state-abc.db"
        ]
    );
}

#[test]
fn rotation_removes_what_an_interrupted_backup_left() {
    let dir = scratch_dir("backups-partial");
    for name in [
        "state-7.db",
        "state-8.db.partial-123-45",
        "state-8.db.partial-123-45-wal",
        "other.db.partial-1",
    ] {
        std::fs::write(dir.join(name), name).unwrap();
    }

    remove_backups_beyond(&dir, 1).unwrap();

    let mut left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["other.db.partial-1", "state-7.db"]);
}

#[test]
fn keeping_zero_removes_every_backup() {
    let dir = scratch_dir("backups-keep-zero");
    std::fs::write(dir.join("state-1.db"), "a").unwrap();
    std::fs::write(dir.join("state-2.db"), "b").unwrap();

    assert_eq!(remove_backups_beyond(&dir, 0).unwrap().len(), 2);
    assert_eq!(backups(&dir).unwrap(), []);
}
