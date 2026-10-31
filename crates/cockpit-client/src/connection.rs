use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use cockpit_protocol::{
    Capability, ChangeBatch, Command, HandshakeReply, Hello, Outcome, Reply, Request, RequestId,
    SUPPORTED_VERSIONS, ServerFrame, Welcome,
};
use serde::de::DeserializeOwned;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::error::{ClientError, CloseReason};
use crate::framing::{FrameReader, encode};

/// Frames waiting for the socket; a request that cannot queue waits within its deadline.
const OUTGOING_QUEUE_FRAMES: usize = 64;

/// One handshaken connection to the daemon. Dropping it closes the socket.
pub struct Connection {
    welcome: Welcome,
    outgoing: mpsc::Sender<Vec<u8>>,
    shared: Arc<Mutex<SharedState>>,
    next_request_id: AtomicU64,
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
}

/// What the reader task and the callers share.
#[derive(Default)]
struct SharedState {
    awaiting_reply: HashMap<RequestId, oneshot::Sender<Outcome>>,
    close_reason: Option<CloseReason>,
}

impl Connection {
    /// Connects to the daemon at `socket_path` and completes the handshake, asking for
    /// `capabilities`, all within `timeout`.
    pub async fn open(
        socket_path: &Path,
        capabilities: &[Capability],
        timeout: Duration,
    ) -> Result<Self, ClientError> {
        Self::open_with_feed(socket_path, capabilities, timeout, None).await
    }

    /// Like [`Connection::open`]; change batches go to `feed`. A connection without a feed
    /// treats a change batch as a protocol violation and closes.
    pub(crate) async fn open_with_feed(
        socket_path: &Path,
        capabilities: &[Capability],
        timeout: Duration,
        feed: Option<mpsc::Sender<ChangeBatch>>,
    ) -> Result<Self, ClientError> {
        let handshake = handshake(socket_path, capabilities);
        let (welcome, frames, write_half) = tokio::time::timeout(timeout, handshake)
            .await
            .map_err(|_| ClientError::DeadlineExceeded)??;
        let shared = Arc::new(Mutex::new(SharedState::default()));
        let (outgoing, outgoing_frames) = mpsc::channel(OUTGOING_QUEUE_FRAMES);
        Ok(Self {
            welcome,
            outgoing,
            reader: tokio::spawn(read_frames(frames, Arc::clone(&shared), feed)),
            writer: tokio::spawn(write_frames(
                write_half,
                outgoing_frames,
                Arc::clone(&shared),
            )),
            shared,
            next_request_id: AtomicU64::new(1),
        })
    }

    /// The version and capabilities the daemon agreed to.
    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    /// Sends `command` and waits for its reply for at most `timeout`. Cancel safe: dropping
    /// the future stops waiting, discards a late reply and leaves the connection usable, but
    /// does not stop an operation the daemon already started.
    pub async fn request(&self, command: Command, timeout: Duration) -> Result<Reply, ClientError> {
        let id = RequestId(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let frame = encode(&Request { id, command })?;
        let (reply_sender, reply_receiver) = oneshot::channel();
        {
            let mut state = lock(&self.shared);
            if let Some(reason) = &state.close_reason {
                return Err(ClientError::Closed(reason.clone()));
            }
            state.awaiting_reply.insert(id, reply_sender);
        }
        let _stop_waiting_on_drop = AwaitingReply {
            shared: &self.shared,
            id,
        };
        let exchange = async {
            self.outgoing.send(frame).await.map_err(|_| self.closed())?;
            reply_receiver.await.map_err(|_| self.closed())
        };
        let outcome = tokio::time::timeout(timeout, exchange)
            .await
            .map_err(|_| ClientError::DeadlineExceeded)??;
        match outcome {
            Outcome::Ok(reply) => Ok(reply),
            Outcome::Error(error) => Err(ClientError::Daemon(error)),
        }
    }

    /// The error for an operation that found the connection closed.
    pub(crate) fn closed(&self) -> ClientError {
        let reason = lock(&self.shared).close_reason.clone();
        ClientError::Closed(reason.unwrap_or(CloseReason::Disconnected))
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}

/// Forgets a request's reply slot when its caller stops waiting, for any reason.
struct AwaitingReply<'a> {
    shared: &'a Mutex<SharedState>,
    id: RequestId,
}

impl Drop for AwaitingReply<'_> {
    fn drop(&mut self) {
        lock(self.shared).awaiting_reply.remove(&self.id);
    }
}

