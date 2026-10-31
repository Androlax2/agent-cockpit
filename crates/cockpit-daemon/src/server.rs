//! The Unix socket: peer-credential check, handshake, framing and per-connection bounds.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cockpit_agent_channel::{ProcessError, ProcessIdentity};
use cockpit_protocol::{
    Capability, ChangeBatch, ErrorCode, HandshakeReply, MAX_FRAME_BYTES, ProtocolError, Refusal,
    RefusalReason, Response, SUPPORTED_VERSIONS, ServerFrame, Welcome, decode_hello,
    decode_request, negotiate,
};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Semaphore, mpsc, oneshot, watch};

use crate::StartError;
use crate::lanes::{LaneLimits, Lanes};
use crate::routes::{Handler, Job, Peer, Route, WorkClass, refusal};

/// Only the owner may connect; the 0700 directory already says so, this is the second lock.
const SOCKET_MODE: u32 = 0o600;
/// Capacity a connection keeps between frames; a larger buffer, left by a big frame, is
/// released instead of being held for the connection's lifetime.
const RETAINED_FRAME_CAPACITY: usize = 64 * 1024;
/// A failed `accept` (out of descriptors) is retried after this pause instead of spinning.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);
/// The owner may follow the change feed; agents never see the whole state (report 11).
const OWNER_CAPABILITIES: &[Capability] = &[Capability::ChangeFeed];

/// How much the daemon accepts at once. Every bound answers or closes when reached.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Open connections per socket; one more is closed at once.
    pub connections: usize,
    /// Requests one connection may have unanswered; the daemon stops reading that
    /// connection until one is answered, so a client that does not read cannot grow memory.
    pub requests_per_connection: usize,
    pub control: LaneLimits,
    pub bulk: LaneLimits,
    /// A connection that has not sent its `hello` by then is closed.
    pub handshake_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            connections: 64,
            requests_per_connection: 32,
            control: LaneLimits {
                workers: 8,
                waiting: 64,
            },
            bulk: LaneLimits {
                workers: 4,
                waiting: 32,
            },
            handshake_timeout: Duration::from_secs(5),
        }
    }
}

impl Limits {
    /// Refuses limits under which the daemon could not serve anything.
    pub(crate) fn validate(&self) -> Result<(), StartError> {
        let invalid = [
            (self.connections == 0, "connections must be at least 1"),
            (
                self.requests_per_connection == 0,
                "requests_per_connection must be at least 1",
            ),
            (self.control.workers == 0, "the control lane needs a worker"),
            (self.bulk.workers == 0, "the bulk lane needs a worker"),
            (
                self.handshake_timeout.is_zero(),
                "handshake_timeout must not be zero",
            ),
        ];
        match invalid.into_iter().find(|(is_invalid, _)| *is_invalid) {
            Some((_, reason)) => Err(StartError::InvalidLimits(reason)),
            None => Ok(()),
        }
    }
}

/// What every connection shares.
pub(crate) struct Shared {
    pub(crate) owner_uid: u32,
    pub(crate) limits: Limits,
    pub(crate) lanes: Lanes,
    pub(crate) handler: Arc<dyn Handler>,
}

/// Which socket a connection came through; it alone decides between owner and agent.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Socket {
    HostOnly,
    Agent,
}

impl Socket {
    pub(crate) fn worker_name(self) -> &'static str {
        match self {
            Socket::HostOnly => "accept",
            Socket::Agent => "accept-agents",
        }
    }
}

