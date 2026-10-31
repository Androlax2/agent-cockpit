//! Issue #217, report 15 scenario 3 ("stop requests served"): with the bulk and the control
//! lanes full, the owner's stops still get a worker of their own. The real socket, lanes and
//! session and resource routes, with a fixture handler whose held jobs fill the lanes:
//! `subscribe` holds a bulk worker, `agent-reports` a control worker and `provider-list` a
//! stop worker. Sessions run the fake provider's agent in a private tmux server.
//!
//! Issue #230: a stop of a session whose launch still runs answers `conflict` at once
//! instead of holding a stop worker until the launch finishes. The fixture provider holds a
//! launch while its gate is closed.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{
    Daemon, Handler, Instance, LaneLimits, Limits, Options, Peer, Route, Routes, WorkClass,
};
use cockpit_protocol::{
    Command, ErrorCode, FeedPosition, Outcome, ProtocolError, Reply, SessionStateView, SessionView,
    Welcome,
};
use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{
    Capabilities, LaunchCommand, LaunchRequest, Provider, ProviderError, ProviderName,
};
use common::GENEROUS;
use tokio::sync::Semaphore;

/// One worker and no waiting place in the lanes the fixture fills, so one held job fills a
/// lane and the next request in it is answered `busy`.
const ONE_PLACE: LaneLimits = LaneLimits {
    workers: 1,
    waiting: 0,
};

/// Jobs the fixture holds until the test releases them.
struct Held {
    started: AtomicUsize,
    gate: Semaphore,
}

impl Held {
    fn job(self: &Arc<Self>, class: WorkClass) -> Route {
        let held = Arc::clone(self);
        Route::Run {
            class,
            job: Box::pin(async move {
                held.started.fetch_add(1, Ordering::SeqCst);
                let _released = held.gate.acquire().await.unwrap();
                Outcome::Ok(Reply::Pong)
            }),
        }
    }

