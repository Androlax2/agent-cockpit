//! #406: an ended or failed session untouched past the retention loses its launches'
//! delivery context and start-file snapshots; its identity, launches, transitions and exits
//! stay, and a live, sleeping or recent session keeps everything.

use cockpit_sessions::{
    LaunchPayload, MIGRATIONS, RetirablePayload, retirable_launch_payloads, retire_launch_payload,
};
use cockpit_turso::Database;

const CUTOFF: i64 = 1_000;
const OLD: i64 = 500;
const RECENT: i64 = 2_000;

async fn database(dir: &tempfile::TempDir) -> Database {
    Database::open(
        &dir.path().join("state.db"),
        &[cockpit_agent_channel::MIGRATIONS, MIGRATIONS],
    )
    .await
    .unwrap()
}

/// A session in `state` last changed at `changed_at`, with one launch (execution `launch`)
/// that recorded a delivery context, two arguments, one variable, a mount and a start file.
async fn session(database: &Database, id: &str, state: &str, changed_at: i64, launch: i64) {
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO sessions (id, provider, checkout_id, holds_checkout, working_dir, state,
                revision, created_at, changed_at)
             VALUES (?1, 'fake', 1, 0, '/p/app', ?2, 3, 0, ?3)",
            (id, state, changed_at),
        )
        .await
        .unwrap();
    write
        .execute(
            "INSERT INTO session_launches (id, session_id, kind, execution_id, launched_at)
             VALUES (?1, ?2, 'start', ?1, 0)",
            (launch, id),
        )
        .await
        .unwrap();
    write
        .execute(
            "INSERT INTO session_transitions (session_id, from_state, to_state, at)
             VALUES (?1, 'running', ?2, ?3)",
            (id, state, changed_at),
        )
        .await
        .unwrap();
    write
        .execute(
            "INSERT INTO session_exits (launch_id, ended_by, code, output)
             VALUES (?1, 'code', 0, 'bye')",
            [launch],
        )
        .await
        .unwrap();
    for sql in [
        "INSERT INTO session_delivery (execution_id, provider_program, reach)
            VALUES (?1, X'2F62696E', 'direct')",
        "INSERT INTO session_delivery_arguments (execution_id, position, value)
            VALUES (?1, 0, X'2D61'), (?1, 1, X'2D62')",
        "INSERT INTO session_delivery_environment (execution_id, position, name, value)
            VALUES (?1, 0, X'4B4559', X'76616C7565')",
        "INSERT INTO session_delivery_mounts (execution_id, host, sandbox)
            VALUES (?1, '/h', '/s')",
        "INSERT INTO session_start_files (launch_id, label, path, file_state)
            VALUES (?1, 'agents', '/p/AGENTS.md', 'missing')",
    ] {
        write.execute(sql, [launch]).await.unwrap();
    }
    write.commit().await.unwrap();
}

/// 1 delivery row (program 4 bytes), 2 arguments (4), 1 variable (3 + 5), 1 mount (2 + 2),
/// 1 start file (6 + 12).
const ONE_LAUNCH: LaunchPayload = LaunchPayload {
    rows: 6,
    bytes: 4 + 4 + 8 + 4 + 18,
};

async fn count(database: &Database, sql: &str) -> i64 {
    let read = database.read().await.unwrap();
    let mut rows = read.query(sql, ()).await.unwrap();
    rows.next().await.unwrap().unwrap().get(0).unwrap()
}

async fn listed(database: &Database) -> Vec<RetirablePayload> {
    let read = database.read().await.unwrap();
    retirable_launch_payloads(&read, CUTOFF, "", 100)
        .await
        .unwrap()
}

#[tokio::test]
async fn only_old_ended_and_failed_sessions_are_listed_with_their_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let database = database(&dir).await;
    session(&database, "s000000000001", "ended", OLD, 1).await;
    session(&database, "s000000000002", "failed", OLD, 2).await;
    session(&database, "s000000000003", "ended", RECENT, 3).await;
    session(&database, "s000000000004", "asleep", OLD, 4).await;
    session(&database, "s000000000005", "running", OLD, 5).await;

    let listed = listed(&database).await;

    assert_eq!(
        listed,
        [
            RetirablePayload {
                session: "s000000000001".to_owned(),
                payload: ONE_LAUNCH,
            },
            RetirablePayload {
                session: "s000000000002".to_owned(),
                payload: ONE_LAUNCH,
            },
        ]
    );
}

#[tokio::test]
async fn listing_pages_by_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let database = database(&dir).await;
    for (id, launch) in [
        ("s000000000001", 1),
        ("s000000000002", 2),
        ("s000000000003", 3),
    ] {
        session(&database, id, "ended", OLD, launch).await;
    }
    let read = database.read().await.unwrap();

    let first = retirable_launch_payloads(&read, CUTOFF, "", 2)
        .await
        .unwrap();
    let rest = retirable_launch_payloads(&read, CUTOFF, &first[1].session, 2)
        .await
        .unwrap();

    assert_eq!(first.len(), 2);
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].session, "s000000000003");
}

#[tokio::test]
async fn retiring_removes_the_payloads_and_keeps_identity_history_and_exit() {
    let dir = tempfile::tempdir().unwrap();
    let database = database(&dir).await;
    session(&database, "s000000000001", "ended", OLD, 1).await;
    session(&database, "s000000000002", "running", OLD, 2).await;

    let write = database.write().await.unwrap();
    let retired = retire_launch_payload(&write, "s000000000001", CUTOFF)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(retired, Some(ONE_LAUNCH));
    for (table, kept) in [
        ("session_delivery", 1),
        ("session_delivery_arguments", 2),
        ("session_delivery_environment", 1),
        ("session_delivery_mounts", 1),
        ("session_start_files", 1),
        ("sessions", 2),
        ("session_launches", 2),
        ("session_transitions", 2),
        ("session_exits", 2),
    ] {
        assert_eq!(
            count(&database, &format!("SELECT count(*) FROM {table}")).await,
            kept,
            "{table}"
        );
    }
    assert!(listed(&database).await.is_empty());
}

#[tokio::test]
async fn a_session_resumed_since_the_listing_keeps_its_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let database = database(&dir).await;
    session(&database, "s000000000001", "ended", OLD, 1).await;
    assert_eq!(listed(&database).await.len(), 1);
    let write = database.write().await.unwrap();
    write
        .execute(
            "UPDATE sessions SET state = 'launching', changed_at = ?1 WHERE id = 's000000000001'",
            [RECENT],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();

    let write = database.write().await.unwrap();
    let retired = retire_launch_payload(&write, "s000000000001", CUTOFF)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(retired, None);
    assert_eq!(
        count(&database, "SELECT count(*) FROM session_delivery").await,
        1
    );
}

#[tokio::test]
async fn retiring_twice_retires_nothing_the_second_time() {
    let dir = tempfile::tempdir().unwrap();
    let database = database(&dir).await;
    session(&database, "s000000000001", "failed", OLD, 1).await;

    for expected in [Some(ONE_LAUNCH), None] {
        let write = database.write().await.unwrap();
        let retired = retire_launch_payload(&write, "s000000000001", CUTOFF)
            .await
            .unwrap();
        write.commit().await.unwrap();
        assert_eq!(retired, expected);
    }
}