/// Binds the socket at `path`, replacing a stale socket file. The caller holds the
/// instance lock, so no live daemon owns that file.
pub(crate) fn bind(path: &Path) -> Result<UnixListener, StartError> {
    match fs::remove_file(path) {
        Ok(()) => tracing::info!(socket = %path.display(), "removed a stale socket"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(StartError::io("remove the stale socket", path)(source)),
    }
    let listener = UnixListener::bind(path).map_err(StartError::io("bind the socket", path))?;
    fs::set_permissions(path, fs::Permissions::from_mode(SOCKET_MODE))
        .map_err(StartError::io("restrict the socket", path))?;
    Ok(listener)
}

/// Accepts connections forever, at most `connections` open at once. Each connection runs in
/// its own task, so a panic there ends that connection only.
pub(crate) async fn accept_connections(
    listener: Arc<UnixListener>,
    shared: Arc<Shared>,
    connections: Arc<Semaphore>,
    socket: Socket,
) {
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(error) => {
                tracing::warn!(%error, "accepting a connection failed; retrying");
                tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
                continue;
            }
        };
        let Ok(connection_place) = Arc::clone(&connections).try_acquire_owned() else {
            tracing::warn!(
                limit = shared.limits.connections,
                "too many connections; closing a new one"
            );
            continue;
        };
        if !is_owner(&stream, shared.owner_uid) {
            continue;
        }
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            let peer = match socket {
                Socket::HostOnly => Peer::Owner,
                Socket::Agent => Peer::Agent {
                    connector: connector(&stream).await,
                },
            };
            serve_connection(stream, &shared, peer).await;
            drop(connection_place);
        });
    }
}

/// Whether the kernel says the peer runs as the daemon's user; payloads never say who sent them.
fn is_owner(stream: &UnixStream, owner_uid: u32) -> bool {
    match stream.peer_cred() {
        Ok(credentials) if credentials.uid() == owner_uid => true,
        Ok(credentials) => {
            tracing::warn!(
                peer_uid = credentials.uid(),
                "refused a connection from another user"
            );
            false
        }
        Err(error) => {
            tracing::warn!(%error, "refused a connection without peer credentials");
            false
        }
    }
}

/// The process the kernel recorded as connecting, identified now, while the connection holds
/// its pid. SO_PEERPIDFD (Linux 6.5) would close the gap between connect and this read; a pid
/// cannot be reused within it without a full wrap of the pid space.
async fn connector(stream: &UnixStream) -> Result<ProcessIdentity, ProcessError> {
    let pid = stream
        .peer_cred()
        .ok()
        .and_then(|credentials| credentials.pid())
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .ok_or_else(|| ProcessError::Unreadable {
            reason: "the kernel named no peer process".to_owned(),
        })?;
    tokio::task::spawn_blocking(move || cockpit_agent_channel::identity_of(pid))
        .await
        .expect("reading the process table does not panic")
}

/// What the connection's writer sends: frames, or the end of the connection.
enum Outbound {
    Frame(ServerFrame),
    /// The change feed this connection followed stopped: closing tells the client to
    /// subscribe again, instead of leaving it waiting for batches that never come.
    Close,
}

