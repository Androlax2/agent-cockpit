//! The change feed on a real state database: snapshot, resume, unchanged publishes,
//! tombstones, retention and epochs.
mod common;

use std::num::NonZeroU64;

use cockpit_messaging::{
    EntityChange, Error, Feed, FeedBatch, FeedCursor, FeedPosition, Published, Refusal,
    ResnapshotReason,
};
use cockpit_turso::Database;
use common::Fixture;
use serde_json::{Map, Value, json};

fn state(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(state) => state,
        other => panic!("not an object: {other}"),
    }
}

async fn publish(database: &Database, id: &str, value: Value) -> Published {
    let write = database.write().await.unwrap();
    let published = cockpit_messaging::publish(&write, "session", id, &state(value))
        .await
        .unwrap();
    write.commit().await.unwrap();
    published
}

async fn retract(database: &Database, id: &str) -> Option<u64> {
    let write = database.write().await.unwrap();
    let revision = cockpit_messaging::retract(&write, "session", id)
        .await
        .unwrap();
    write.commit().await.unwrap();
    revision
}

/// A commit that changes no published entity.
async fn unrelated_commit(database: &Database) {
    let write = database.write().await.unwrap();
    write
        .execute("CREATE TABLE IF NOT EXISTS other (n INTEGER)", ())
        .await
        .unwrap();
    write
        .execute("INSERT INTO other VALUES (1)", ())
        .await
        .unwrap();
    write.commit().await.unwrap();
}

fn upsert(id: &str, revision: u64, value: Value) -> EntityChange {
    EntityChange {
        kind: "session".to_owned(),
        id: id.to_owned(),
        revision,
        state: Some(state(value)),
    }
}

fn tombstone(id: &str, revision: u64) -> EntityChange {
    EntityChange {
        kind: "session".to_owned(),
        id: id.to_owned(),
        revision,
        state: None,
    }
}

async fn snapshot(database: &Database) -> (Feed, FeedBatch) {
    let (feed, batch) = Feed::open(database, FeedPosition::Snapshot).await.unwrap();
    (feed, batch.expect("a snapshot is always a batch"))
}

async fn newest_revision(database: &Database) -> u64 {
    let revision = database.read().await.unwrap().revision().await.unwrap();
    u64::try_from(revision).unwrap()
}

#[tokio::test]
async fn a_snapshot_holds_every_published_entity_at_the_newest_revision() {
    let fixture = Fixture::new().await;
    publish(&fixture.database, "b", json!({"state": "idle"})).await;
    publish(&fixture.database, "a", json!({"state": "working"})).await;
    publish(&fixture.database, "gone", json!({"state": "idle"})).await;
    retract(&fixture.database, "gone").await;

    let (_, batch) = snapshot(&fixture.database).await;

    assert_eq!(batch.follows, FeedPosition::Snapshot);
    assert_eq!(
        batch.cursor,
        FeedCursor {
            epoch: fixture.database.epoch().to_owned(),
            revision: newest_revision(&fixture.database).await,
        }
    );
    assert_eq!(
        batch.changes,
        [
            upsert("a", 1, json!({"state": "working"})),
            upsert("b", 1, json!({"state": "idle"}))
        ]
    );
}

#[tokio::test]
async fn the_next_batch_follows_the_snapshot_with_each_changed_entity_once() {
    let fixture = Fixture::new().await;
    publish(&fixture.database, "a", json!({"state": "idle"})).await;
    let (mut feed, first) = snapshot(&fixture.database).await;
    publish(&fixture.database, "a", json!({"state": "working"})).await;
    publish(&fixture.database, "b", json!({"state": "idle"})).await;
    publish(&fixture.database, "a", json!({"state": "waiting"})).await;

    let batch = feed.next(&fixture.database).await.unwrap().unwrap();

    assert_eq!(batch.follows, FeedPosition::After(first.cursor));
    assert_eq!(
        batch.cursor.revision,
        newest_revision(&fixture.database).await
    );
    assert_eq!(
        batch.changes,
        [
            upsert("a", 3, json!({"state": "waiting"})),
            upsert("b", 1, json!({"state": "idle"}))
        ]
    );
}

