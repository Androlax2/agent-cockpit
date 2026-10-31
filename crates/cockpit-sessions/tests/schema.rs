//! The sessions schema on a real state database: a stored state is always one the lifecycle
//! can read back. The lifecycle itself is driven through the daemon's socket in
//! `cockpit-daemon/tests/sessions.rs`.

use cockpit_sessions::{MIGRATIONS, SessionId};
use cockpit_turso::Database;

const INSERT: &str = "INSERT INTO sessions (id, provider, checkout_id, holds_checkout, working_dir,
        state, stopping_to, revision, created_at, changed_at)
    VALUES (?1, 'fake', 1, 0, '/p/app', ?2, ?3, 1, 0, 0)";

async fn database(dir: &tempfile::TempDir) -> Database {
    Database::open(&dir.path().join("state.db"), &[MIGRATIONS])
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
