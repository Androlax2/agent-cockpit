//! Issue #41's change feed at the daemon's socket: snapshot, then each commit as it happens,
//! resume from a cursor across a restart, and `resnapshot-required` for a cursor of another
//! epoch or older than the retained history; never offered on the agent socket.
//!
//! The daemon runs in this process, as in `sessions.rs`, so the test can publish entities the
//! way a producer does: in a write transaction on the daemon's own state database.
//! They publish `attempt` entities: the daemon itself owns the `session` and `checkout`
//! kinds and retracts any it did not derive (#199).

mod common;

use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Subscription};
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{
    Capability, Change, ChangeBatch, ChangeEvent, Command, CommitRevision, Cursor, DatabaseEpoch,
    EntityKind, EntityRef, EntityRevision, ErrorCode, FeedPosition, Hello, Outcome, Reply, Request,
    RequestId, ServerFrame,
};
use common::GENEROUS;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

struct Home {
    dir: tempfile::TempDir,
    database: Mutex<Option<Arc<cockpit_turso::Database>>>,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
            database: Mutex::new(None),
        }
    }

    /// Starts a daemon on this home's state. A daemon dropped just before releases its
    /// database once its aborted tasks are gone, so the start is retried until then.
    async fn start(&self) -> Daemon {
        self.start_with(Limits::default()).await
    }

    async fn start_with(&self, limits: Limits) -> Daemon {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let options = Options {
                runtime_dir: self.dir.path().join("run/cockpit"),
                state_dir: self.dir.path().join("state/cockpit"),
                limits,
            };
            let started = match Instance::acquire(options) {
                Ok(instance) => {
                    instance
                        .start(
                            common::wiring_in(self.dir.path(), Vec::new()),
                            |capabilities| {
                                *self.database.lock().unwrap() =
                                    Some(Arc::clone(&capabilities.database));
                                Arc::new(Routes::new(
                                    capabilities,
                                    cockpit_config::Environment::default(),
                                ))
                            },
                        )
                        .await
                }
                Err(error) => Err(error),
            };
            match started {
                Ok(daemon) => return daemon,
                Err(_) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => panic!("the daemon did not start: {error}"),
            }
        }
    }

    /// Stops the daemon and lets go of its database, as a crash would.
    fn stop(&self, daemon: Daemon) {
        drop(daemon);
        *self.database.lock().unwrap() = None;
    }

    fn database(&self) -> Arc<cockpit_turso::Database> {
        Arc::clone(
            self.database
                .lock()
                .unwrap()
                .as_ref()
                .expect("a daemon runs"),
        )
    }

    /// What a producer does: publish the entity's state with the change that caused it.
    async fn publish(&self, kind: &str, id: &str, state: Value) {
        let database = self.database();
        let write = database.write().await.unwrap();
        cockpit_messaging::publish(&write, kind, id, &object(state))
            .await
            .unwrap();
        write.commit().await.unwrap();
    }

    async fn retract(&self, kind: &str, id: &str) {
        let database = self.database();
        let write = database.write().await.unwrap();
        cockpit_messaging::retract(&write, kind, id).await.unwrap();
        write.commit().await.unwrap();
    }

    fn socket_path(&self) -> PathBuf {
        self.dir
            .path()
            .join("run/cockpit")
            .join(cockpit_config::DAEMON_SOCKET)
    }

    fn agent_socket_path(&self) -> PathBuf {
        self.dir
            .path()
            .join("run/cockpit")
            .join(cockpit_config::AGENT_SOCKET_DIR)
            .join(cockpit_config::AGENT_SOCKET)
    }

    fn subscription(&self, applied: Option<Cursor>) -> Subscription {
        Subscription::new(self.socket_path(), applied, GENEROUS)
    }
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(state) => state,
        other => panic!("not an object: {other}"),
    }
}

fn upsert(kind: EntityKind, id: &str, revision: u64, state: Value) -> ChangeEvent {
    ChangeEvent {
        entity: EntityRef {
            kind,
            id: id.to_owned(),
        },
        entity_revision: EntityRevision(revision),
        change: Change::Upsert {
            state: object(state),
        },
    }
}

fn tombstone(kind: EntityKind, id: &str, revision: u64) -> ChangeEvent {
    ChangeEvent {
        entity: EntityRef {
            kind,
            id: id.to_owned(),
        },
        entity_revision: EntityRevision(revision),
        change: Change::Tombstone,
    }
}

/// The next batch, applied: the subscriber's cursor moves to its end.
async fn apply_next(subscription: &mut Subscription) -> ChangeBatch {
    let pending = tokio::time::timeout(GENEROUS, subscription.next_batch())
        .await
        .expect("a batch arrives")
        .expect("the feed delivers");
    let batch = pending.batch().clone();
    pending.applied();
    batch
}

/// A connection that asks for the change feed and speaks frames by hand, to see the
/// daemon's answers that `Subscription` follows on its own.
struct RawFeedPeer {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
    next_id: u64,
}

