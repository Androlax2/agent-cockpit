//! Serves `subscribe` from `cockpit-messaging`'s change feed and converts its batches to the
//! wire at this boundary. The feed's rules live in that crate; this file maps and streams.

use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use cockpit_messaging::{EntityChange, Feed, FeedBatch, FeedCursor, Refusal};
use cockpit_protocol::{
    Change, ChangeBatch, ChangeEvent, CommitRevision, Cursor, DatabaseEpoch, EntityKind, EntityRef,
    EntityRevision, ErrorCode, FeedPosition, Outcome, Reply,
};
use cockpit_turso::Database;
use tokio::sync::{mpsc, watch};

use crate::routes::{Route, refusal};

/// Batches built ahead of the connection; the feed waits when the client reads slower.
const STREAM_BATCHES: usize = 8;

pub(crate) fn route(from: FeedPosition, database: Arc<Database>) -> Route {
    Route::Stream {
        job: Box::pin(async move {
            match start(from, database).await {
                Ok((cursor, batches)) => (Outcome::Ok(Reply::Subscribed { cursor }), Some(batches)),
                Err(outcome) => (outcome, None),
            }
        }),
    }
}

/// Opens the feed and starts streaming it. The answer's cursor is where the subscriber stands
/// once it applied the first batch, if one comes at once.
async fn start(
    from: FeedPosition,
    database: Arc<Database>,
) -> Result<(Cursor, mpsc::Receiver<ChangeBatch>), Outcome> {
    // Watched before the feed is read, so a commit in between wakes the stream.
    let commits = database.watch_commits();
    let (feed, first) = Feed::open(&database, feed_position(from))
        .await
        .map_err(|error| feed_error(&error))?;
    let cursor = wire_cursor(feed.cursor().clone());
    let (sender, batches) = mpsc::channel(STREAM_BATCHES);
    if let Some(first) = first {
        sender
            .try_send(change_batch(first))
            .expect("a new channel has room for the first batch");
    }
    tokio::spawn(stream(feed, database, commits, sender));
    Ok((cursor, batches))
}

/// Sends a batch for each commit that changed a published entity, until the connection
/// stops reading the feed or the feed cannot continue.
async fn stream(
    mut feed: Feed,
    database: Arc<Database>,
    mut commits: watch::Receiver<()>,
    sender: mpsc::Sender<ChangeBatch>,
) {
    loop {
        tokio::select! {
            changed = commits.changed() => if changed.is_err() { return },
            () = sender.closed() => return,
        }
        match feed.next(&database).await {
            Ok(None) => {}
            Ok(Some(batch)) => {
                if sender.send(change_batch(batch)).await.is_err() {
                    return;
                }
            }
            Err(error) => {
                tracing::info!(%error, "a change feed cannot continue; its client subscribes again");
                return;
            }
        }
    }
}

fn feed_error(error: &cockpit_messaging::Error) -> Outcome {
    match error {
        cockpit_messaging::Error::Refused(refusal @ Refusal::ResnapshotRequired(_)) => {
            self::refusal(ErrorCode::ResnapshotRequired, &refusal.to_string())
        }
        cockpit_messaging::Error::Refused(refusal) => {
            self::refusal(ErrorCode::InvalidArgument, &refusal.to_string())
        }
        error => {
            tracing::error!(%error, "the change feed failed");
            self::refusal(ErrorCode::Internal, "the change feed failed")
        }
    }
}

fn feed_position(position: FeedPosition) -> cockpit_messaging::FeedPosition {
    match position {
        FeedPosition::Snapshot => cockpit_messaging::FeedPosition::Snapshot,
        FeedPosition::Cursor(cursor) => cockpit_messaging::FeedPosition::After(FeedCursor {
            epoch: cursor.epoch.0,
            revision: cursor.revision.0,
        }),
    }
}

fn wire_cursor(cursor: FeedCursor) -> Cursor {
    Cursor {
        epoch: DatabaseEpoch(cursor.epoch),
        revision: CommitRevision(cursor.revision),
    }
}

fn change_batch(batch: FeedBatch) -> ChangeBatch {
    ChangeBatch {
        follows: match batch.follows {
            cockpit_messaging::FeedPosition::Snapshot => FeedPosition::Snapshot,
            cockpit_messaging::FeedPosition::After(cursor) => {
                FeedPosition::Cursor(wire_cursor(cursor))
            }
        },
        cursor: wire_cursor(batch.cursor),
        changes: batch.changes.into_iter().map(change_event).collect(),
    }
}

fn change_event(change: EntityChange) -> ChangeEvent {
    ChangeEvent {
        entity: EntityRef {
            kind: entity_kind(change.kind),
            id: change.id,
        },
        entity_revision: EntityRevision(change.revision),
        change: match change.state {
            Some(state) => Change::Upsert { state },
            None => Change::Tombstone,
        },
    }
}

/// A known kind's variant, or the producer's own kind kept as sent.
fn entity_kind(kind: String) -> EntityKind {
    serde_json::from_value(serde_json::Value::String(kind))
        .expect("every string reads as an entity kind")
}

/// Captured change rows kept for clients that resume; an older cursor gets a snapshot.
const RETAINED_CHANGES: NonZeroU64 = NonZeroU64::new(100_000).expect("not zero");
const RETENTION_PERIOD: Duration = Duration::from_secs(60 * 60);

/// Discards the change history past [`RETAINED_CHANGES`] at start and every hour after.
pub(crate) async fn discard_history_forever(database: Arc<Database>) {
    let mut period = tokio::time::interval(RETENTION_PERIOD);
    loop {
        period.tick().await;
        match discard_history(&database).await {
            Ok(Some(retained_after)) => {
                tracing::info!(retained_after, "discarded change history");
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "discarding change history failed; retrying next period");
            }
        }
    }
}

async fn discard_history(database: &Database) -> Result<Option<u64>, cockpit_messaging::Error> {
    let write = database.write().await?;
    let retained_after = cockpit_messaging::discard_history(&write, RETAINED_CHANGES).await?;
    write.commit().await?;
    Ok(retained_after)
}
