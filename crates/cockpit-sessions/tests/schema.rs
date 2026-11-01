//! The sessions schema on a real state database: a stored state is always one the lifecycle
//! can read back. The lifecycle itself is driven through the daemon's socket in
//! `cockpit-daemon/tests/sessions.rs`.

use cockpit_sessions::{MIGRATIONS, SessionId};
use cockpit_turso::Database;

const INSERT: &str = "INSERT INTO sessions (id, provider, checkout_id, holds_checkout, working_dir,
        state, stopping_to, revision, created_at, changed_at)
    VALUES (?1, 'fake', 1, 0, '/p/app', ?2, ?3, 1, 0, 0)";

async fn database(dir: &tempfile::TempDir) -> Database {
    // Version 2 moves the agent channel's reports, so the channel's tables come first.
    Database::open(
        &dir.path().join("state.db"),
        &[cockpit_agent_channel::MIGRATIONS, MIGRATIONS],
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn a_stopping_session_must_name_where_it_stops_to() {
    let dir = tempfile::tempdir().unwrap();
    let database = database(&dir).await;
    let write = database.write().await.unwrap();

    let without_target = write
        .execute(INSERT, ("s000000000001", "stopping", None::<&str>))
        .await;
    let with_target = write
        .execute(INSERT, ("s000000000002", "stopping", Some("asleep")))
        .await;

    assert!(without_target.is_err());
    assert!(with_target.is_ok());
}

#[tokio::test]
async fn only_a_stopping_session_carries_a_stop_target() {
    let dir = tempfile::tempdir().unwrap();
    let database = database(&dir).await;
    let write = database.write().await.unwrap();

    let refused = write
        .execute(INSERT, ("s000000000001", "running", Some("ended")))
        .await;

    assert!(refused.is_err());
}

#[tokio::test]
async fn a_state_the_lifecycle_does_not_know_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let database = database(&dir).await;
    let write = database.write().await.unwrap();

    let refused = write
        .execute(INSERT, ("s000000000001", "hibernating", None::<&str>))
        .await;

    assert!(refused.is_err());
}

#[test]
fn a_session_id_is_s_and_twelve_lower_case_hex_digits() {
    assert!(SessionId::parse("s0123456789ab").is_ok());
    for refused in [
        "",
        "s0123456789a",
        "s0123456789AB",
        "x0123456789ab",
        "s0123456789ag",
    ] {
        assert!(SessionId::parse(refused).is_err(), "{refused:?}");
    }
}

#[tokio::test]
async fn delivery_context_survives_reopen_and_refuses_incomplete_mounts() {
    let dir = tempfile::tempdir().unwrap();
    {
        let database = database(&dir).await;
        let write = database.write().await.unwrap();
        write.execute("INSERT INTO session_delivery (execution_id, provider_program, reach, reason) VALUES (1, X'2F61', 'mounted', NULL)", ()).await.unwrap();
        assert!(write.execute("INSERT INTO session_delivery_mounts (execution_id, host, sandbox) VALUES (1, '', '/tmp/codex-daemon-1')", ()).await.is_err());
        write.execute("INSERT INTO session_delivery_mounts (execution_id, host, sandbox) VALUES (1, '/private/mount', '/tmp/codex-daemon-1')", ()).await.unwrap();
        assert!(write.execute("INSERT INTO session_delivery (execution_id, provider_program, reach, reason) VALUES (2, X'2F61', 'unreachable', NULL)", ()).await.is_err());
        write.commit().await.unwrap();
    }
    let database = database(&dir).await;
    let read = database.read().await.unwrap();
    let mut rows = read
        .query(
            "SELECT host, sandbox FROM session_delivery_mounts WHERE execution_id = 1",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), "/private/mount");
    assert_eq!(row.get::<String>(1).unwrap(), "/tmp/codex-daemon-1");
}

#[tokio::test]
async fn delivery_launch_keeps_opaque_words_and_order_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    {
        let database = database(&dir).await;
        let write = database.write().await.unwrap();
        assert!(
            write
                .execute(
                    "INSERT INTO session_delivery (execution_id, reach) VALUES (1, 'direct')",
                    ()
                )
                .await
                .is_err()
        );
        write.execute("INSERT INTO session_delivery (execution_id, provider_program, reach) VALUES (1, X'2FFF', 'direct')", ()).await.unwrap();
        write.execute("INSERT INTO session_delivery_arguments (execution_id, position, value) VALUES (1, 1, X''), (1, 0, X'FF')", ()).await.unwrap();
        write.execute("INSERT INTO session_delivery_environment (execution_id, position, name, value) VALUES (1, 0, X'41', X'FE')", ()).await.unwrap();
        assert!(write.execute("INSERT INTO session_delivery_arguments (execution_id, position, value) VALUES (1, 0, X'01')", ()).await.is_err());
        write.commit().await.unwrap();
    }
    let database = database(&dir).await;
    let read = database.read().await.unwrap();
    let mut rows = read
        .query(
            "SELECT provider_program FROM session_delivery WHERE execution_id = 1",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<Vec<u8>>(0)
            .unwrap(),
        vec![b'/', 0xff]
    );
    let mut rows = read
        .query(
            "SELECT value FROM session_delivery_arguments WHERE execution_id = 1 ORDER BY position",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<Vec<u8>>(0)
            .unwrap(),
        vec![0xff]
    );
    assert!(
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<Vec<u8>>(0)
            .unwrap()
            .is_empty()
    );
    let mut rows = read
        .query(
            "SELECT name, value FROM session_delivery_environment WHERE execution_id = 1",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<Vec<u8>>(0).unwrap(), vec![b'A']);
    assert_eq!(row.get::<Vec<u8>>(1).unwrap(), vec![0xfe]);
}