impl RawFeedPeer {
    async fn connect(home: &Home) -> RawFeedPeer {
        let stream = UnixStream::connect(home.socket_path()).await.unwrap();
        let (read_half, writer) = stream.into_split();
        let mut peer = RawFeedPeer {
            reader: BufReader::new(read_half),
            writer,
            next_id: 1,
        };
        let hello = Hello {
            protocol_versions: vec![cockpit_protocol::ProtocolVersion(1)],
            capabilities: vec![Capability::ChangeFeed],
        };
        peer.send(&serde_json::to_value(hello).unwrap()).await;
        let welcome = peer.read_value().await;
        assert_eq!(welcome["capabilities"], json!(["change-feed"]), "{welcome}");
        peer
    }

    async fn send(&mut self, value: &Value) {
        let mut line = serde_json::to_vec(value).unwrap();
        line.push(b'\n');
        self.writer.write_all(&line).await.unwrap();
    }

    async fn read_value(&mut self) -> Value {
        let mut line = String::new();
        tokio::time::timeout(GENEROUS, self.reader.read_line(&mut line))
            .await
            .expect("the daemon answers")
            .unwrap();
        serde_json::from_str(&line).expect("the daemon sends JSON")
    }

    /// `None` once the daemon closed the connection.
    async fn read_line_or_end(&mut self) -> Option<String> {
        let mut line = String::new();
        let read = tokio::time::timeout(GENEROUS, self.reader.read_line(&mut line))
            .await
            .expect("the daemon answers or closes")
            .unwrap();
        (read > 0).then_some(line)
    }

    async fn read_frame(&mut self) -> ServerFrame {
        serde_json::from_value(self.read_value().await).expect("a server frame")
    }

    async fn subscribe(&mut self, from: FeedPosition) -> Outcome {
        let id = RequestId(self.next_id);
        self.next_id += 1;
        let request = Request {
            id,
            command: Command::Subscribe { from },
        };
        self.send(&serde_json::to_value(request).unwrap()).await;
        match self.read_frame().await {
            ServerFrame::Response(response) if response.id == id => response.outcome,
            other => panic!("not the answer to subscribe: {other:?}"),
        }
    }
}

fn error_code(outcome: &Outcome) -> Option<&ErrorCode> {
    match outcome {
        Outcome::Error(error) => Some(&error.code),
        Outcome::Ok(_) => None,
    }
}

#[tokio::test]
async fn a_subscriber_gets_the_snapshot_then_each_commit_as_it_happens() {
    let home = Home::new();
    let _daemon = home.start().await;
    home.publish("attempt", "s-1", json!({"state": "working"}))
        .await;
    let mut subscription = home.subscription(None);

    let snapshot = apply_next(&mut subscription).await;
    home.publish("attempt", "s-2", json!({"state": "idle"}))
        .await;
    home.publish("widget", "w-1", json!({"size": 3})).await;
    let mut streamed = Vec::new();
    while streamed.len() < 2 {
        streamed.extend(apply_next(&mut subscription).await.changes);
    }

    assert_eq!(snapshot.follows, FeedPosition::Snapshot);
    assert_eq!(
        snapshot.changes,
        [upsert(
            EntityKind::Attempt,
            "s-1",
            1,
            json!({"state": "working"})
        )]
    );
    assert_eq!(
        streamed,
        [
            upsert(EntityKind::Attempt, "s-2", 1, json!({"state": "idle"})),
            upsert(
                EntityKind::Unknown("widget".to_owned()),
                "w-1",
                1,
                json!({"size": 3})
            ),
        ]
    );
}

#[tokio::test]
async fn a_cursor_resumes_after_a_daemon_restart_with_only_what_changed_since() {
    let home = Home::new();
    let daemon = home.start().await;
    home.publish("attempt", "s-1", json!({"state": "working"}))
        .await;
    let mut subscription = home.subscription(None);
    apply_next(&mut subscription).await;
    let applied = subscription.applied_cursor().unwrap().clone();
    drop(subscription);
    home.stop(daemon);

    let _daemon = home.start().await;
    home.publish("attempt", "s-2", json!({"state": "idle"}))
        .await;
    home.retract("attempt", "s-1").await;
    let mut resumed = home.subscription(Some(applied.clone()));
    let mut replayed = Vec::new();
    let first = apply_next(&mut resumed).await;
    let follows = first.follows.clone();
    replayed.extend(first.changes);
    while replayed.len() < 2 {
        replayed.extend(apply_next(&mut resumed).await.changes);
    }

    assert_eq!(follows, FeedPosition::Cursor(applied));
    assert_eq!(
        replayed,
        [
            upsert(EntityKind::Attempt, "s-2", 1, json!({"state": "idle"})),
            tombstone(EntityKind::Attempt, "s-1", 2),
        ]
    );
}

