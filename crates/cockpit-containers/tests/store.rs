//! The engine the owner records for a project (#390): one per project, replaced by recording
//! again, which returns the one it replaced.

use cockpit_containers::{
    MIGRATIONS, RecordedEngine, record_engine, recorded_engine, recorded_engines,
};
use cockpit_turso::Database;

fn engine(project: &str, engine_id: &str, at: i64) -> RecordedEngine {
    RecordedEngine {
        project: project.to_owned(),
        user: "torlek-e3".to_owned(),
        engine_id: engine_id.to_owned(),
        recorded_at: at,
        recorded_through: Some("cli".to_owned()),
    }
}

async fn database(dir: &tempfile::TempDir) -> Database {
    Database::open(&dir.path().join("state.db"), &[MIGRATIONS])
        .await
        .unwrap()
}

#[tokio::test]
async fn a_recorded_engine_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let database = database(&dir).await;
    let write = database.write().await.unwrap();

    let replaced = record_engine(&write, &engine("shop", "engine-a", 1_000))
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(replaced, None);
    let read = database.read().await.unwrap();
    assert_eq!(
        recorded_engine(&read, "shop").await.unwrap(),
        Some(engine("shop", "engine-a", 1_000))
    );
    assert_eq!(recorded_engine(&read, "other").await.unwrap(), None);
}

#[tokio::test]
async fn recording_again_replaces_the_engine_and_returns_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let database = database(&dir).await;
    let write = database.write().await.unwrap();
    record_engine(&write, &engine("shop", "engine-a", 1_000))
        .await
        .unwrap();

    let replaced = record_engine(&write, &engine("shop", "engine-b", 2_000))
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(replaced, Some(engine("shop", "engine-a", 1_000)));
    let read = database.read().await.unwrap();
    assert_eq!(
        recorded_engine(&read, "shop").await.unwrap(),
        Some(engine("shop", "engine-b", 2_000))
    );
}

/// The daemon runs one proxy per recorded engine (#390 part 6c).
#[tokio::test]
async fn every_recorded_engine_is_listed_by_project() {
    let dir = tempfile::tempdir().unwrap();
    let database = database(&dir).await;
    let write = database.write().await.unwrap();
    record_engine(&write, &engine("shop", "engine-a", 1_000))
        .await
        .unwrap();
    record_engine(&write, &engine("blog", "engine-b", 2_000))
        .await
        .unwrap();
    write.commit().await.unwrap();

    let read = database.read().await.unwrap();
    assert_eq!(
        recorded_engines(&read).await.unwrap(),
        vec![
            engine("blog", "engine-b", 2_000),
            engine("shop", "engine-a", 1_000)
        ]
    );
}