    async fn wait_until_started(&self, count: usize) {
        let deadline = Instant::now() + GENEROUS;
        while self.started.load(Ordering::SeqCst) < count {
            assert!(Instant::now() < deadline, "the held jobs did not start");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn release(&self) {
        self.gate.add_permits(Semaphore::MAX_PERMITS);
    }
}

/// Launches the fixture provider holds while the gate is closed, as a provider still
/// starting up would hold its session's launch. Open unless a test closes it.
#[derive(Default)]
struct LaunchGate {
    is_closed: AtomicBool,
    held: AtomicUsize,
}

impl LaunchGate {
    async fn wait_until_held(&self) {
        let deadline = Instant::now() + GENEROUS;
        while self.held.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "no launch reached the gate");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

/// The fake provider, whose launches wait at the gate. Blocking in `launch` breaks the
/// provider contract on purpose: it holds a launch, and with it the session's lock, for as
/// long as the test wants, on one of the runtime's workers.
struct GatedProvider {
    inner: FakeProvider,
    gate: Arc<LaunchGate>,
}

impl Provider for GatedProvider {
    fn name(&self) -> &ProviderName {
        self.inner.name()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        self.gate.held.fetch_add(1, Ordering::SeqCst);
        while self.gate.is_closed.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.gate.held.fetch_sub(1, Ordering::SeqCst);
        self.inner.launch(request)
    }
}

struct FixtureHandler {
    held: Arc<Held>,
    routes: Routes,
}

impl Handler for FixtureHandler {
    fn route(&self, command: Command, welcome: &Welcome, peer: Peer) -> Route {
        match command {
            Command::Subscribe { .. } => self.held.job(WorkClass::Bulk),
            Command::AgentReports => self.held.job(WorkClass::Control),
            Command::ProviderList => self.held.job(WorkClass::Stop),
            command => self.routes.route(command, welcome, peer),
        }
    }
}

struct Home {
    _dir: tempfile::TempDir,
    /// `_dir`, canonical: the agent and tmux paths the daemon records are canonical.
    root: PathBuf,
    held: Arc<Held>,
    launches: Arc<LaunchGate>,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::tempdir().unwrap();
        Home {
            root: dir.path().canonicalize().unwrap(),
            _dir: dir,
            held: Arc::new(Held {
                started: AtomicUsize::new(0),
                gate: Semaphore::new(0),
            }),
            launches: Arc::default(),
        }
    }

    fn path(&self) -> PathBuf {
        self.root.clone()
    }

    async fn start(&self, limits: Limits) -> Daemon {
        let mut wiring = common::wiring_in(
            &self.path(),
            vec![Arc::new(GatedProvider {
                inner: FakeProvider::new(
                    ProviderName::new("fake").unwrap(),
                    self.path().join("agent.log"),
                ),
                gate: Arc::clone(&self.launches),
            })],
        );
        wiring.sessions.stop = cockpit_processes::Escalation {
            grace: Duration::from_secs(2),
            kill_wait: Duration::from_secs(5),
        };
        let options = Options {
            runtime_dir: self.path().join("run/cockpit"),
            state_dir: self.path().join("state/cockpit"),
            limits,
        };
        Instance::acquire(options)
            .unwrap()
            .start(wiring, |capabilities| {
                Arc::new(FixtureHandler {
                    held: Arc::clone(&self.held),
                    routes: Routes::new(
                        capabilities,
                        cockpit_config::Environment {
                            home: Some(self.path().join("home").into()),
                            ..cockpit_config::Environment::default()
                        },
                    ),
                })
            })
            .await
            .unwrap()
    }

    /// A Git project registered through the daemon, returning its root.
    async fn project(&self, client: &Connection) -> PathBuf {
        let root = self.path().join("projects/app");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("README"), "fixture\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "--quiet", "-m", "initial"]);
        request(
            client,
            Command::ProjectAdd {
                path: root.display().to_string(),
                name: None,
            },
        )
        .await
        .unwrap();
        root
    }

    /// Fills the bulk and the control lanes with held jobs, and proves both are full.
    async fn fill_bulk_and_control(&self, client: &Arc<Connection>) {
        for command in [subscribe(), Command::AgentReports] {
            let client = Arc::clone(client);
            tokio::spawn(async move { client.request(command, GENEROUS).await });
        }
        self.held.wait_until_started(2).await;
        assert_busy_in(request(client, subscribe()).await, "bulk");
        assert_busy_in(request(client, Command::Ping).await, "control");
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        self.held.release();
        self.launches.is_closed.store(false, Ordering::SeqCst);
        let _ = Process::new("tmux")
            .arg("-S")
            .arg(self.path().join("tmux.sock"))
            .arg("kill-server")
            .env_remove("TMUX")
            .output();
    }
}

fn git(dir: &Path, arguments: &[&str]) {
    let output = Process::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.com"])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn one_place_in_bulk_and_control() -> Limits {
    Limits {
        control: ONE_PLACE,
        bulk: ONE_PLACE,
        ..Limits::default()
    }
}

async fn connect(daemon: &Daemon) -> Arc<Connection> {
    Arc::new(
        Connection::open(daemon.socket_path(), GENEROUS)
            .await
            .unwrap(),
    )
}

async fn request(client: &Connection, command: Command) -> Result<Reply, ClientError> {
    client.request(command, Duration::from_secs(60)).await
}

fn subscribe() -> Command {
    Command::Subscribe {
        from: FeedPosition::Snapshot,
    }
}

/// Asserts `answer` is the `busy` refusal of `lane`.
fn assert_busy_in(answer: Result<Reply, ClientError>, lane: &str) {
    let error = common::refused(answer);
    assert_eq!(error.code, ErrorCode::Busy, "{error:?}");
    assert!(
        error
            .message
            .starts_with(&format!("the {lane} lane is full")),
        "{:?}",
        error.message
    );
}

/// Starts a session, fills the bulk and control lanes, then sends the stop `stop_of` makes
/// for that session; returns the session as the stop left it.
async fn stop_a_session_while_full(stop_of: fn(String) -> Command) -> SessionView {
    let home = Home::new();
    let daemon = home.start(one_place_in_bulk_and_control()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = match request(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
        },
    )
    .await
    {
        Ok(Reply::Session { session }) => session,
        other => panic!("session-new: {other:?}"),
    };
    home.fill_bulk_and_control(&client).await;

    match request(&client, stop_of(started.id)).await {
        Ok(Reply::Session { session }) => *session,
        other => panic!("the stop was not served: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_session_is_served_while_the_bulk_and_control_lanes_are_full() {
    let ended = stop_a_session_while_full(|session| Command::SessionEnd { session }).await;

    assert_eq!(ended.state, SessionStateView::Ended);
}

#[tokio::test(flavor = "multi_thread")]
async fn putting_a_session_to_sleep_is_served_while_the_bulk_and_control_lanes_are_full() {
    let asleep = stop_a_session_while_full(|session| Command::SessionSleep { session }).await;

    assert_eq!(asleep.state, SessionStateView::Asleep);
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_an_execution_is_served_while_the_bulk_and_control_lanes_are_full() {
    let home = Home::new();
    let daemon = home.start(one_place_in_bulk_and_control()).await;
    let client = connect(&daemon).await;
    home.fill_bulk_and_control(&client).await;

    let answer = request(&client, Command::ResourceStop { reservation: 999 }).await;

    let error = common::refused(answer);
    assert_ne!(error.code, ErrorCode::Busy, "{error:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_stop_lane_answers_busy_naming_the_stop_lane() {
    let home = Home::new();
    let daemon = home
        .start(Limits {
            stop: ONE_PLACE,
            ..Limits::default()
        })
        .await;
    let client = connect(&daemon).await;
    let held_client = Arc::clone(&client);
    tokio::spawn(async move { held_client.request(Command::ProviderList, GENEROUS).await });
    home.held.wait_until_started(1).await;

    let answer = request(
        &client,
        Command::SessionEnd {
            session: "s-unknown".to_owned(),
        },
    )
    .await;

    assert_busy_in(answer, "stop");
}

/// Starts a session whose launch the gate holds, in a daemon whose stop lane has one worker
/// and no waiting place; returns the session's id and the pending `session-new` reply.
async fn a_held_launch(
    home: &Home,
) -> (
    Daemon,
    Arc<Connection>,
    String,
    tokio::task::JoinHandle<Result<Reply, ClientError>>,
) {
    let daemon = home
        .start(Limits {
            stop: ONE_PLACE,
            ..Limits::default()
        })
        .await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    home.launches.is_closed.store(true, Ordering::SeqCst);
    let starting = {
        let client = Arc::clone(&client);
        tokio::spawn(async move {
            request(
                &client,
                Command::SessionNew {
                    dir: root.display().to_string(),
                    agent: Some("fake".to_owned()),
                    prompt: None,
                    clone: None,
                },
            )
            .await
        })
    };
    home.launches.wait_until_held().await;
    let id = match request(&client, Command::SessionList { ended: false }).await {
        Ok(Reply::Sessions { sessions }) if sessions.len() == 1 => sessions[0].id.clone(),
        other => panic!("session-list: {other:?}"),
    };
    (daemon, client, id, starting)
}

/// Sends the stop `stop_of` makes for a session whose launch is held, twice: with one stop
/// worker, the second is answered `busy` if the first still holds it. Returns the second
/// answer's refusal.
async fn stop_a_launching_session_twice(stop_of: fn(String) -> Command) -> ProtocolError {
    let home = Home::new();
    let (_daemon, client, id, _starting) = a_held_launch(&home).await;

    let first = common::refused(request(&client, stop_of(id.clone())).await);
    assert_eq!(first.code, ErrorCode::Conflict, "{first:?}");
    common::refused(request(&client, stop_of(id)).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ending_a_launching_session_answers_conflict_without_holding_a_stop_worker() {
    let error = stop_a_launching_session_twice(|session| Command::SessionEnd { session }).await;

    assert_eq!(error.code, ErrorCode::Conflict, "{error:?}");
    assert!(
        error.message.contains("is launching its agent"),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn putting_a_launching_session_to_sleep_answers_conflict_without_holding_a_stop_worker() {
    let error = stop_a_launching_session_twice(|session| Command::SessionSleep { session }).await;

    assert_eq!(error.code, ErrorCode::Conflict, "{error:?}");
    assert!(
        error.message.contains("is launching its agent"),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_refused_an_end_mid_launch_ends_once_its_launch_finishes() {
    let home = Home::new();
    let (_daemon, client, id, starting) = a_held_launch(&home).await;
    let refused = common::refused(
        request(
            &client,
            Command::SessionEnd {
                session: id.clone(),
            },
        )
        .await,
    );
    assert_eq!(refused.code, ErrorCode::Conflict, "{refused:?}");

    home.launches.is_closed.store(false, Ordering::SeqCst);
    match starting.await.unwrap() {
        Ok(Reply::Session { session }) => assert_eq!(session.state, SessionStateView::Running),
        other => panic!("session-new: {other:?}"),
    }
    let ended = request(&client, Command::SessionEnd { session: id }).await;

    match ended {
        Ok(Reply::Session { session }) => assert_eq!(session.state, SessionStateView::Ended),
        other => panic!("session-end: {other:?}"),
    }
}
