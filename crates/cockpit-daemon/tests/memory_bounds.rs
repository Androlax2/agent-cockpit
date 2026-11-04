//! Issue #406's memory bounds at the daemon's socket: what a change-feed subscriber that
//! stops reading makes the daemon hold, bounded in bytes by `memory.feed_buffer_kib`.
//!
//! The daemon runs in this process, as in `change_feed.rs`, so the tests publish entities the
//! way a producer does and read the process's own resident memory.

mod candidate;
mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{
    Capability, Change, Command, EntityKind, FeedPosition, Hello, Request, RequestId, ServerFrame,
};
use common::GENEROUS;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

struct Home {
    /// Sockets only: a socket path under a deep directory exceeds the kernel's limit.
    runtime: tempfile::TempDir,
    state: tempfile::TempDir,
    database: Mutex<Option<Arc<cockpit_turso::Database>>>,
    /// What one subscriber may have waiting: the owner's `memory.feed_buffer_kib`.
    feed_buffer_bytes: u32,
}

/// The default `memory.feed_buffer_kib`, in bytes.
const DEFAULT_FEED_BUFFER_BYTES: u32 = 1024 * 1024;

impl Home {
    /// `state_in` holds the state database, which a measurement grows by tens of megabytes.
    fn new(state_in: Option<&Path>, feed_buffer_bytes: u32) -> Home {
        Home {
            feed_buffer_bytes,
            runtime: tempfile::tempdir().unwrap(),
            state: match state_in {
                Some(dir) => tempfile::tempdir_in(dir).unwrap(),
                None => tempfile::tempdir().unwrap(),
            },
            database: Mutex::new(None),
        }
    }

    async fn start(&self) -> Daemon {
        let options = Options {
            runtime_dir: self.runtime.path().join("run/cockpit"),
            state_dir: self.state.path().join("state/cockpit"),
            limits: Limits::default(),
        };
        Instance::acquire(options)
            .expect("no other daemon holds this home")
            .start(
                cockpit_daemon::Wiring {
                    feed_buffer_bytes: self.feed_buffer_bytes,
                    // The private tmux socket goes with the sockets, for the same reason.
                    ..common::wiring_in(self.runtime.path(), Vec::new())
                },
                |capabilities| {
                    *self.database.lock().unwrap() = Some(Arc::clone(&capabilities.database));
                    Arc::new(Routes::new(
                        capabilities,
                        cockpit_config::Environment::default(),
                    ))
                },
            )
            .await
            .expect("the daemon starts")
    }

    /// What a producer does: publish each entity's state in the commit that changed them,
    /// as the daemon publishes every changed session in one.
    async fn publish(&self, states: Vec<(String, Map<String, Value>)>) {
        let database = Arc::clone(
            self.database
                .lock()
                .unwrap()
                .as_ref()
                .expect("a daemon runs"),
        );
        let write = database.write().await.unwrap();
        for (id, state) in &states {
            cockpit_messaging::publish(&write, "attempt", id, state)
                .await
                .unwrap();
        }
        write.commit().await.unwrap();
    }

    fn socket_path(&self) -> PathBuf {
        self.runtime
            .path()
            .join("run/cockpit")
            .join(cockpit_config::DAEMON_SOCKET)
    }
}

/// A subscriber that follows the feed from a snapshot. It reads only when asked, so one that
/// is never asked is a frozen viewer: its connection stays open and nothing is read.
struct Subscriber {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    _writer: tokio::net::unix::OwnedWriteHalf,
}

impl Subscriber {
    async fn subscribe(home: &Home) -> Subscriber {
        let stream = UnixStream::connect(home.socket_path()).await.unwrap();
        let (read_half, mut writer) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let hello = Hello {
            protocol_versions: vec![cockpit_protocol::ProtocolVersion(1)],
            capabilities: vec![Capability::ChangeFeed],
        };
        send(&mut writer, &serde_json::to_value(hello).unwrap()).await;
        let welcome = read_value(&mut reader).await;
        assert_eq!(welcome["capabilities"], json!(["change-feed"]), "{welcome}");
        let request = Request {
            id: RequestId(1),
            command: Command::Subscribe {
                from: FeedPosition::Snapshot,
            },
        };
        send(&mut writer, &serde_json::to_value(request).unwrap()).await;
        let answer = read_value(&mut reader).await;
        assert_eq!(
            answer["outcome"]["ok"]["kind"],
            json!("subscribed"),
            "{answer}"
        );
        Subscriber {
            reader,
            _writer: writer,
        }
    }

    /// The round of each published state in the next batch that carries one. The snapshot of
    /// an empty daemon carries view stamps only and is skipped.
    async fn next_rounds(&mut self) -> Vec<u64> {
        loop {
            let frame = serde_json::from_value(read_value(&mut self.reader).await)
                .expect("the daemon sends server frames");
            let ServerFrame::Changes(batch) = frame else {
                panic!("not a change batch: {frame:?}");
            };
            let rounds: Vec<u64> = batch
                .changes
                .into_iter()
                .filter(|event| event.entity.kind == EntityKind::Attempt)
                .map(|event| match event.change {
                    Change::Upsert { state } => state["round"].as_u64().expect("a round"),
                    Change::Tombstone => panic!("nothing is retracted here"),
                })
                .collect();
            if !rounds.is_empty() {
                return rounds;
            }
        }
    }
}