async fn serve_connection(stream: UnixStream, shared: &Shared, peer: Peer) {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut frame = Vec::new();
    let offered = match peer {
        Peer::Owner => OWNER_CAPABILITIES,
        Peer::Agent { .. } => &[],
    };
    let handshake = tokio::time::timeout(
        shared.limits.handshake_timeout,
        handshake(&mut reader, &mut write_half, &mut frame, offered),
    );
    let welcome = match handshake.await {
        Ok(Some(welcome)) => welcome,
        Ok(None) => return,
        Err(_) => {
            tracing::info!("closed a connection that sent no hello in time");
            return;
        }
    };
    let (outgoing, frames) = mpsc::channel(shared.limits.requests_per_connection);
    let writer = tokio::spawn(write_frames(write_half, frames));
    // Set while a `subscribe` runs or its feed streams: one feed per connection.
    let following = Arc::new(AtomicBool::new(false));
    // Dropped when the client stops sending, which stops the feed forwarding to it.
    let (connection_open, connection_closed) = watch::channel(());
    loop {
        // A place for this request's answer, taken before reading it: the connection is
        // read only while it has room for another answer.
        let Ok(answer) = outgoing.clone().reserve_owned().await else {
            break;
        };
        frame.clear();
        frame.shrink_to(RETAINED_FRAME_CAPACITY);
        match next_frame(&mut reader, &mut frame).await {
            Ok(()) => {}
            Err(FrameError::TooLarge) => {
                answer.send(Outbound::Frame(malformed(format!(
                    "the frame is larger than {MAX_FRAME_BYTES} bytes"
                ))));
                break;
            }
            Err(FrameError::Closed) => break,
        }
        let Ok(text) = std::str::from_utf8(&frame) else {
            answer.send(Outbound::Frame(malformed(
                "the frame is not UTF-8".to_owned(),
            )));
            continue;
        };
        let request = match decode_request(text) {
            Ok(request) => request,
            Err(rejection) => {
                answer.send(Outbound::Frame(rejection.into_frame()));
                continue;
            }
        };
        let id = request.id;
        let respond =
            move |outcome| Outbound::Frame(ServerFrame::Response(Response { id, outcome }));
        let (class, job, stream_start) =
            match shared
                .handler
                .route(request.command, &welcome, peer.clone())
            {
                Route::Answer(outcome) => {
                    answer.send(respond(outcome));
                    continue;
                }
                Route::Run { class, job } => (class, job, None),
                Route::Stream { job } => {
                    if following.swap(true, Ordering::SeqCst) {
                        answer.send(respond(refusal(
                            ErrorCode::InvalidRequest,
                            "this connection follows the change feed already; subscribe on a \
                         new connection",
                        )));
                        continue;
                    }
                    let (started, start) = oneshot::channel();
                    let job: Job = Box::pin(async move {
                        let (outcome, batches) = job.await;
                        if let Some(batches) = batches {
                            let _ = started.send(batches);
                        }
                        outcome
                    });
                    (WorkClass::Bulk, job, Some(start))
                }
            };
        match shared.lanes.admit(class, job) {
            Ok(run) => {
                let feed = stream_start.map(|start| {
                    (
                        start,
                        outgoing.clone(),
                        connection_closed.clone(),
                        Arc::clone(&following),
                    )
                });
                tokio::spawn(async move {
                    let outcome = run.await;
                    let Some((start, outgoing, connection_closed, following)) = feed else {
                        answer.send(respond(outcome));
                        return;
                    };
                    // The job has finished, so whether it started a stream is known now.
                    let batches = start.await.ok();
                    if batches.is_none() {
                        following.store(false, Ordering::SeqCst);
                    }
                    // The answer is queued before the first batch, as the client expects.
                    answer.send(respond(outcome));
                    if let Some(batches) = batches {
                        forward(batches, outgoing, connection_closed).await;
                    }
                });
            }
            Err(busy) => {
                following.store(false, Ordering::SeqCst);
                answer.send(respond(busy));
            }
        }
    }
    // A feed forwarding holds a sender too: stopped first, or the writer would wait on it
    // forever. Requests still running keep their answer places, so the writer delivers
    // their answers before it stops.
    drop(connection_open);
    drop(outgoing);
    let _ = writer.await;
}

/// Sends the feed's batches on the connection until the feed or the connection stops. A feed
/// that stops first closes the connection.
async fn forward(
    mut batches: mpsc::Receiver<ChangeBatch>,
    outgoing: mpsc::Sender<Outbound>,
    mut connection_closed: watch::Receiver<()>,
) {
    loop {
        let batch = tokio::select! {
            batch = batches.recv() => batch,
            _ = connection_closed.changed() => return,
        };
        let frame = match batch {
            Some(batch) => Outbound::Frame(ServerFrame::Changes(batch)),
            None => Outbound::Close,
        };
        let is_close = matches!(frame, Outbound::Close);
        if outgoing.send(frame).await.is_err() || is_close {
            return;
        }
    }
}

