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
    /// A batch handed to the caller and not applied yet; the next call hands it out again.
    unapplied: Option<ChangeBatch>,
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
            unapplied: None,
            stream: None,
        }
    }

    /// The cursor of the last batch marked applied.
    pub fn applied_cursor(&self) -> Option<&Cursor> {
        self.applied.as_ref()
    }

    /// The batch not applied yet, or else the next complete batch, subscribing first when
    /// there is no connection.
    ///
    /// Cancel safe once subscribed. A disconnect or a gap fails this call and drops the
    /// connection; the next call subscribes again from the applied cursor.
    pub async fn next_batch(&mut self) -> Result<PendingBatch<'_>, ClientError> {
        if self.unapplied.is_none() {
            let batch = self.receive().await?;
            self.unapplied = Some(batch);
        }
        Ok(PendingBatch { subscription: self })
    }

    async fn receive(&mut self) -> Result<ChangeBatch, ClientError> {
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
        Ok(batch)
    }

    async fn subscribe(&self) -> Result<FeedStream, ClientError> {
        let (feed, batches) = mpsc::channel(FEED_QUEUE_BATCHES);
        let connection =
            Connection::open_with_feed(&self.socket_path, self.timeout, Some(feed)).await?;
        if !connection
            .welcome()
            .capabilities
            .contains(&Capability::ChangeFeed)
        {
            return Err(ClientError::NotGranted(Capability::ChangeFeed));
        }
        let from = self
            .applied
            .clone()
            .map_or(FeedPosition::Snapshot, FeedPosition::Cursor);
        let resumes = matches!(from, FeedPosition::Cursor(_));
        match subscribe_from(&connection, from, self.timeout).await {
            Err(ClientError::Daemon(ProtocolError {
                code: ErrorCode::ResnapshotRequired,
                ..
            })) if resumes => {
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
/// keeps the cursor, and the next [`Subscription::next_batch`] returns the same batch.
#[must_use = "a batch only advances the cursor once `applied` is called"]
pub struct PendingBatch<'a> {
    subscription: &'a mut Subscription,
}

impl PendingBatch<'_> {
    /// The changes to apply. When `follows` is a snapshot, they replace all of the caller's
    /// state.
    pub fn batch(&self) -> &ChangeBatch {
        self.subscription
            .unapplied
            .as_ref()
            .expect("a pending batch exists only while its batch is unapplied")
    }

    /// Records that the whole batch, deletions included, is applied, and advances the cursor
    /// to its end.
    pub fn applied(self) {
        let batch = self
            .subscription
            .unapplied
            .take()
            .expect("a pending batch exists only while its batch is unapplied");
        self.subscription.applied = Some(batch.cursor);
    }
}
