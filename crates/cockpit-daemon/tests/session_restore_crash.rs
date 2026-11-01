//! Reboot restore after a daemon crash, at the daemon's start (#251, #252, #256): what a
//! crash or a broken state database leaves midway through a stop or a launch is finished,
//! reported or refused there, before any request is accepted.
//!
//! Same setting as `sessions.rs`: the daemon runs in this process with the fake provider, a
//! real state database, real processes and a private tmux server. Each fixture rewrites the
//! stored state the way the interrupted step leaves it.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes, StartError};
use cockpit_processes::{EndReason, ExecutionId, LaunchContainment, Phase, Supervisor};
use cockpit_protocol::{Command, LaunchKindView, Reply, SessionStateView, SessionView};
use cockpit_provider_api::ProviderName;
use cockpit_provider_api::fake::{FakeLaunch as AgentLaunch, FakeProvider};
use common::GENEROUS;

struct Home {
    dir: tempfile::TempDir,
    /// The state database of the running daemon, for the fixtures.
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

    fn options(&self) -> Options {
        Options {
            runtime_dir: self.path().join("run/cockpit"),
            state_dir: self.path().join("state/cockpit"),
            limits: Limits::default(),
        }
    }

    fn launches(&self) -> Vec<AgentLaunch> {
        FakeProvider::launches(&self.path().join("agent.log"))
    }

    /// Waits until the fake agent logged `count` launches: a test waits for a launch
    /// before it stops that agent or restarts the daemon (#231).
    async fn wait_for_launches(&self, count: usize) -> Vec<AgentLaunch> {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let launches = self.launches();
            if launches.len() >= count {
                return launches;
            }
            assert!(Instant::now() < deadline, "the agent logged {launches:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn start(&self) -> Daemon {
        self.try_start()
            .await
            .unwrap_or_else(|error| panic!("the daemon did not start: {error}"))
    }

    /// Starts a daemon on this home's state. A daemon dropped just before releases its
    /// instance lock and database once its aborted tasks are gone, so only those two
    /// refusals are retried; any other start error is answered.
    async fn try_start(&self) -> Result<Daemon, StartError> {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let provider = FakeProvider::new(
                ProviderName::new("fake").unwrap(),
                self.path().join("agent.log"),
            );
            let mut wiring = common::wiring_in(&self.path(), vec![Arc::new(provider)]);
            wiring.sessions.stop = cockpit_processes::Escalation {
                grace: Duration::from_secs(2),
                kill_wait: Duration::from_secs(5),
            };
            let started = match Instance::acquire(self.options()) {
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
                Err(StartError::AlreadyRunning { .. } | StartError::Database(_))
                    if Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                started => return started,
            }
        }
    }

    fn database(&self) -> Arc<cockpit_turso::Database> {
        self.database.lock().unwrap().clone().unwrap()
    }

    /// Runs `sql` with `session` as `?1` in one transaction on the running daemon's state.
    async fn rewrite(&self, sql: &str, session: &str) {
        let database = self.database();
        let write = database.write().await.unwrap();
        write.execute(sql, [session]).await.unwrap();
        write.commit().await.unwrap();
    }

    /// Drops `table` from the running daemon's state: every later write to it fails.
    async fn drop_table(&self, table: &str) {
        let database = self.database();
        let write = database.write().await.unwrap();
        write
            .execute(&format!("DROP TABLE {table}"), ())
            .await
            .unwrap();
        write.commit().await.unwrap();
    }

    /// Stops `daemon` as a crash would leave things: its agents keep running.
    fn stop(&self, daemon: Daemon) {
        drop(daemon);
        self.database.lock().unwrap().take();
    }

    /// The tmux server and every agent in it are gone.
    fn lose_every_agent(&self) {
        let _ = Process::new("tmux")
            .arg("-S")
            .arg(self.path().join("tmux.sock"))
            .arg("kill-server")
            .env_remove("TMUX")
            .output();
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

    /// Starts a daemon and one session in a project, and waits for its agent's launch.
    async fn running_session(&self) -> (Daemon, SessionView) {
        let daemon = self.start().await;
        let client = connect(&daemon).await;
        let root = self.project(&client).await;
        let started = session_reply(
            &client,
            Command::SessionNew {
                dir: root.display().to_string(),
                agent: Some("fake".to_owned()),
                prompt: None,
                clone: None,
            },
        )
        .await;
        self.wait_for_launches(1).await;
        (daemon, started)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        self.lose_every_agent();
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

async fn connect(daemon: &Daemon) -> Connection {
    Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap()
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

async fn listed(client: &Connection, id: &str) -> SessionView {
    match request(client, Command::SessionList { ended: true }).await {
        Reply::Sessions { sessions } => sessions
            .into_iter()
            .find(|session| session.id == id)
            .unwrap_or_else(|| panic!("session {id} is listed")),
        other => panic!("session-list: {other:?}"),
    }
}

fn is_alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| !stat.contains(") Z "))
}

/// The start error of a daemon that must refuse to start.
async fn refused_start(home: &Home) -> StartError {
    match home.try_start().await {
        Ok(_) => panic!("the daemon started"),
        Err(error) => error,
    }
}

/// #251: a stop left under way is finished at the next start; when the state database
/// fails while it is recorded, the start stops instead of reporting an unfinished stop.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_error_finishing_a_stop_on_restore_stops_the_daemon_start() {
    let home = Home::new();
    let (daemon, started) = home.running_session().await;
    home.rewrite(
        "UPDATE sessions SET state = 'stopping', stopping_to = 'asleep' WHERE id = ?1",
        &started.id,
    )
    .await;
    // Every transition is recorded in this table, so finishing the stop fails to write.
    home.drop_table("session_transitions").await;
    home.stop(daemon);

    let error = refused_start(&home).await;

    assert!(matches!(error, StartError::Restore(_)), "{error}");
    assert!(error.to_string().contains("session_transitions"), "{error}");
}

/// #251: the relaunch that finishes an interrupted restart fails on a database error from
/// another capability's table (the agent channel's launch tokens); that error, wrapped by
/// the capability, stops the start too.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_error_relaunching_an_interrupted_restart_stops_the_daemon_start() {
    let home = Home::new();
    let (daemon, started) = home.running_session().await;
    home.rewrite(
        "UPDATE sessions SET state = 'stopping', stopping_to = 'restart' WHERE id = ?1",
        &started.id,
    )
    .await;
    home.drop_table("agent_channel_launches").await;
    home.stop(daemon);

