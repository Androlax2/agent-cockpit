use std::path::PathBuf;
use std::time::Duration;

use cockpit_protocol::{
    Capability, ChangeBatch, Command, Cursor, ErrorCode, FeedPosition, ProtocolError, Reply,
};
use tokio::sync::mpsc;

use crate::connection::Connection;
use crate::error::ClientError;

/// Complete batches read ahead of the caller; the socket waits when they are not consumed.
const FEED_QUEUE_BATCHES: usize = 16;

/// Follows the daemon's change feed on a connection of its own, from a cursor the caller
/// advances only after applying a batch.
pub struct Subscription {
    socket_path: PathBuf,
    timeout: Duration,
    applied: Option<Cursor>,
    stream: Option<FeedStream>,
}

struct FeedStream {
    connection: Connection,
    batches: mpsc::Receiver<ChangeBatch>,
}

impl Subscription {
    /// A subscription that resumes after `applied`, or starts from a snapshot when it is
    /// `None`. It connects on the first [`Subscription::next_batch`]; `timeout` bounds each
    /// connect and subscribe exchange.
    pub fn new(socket_path: PathBuf, applied: Option<Cursor>, timeout: Duration) -> Self {
        Self {
            socket_path,
            timeout,
            applied,
            stream: None,
        }
    }

    /// The cursor of the last batch marked applied.
    pub fn applied_cursor(&self) -> Option<&Cursor> {
        self.applied.as_ref()
    }

    /// Waits for the next complete batch, subscribing first when there is no connection.
    ///
    /// Cancel safe once subscribed. A disconnect or a gap fails this call and drops the
    /// connection; the next call subscribes again from the applied cursor.
    pub async fn next_batch(&mut self) -> Result<PendingBatch<'_>, ClientError> {
        let stream = match &mut self.stream {
            Some(stream) => stream,
            None => self.stream.insert(self.subscribe().await?),
        };
        let Some(batch) = stream.batches.recv().await else {
            let error = stream.connection.closed();
            self.stream = None;
            return Err(error);
        };
        if let FeedPosition::Cursor(follows) = &batch.follows
            && self.applied.as_ref() != Some(follows)
        {
            self.stream = None;
            return Err(ClientError::FeedGap {
                applied: self.applied.clone(),
                follows: batch.follows,
            });
        }
        Ok(PendingBatch {
            subscription: self,
            batch,
        })
    }

    async fn subscribe(&self) -> Result<FeedStream, ClientError> {
        let (feed, batches) = mpsc::channel(FEED_QUEUE_BATCHES);
        let connection = Connection::open_with_feed(
            &self.socket_path,
            &[Capability::ChangeFeed],
            self.timeout,
            Some(feed),
        )
        .await?;
        if !connection
            .welcome()
            .capabilities
            .contains(&Capability::ChangeFeed)
        {
            return Err(ClientError::NotGranted(Capability::ChangeFeed));
        }
        let from = match &self.applied {
            Some(cursor) => FeedPosition::Cursor(cursor.clone()),
            None => FeedPosition::Snapshot,
        };
        match subscribe_from(&connection, from, self.timeout).await {
            Err(ClientError::Daemon(ProtocolError {
                code: ErrorCode::ResnapshotRequired,
                ..
            })) if self.applied.is_some() => {
                subscribe_from(&connection, FeedPosition::Snapshot, self.timeout).await?;
            }
            result => result?,
        }
        Ok(FeedStream {
            connection,
            batches,
        })
    }
}

async fn subscribe_from(
    connection: &Connection,
    from: FeedPosition,
    timeout: Duration,
) -> Result<(), ClientError> {
    match connection
        .request(Command::Subscribe { from }, timeout)
        .await?
    {
        Reply::Subscribed { .. } => Ok(()),
        other => Err(ClientError::UnexpectedReply(other)),
    }
}

/// A batch the caller has not applied yet. Dropping it without [`PendingBatch::applied`]
/// keeps the cursor, so the batch arrives again after the subscription reconnects.
#[must_use = "a batch only advances the cursor once `applied` is called"]
pub struct PendingBatch<'a> {
    subscription: &'a mut Subscription,
    batch: ChangeBatch,
}

impl PendingBatch<'_> {
    /// The changes to apply. When `follows` is a snapshot, they replace all of the caller's
    /// state.
    pub fn batch(&self) -> &ChangeBatch {
        &self.batch
    }

    /// Records that the whole batch, deletions included, is applied, and advances the cursor
    /// to its end.
    pub fn applied(self) {
        self.subscription.applied = Some(self.batch.cursor);
    }
}
