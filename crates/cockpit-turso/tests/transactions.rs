//! Opening the state file, the single-opener rule and write/read transactions.
mod common;

use cockpit_turso::{Database, Error};
use common::{NOTE_MIGRATIONS, TASK_MIGRATIONS, insert_task, scratch_dir, task_titles};

#[tokio::test]
async fn a_committed_write_is_visible_to_later_reads() {
    let path = scratch_dir("committed-write").join("state.db");
    let database = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();

    insert_task(&database, "write the runner").await;

    assert_eq!(task_titles(&database).await, ["write the runner"]);
}

#[tokio::test]
async fn a_second_opener_of_the_same_file_is_refused() {
    let path = scratch_dir("second-opener").join("state.db");
    let _first = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();

    let second = Database::open(&path, &[TASK_MIGRATIONS]).await;

    assert!(
        matches!(second, Err(Error::AlreadyOpen { .. })),
        "{second:?}"
    );
}

#[tokio::test]
async fn closing_the_database_lets_the_file_be_opened_again() {
    let path = scratch_dir("reopen").join("state.db");
    let first = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    insert_task(&first, "survives a reopen").await;
    drop(first);

    let second = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();

    assert_eq!(task_titles(&second).await, ["survives a reopen"]);
}

#[tokio::test]
async fn a_dropped_write_transaction_is_rolled_back() {
    let path = scratch_dir("dropped-write").join("state.db");
    let database = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    let write = database.write().await.unwrap();
    write
        .execute("INSERT INTO tasks (title) VALUES ('abandoned')", ())
        .await
        .unwrap();
    drop(write);

    insert_task(&database, "kept").await;

    assert_eq!(task_titles(&database).await, ["kept"]);
}

/// Report 02: one caller opens the transaction and hands it to each capability's
/// transaction-taking function; a failure in any of them leaves none of their rows.
#[tokio::test]
async fn a_failing_capability_rolls_back_every_capability_in_the_transaction() {
    let path = scratch_dir("multi-capability").join("state.db");
    let database = Database::open(&path, &[TASK_MIGRATIONS, NOTE_MIGRATIONS])
        .await
        .unwrap();

    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO tasks (id, title) VALUES (1, 'split the work')",
            (),
        )
        .await
        .unwrap();
    let note_failure = write
        .execute("INSERT INTO notes (task_id, body) VALUES (1, NULL)", ())
        .await;
    assert!(note_failure.is_err());
    write.rollback().await.unwrap();

    assert_eq!(task_titles(&database).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_read_transaction_refuses_writes() {
    let path = scratch_dir("read-only").join("state.db");
    let database = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    let read = database.read().await.unwrap();

    let write_attempt = read
        .execute("INSERT INTO tasks (title) VALUES ('sneaked in')", ())
        .await;

    assert!(write_attempt.is_err());
}

#[tokio::test]
async fn a_read_transaction_keeps_its_snapshot_while_a_write_commits() {
    let path = scratch_dir("snapshot").join("state.db");
    let database = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    insert_task(&database, "before the snapshot").await;
    let revision_before = database.read().await.unwrap().revision().await.unwrap();
    let read = database.read().await.unwrap();

    insert_task(&database, "after the snapshot").await;

    assert_eq!(read.revision().await.unwrap(), revision_before);
    let mut rows = read.query("SELECT count(*) FROM tasks", ()).await.unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn the_database_epoch_is_stable_across_reopens() {
    let path = scratch_dir("epoch-reopen").join("state.db");
    let first_epoch = Database::open(&path, &[TASK_MIGRATIONS])
        .await
        .unwrap()
        .epoch()
        .to_string();

    let reopened = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();

    assert_eq!(reopened.epoch(), first_epoch);
}