    let error = refused_start(&home).await;

    assert!(matches!(error, StartError::Restore(_)), "{error}");
    assert!(
        error.to_string().contains("agent_channel_launches"),
        "{error}"
    );
}

/// #252: a daemon stopped after a restart's stop committed `launching`, before the launch,
/// leaves the old agent stopped; the next start launches the conversation again as the
/// restart the owner asked for, as a crash one step earlier does (#205).
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_interrupted_after_its_stop_committed_relaunches_as_a_restart() {
    let home = Home::new();
    let (daemon, started) = home.running_session().await;
    home.rewrite(
        "UPDATE sessions SET state = 'launching', stopping_to = NULL WHERE id = ?1",
        &started.id,
    )
    .await;
    home.rewrite(
        "INSERT INTO session_transitions (session_id, from_state, to_state, reason, at)
             VALUES (?1, 'stopping (restart)', 'launching', NULL, 0)",
        &started.id,
    )
    .await;
    home.stop(daemon);
    home.lose_every_agent();

    let daemon = home.start().await;
    let client = connect(&daemon).await;

    let restarted = listed(&client, &started.id).await;
    assert_eq!(restarted.state, SessionStateView::Running, "{restarted:?}");
    assert_eq!(
        restarted.launch.map(|launch| launch.kind),
        Some(LaunchKindView::Restart)
    );
    let launches = home.wait_for_launches(2).await;
    assert_eq!(
        (launches[1].mode.as_str(), launches[1].conversation.as_str()),
        ("resume", "conversation-1")
    );
}

/// The reserved execution of a launch a crash interrupted before its terminal opened: the
/// session is `launching` and its latest launch names an execution whose program never
/// ran, as the launch transaction commits them together (#256).
async fn interrupt_a_launch_after_its_reservation(home: &Home, session: &str) -> ExecutionId {
    let database = home.database();
    let supervisor = Supervisor::new(
        home.options().executions_dir(),
        LaunchContainment::ProcessGroup,
        cockpit_processes::SystemdTools::from_path(),
    );
    let write = database.write().await.unwrap();
    let reserved = supervisor.reserve(&write, database.epoch()).await.unwrap();
    write
        .execute(
            "INSERT INTO session_launches (session_id, kind, execution_id, launched_at)
                 VALUES (?1, 'wake', ?2, 0)",
            (session, reserved.0),
        )
        .await
        .unwrap();
    write
        .execute(
            "UPDATE sessions SET state = 'launching' WHERE id = ?1",
            [session],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    reserved
}

/// #256: the restore releases the never-started execution, without waiting for a start that
/// cannot come, and reopens the conversation once.
#[tokio::test(flavor = "multi_thread")]
async fn a_launch_interrupted_after_its_reservation_is_reopened_and_its_execution_released() {
    let home = Home::new();
    let (daemon, started) = home.running_session().await;
    // The previous agent was stopped before the launch, as a wake or a restart does.
    home.lose_every_agent();
    let reserved = interrupt_a_launch_after_its_reservation(&home, &started.id).await;
    home.stop(daemon);

    let daemon = home.start().await;
    let client = connect(&daemon).await;

    let reopened = listed(&client, &started.id).await;
    assert_eq!(reopened.state, SessionStateView::Running, "{reopened:?}");
    let execution = cockpit_processes::execution(&home.database(), reserved)
        .await
        .unwrap();
    assert_eq!(execution.phase, Phase::Ended(EndReason::NeverStarted));
    let launches = home.wait_for_launches(2).await;
    assert!(is_alive(launches[1].pid));
}

/// #256: `end` from `launching` releases the never-started execution too, and ends the
/// session instead of refusing on a stop it cannot confirm.
#[tokio::test(flavor = "multi_thread")]
async fn ending_a_launch_interrupted_after_its_reservation_releases_its_execution() {
    let home = Home::new();
    let (daemon, started) = home.running_session().await;
    let agent = home.launches()[0].pid;
    let client = connect(&daemon).await;
    request(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await;
    let reserved = interrupt_a_launch_after_its_reservation(&home, &started.id).await;

    let ended = session_reply(
        &client,
        Command::SessionEnd {
            session: started.id.clone(),
        },
    )
    .await;

    assert_eq!(ended.state, SessionStateView::Ended, "{ended:?}");
    assert!(!is_alive(agent));
    let execution = cockpit_processes::execution(&home.database(), reserved)
        .await
        .unwrap();
    assert_eq!(execution.phase, Phase::Ended(EndReason::NeverStarted));
}
