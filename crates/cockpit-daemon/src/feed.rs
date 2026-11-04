//! Serves `subscribe` from `cockpit-messaging`'s change feed and converts its batches to the
//! wire at this boundary. The feed's rules live in that crate; this file maps and streams.

use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use cockpit_messaging::{EntityChange, Feed, FeedBatch, FeedCursor, Refusal};
use cockpit_protocol::{
    Change, ChangeBatch, ChangeEvent, CommitRevision, Cursor, DatabaseEpoch, EntityKind, EntityRef,
    EntityRevision, ErrorCode, FeedPosition, Outcome, Reply, ServerFrame,
};
use cockpit_turso::Database;
use tokio::sync::{mpsc, watch};

use crate::routes::{BufferedBatch, Route, refusal};

/// Batches built ahead of the connection; the feed waits when the client reads slower.
const STREAM_BATCHES: usize = 8;

/// The bytes of batches one subscriber has waiting to be written, against what it may have
/// (#406): beside the count above, what a subscriber that stopped reading makes the daemon
/// hold. The feed builds no batch while the buffer holds its bytes or more, so it holds at
/// most one batch beyond them, and a batch larger than the whole buffer still passes.
struct Buffer {
    used_bytes: Arc<watch::Sender<usize>>,
    bytes: usize,
}

impl Buffer {
    fn new(bytes: u32) -> Buffer {
        Buffer {
            used_bytes: Arc::new(watch::Sender::new(0)),
            // An empty buffer always has room, so a feed of no bytes still streams.
            bytes: (bytes as usize).max(1),
        }
    }

    /// Waits until the buffer holds less than its bytes.
    async fn has_room(&self) {
        let mut used_bytes = self.used_bytes.subscribe();
        used_bytes
            .wait_for(|used_bytes| *used_bytes < self.bytes)
            .await
            .expect("the buffer keeps its own sender");
    }

    /// Counts `line` in the buffer until its connection wrote it.
    fn hold(&self, line: Vec<u8>) -> BufferedBatch {
        self.used_bytes
            .send_modify(|used_bytes| *used_bytes += line.len());
        BufferedBatch {
            room: BufferRoom {
                used_bytes: Arc::clone(&self.used_bytes),
                bytes: line.len(),
            },
            line,
        }
    }
}

/// A batch's bytes of its subscriber's buffer, given back when dropped.
pub(crate) struct BufferRoom {
    used_bytes: Arc<watch::Sender<usize>>,
    bytes: usize,
}

impl Drop for BufferRoom {
    fn drop(&mut self) {
        self.used_bytes
            .send_modify(|used_bytes| *used_bytes -= self.bytes);
    }
}

/// `batch` as the line its connection writes, or `None` when it exceeds the frame limit.
/// Kept as the line, not as the batch: the line is what the buffer counts, and several times
/// smaller than the parsed states.
fn batch_line(batch: FeedBatch) -> Option<Vec<u8>> {
    let mut line = crate::server::encode(&ServerFrame::Changes(change_batch(batch)))?;
    line.shrink_to_fit();
    Some(line)
}

pub(crate) fn route(from: FeedPosition, database: Arc<Database>, buffer_bytes: u32) -> Route {
    Route::Stream {
        job: Box::pin(async move {
            match start(from, database, buffer_bytes).await {
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
    buffer_bytes: u32,
) -> Result<(Cursor, mpsc::Receiver<BufferedBatch>), Outcome> {
    // Watched before the feed is read, so a commit in between wakes the stream.
    let commits = database.watch_commits();
    let (feed, first) = Feed::open(&database, feed_position(from))
        .await
        .map_err(|error| feed_error(&error))?;
    let cursor = wire_cursor(feed.cursor().clone());
    let buffer = Buffer::new(buffer_bytes);
    let (sender, batches) = mpsc::channel(STREAM_BATCHES);
    if let Some(first) = first {
        let Some(line) = batch_line(first) else {
            // The sender is dropped: the feed ends before its first batch and its
            // connection closes, as for any batch too large to send.
            tracing::error!("a change feed's first batch exceeds the frame limit; closing it");
            return Ok((cursor, batches));
        };
        if sender.try_send(buffer.hold(line)).is_err() {
            unreachable!("a new channel has room for the first batch");
        }
    }
    tokio::spawn(stream(feed, database, commits, sender, buffer));
    Ok((cursor, batches))
}

/// Sends a batch for each commit that changed a published entity, until the connection
/// stops reading the feed or the feed cannot continue.
async fn stream(
    mut feed: Feed,
    database: Arc<Database>,
    mut commits: watch::Receiver<()>,
    sender: mpsc::Sender<BufferedBatch>,
    buffer: Buffer,
) {
    loop {
        tokio::select! {
            changed = commits.changed() => if changed.is_err() { return },
            () = sender.closed() => return,
        }
        // No batch is built for a subscriber whose buffer is full: the commits made while
        // this waits are in the next batch, each entity once.
        tokio::select! {
            () = buffer.has_room() => {}
            () = sender.closed() => return,
        }
        match feed.next(&database).await {
            Ok(None) => {}
            Ok(Some(batch)) => {
                let Some(line) = batch_line(batch) else {
                    tracing::error!("a change batch exceeds the frame limit; closing its feed");
                    return;
                };
                if sender.send(buffer.hold(line)).await.is_err() {
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

const RETENTION_PERIOD: Duration = Duration::from_secs(60 * 60);

/// Discards the change history past the newest `keep_changes` captured rows
/// (`storage.feed_history_changes`) at start and every hour after; an older cursor gets a
/// snapshot.
pub(crate) async fn discard_history_forever(database: Arc<Database>, keep_changes: NonZeroU64) {
    let mut period = tokio::time::interval(RETENTION_PERIOD);
    loop {
        period.tick().await;
        match discard_history(&database, keep_changes).await {
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

async fn discard_history(
    database: &Database,
    keep_changes: NonZeroU64,
) -> Result<Option<u64>, cockpit_messaging::Error> {
    let write = database.write().await?;
    let retained_after = cockpit_messaging::discard_history(&write, keep_changes).await?;
    write.commit().await?;
    Ok(retained_after)
}