#[tokio::test]
async fn publishing_the_same_state_again_writes_nothing_and_sends_no_batch() {
    let fixture = Fixture::new().await;
    publish(
        &fixture.database,
        "a",
        json!({"state": "idle", "tokens": 3}),
    )
    .await;
    let (mut feed, _) = snapshot(&fixture.database).await;
    let revision = newest_revision(&fixture.database).await;

    let again = publish(
        &fixture.database,
        "a",
        json!({"tokens": 3, "state": "idle"}),
    )
    .await;

    assert_eq!(again, Published::Unchanged { revision: 1 });
    assert_eq!(newest_revision(&fixture.database).await, revision);
    assert_eq!(feed.next(&fixture.database).await.unwrap(), None);
}

#[tokio::test]
async fn commits_that_change_no_entity_produce_no_batch_but_the_next_one_follows_on() {
    let fixture = Fixture::new().await;
    let (mut feed, first) = snapshot(&fixture.database).await;
    unrelated_commit(&fixture.database).await;
    assert_eq!(feed.next(&fixture.database).await.unwrap(), None);

    publish(&fixture.database, "a", json!({"state": "idle"})).await;
    let batch = feed.next(&fixture.database).await.unwrap().unwrap();

    assert_eq!(batch.follows, FeedPosition::After(first.cursor));
    assert_eq!(batch.changes, [upsert("a", 1, json!({"state": "idle"}))]);
}

#[tokio::test]
async fn a_retracted_entity_reaches_the_feed_as_a_tombstone_and_its_revision_keeps_growing() {
    let fixture = Fixture::new().await;
    publish(&fixture.database, "a", json!({"state": "idle"})).await;
    let (mut feed, _) = snapshot(&fixture.database).await;

    let tombstone_revision = retract(&fixture.database, "a").await;
    let deleted = feed.next(&fixture.database).await.unwrap().unwrap();
    let again = publish(&fixture.database, "a", json!({"state": "idle"})).await;

    assert_eq!(tombstone_revision, Some(2));
    assert_eq!(deleted.changes, [tombstone("a", 2)]);
    assert_eq!(again, Published::Changed { revision: 3 });
    assert_eq!(retract(&fixture.database, "never-published").await, None);
}

#[tokio::test]
async fn resuming_after_a_cursor_replays_only_what_changed_since() {
    let fixture = Fixture::new().await;
    publish(&fixture.database, "a", json!({"state": "idle"})).await;
    let (_, first) = snapshot(&fixture.database).await;
    publish(&fixture.database, "b", json!({"state": "idle"})).await;
    retract(&fixture.database, "a").await;

    let (feed, replay) = Feed::open(&fixture.database, FeedPosition::After(first.cursor.clone()))
        .await
        .unwrap();
    let replay = replay.unwrap();

    assert_eq!(replay.follows, FeedPosition::After(first.cursor));
    assert_eq!(
        replay.changes,
        [upsert("b", 1, json!({"state": "idle"})), tombstone("a", 2)]
    );
    assert_eq!(feed.cursor(), &replay.cursor);
}

#[tokio::test]
async fn resuming_an_up_to_date_cursor_starts_with_no_batch() {
    let fixture = Fixture::new().await;
    publish(&fixture.database, "a", json!({"state": "idle"})).await;
    let (_, first) = snapshot(&fixture.database).await;

    let (feed, replay) = Feed::open(&fixture.database, FeedPosition::After(first.cursor.clone()))
        .await
        .unwrap();

    assert_eq!(replay, None);
    assert_eq!(feed.cursor(), &first.cursor);
}

fn resnapshot_reason(result: Result<(Feed, Option<FeedBatch>), Error>) -> ResnapshotReason {
    match result {
        Err(Error::Refused(Refusal::ResnapshotRequired(reason))) => reason,
        other => panic!("not refused resnapshot-required: {other:?}"),
    }
}

#[tokio::test]
async fn a_cursor_of_another_epoch_needs_a_snapshot() {
    let fixture = Fixture::new().await;
    let (_, first) = snapshot(&fixture.database).await;
    let other_epoch = FeedCursor {
        epoch: "another-database".to_owned(),
        ..first.cursor
    };

    let resumed = Feed::open(&fixture.database, FeedPosition::After(other_epoch)).await;

    assert_eq!(resnapshot_reason(resumed), ResnapshotReason::OtherEpoch);
}

