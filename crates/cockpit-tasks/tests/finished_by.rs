//! #157 (decided in #201): an item keeps the session that was doing it when it was
//! finished, and `session_work` answers, per session, what it is doing and what it finished.
//! A session's "finished" label is computed from this, never from its agent's state.

mod common;

use cockpit_authorization::SessionId;
use cockpit_tasks::{Actor, Item, ItemId, SessionWork};
use common::{Fixture, NOW};

async fn start(fixture: &Fixture, actor: &Actor, id: ItemId) -> Item {
    let write = fixture.database.write().await.unwrap();
    let started = cockpit_tasks::start(&write, actor, id, NOW).await.unwrap();
    write.commit().await.unwrap();
    started
}

async fn finish(fixture: &Fixture, actor: &Actor, id: ItemId) -> Item {
    let write = fixture.database.write().await.unwrap();
    let finished = cockpit_tasks::finish(&write, actor, id, Some("done it"), NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();
    finished
}

async fn work_of(fixture: &Fixture, session: &str) -> Option<SessionWork> {
    let read = fixture.database.read().await.unwrap();
    cockpit_tasks::session_work(&read)
        .await
        .unwrap()
        .remove(&SessionId::new(session).unwrap())
}

#[tokio::test]
async fn a_session_that_finishes_its_item_is_kept_as_its_finisher() {
    let fixture = Fixture::new().await;
    let session = fixture.session("s1", "app").await;
    let item = fixture.owner_adds("app", "Fix the login").await;
    start(&fixture, &session, item.id).await;

    let finished = finish(&fixture, &session, item.id).await;

    assert_eq!(finished.finished_by, Some(SessionId::new("s1").unwrap()));
    assert_eq!(
        work_of(&fixture, "s1").await,
        Some(SessionWork {
            doing: 0,
            finished: 1
        })
    );
}

#[tokio::test]
async fn the_owner_finishing_a_session_s_item_keeps_that_session_as_its_finisher() {
    let fixture = Fixture::new().await;
    let session = fixture.session("s1", "app").await;
    let item = fixture.owner_adds("app", "Fix the login").await;
    start(&fixture, &session, item.id).await;

    let finished = finish(&fixture, &Actor::Owner, item.id).await;

    assert_eq!(finished.finished_by, Some(SessionId::new("s1").unwrap()));
}

#[tokio::test]
async fn an_item_the_owner_finished_from_todo_has_no_finisher() {
    let fixture = Fixture::new().await;
    let item = fixture.owner_adds("app", "Fix the login").await;

    let finished = finish(&fixture, &Actor::Owner, item.id).await;

    assert_eq!(finished.finished_by, None);
}

#[tokio::test]
async fn reopening_an_item_drops_its_finisher_and_the_session_s_finished_count() {
    let fixture = Fixture::new().await;
    let session = fixture.session("s1", "app").await;
    let item = fixture.owner_adds("app", "Fix the login").await;
    start(&fixture, &session, item.id).await;
    finish(&fixture, &session, item.id).await;

    let write = fixture.database.write().await.unwrap();
    let reopened = cockpit_tasks::reopen(&write, &Actor::Owner, item.id, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(reopened.finished_by, None);
    assert_eq!(work_of(&fixture, "s1").await, None);
}

#[tokio::test]
async fn a_session_doing_one_item_after_finishing_another_counts_both() {
    let fixture = Fixture::new().await;
    let session = fixture.session("s1", "app").await;
    let first = fixture.owner_adds("app", "Fix the login").await;
    let second = fixture.owner_adds("app", "Fix the logout").await;
    start(&fixture, &session, first.id).await;
    finish(&fixture, &session, first.id).await;

    start(&fixture, &session, second.id).await;

    assert_eq!(
        work_of(&fixture, "s1").await,
        Some(SessionWork {
            doing: 1,
            finished: 1
        })
    );
}
