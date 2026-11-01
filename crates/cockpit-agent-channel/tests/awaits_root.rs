//! `awaits_root` (#279): only a pending launch whose root is not recorded yet is worth
//! waiting for; every other token is identified, or refused, at once.

mod common;

use cockpit_agent_channel::{LaunchToken, awaits_root, mint, record_root};
use cockpit_turso::Database;
use common::{identify_committed, launch, open, session, this_process, this_process_identity};

async fn awaits(database: &Database, token: Option<&str>) -> bool {
    let read = database.read().await.unwrap();
    awaits_root(&read, token).await.unwrap()
}

async fn mint_committed(database: &Database, id: &str) -> LaunchToken {
    let write = database.write().await.unwrap();
    let token = mint(&write, &session(id)).await.unwrap();
    write.commit().await.unwrap();
    token
}

#[tokio::test]
async fn a_launch_without_its_root_awaits_it() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let token = mint_committed(&database, "s1").await;

    assert!(awaits(&database, Some(token.as_str())).await);
}

#[tokio::test]
async fn a_launch_awaits_its_root_no_longer_once_it_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let token = mint_committed(&database, "s1").await;
    let write = database.write().await.unwrap();
    record_root(&write, &session("s1"), &this_process_identity())
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert!(!awaits(&database, Some(token.as_str())).await);
}

#[tokio::test]
async fn a_spent_token_awaits_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let token = launch(&database, &session("s1"), &this_process_identity()).await;
    identify_committed(&database, &this_process(), Some(token.as_str()))
        .await
        .unwrap();

    assert!(!awaits(&database, Some(token.as_str())).await);
}

#[tokio::test]
async fn unknown_malformed_and_missing_tokens_await_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    mint_committed(&database, "s1").await;

    for token in [Some("0".repeat(64).as_str()), Some("s1"), None] {
        assert!(!awaits(&database, token).await, "{token:?}");
    }
}
