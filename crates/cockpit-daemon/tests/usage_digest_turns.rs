//! Time with the agent through the daemon (#700): a session's turn opens when its agent reports
//! `working` and ends when it reports anything else or the session stops running, and the
//! owner reads it back with `usage-digest`.
//!
//! Same setting as `finished_label.rs`: the daemon runs in this process with the fake provider,
//! a real state database, real processes and a private tmux server.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{Command, Reply, SessionView};
use cockpit_provider_api::ProviderName;
use cockpit_provider_api::fake::{FakeLaunch as AgentLaunch, FakeProvider};
use cockpit_sessions::AgentState;
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

    fn launches_of(&self, session: &str) -> Vec<AgentLaunch> {
        FakeProvider::launches(&self.path().join("agent.log"))
            .into_iter()
            .filter(|launch| launch.session == session)
            .collect()
    }

    async fn start(&self) -> Daemon {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let options = Options {
                runtime_dir: self.path().join("run/cockpit"),
                state_dir: self.path().join("state/cockpit"),
                limits: Limits::default(),
            };
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
                ..common::unsandboxed_settings()
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

    fn database(&self) -> Arc<cockpit_turso::Database> {
        self.database.lock().unwrap().clone().unwrap()
    }

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

    /// Reports `state` as the agent of `launch`, from its real process tree.
    async fn report_as_agent(&self, launch: &AgentLaunch, state: AgentState) {
        let database = self.database();
        let write = database.write().await.unwrap();
        let agent = cockpit_agent_channel::identity_of(u32::try_from(launch.pid).unwrap());
        let peer = cockpit_agent_channel::ancestry_of(&agent.unwrap());
        cockpit_sessions::report(
            &write,
            &peer,
            Some(launch.token.as_str()),
            cockpit_sessions::Report {
                state: Some(state),
                ..cockpit_sessions::Report::default()
            },
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
    }

    fn kill_every_agent(&self) {
        let _ = Process::new("tmux")
            .arg("-S")
            .arg(self.path().join("tmux.sock"))
            .arg("kill-server")
            .env_remove("TMUX")
            .output();
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        self.kill_every_agent();
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

async fn connect(daemon: &Daemon) -> Connection {
    Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap()
}

async fn request(client: &Connection, command: Command) -> Result<Reply, ClientError> {
    client.request(command, Duration::from_secs(60)).await
}

async fn session_reply(client: &Connection, command: Command) -> SessionView {
    match request(client, command).await {
        Ok(Reply::Session { session }) => *session,
        other => panic!("{other:?}"),
    }
}

async fn new_session(client: &Connection, dir: &Path) -> SessionView {
    session_reply(
        client,
        Command::SessionNew {
            dir: dir.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
            model: None,
            effort: None,
            compaction_threshold: None,
        },
    )
    .await
}

/// What the owner's digest says of the last two UTC days (a test that runs across midnight still
/// adds up): the turns and seconds with the agent, and the names of the projects with a turn.
#[derive(Debug, Default, PartialEq, Eq)]
struct Worked {
    sessions: u64,
    turns: u64,
    seconds_with_agent: u64,
    projects: Vec<String>,
}

async fn worked(client: &Connection) -> Worked {
    let reply = request(client, Command::UsageDigest { days: 2 }).await;
    let Ok(Reply::UsageDigest { digest }) = reply else {
        panic!("usage-digest: {reply:?}");
    };
    let mut worked = Worked::default();
    for day in &digest.days {
        worked.sessions += day.sessions;
        worked.turns += day.turns;
        worked.seconds_with_agent += day.seconds_with_agent;
        worked.projects.extend(
            day.projects
                .iter()
                .filter(|project| project.turns > 0)
                .map(|project| project.project.clone()),
        );
    }
    worked
}

async fn worked_once_a_turn_is_recorded(client: &Connection) -> Worked {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let seen = worked(client).await;
        if seen.turns > 0 || Instant::now() > deadline {
            return seen;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_the_agent_works_is_in_the_digest_under_its_project_once_it_ends() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    let launch = home.launches_of(&started.id).remove(0);
    assert_eq!(worked(&client).await, Worked::default());

    home.report_as_agent(&launch, AgentState::Working).await;
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    home.report_as_agent(&launch, AgentState::Waiting).await;

    let seen = worked_once_a_turn_is_recorded(&client).await;
    assert_eq!(seen.sessions, 1, "{seen:?}");
    assert_eq!(seen.turns, 1, "{seen:?}");
    assert_eq!(seen.projects, vec!["app".to_owned()], "{seen:?}");
    assert!(
        (2..=4).contains(&seen.seconds_with_agent),
        "a turn of about two seconds: {seen:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_still_working_when_its_session_ends_is_recorded_up_to_the_end() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    let launch = home.launches_of(&started.id).remove(0);
    home.report_as_agent(&launch, AgentState::Working).await;
    tokio::time::sleep(Duration::from_millis(1_100)).await;

    let ended = request(
        &client,
        Command::SessionEnd {
            session: started.id.clone(),
        },
    )
    .await;
    assert!(ended.is_ok(), "{ended:?}");

    let seen = worked_once_a_turn_is_recorded(&client).await;
    assert_eq!(seen.turns, 1, "{seen:?}");
    assert!(seen.seconds_with_agent >= 1, "{seen:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_survives_a_daemon_restart_while_the_agent_works() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    let launch = home.launches_of(&started.id).remove(0);
    home.report_as_agent(&launch, AgentState::Working).await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    drop(client);
    drop(daemon);
    home.database.lock().unwrap().take();

    let daemon = home.start().await;
    let client = connect(&daemon).await;
    home.report_as_agent(&launch, AgentState::Waiting).await;

    let seen = worked_once_a_turn_is_recorded(&client).await;
    assert_eq!(seen.turns, 1, "{seen:?}");
    assert!(seen.seconds_with_agent >= 1, "{seen:?}");
}
