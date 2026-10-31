//! Issue #174: a status poll that writes nothing reads in a read transaction, so it never
//! waits for the writer; only a poll that must move the expiry needs one. Real database.

mod common;

use cockpit_approvals::{
    EXPIRES_AFTER_SECONDS, Error, GuideRequest, Refusal, RunState, ask_guide,
    guide_status_if_fresh, parse_guide, run_status_if_fresh,
};
use common::{Fixture, NOW, session};

/// The poll interval after which a waiting request's expiry is moved.
const REFRESH_AFTER: i64 = 60;

async fn asked_guide(fixture: &Fixture) -> GuideRequest {
    let owner = session("s1");
    fixture.grant(&owner).await;
    let guide = parse_guide("# Connect the app\nOne\nTwo\n").unwrap();
    let write = fixture.database.write().await.unwrap();
    let asked = ask_guide(&write, &owner, &guide, fixture.cwd(), None, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();
    asked
}

#[tokio::test]
async fn a_fresh_run_poll_reads_while_another_writer_holds_the_lock() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let _writer = fixture.database.write().await.unwrap();

    let read = fixture.database.read().await.unwrap();
    let seen = run_status_if_fresh(&read, &session("s1"), run.id, NOW + REFRESH_AFTER - 1).await;

    assert_eq!(seen.unwrap(), Some(run));
}

#[tokio::test]
async fn a_run_poll_that_must_move_the_expiry_is_left_to_run_status() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;

    let read = fixture.database.read().await.unwrap();
    let seen = run_status_if_fresh(&read, &session("s1"), run.id, NOW + REFRESH_AFTER).await;

    assert_eq!(seen.unwrap(), None);
}

#[tokio::test]
async fn an_expired_run_reads_without_a_write() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;

    let read = fixture.database.read().await.unwrap();
    let seen = run_status_if_fresh(&read, &session("s1"), run.id, NOW + EXPIRES_AFTER_SECONDS)
        .await
        .unwrap()
        .expect("an expired request has no expiry to move");

    assert_eq!(seen.state, RunState::Expired);
}

#[tokio::test]
async fn another_sessions_run_is_not_found_read_only_too() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;

    let read = fixture.database.read().await.unwrap();
    let seen = run_status_if_fresh(&read, &session("s2"), run.id, NOW).await;

    assert!(matches!(
        seen,
        Err(Error::Refused(Refusal::NotFound { kind: "run request", id })) if id == run.id.0
    ));
}

#[tokio::test]
async fn a_fresh_guide_poll_reads_while_another_writer_holds_the_lock() {
    let fixture = Fixture::new().await;
    let guide = asked_guide(&fixture).await;
    let _writer = fixture.database.write().await.unwrap();

    let read = fixture.database.read().await.unwrap();
    let seen =
        guide_status_if_fresh(&read, &session("s1"), guide.id, NOW + REFRESH_AFTER - 1).await;

    assert_eq!(seen.unwrap(), Some(guide));
}

#[tokio::test]
async fn a_guide_poll_that_must_move_the_expiry_is_left_to_guide_status() {
    let fixture = Fixture::new().await;
    let guide = asked_guide(&fixture).await;

    let read = fixture.database.read().await.unwrap();
    let seen = guide_status_if_fresh(&read, &session("s1"), guide.id, NOW + REFRESH_AFTER).await;

    assert_eq!(seen.unwrap(), None);
}
