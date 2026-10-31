//! Issue #26's overload fixture (decided in #108): the real socket, handshake, lanes and
//! panic containment, with a fixture handler that stands in for bulk work. No protocol
//! command does bulk work yet, so the fixture gives `subscribe` that role; `ping` stays the
//! daemon's own control command.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{
    Daemon, Handler, Instance, LaneLimits, Limits, Options, Peer, Route, Routes, StartError,
    WorkClass,
};
use cockpit_protocol::{
    Command, CommitRevision, Cursor, DatabaseEpoch, ErrorCode, FeedPosition, MAX_FRAME_BYTES,
    Outcome, ProtocolError, Reply, Welcome,
};
use common::GENEROUS;

/// Report 03's no-stall bound; a control answer slower than this under load is a failure.
const CONTROL_DEADLINE: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
enum BulkBehavior {
    /// Blocks a blocking-pool thread until the test opens the gate.
    BlockUntilReleased,
    /// Panics inside the job.
    PanicInJob,
    /// Panics while routing, on the connection's own task.
    PanicWhileRouting,
    /// Answers with a reply larger than the frame limit.
    OversizedAnswer,
}

struct Fixture {
    behavior: BulkBehavior,
    started: AtomicUsize,
    gate: (Mutex<bool>, Condvar),
}

impl Fixture {
    fn new(behavior: BulkBehavior) -> Arc<Fixture> {
        Arc::new(Fixture {
            behavior,
            started: AtomicUsize::new(0),
            gate: (Mutex::new(false), Condvar::new()),
        })
    }

    fn release(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }

    async fn wait_until_started(&self, count: usize) {
        let deadline = Instant::now() + GENEROUS;
        while self.started.load(Ordering::SeqCst) < count {
            assert!(Instant::now() < deadline, "bulk jobs did not start");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

/// The handler is shared with the daemon; the test keeps its own `Arc` to the fixture.
struct FixtureHandler {
    fixture: Arc<Fixture>,
    routes: Routes,
}

impl Handler for FixtureHandler {
    fn route(&self, command: Command, welcome: &Welcome, peer: Peer) -> Route {
        if !matches!(command, Command::Subscribe { .. }) {
            return self.routes.route(command, welcome, peer);
        }
        let fixture = Arc::clone(&self.fixture);
        if let BulkBehavior::PanicWhileRouting = fixture.behavior {
            panic!("fixture: routing failed");
        }
        Route::Run {
            class: WorkClass::Bulk,
            job: Box::pin(async move {
                match fixture.behavior {
                    BulkBehavior::PanicInJob => panic!("fixture: the job failed"),
                    BulkBehavior::OversizedAnswer => {
                        return Outcome::Ok(Reply::Subscribed {
                            cursor: Cursor {
                                epoch: DatabaseEpoch("x".repeat(MAX_FRAME_BYTES)),
                                revision: CommitRevision(0),
                            },
                        });
                    }
                    _ => {}
                }
                tokio::task::spawn_blocking(move || {
                    fixture.started.fetch_add(1, Ordering::SeqCst);
                    let (released, opened) = &fixture.gate;
                    let mut released = released.lock().unwrap();
                    while !*released {
                        released = opened.wait(released).unwrap();
                    }
                })
                .await
                .expect("the blocking job finishes");
                Outcome::Ok(Reply::Subscribed {
                    cursor: Cursor {
                        epoch: DatabaseEpoch("fixture".to_owned()),
                        revision: CommitRevision(0),
                    },
                })
            }),
        }
    }
}

struct Running {
    daemon: Daemon,
    _home: tempfile::TempDir,
}

fn lane(workers: usize, waiting: usize) -> LaneLimits {
    LaneLimits { workers, waiting }
}

fn options_in(home: &tempfile::TempDir, bulk: LaneLimits) -> Options {
    Options {
        runtime_dir: home.path().join("run/cockpit"),
        state_dir: home.path().join("state/cockpit"),
        limits: Limits {
            bulk,
            ..Limits::default()
        },
    }
}

async fn start(fixture: &Arc<Fixture>, bulk: LaneLimits) -> Running {
    let home = tempfile::tempdir().unwrap();
    let daemon = Instance::acquire(options_in(&home, bulk))
        .unwrap()
        .start(|database| {
            Arc::new(FixtureHandler {
                fixture: Arc::clone(fixture),
                routes: Routes::new(
                    database,
                    cockpit_projects::Git::from_path(),
                    cockpit_config::Environment::default(),
                    cockpit_daemon::runner(Vec::new()).unwrap(),
                ),
            })
        })
        .await
        .unwrap();
    Running {
        daemon,
        _home: home,
    }
}

async fn connect(running: &Running) -> Arc<Connection> {
    Arc::new(
        Connection::open(running.daemon.socket_path(), GENEROUS)
            .await
            .unwrap(),
    )
}

fn subscribe() -> Command {
    Command::Subscribe {
        from: FeedPosition::Snapshot,
    }
}

fn error_code(result: &Result<Reply, ClientError>) -> Option<ErrorCode> {
    match result {
        Err(ClientError::Daemon(ProtocolError { code, .. })) => Some(code.clone()),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn control_requests_are_answered_while_bulk_work_saturates_its_workers() {
    let fixture = Fixture::new(BulkBehavior::BlockUntilReleased);
    let running = start(&fixture, lane(2, 1)).await;
    let bulk_client = connect(&running).await;
    let running_jobs: Vec<_> = (0..2)
        .map(|_| {
            let client = Arc::clone(&bulk_client);
            tokio::spawn(async move { client.request(subscribe(), GENEROUS).await })
        })
        .collect();
    fixture.wait_until_started(2).await;
    // One poll queues the waiting request's frame, so it reaches the daemon before the
    // overflow request on the same connection.
    let waiting_job = bulk_client.request(subscribe(), GENEROUS);
    tokio::pin!(waiting_job);
    tokio::select! {
        biased;
        early = &mut waiting_job => panic!("answered while the workers are held: {early:?}"),
        () = std::future::ready(()) => {}
    }

    let overflow = bulk_client.request(subscribe(), GENEROUS).await;
    let control_client = connect(&running).await;
    let mut slowest_control = Duration::ZERO;
    for client in [&bulk_client, &control_client].repeat(10) {
        let asked = Instant::now();
        let reply = client.request(Command::Ping, CONTROL_DEADLINE).await;
        slowest_control = slowest_control.max(asked.elapsed());
        assert_eq!(reply.unwrap(), Reply::Pong);
    }
    let started_while_saturated = fixture.started.load(Ordering::SeqCst);
    fixture.release();
    let mut finished = vec![waiting_job.await];
    for request in running_jobs {
        finished.push(request.await.unwrap());
    }

    assert_eq!(error_code(&overflow), Some(ErrorCode::Busy), "{overflow:?}");
    assert_eq!(started_while_saturated, 2, "only two bulk workers run");
    assert!(slowest_control < CONTROL_DEADLINE, "{slowest_control:?}");
    assert!(
        finished
            .iter()
            .all(|result| matches!(result, Ok(Reply::Subscribed { .. }))),
        "{finished:?}"
    );
    assert_eq!(
        fixture.started.load(Ordering::SeqCst),
        3,
        "the waiting job ran"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_that_panics_is_answered_internal_and_frees_its_worker() {
    let fixture = Fixture::new(BulkBehavior::PanicInJob);
    let running = start(&fixture, lane(1, 0)).await;
    let client = connect(&running).await;

    let mut codes = Vec::new();
    for _ in 0..3 {
        codes.push(error_code(&client.request(subscribe(), GENEROUS).await));
    }

    assert_eq!(
        codes,
        vec![Some(ErrorCode::Internal); 3],
        "never busy: the worker is freed"
    );
    assert_eq!(
        client.request(Command::Ping, GENEROUS).await.unwrap(),
        Reply::Pong
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panic_while_routing_ends_only_that_connection() {
    let fixture = Fixture::new(BulkBehavior::PanicWhileRouting);
    let running = start(&fixture, lane(1, 0)).await;
    let failing = connect(&running).await;
    let bystander = connect(&running).await;

    let failed = failing.request(subscribe(), GENEROUS).await;

    assert!(matches!(failed, Err(ClientError::Closed(_))), "{failed:?}");
    assert_eq!(
        bystander.request(Command::Ping, GENEROUS).await.unwrap(),
        Reply::Pong
    );
    let reconnected = connect(&running).await;
    assert_eq!(
        reconnected.request(Command::Ping, GENEROUS).await.unwrap(),
        Reply::Pong
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_over_the_frame_limit_is_replaced_by_an_internal_refusal() {
    let fixture = Fixture::new(BulkBehavior::OversizedAnswer);
    let running = start(&fixture, Limits::default().bulk).await;
    let client = connect(&running).await;

    let answer = client.request(subscribe(), GENEROUS).await;

    assert_eq!(error_code(&answer), Some(ErrorCode::Internal), "{answer:?}");
    assert_eq!(
        client.request(Command::Ping, GENEROUS).await.unwrap(),
        Reply::Pong
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_daemon_stops_accepting_connections() {
    let fixture = Fixture::new(BulkBehavior::BlockUntilReleased);
    let running = start(&fixture, Limits::default().bulk).await;
    let socket_path = running.daemon.socket_path().to_path_buf();

    drop(running.daemon);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let refused = Connection::open(&socket_path, Duration::from_secs(1)).await;
    assert!(
        matches!(refused, Err(ClientError::Connect { .. })),
        "{refused:?}"
    );
}

#[test]
fn limits_that_would_serve_nothing_stop_startup() {
    let home = tempfile::tempdir().unwrap();
    let mut options = options_in(&home, Limits::default().bulk);
    options.limits.requests_per_connection = 0;

    let refused = Instance::acquire(options);

    assert!(
        matches!(
            refused,
            Err(StartError::InvalidLimits(
                "requests_per_connection must be at least 1"
            ))
        ),
        "{refused:?}"
    );
}
