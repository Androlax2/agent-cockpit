//! Scheduling at the daemon's socket (#43): `queue-dispatch` places a queued item on an idle
//! session of its project, and `queue-explain` answers why the rest waits.
//!
//! The daemon runs in this process with the fake provider, a real state database, real
//! processes and a private tmux server. Its periodic pass is left at the fixture's hour and
//! its idle window at zero, so each pass is the test's own.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{Command, ErrorCode, Reply, SessionView, TaskStatusView};
use cockpit_provider_api::ProviderName;
use cockpit_provider_api::fake::{FakeLaunch as AgentLaunch, FakeProvider};
use cockpit_sessions::AgentState;
use common::{GENEROUS, refused};

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
            wiring.scheduling.idle_window = Duration::ZERO;
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

    /// A session of the project whose agent reported `state` from its real process tree.
    async fn session_reporting(
        &self,
        client: &Connection,
        root: &Path,
        state: AgentState,
    ) -> SessionView {
        let started = new_session(client, root).await;
        let launch = self.first_launch_of(&started.id).await;
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
        started
    }

    async fn first_launch_of(&self, session: &str) -> AgentLaunch {
        let deadline = Instant::now() + GENEROUS;
        loop {
            if let Some(launch) = self.launches_of(session).into_iter().next() {
                return launch;
            }
            assert!(
                Instant::now() < deadline,
                "{session} never launched its agent"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
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

async fn new_session(client: &Connection, dir: &Path) -> SessionView {
    match request(
        client,
        Command::SessionNew {
            dir: dir.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
        },
    )
    .await
    {
        Ok(Reply::Session { session }) => *session,
        other => panic!("session-new: {other:?}"),
    }
}

/// A queued item of project `app`; answers its number.
async fn queued_item(client: &Connection, title: &str) -> i64 {
    let added = request(
        client,
        Command::TaskAdd {
            launch_token: None,
            project: Some("app".to_owned()),
            title: title.to_owned(),
            notes: String::new(),
            item_kind: None,
            priority: None,
            after: Vec::new(),
        },
    )
    .await;
    let Ok(Reply::Task { task }) = added else {
        panic!("task-add: {added:?}");
    };
    request(
        client,
        Command::TaskQueue {
            item: task.id,
            queued: true,
        },
    )
    .await
    .unwrap();
    task.id
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_dispatch_places_a_queued_item_on_the_idle_session_of_its_project() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let idle = home
        .session_reporting(&client, &root, AgentState::Waiting)
        .await;
    let item = queued_item(&client, "Fix the login").await;

    let dispatched = request(&client, Command::QueueDispatch).await;

    let Ok(Reply::QueueDispatched {
        placed, waiting, ..
    }) = dispatched
    else {
        panic!("queue-dispatch: {dispatched:?}");
    };
    assert_eq!(placed.len(), 1, "{placed:?}");
    assert_eq!(
        (placed[0].item, placed[0].session.as_str()),
        (item, idle.id.as_str())
    );
    assert_eq!(waiting, 0);
    let shown = request(
        &client,
        Command::TaskShow {
            launch_token: None,
            item,
        },
    )
    .await;
    let Ok(Reply::Task { task }) = shown else {
        panic!("task-show: {shown:?}");
    };
    assert_eq!(task.status, TaskStatusView::Doing);
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_explain_names_the_working_session_that_cannot_take_the_item() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let working = home
        .session_reporting(&client, &root, AgentState::Working)
        .await;
    let item = queued_item(&client, "Fix the login").await;
    request(&client, Command::QueueDispatch).await.unwrap();

    let explained = request(&client, Command::QueueExplain { item: Some(item) }).await;

    let Ok(Reply::QueueExplanations { explanations }) = explained else {
        panic!("queue-explain: {explained:?}");
    };
    assert_eq!(explanations.len(), 1, "{explanations:?}");
    let explanation = &explanations[0];
    assert_eq!(explanation.reason, "no-session-free");
    assert_eq!(explanation.sessions.len(), 1);
    assert_eq!(explanation.sessions[0].session, working.id);
    assert_eq!(explanation.sessions[0].why, "its agent is working");
    assert_eq!(explanation.action_by, "cockpit");
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_explain_of_an_item_the_last_pass_did_not_explain_is_not_found() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;

    let error = refused(request(&client, Command::QueueExplain { item: Some(42) }).await);

    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(
        error.message.starts_with("item 42 has no explanation"),
        "{error:?}"
    );
}