/// The state stays consistent under a panic elsewhere: every update is a single map or
/// field operation, so a poisoned lock is still safe to use.
fn lock(shared: &Mutex<SharedState>) -> MutexGuard<'_, SharedState> {
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Records why the connection stopped (the first reason wins) and fails every waiting request.
fn close(shared: &Mutex<SharedState>, reason: CloseReason) {
    let mut state = lock(shared);
    state.close_reason.get_or_insert(reason);
    state.awaiting_reply.clear();
}

async fn handshake(
    socket_path: &Path,
    capabilities: &[Capability],
) -> Result<(Welcome, FrameReader<OwnedReadHalf>, OwnedWriteHalf), ClientError> {
    let stream = UnixStream::connect(socket_path)
        .await
        .map_err(|source| ClientError::Connect {
            socket_path: socket_path.to_path_buf(),
            source,
        })?;
    let (read_half, mut write_half) = stream.into_split();
    let hello = Hello {
        protocol_versions: SUPPORTED_VERSIONS.to_vec(),
        capabilities: capabilities.to_vec(),
    };
    write_half
        .write_all(&encode(&hello)?)
        .await
        .map_err(|error| ClientError::Closed(CloseReason::Io(error.to_string())))?;
    let mut frames = FrameReader::new(read_half);
    let reply_frame = frames.next_frame().await.map_err(ClientError::Closed)?;
    match decode::<HandshakeReply>(&reply_frame).map_err(ClientError::Closed)? {
        HandshakeReply::Accepted(welcome) => {
            if !SUPPORTED_VERSIONS.contains(&welcome.protocol_version) {
                return Err(ClientError::Closed(CloseReason::InvalidFrame(format!(
                    "the daemon chose protocol version {}, which this client did not offer",
                    welcome.protocol_version.0
                ))));
            }
            Ok((welcome, frames, write_half))
        }
        HandshakeReply::Refused(refusal) => Err(ClientError::Refused(refusal)),
    }
}

fn decode<T: DeserializeOwned>(frame: &[u8]) -> Result<T, CloseReason> {
    serde_json::from_slice(frame).map_err(|error| CloseReason::InvalidFrame(error.to_string()))
}

/// Routes each reply to its waiting request and each change batch to `feed`, until the
/// connection fails; then records why.
async fn read_frames(
    mut frames: FrameReader<OwnedReadHalf>,
    shared: Arc<Mutex<SharedState>>,
    feed: Option<mpsc::Sender<ChangeBatch>>,
) {
    let reason = loop {
        let frame = match frames.next_frame().await.and_then(|frame| decode(&frame)) {
            Ok(frame) => frame,
            Err(reason) => break reason,
        };
        match frame {
            ServerFrame::Response(response) => {
                // No slot means the caller stopped waiting: a late reply is discarded.
                let waiting = lock(&shared).awaiting_reply.remove(&response.id);
                if let Some(reply_sender) = waiting {
                    let _ = reply_sender.send(response.outcome);
                }
            }
            ServerFrame::Changes(batch) => {
                let Some(feed) = &feed else {
                    break CloseReason::InvalidFrame(
                        "a change batch arrived on a connection without a subscription".to_owned(),
                    );
                };
                if feed.send(batch).await.is_err() {
                    break CloseReason::Disconnected;
                }
            }
            ServerFrame::MalformedFrame(error) => break CloseReason::RejectedFrame(error),
        }
    };
    close(&shared, reason);
}

async fn write_frames(
    mut write_half: OwnedWriteHalf,
    mut outgoing_frames: mpsc::Receiver<Vec<u8>>,
    shared: Arc<Mutex<SharedState>>,
) {
    while let Some(frame) = outgoing_frames.recv().await {
        if let Err(error) = write_half.write_all(&frame).await {
            close(&shared, CloseReason::Io(error.to_string()));
            return;
        }
    }
}
