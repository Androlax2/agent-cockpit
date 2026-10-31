//! Change capture: committed changes grouped by transaction, ordered by commit revision.
mod common;

use cockpit_turso::{Change, ChangeKind, Database};
use common::{NOTE_MIGRATIONS, TASK_MIGRATIONS, insert_task, scratch_dir};

async fn open(name: &str) -> Database {
    let path = scratch_dir(name).join("state.db");
    Database::open(&path, &[TASK_MIGRATIONS, NOTE_MIGRATIONS])
        .await
        .unwrap()
}

fn change(kind: ChangeKind, table: &str, row_id: i64) -> Change {
    Change {
        kind,
        table: table.to_string(),
        row_id,
    }
}

#[tokio::test]
async fn one_transaction_over_two_tables_is_one_commit() {
    let database = open("cdc-one-commit").await;
    let revision = database.read().await.unwrap().revision().await.unwrap();

    let write = database.write().await.unwrap();
    write
        .execute("INSERT INTO tasks (id, title) VALUES (7, 'plan')", ())
        .await
        .unwrap();
    write
        .execute(
            "INSERT INTO notes (id, task_id, body) VALUES (3, 7, 'why')",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();

    let commits = database
        .read()
        .await
        .unwrap()
        .commits_after(revision, 10)
        .await
        .unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(
        commits[0].changes,
        [
            change(ChangeKind::Insert, "tasks", 7),
            change(ChangeKind::Insert, "notes", 3)
        ]
    );
}

#[tokio::test]
async fn each_commit_carries_its_own_changes_in_commit_order() {
    let database = open("cdc-order").await;
    let revision = database.read().await.unwrap().revision().await.unwrap();
    insert_task(&database, "first").await;
    let write = database.write().await.unwrap();
    write
        .execute("UPDATE tasks SET title = 'renamed' WHERE id = 1", ())
        .await
        .unwrap();
    write.commit().await.unwrap();
    let write = database.write().await.unwrap();
    write
        .execute("DELETE FROM tasks WHERE id = 1", ())
        .await
        .unwrap();
    write.commit().await.unwrap();

    let commits = database
        .read()
        .await
        .unwrap()
        .commits_after(revision, 10)
        .await
        .unwrap();

    let changes: Vec<&[Change]> = commits.iter().map(|commit| &commit.changes[..]).collect();
    assert_eq!(
        changes,
        [
            &[change(ChangeKind::Insert, "tasks", 1)][..],
            &[change(ChangeKind::Update, "tasks", 1)][..],
            &[change(ChangeKind::Delete, "tasks", 1)][..],
        ]
    );
    assert!(
        commits
            .windows(2)
            .all(|pair| pair[0].revision < pair[1].revision)
    );
}

#[tokio::test]
async fn a_rolled_back_transaction_leaves_no_changes() {
    let database = open("cdc-rollback").await;
    let revision = database.read().await.unwrap().revision().await.unwrap();
    let write = database.write().await.unwrap();
    write
        .execute("INSERT INTO tasks (title) VALUES ('never happened')", ())
        .await
        .unwrap();
    write.rollback().await.unwrap();

    let read = database.read().await.unwrap();

    assert_eq!(read.revision().await.unwrap(), revision);
    assert_eq!(read.commits_after(revision, 10).await.unwrap(), []);
}

#[tokio::test]
async fn the_revision_is_the_last_commit_a_reader_can_see() {
    let database = open("cdc-revision").await;
    insert_task(&database, "one").await;
    insert_task(&database, "two").await;

    let read = database.read().await.unwrap();
    let commits = read.commits_after(0, 100).await.unwrap();

    assert_eq!(
        read.revision().await.unwrap(),
        commits.last().unwrap().revision
    );
}

#[tokio::test]
async fn reading_after_a_revision_stops_at_the_commit_limit() {
    let database = open("cdc-limit").await;
    let revision = database.read().await.unwrap().revision().await.unwrap();
    for title in ["a", "b", "c"] {
        insert_task(&database, title).await;
    }
    let read = database.read().await.unwrap();

    let first_two = read.commits_after(revision, 2).await.unwrap();
    let rest = read
        .commits_after(first_two.last().unwrap().revision, 2)
        .await
        .unwrap();

    assert_eq!(first_two.len(), 2);
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].changes, [change(ChangeKind::Insert, "tasks", 3)]);
}
