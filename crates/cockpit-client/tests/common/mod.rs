//! A scripted daemon on a real Unix socket: each test plays the daemon's side frame by frame.
#![allow(
    dead_code,
    reason = "each test file uses a different part of the fake daemon"
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use cockpit_protocol::{
    Capability, Change, ChangeBatch, ChangeEvent, Command, CommitRevision, Cursor, DatabaseEpoch,
    EntityKind, EntityRef, EntityRevision, ErrorCode, FeedPosition, Hello, Outcome, ProtocolError,
    Reply, Request, RequestId, Response, SUPPORTED_VERSIONS, ServerFrame, negotiate,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};

/// Long enough never to fire in a passing test, short enough to fail a hung one quickly.
pub const GENEROUS: Duration = Duration::from_secs(5);

pub struct FakeDaemon {
    listener: UnixListener,
    pub socket_path: PathBuf,
}

impl FakeDaemon {
    pub fn start() -> Self {
        static NEXT_SOCKET: AtomicU32 = AtomicU32::new(0);
        let socket_path = std::env::temp_dir().join(format!(
            "cockpit-client-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).expect("the fake daemon binds its socket");
        Self {
            listener,
            socket_path,
        }
    }

    pub async fn accept(&self) -> Peer {
        let (stream, _) = tokio::time::timeout(GENEROUS, self.listener.accept())
            .await
            .expect("a client connects")
            .expect("accept succeeds");
        Peer::new(stream)
    }

    /// Accepts a client and answers its hello with `reply`; returns the hello as sent.
    pub async fn answer_hello(&self, reply: &impl serde::Serialize) -> (serde_json::Value, Peer) {
        let mut peer = self.accept().await;
        let hello = peer.read_frame().await;
        peer.send(reply).await;
        (hello, peer)
    }

    /// Serves a subscription's start: handshake with the change feed, `subscribed` at the
    /// end of `first`, then `first`. Returns where the client asked to start.
    pub async fn serve_feed(&self, first: &ChangeBatch) -> (FeedPosition, Peer) {
        let mut peer = self.accept_handshake(&[Capability::ChangeFeed]).await;
        let (id, from) = peer.read_subscribe().await;
        peer.reply(id, subscribed(first.cursor.clone())).await;
        peer.send_batch(first).await;
        (from, peer)
    }

    /// Accepts a client and answers its hello as a daemon offering `daemon_capabilities`.
    pub async fn accept_handshake(&self, daemon_capabilities: &[Capability]) -> Peer {
        let mut peer = self.accept().await;
        let hello: Hello = serde_json::from_value(peer.read_frame().await).expect("a hello");
        peer.send(&negotiate(&hello, &SUPPORTED_VERSIONS, daemon_capabilities))
            .await;
        peer
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

pub struct Peer {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl Peer {
    fn new(stream: UnixStream) -> Self {
        let (read_half, writer) = stream.into_split();
        Self {
            reader: BufReader::new(read_half),
            writer,
        }
    }

    pub async fn read_frame(&mut self) -> serde_json::Value {
        let mut line = String::new();
        let read = tokio::time::timeout(GENEROUS, self.reader.read_line(&mut line))
            .await
            .expect("the client sends a frame")
            .expect("the socket is readable");
        assert!(read > 0, "the client closed the connection");
        serde_json::from_str(&line).expect("the client sends JSON")
    }

    pub async fn read_request(&mut self) -> Request {
        serde_json::from_value(self.read_frame().await).expect("a request")
    }

    /// Reads a subscribe request and returns its id and starting position.
    pub async fn read_subscribe(&mut self) -> (RequestId, FeedPosition) {
        let request = self.read_request().await;
        let Command::Subscribe { from } = request.command else {
            panic!("expected subscribe, got {:?}", request.command);
        };
        (request.id, from)
    }

    pub async fn send(&mut self, frame: &impl serde::Serialize) {
        let mut line = serde_json::to_vec(frame).expect("the frame serializes");
        line.push(b'\n');
        self.send_raw(&line).await;
    }

    pub async fn send_raw(&mut self, bytes: &[u8]) {
        self.writer
            .write_all(bytes)
            .await
            .expect("the client is connected");
    }

    pub async fn reply(&mut self, id: RequestId, reply: Reply) {
        self.send(&ServerFrame::Response(Response {
            id,
            outcome: Outcome::Ok(reply),
        }))
        .await;
    }

    pub async fn fail(&mut self, id: RequestId, code: ErrorCode) {
        self.send(&ServerFrame::Response(Response {
            id,
            outcome: Outcome::Error(ProtocolError {
                code,
                message: "scripted failure".to_owned(),
            }),
        }))
        .await;
    }

    pub async fn send_batch(&mut self, batch: &ChangeBatch) {
        self.send(&ServerFrame::Changes(batch.clone())).await;
    }

    /// Waits until the client closes its side of the connection.
    pub async fn expect_closed(&mut self) {
        let mut rest = String::new();
        let read = tokio::time::timeout(GENEROUS, self.reader.read_line(&mut rest))
            .await
            .expect("the client closes the connection")
            .expect("the socket is readable");
        assert_eq!(
            (read, rest.as_str()),
            (0, ""),
            "the client sent more frames"
        );
    }
}

pub fn cursor(epoch: &str, revision: u64) -> Cursor {
    Cursor {
        epoch: DatabaseEpoch(epoch.to_owned()),
        revision: CommitRevision(revision),
    }
}

pub fn subscribed(cursor: Cursor) -> Reply {
    Reply::Subscribed { cursor }
}

/// A batch with one task tombstone per entry of `task_ids`.
pub fn batch(follows: FeedPosition, ends_at: Cursor, task_ids: &[&str]) -> ChangeBatch {
    ChangeBatch {
        follows,
        cursor: ends_at,
        changes: task_ids
            .iter()
            .map(|task_id| ChangeEvent {
                entity: EntityRef {
                    kind: EntityKind::Task,
                    id: (*task_id).to_owned(),
                },
                entity_revision: EntityRevision(1),
                change: Change::Tombstone,
            })
            .collect(),
    }
}