/// Reads the `hello` and answers it, granting what both sides offer of `offered`; `None`
/// when the connection ends here.
async fn handshake(
    reader: &mut BufReader<OwnedReadHalf>,
    writer: &mut OwnedWriteHalf,
    frame: &mut Vec<u8>,
    offered: &[Capability],
) -> Option<Welcome> {
    let read = next_frame(reader, frame).await;
    let reply = match read {
        Err(FrameError::Closed) => return None,
        Err(FrameError::TooLarge) => malformed_hello(format!(
            "the first frame is larger than {MAX_FRAME_BYTES} bytes"
        )),
        Ok(()) => match std::str::from_utf8(frame) {
            Err(_) => malformed_hello("the first frame is not UTF-8".to_owned()),
            Ok(text) => match decode_hello(text, &SUPPORTED_VERSIONS) {
                Ok(hello) => negotiate(&hello, &SUPPORTED_VERSIONS, offered),
                Err(refusal) => HandshakeReply::Refused(refusal),
            },
        },
    };
    let line = encode(&reply).expect("a handshake reply is far below the frame limit");
    writer.write_all(&line).await.ok()?;
    match reply {
        HandshakeReply::Accepted(welcome) => Some(welcome),
        HandshakeReply::Refused(refusal) => {
            tracing::info!(reason = ?refusal.reason, "refused a handshake");
            None
        }
    }
}

async fn write_frames(mut writer: OwnedWriteHalf, mut frames: mpsc::Receiver<Outbound>) {
    while let Some(outbound) = frames.recv().await {
        let frame = match outbound {
            Outbound::Frame(frame) => frame,
            Outbound::Close => {
                tracing::info!("a change feed stopped; closing its connection");
                return;
            }
        };
        let line = match (encode(&frame), frame) {
            (Some(line), _) => line,
            (None, ServerFrame::Response(Response { id, .. })) => {
                let too_large = ServerFrame::Response(Response {
                    id,
                    outcome: refusal(
                        ErrorCode::Internal,
                        "the answer is larger than the frame limit; the request may have \
                         taken effect",
                    ),
                });
                encode(&too_large).expect("a refusal is far below the frame limit")
            }
            (None, _) => continue,
        };
        if writer.write_all(&line).await.is_err() {
            return;
        }
    }
}

/// `frame` as one line of JSON. The daemon's own frames always serialize; one larger than
/// the limit is a daemon bug, logged; the caller answers its request with a refusal.
fn encode(frame: &impl serde::Serialize) -> Option<Vec<u8>> {
    let mut line = serde_json::to_vec(frame).expect("protocol frames serialize");
    line.push(b'\n');
    if line.len() > MAX_FRAME_BYTES {
        tracing::error!(
            bytes = line.len(),
            "an answer exceeds the frame limit; not sent"
        );
        return None;
    }
    Some(line)
}

fn malformed_hello(message: String) -> HandshakeReply {
    HandshakeReply::Refused(Refusal {
        reason: RefusalReason::MalformedHello,
        message,
        supported_versions: SUPPORTED_VERSIONS.to_vec(),
    })
}

fn malformed(message: String) -> ServerFrame {
    ServerFrame::MalformedFrame(ProtocolError::new(ErrorCode::InvalidRequest, message))
}

enum FrameError {
    /// The peer closed the connection, cut a frame short or the socket failed.
    Closed,
    TooLarge,
}

/// Reads the next line into `frame` without its newline. The client's twin is
/// `cockpit-client/src/framing.rs`; both follow `cockpit-protocol`'s framing rule.
async fn next_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    frame: &mut Vec<u8>,
) -> Result<(), FrameError> {
    frame.clear();
    let read = reader
        .take(MAX_FRAME_BYTES as u64)
        .read_until(b'\n', frame)
        .await
        .map_err(|_| FrameError::Closed)?;
    match frame.pop() {
        Some(b'\n') => Ok(()),
        Some(_) if read == MAX_FRAME_BYTES => Err(FrameError::TooLarge),
        _ => Err(FrameError::Closed),
    }
}
