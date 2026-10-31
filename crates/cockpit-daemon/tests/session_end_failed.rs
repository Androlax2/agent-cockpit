//! Issue #221 at the daemon's socket: `end` from `failed` or `launching` stops the
//! session's surviving agent before it records the session ended.
//!
//! Same setting as `session_robustness.rs`: the daemon runs in this process with the fake
//! provider, a real state database, real processes and a private tmux server.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{Command, Reply, SessionStateView, SessionView};
use cockpit_provider_api::ProviderName;
use cockpit_provider_api::fake::{FakeLaunch as AgentLaunch, FakeProvider};
use common::GENEROUS;

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

    fn path(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    fn launches(&self) -> Vec<AgentLaunch> {
        FakeProvider::launches(&self.path().join("agent.log"))
    }

    async fn start(&self) -> Daemon {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let provider = FakeProvider::new(
                ProviderName::new("fake").unwrap(),
                self.path().join("agent.log"),
            );
            let mut wiring = common::wiring_in(&self.path(), vec![Arc::new(provider)]);
            wiring.sessions = cockpit_sessions::Settings {
                stop: cockpit_processes::Escalation {
                    grace: Duration::from_secs(2),
                    kill_wait: Duration::from_secs(5),
                },
                ..cockpit_sessions::Settings::default()
            };
            let options = Options {
                runtime_dir: self.path().join("run/cockpit"),
                state_dir: self.path().join("state/cockpit"),
                limits: Limits::default(),
            };
            let started = match Instance::acquire(options) {
                Ok(instance) => {
                    instance
                        .start(wiring, |capabilities| {
                            *self.database.lock().unwrap() =
                                Some(Arc::clone(&capabilities.database));
                            Arc::new(Routes::new(
                                capabilities,
                                cockpit_config::Environment {
                                    home: Some(self.path().join("home").into()),
                                    ..cockpit_config::Environment::default()
                                },
                            ))
                        })
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

    /// Rewrites the session's stored state: the fixture for what a failed restart or a
    /// crash mid-launch leaves behind while its agent still runs.
    async fn store_state(&self, assignment: &str, session: &str) {
        let database = self.database.lock().unwrap().clone().unwrap();
        let write = database.write().await.unwrap();
        write
            .execute(
                &format!("UPDATE sessions SET {assignment} WHERE id = ?1"),
                [session],
            )
            .await
            .unwrap();
        write.commit().await.unwrap();
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
        .await;
        root
    }
}

impl Drop for Home {
    fn drop(&mut self) {
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
    assert!(output.status.success(), "git {arguments:?}");
}

async fn request(client: &Connection, command: Command) -> Reply {
    client
        .request(command.clone(), Duration::from_secs(60))
        .await
        .unwrap_or_else(|error| panic!("{command:?}: {error}"))
}

async fn session_reply(client: &Connection, command: Command) -> SessionView {
    match request(client, command.clone()).await {
        Reply::Session { session } => *session,
        other => panic!("{command:?}: {other:?}"),
    }
}

fn is_alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
        && !std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .unwrap_or_default()
            .contains(") Z ")
}

/// A running session of the fake provider, with its stored state then rewritten to
/// `assignment` while its agent keeps running.
async fn session_left_in(home: &Home, client: &Connection, assignment: &str) -> SessionView {
    let root = home.project(client).await;
    let started = session_reply(
        client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
        },
    )
    .await;
    home.store_state(assignment, &started.id).await;
    started
}

/// Ends a session left in `assignment` while its agent runs, and answers whether that
/// agent is still alive afterwards.
async fn end_while_its_agent_runs(assignment: &str) -> (SessionView, bool) {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let session = session_left_in(&home, &client, assignment).await;
    let agent = home.launches()[0].pid;
    assert!(is_alive(agent));

    let ended = session_reply(
        &client,
        Command::SessionEnd {
            session: session.id.clone(),
        },
    )
    .await;
    (ended, is_alive(agent))
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_failed_session_whose_agent_survived_stops_that_agent_first() {
    let (ended, agent_alive) =
        end_while_its_agent_runs("state = 'failed', reason = 'its restart failed'").await;

    assert_eq!(ended.state, SessionStateView::Ended);
    assert!(!agent_alive);
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_session_left_launching_stops_its_agent_first() {
    let (ended, agent_alive) = end_while_its_agent_runs("state = 'launching'").await;

    assert_eq!(ended.state, SessionStateView::Ended);
    assert!(!agent_alive);
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_failed_session_whose_agent_is_already_gone_ends_it() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let session = session_left_in(&home, &client, "state = 'failed'").await;
    let agent = home.launches()[0].pid;
    Process::new("kill")
        .arg(agent.to_string())
        .status()
        .unwrap();
    let deadline = Instant::now() + GENEROUS;
    while is_alive(agent) {
        assert!(Instant::now() < deadline, "the agent did not exit");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let ended = session_reply(
        &client,
        Command::SessionEnd {
            session: session.id.clone(),
        },
    )
    .await;

    assert_eq!(ended.state, SessionStateView::Ended);
    assert!(!is_alive(agent));
}
