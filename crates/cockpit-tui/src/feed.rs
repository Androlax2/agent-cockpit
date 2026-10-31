//! Follows the daemon's change feed for the TUI (#199): sessions and checkouts arrive as
//! batches instead of list requests. The loop that owns the [`crate::App`] applies each batch
//! and says whether it did; only then does the cursor advance.

use std::path::PathBuf;
use std::time::Duration;

use cockpit_client::Subscription;
use cockpit_protocol::ChangeBatch;
use tokio::sync::{mpsc, oneshot};

use crate::FeedError;

/// Bounds each connect and subscribe exchange.
const SUBSCRIBE_DEADLINE: Duration = Duration::from_secs(2);
/// How long a lost or refused feed waits before it subscribes again.
const RETRY: Duration = Duration::from_secs(1);

/// What the feed hands the loop that owns the board.
#[derive(Debug)]
pub enum FeedEvent {
    /// Apply `batch` whole, then answer on `applied` whether it was.
    Batch {
        batch: ChangeBatch,
        applied: oneshot::Sender<Result<(), FeedError>>,
    },
    /// The feed stopped; it subscribes again after a pause. What the board shows of sessions
    /// and checkouts is not current until the next batch.
    Lost(String),
}

/// Follows the feed of the daemon at `socket` until `events` is closed. A disconnect resumes
/// from the last applied batch; a batch the board refused starts again from a snapshot.
pub async fn follow_feed(socket: PathBuf, events: mpsc::Sender<FeedEvent>) {
    let mut subscription = Subscription::new(socket.clone(), None, SUBSCRIBE_DEADLINE);
    loop {
        let problem = match subscription.next_batch().await {
            Err(error) => error.to_string(),
            Ok(pending) => {
                let (applied, outcome) = oneshot::channel();
                let batch = pending.batch().clone();
                if events
                    .send(FeedEvent::Batch { batch, applied })
                    .await
                    .is_err()
                {
                    return;
                }
                match outcome.await {
                    Ok(Ok(())) => {
                        pending.applied();
                        continue;
                    }
                    Ok(Err(refused)) => {
                        subscription = Subscription::new(socket.clone(), None, SUBSCRIBE_DEADLINE);
                        refused.to_string()
                    }
                    Err(_) => return,
                }
            }
        };
        tracing::warn!(problem, "the change feed stopped; subscribing again");
        if events.send(FeedEvent::Lost(problem)).await.is_err() {
            return;
        }
        tokio::time::sleep(RETRY).await;
    }
}