#[tokio::test]
async fn a_cursor_past_the_newest_revision_needs_a_snapshot() {
    let fixture = Fixture::new().await;
    let (_, first) = snapshot(&fixture.database).await;
    let ahead = FeedCursor {
        revision: first.cursor.revision + 100,
        ..first.cursor.clone()
    };

    let resumed = Feed::open(&fixture.database, FeedPosition::After(ahead)).await;

    assert_eq!(
        resnapshot_reason(resumed),
        ResnapshotReason::Ahead {
            newest: first.cursor.revision
        }
    );
}

async fn discard_all_but_newest(database: &Database) -> Option<u64> {
    let write = database.write().await.unwrap();
    let retained_after = cockpit_messaging::discard_history(&write, NonZeroU64::new(1).unwrap())
        .await
        .unwrap();
    write.commit().await.unwrap();
    retained_after
}

#[tokio::test]
async fn a_cursor_older_than_the_retained_history_needs_a_snapshot() {
    let fixture = Fixture::new().await;
    publish(&fixture.database, "a", json!({"state": "idle"})).await;
    let (_, old) = snapshot(&fixture.database).await;
    publish(&fixture.database, "a", json!({"state": "working"})).await;
    publish(&fixture.database, "a", json!({"state": "waiting"})).await;

    let retained_after = discard_all_but_newest(&fixture.database).await.unwrap();
    let resumed = Feed::open(&fixture.database, FeedPosition::After(old.cursor)).await;
    let (_, fresh) = snapshot(&fixture.database).await;

    assert_eq!(
        resnapshot_reason(resumed),
        ResnapshotReason::Discarded { retained_after }
    );
    assert_eq!(fresh.changes, [upsert("a", 3, json!({"state": "waiting"}))]);
}

#[tokio::test]
async fn a_cursor_at_the_retained_history_resumes() {
    let fixture = Fixture::new().await;
    publish(&fixture.database, "a", json!({"state": "idle"})).await;
    publish(&fixture.database, "b", json!({"state": "idle"})).await;
    let retained_after = discard_all_but_newest(&fixture.database).await.unwrap();
    publish(&fixture.database, "c", json!({"state": "idle"})).await;
    let cursor = FeedCursor {
        epoch: fixture.database.epoch().to_owned(),
        revision: retained_after,
    };

    let (_, replay) = Feed::open(&fixture.database, FeedPosition::After(cursor))
        .await
        .unwrap();

    let ids: Vec<String> = replay
        .unwrap()
        .changes
        .into_iter()
        .map(|change| change.id)
        .collect();
    assert_eq!(ids, ["b", "c"]);
}

#[tokio::test]
async fn a_streaming_feed_needs_a_snapshot_once_its_history_is_discarded() {
    let fixture = Fixture::new().await;
    let (mut feed, _) = snapshot(&fixture.database).await;
    publish(&fixture.database, "a", json!({"state": "idle"})).await;
    publish(&fixture.database, "a", json!({"state": "working"})).await;
    discard_all_but_newest(&fixture.database).await.unwrap();

    let next = feed.next(&fixture.database).await;

    assert!(
        matches!(
            next,
            Err(Error::Refused(Refusal::ResnapshotRequired(
                ResnapshotReason::Discarded { .. }
            )))
        ),
        "{next:?}"
    );
}

#[tokio::test]
async fn an_invalid_kind_or_id_is_refused() {
    let fixture = Fixture::new().await;
    let write = fixture.database.write().await.unwrap();
    let empty = Map::new();

    let bad_kind = cockpit_messaging::publish(&write, "Session", "a", &empty).await;
    let bad_id = cockpit_messaging::publish(&write, "session", "", &empty).await;

    for (field, result) in [("kind", bad_kind), ("id", bad_id)] {
        assert!(
            matches!(
                &result,
                Err(Error::Refused(Refusal::InvalidArgument { field: refused, .. })) if *refused == field
            ),
            "{result:?}"
        );
    }
}
