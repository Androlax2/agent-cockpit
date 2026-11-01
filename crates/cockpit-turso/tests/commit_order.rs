//! A write transaction's commit order: a clock-free order between transactions (#273).
mod common;

use cockpit_turso::Database;
use common::{NOTE_MIGRATIONS, TASK_MIGRATIONS, scratch_dir};

async fn open(name: &str) -> Database {
    let path = scratch_dir(name).join("state.db");
    Database::open(&path, &[TASK_MIGRATIONS, NOTE_MIGRATIONS])
        .await
        .unwrap()
}

/// Commits a transaction that writes one task, returning its commit order and revision.
async fn committed_task(database: &Database, id: i64) -> (i64, i64) {
    let write = database.write().await.unwrap();
    write
        .execute("INSERT INTO tasks (id, title) VALUES (?1, 'plan')", [id])
        .await
        .unwrap();
    let order = write.commit_order().await.unwrap();
    write.commit().await.unwrap();
    let revision = database.read().await.unwrap().revision().await.unwrap();
    (order, revision)
}

#[tokio::test]
async fn a_transactions_commit_order_lies_between_the_previous_commit_and_its_own() {
    let database = open("commit-order-between").await;
    let (_, previous) = committed_task(&database, 1).await;

    let (order, own) = committed_task(&database, 2).await;

    assert!(previous <= order && order < own, "{previous} {order} {own}");
}

#[tokio::test]
async fn a_later_transaction_orders_after_an_earlier_one() {
    let database = open("commit-order-later").await;
    let (earlier, _) = committed_task(&database, 1).await;

    let (later, _) = committed_task(&database, 2).await;

    assert!(earlier < later, "{earlier} {later}");
}

#[tokio::test]
async fn the_first_transaction_of_a_database_has_an_order() {
    let database = open("commit-order-first").await;
    let write = database.write().await.unwrap();

    let order = write.commit_order().await.unwrap();

    assert!(order >= 0, "{order}");
}