async fn send(writer: &mut tokio::net::unix::OwnedWriteHalf, value: &Value) {
    let mut line = serde_json::to_vec(value).unwrap();
    line.push(b'\n');
    writer.write_all(&line).await.unwrap();
}

async fn read_value(reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>) -> Value {
    let mut line = String::new();
    tokio::time::timeout(GENEROUS, reader.read_line(&mut line))
        .await
        .expect("the daemon answers")
        .unwrap();
    serde_json::from_str(&line).expect("the daemon sends JSON")
}

/// An entity state of about `bytes` bytes that differs each `round`.
fn state_of(bytes: usize, entity: usize, round: usize) -> Map<String, Value> {
    let stamp = format!("{entity:04}-{round:04}-");
    let mut text = stamp.repeat(bytes / stamp.len() + 1);
    text.truncate(bytes);
    match json!({ "round": round, "text": text }) {
        Value::Object(state) => state,
        other => panic!("not an object: {other}"),
    }
}

/// This process's resident memory, in KiB.
fn resident_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("Linux reports its status");
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kib| kib.parse().ok())
        .expect("the status names the resident memory")
}

/// Publishes `ENTITIES` states of `STATE_BYTES` each in one commit, `ROUNDS` times over, and
/// answers how much the process's resident memory grew.
async fn growth_under_load(home: &Home) -> u64 {
    const ENTITIES: usize = 16;
    const STATE_BYTES: usize = 32 * 1024;
    const ROUNDS: usize = 20;
    let before = resident_kib();
    for round in 0..ROUNDS {
        let states = (0..ENTITIES)
            .map(|entity| {
                (
                    format!("entity-{entity}"),
                    state_of(STATE_BYTES, entity, round),
                )
            })
            .collect();
        home.publish(states).await;
        // Each subscriber's stream wakes between rounds, so every round is a batch of its own.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    resident_kib().saturating_sub(before)
}

/// The before/after measurement of #406's feed bound; not a pass or fail.
///
///     MEMORY_BOUNDS_STATE_DIR=<dir on a real disk> cargo nextest run -p cockpit-daemon \
///         --test memory_bounds --run-ignored only --no-capture
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: it prints the memory stalled subscribers hold"]
async fn measure_what_stalled_subscribers_hold() {
    const SUBSCRIBERS: usize = 24;
    let state_dir = std::env::var_os("MEMORY_BOUNDS_STATE_DIR").map(PathBuf::from);

    let control_home = Home::new(state_dir.as_deref(), DEFAULT_FEED_BUFFER_BYTES);
    let control = control_home.start().await;
    let without_subscribers = growth_under_load(&control_home).await;
    drop(control);

    let home = Home::new(state_dir.as_deref(), DEFAULT_FEED_BUFFER_BYTES);
    let _daemon = home.start().await;
    let mut subscribers = Vec::new();
    for _ in 0..SUBSCRIBERS {
        subscribers.push(Subscriber::subscribe(&home).await);
    }
    let with_subscribers = growth_under_load(&home).await;

    let held = with_subscribers.saturating_sub(without_subscribers);
    println!(
        "stalled_subscribers={SUBSCRIBERS} growth_without_kib={without_subscribers} \
         growth_with_kib={with_subscribers} held_kib={held} held_per_subscriber_kib={}",
        held / SUBSCRIBERS as u64
    );
}

/// One entity's state, far larger than the buffer the tests below give a subscriber and than
/// what the kernel takes for a socket nobody reads.
const LARGE_STATE_BYTES: usize = 1024 * 1024;
const SMALL_BUFFER_BYTES: u32 = 64 * 1024;

async fn publish_round(home: &Home, round: usize) {
    home.publish(vec![(
        "entity".to_owned(),
        state_of(LARGE_STATE_BYTES, 0, round),
    )])
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subscriber_that_stopped_reading_is_owed_its_buffer_not_every_commit() {
    const ROUNDS: usize = 20;
    let home = Home::new(None, SMALL_BUFFER_BYTES);
    let _daemon = home.start().await;
    let mut subscriber = Subscriber::subscribe(&home).await;

    for round in 0..ROUNDS {
        publish_round(&home, round).await;
        // The stream wakes between rounds, so each round would be a batch of its own.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The buffer holds its bytes and at most one batch beyond them, and the commits since
    // are one more. Without the bound the eight batches of the channel wait too.
    let mut batches = 0;
    loop {
        let rounds = subscriber.next_rounds().await;
        batches += 1;
        if rounds == [ROUNDS as u64 - 1] {
            break;
        }
    }
    assert!(batches <= 4, "{batches} batches waited for the subscriber");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_larger_than_the_buffer_still_reaches_its_subscriber() {
    let home = Home::new(None, SMALL_BUFFER_BYTES);
    let _daemon = home.start().await;
    let mut subscriber = Subscriber::subscribe(&home).await;

    publish_round(&home, 7).await;

    assert_eq!(
        subscriber.next_rounds().await,
        [7],
        "the batch passes alone"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_written_batch_gives_its_room_back_to_the_next() {
    let home = Home::new(None, SMALL_BUFFER_BYTES);
    let _daemon = home.start().await;
    let mut subscriber = Subscriber::subscribe(&home).await;

    for round in 0..5 {
        publish_round(&home, round).await;
        assert_eq!(subscriber.next_rounds().await, [round as u64]);
    }
}