#[tokio::test]
async fn a_cursor_of_another_epoch_is_answered_resnapshot_required() {
    let home = Home::new();
    let _daemon = home.start().await;
    let mut peer = RawFeedPeer::connect(&home).await;
    let foreign = Cursor {
        epoch: DatabaseEpoch("another-installation".to_owned()),
        revision: CommitRevision(1),
    };

    let refused = peer.subscribe(FeedPosition::Cursor(foreign.clone())).await;
    let mut subscription = home.subscription(Some(foreign));
    let snapshot = apply_next(&mut subscription).await;

    assert_eq!(error_code(&refused), Some(&ErrorCode::ResnapshotRequired));
    assert_eq!(snapshot.follows, FeedPosition::Snapshot);
}

#[tokio::test]
async fn a_cursor_older_than_the_retained_history_is_answered_resnapshot_required() {
    let home = Home::new();
    let _daemon = home.start().await;
    home.publish("attempt", "s-1", json!({"state": "working"}))
        .await;
    let mut subscription = home.subscription(None);
    apply_next(&mut subscription).await;
    let old = subscription.applied_cursor().unwrap().clone();
    drop(subscription);
    home.publish("attempt", "s-1", json!({"state": "idle"}))
        .await;
    home.publish("attempt", "s-1", json!({"state": "waiting"}))
        .await;
    // What the daemon's retention worker does with its own bound.
    let database = home.database();
    let write = database.write().await.unwrap();
    cockpit_messaging::discard_history(&write, NonZeroU64::new(1).unwrap())
        .await
        .unwrap();
    write.commit().await.unwrap();
    let mut peer = RawFeedPeer::connect(&home).await;

    let refused = peer.subscribe(FeedPosition::Cursor(old.clone())).await;
    let subscribed = peer.subscribe(FeedPosition::Snapshot).await;
    let snapshot = match peer.read_frame().await {
        ServerFrame::Changes(batch) => batch,
        other => panic!("not a batch: {other:?}"),
    };

    assert_eq!(error_code(&refused), Some(&ErrorCode::ResnapshotRequired));
    assert!(
        matches!(subscribed, Outcome::Ok(Reply::Subscribed { .. })),
        "{subscribed:?}"
    );
    assert_eq!(
        snapshot.changes,
        [upsert(
            EntityKind::Attempt,
            "s-1",
            3,
            json!({"state": "waiting"})
        )]
    );
}

#[tokio::test]
async fn a_connection_follows_one_feed_at_a_time() {
    let home = Home::new();
    let _daemon = home.start().await;
    let mut peer = RawFeedPeer::connect(&home).await;
    let subscribed = peer.subscribe(FeedPosition::Snapshot).await;
    assert!(matches!(peer.read_frame().await, ServerFrame::Changes(_)));

    let second = peer.subscribe(FeedPosition::Snapshot).await;

    assert!(
        matches!(subscribed, Outcome::Ok(Reply::Subscribed { .. })),
        "{subscribed:?}"
    );
    assert_eq!(error_code(&second), Some(&ErrorCode::InvalidRequest));
}

#[tokio::test]
async fn the_agent_socket_never_offers_the_change_feed() {
    let home = Home::new();
    let _daemon = home.start().await;
    let mut subscription = Subscription::new(home.agent_socket_path(), None, GENEROUS);

    let refused = subscription.next_batch().await.err();

    assert!(
        matches!(
            refused,
            Some(ClientError::NotGranted(Capability::ChangeFeed))
        ),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_connection_with_one_answer_place_still_gets_its_batches() {
    let home = Home::new();
    let _daemon = home
        .start_with(Limits {
            requests_per_connection: 1,
            ..Limits::default()
        })
        .await;
    home.publish("attempt", "s-1", json!({"state": "working"}))
        .await;
    let mut subscription = home.subscription(None);

    let snapshot = apply_next(&mut subscription).await;
    home.publish("attempt", "s-1", json!({"state": "idle"}))
        .await;
    let streamed = apply_next(&mut subscription).await;

    assert_eq!(snapshot.changes.len(), 1);
    assert_eq!(
        streamed.changes,
        [upsert(
            EntityKind::Attempt,
            "s-1",
            2,
            json!({"state": "idle"})
        )]
    );
}

#[tokio::test]
async fn a_snapshot_too_large_for_one_frame_closes_the_connection_instead_of_skipping_it() {
    let home = Home::new();
    let _daemon = home.start().await;
    let large = "x".repeat(cockpit_protocol::MAX_FRAME_BYTES);
    home.publish("attempt", "s-1", json!({"note": large})).await;
    let mut peer = RawFeedPeer::connect(&home).await;

    let subscribed = peer.subscribe(FeedPosition::Snapshot).await;
    let after_answer = peer.read_line_or_end().await;

    assert!(
        matches!(subscribed, Outcome::Ok(Reply::Subscribed { .. })),
        "{subscribed:?}"
    );
    assert_eq!(after_answer, None);
}
