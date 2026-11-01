//! Issue #146 at the daemon's socket: `restart --all` restarts only the sessions whose start
//! files changed since their launch, now when their agent waits at its prompt and at the end
//! of its turn otherwise (decided in #177); and the wake paths other capabilities call.
//!
//! Same setting as `sessions.rs`: the daemon runs in this process with a fake provider that
//! implements only the baseline contract, a real state database, real processes and a
//! private tmux server. Its agent logs one line per launch, then sleeps until stopped.
#![cfg(target_os = "linux")]

mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{Command, LaunchKindView, Reply, SessionStateView, SessionView};
use cockpit_provider_api::{
    Capabilities as ProviderCapabilities, Capability, Conversation, ConversationId, LaunchCommand,
    LaunchRequest, Provenance, Provider, ProviderError, ProviderName,
};
use cockpit_sessions::{AgentState, SessionId, Sessions, WakeCause};
use common::GENEROUS;

const AGENT_SCRIPT: &str = r#"printf '%s %s pid=%s token=%s\n' "$COCKPIT_SESSION" "$1" "$$" "$COCKPIT_LAUNCH_TOKEN" >> "$FAKE_AGENT_LOG"
exec sleep 600"#;

/// The start file the fake provider's agents read at launch.
const START_FILE: &str = "instructions";

struct FakeProvider {
    name: ProviderName,
    log: PathBuf,
}

impl Provider for FakeProvider {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::none().with(
            Capability::Resume,
            Provenance::Tested {
                version: "fake".to_owned(),
            },
        )
    }

    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        let (mode, conversation) = match request.conversation {
            Conversation::New => (
                "new",
                ConversationId::new(format!("conversation-{}", request.working_dir.display()))
                    .unwrap(),
            ),
            Conversation::Resume(id) => ("resume", id.clone()),
        };
        Ok(LaunchCommand {
            program: "/bin/sh".into(),
            arguments: vec![
                "-c".into(),
                AGENT_SCRIPT.into(),
                "fake-agent".into(),
                OsString::from(mode),
            ],
            environment: vec![
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("FAKE_AGENT_LOG".into(), self.log.clone().into()),
            ],
            conversation: Some(conversation),
        })
    }
}

/// One launch line of the fake agent's log.
#[derive(Debug, Clone)]
struct AgentLaunch {
    session: String,
    mode: String,
    pid: i32,
    token: String,
}

struct Home {
    dir: tempfile::TempDir,
    database: Mutex<Option<Arc<cockpit_turso::Database>>>,
    sessions: Mutex<Option<Arc<Sessions>>>,
}

impl Home {
    fn new() -> Home {
        let home = Home {
            dir: tempfile::tempdir().unwrap(),
            database: Mutex::new(None),
            sessions: Mutex::new(None),
        };
        std::fs::write(home.start_file(), "first version\n").unwrap();
        home
    }

    fn path(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    fn start_file(&self) -> PathBuf {
        self.path().join("instructions.md")
    }

    /// The owner edits the start file: every agent launched before runs on the old version.
    fn edit_start_file(&self) {
        let previous = std::fs::read_to_string(self.start_file()).unwrap();
        std::fs::write(self.start_file(), previous + "one more rule\n").unwrap();
    }

    /// Every launch the fake agent logged so far, without waiting for any.
    fn logged_launches(&self) -> Vec<AgentLaunch> {
        std::fs::read_to_string(self.path().join("agent.log"))
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let words: Vec<&str> = line.split(' ').collect();
                let field = |prefix: &str| {
                    words
                        .iter()
                        .find_map(|word| word.strip_prefix(prefix))
                        .unwrap()
                        .to_owned()
                };
                AgentLaunch {
                    session: words[0].to_owned(),
                    mode: words[1].to_owned(),
                    pid: field("pid=").parse().unwrap(),
                    token: field("token="),
                }
            })
            .collect()
    }

    /// Every launch of `session` the fake agent logged, once it logged `count`.
    async fn launches_of(&self, session: &str, count: usize) -> Vec<AgentLaunch> {
        common::once_logged(count, || self.logged_launches_of(session)).await
    }

    fn logged_launches_of(&self, session: &str) -> Vec<AgentLaunch> {
        self.logged_launches()
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
            let provider = FakeProvider {
                name: ProviderName::new("fake").unwrap(),
                log: self.path().join("agent.log"),
            };
            let mut wiring = common::wiring_in(&self.path(), vec![Arc::new(provider)]);
            wiring.sessions = cockpit_sessions::Settings {
                stop: cockpit_processes::Escalation {
                    grace: Duration::from_secs(2),
                    kill_wait: Duration::from_secs(5),
                },
                start_files: vec![cockpit_config::StartFile {
                    label: START_FILE.to_owned(),
                    path: self.start_file(),
                    providers: vec!["fake".to_owned()],
                }],
                ..cockpit_sessions::Settings::default()
            };
            wiring.session_tick = Duration::from_millis(100);
            let started = match Instance::acquire(options) {
                Ok(instance) => {
                    instance
                        .start(wiring, |capabilities| {
                            *self.database.lock().unwrap() =
                                Some(Arc::clone(&capabilities.database));
                            *self.sessions.lock().unwrap() =
                                Some(Arc::clone(&capabilities.sessions));
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

    fn sessions(&self) -> Arc<Sessions> {
        self.sessions.lock().unwrap().clone().unwrap()
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

    /// Reports `state` as the agent of `launch`, from its real process tree. Only a launch's
    /// first report carries its token; later ones are identified by their tree alone.
    async fn report_as_agent(&self, launch: &AgentLaunch, state: AgentState, first: bool) {
        let database = self.database.lock().unwrap().clone().unwrap();
        let write = database.write().await.unwrap();
        let agent = cockpit_agent_channel::identity_of(u32::try_from(launch.pid).unwrap());
        let peer = cockpit_agent_channel::ancestry_of(&agent.unwrap());
        cockpit_sessions::report(
            &write,
            &peer,
            first.then_some(launch.token.as_str()),
            cockpit_sessions::Report {
                state: Some(state),
                ..cockpit_sessions::Report::default()
            },
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
    }

    /// The reason recorded with the session's latest move to launching.
    async fn latest_launching_reason(&self, session: &str) -> Option<String> {
        let database = self.database.lock().unwrap().clone().unwrap();
        let read = database.read().await.unwrap();
        let mut rows = read
            .query(
                "SELECT reason FROM session_transitions
                 WHERE session_id = ?1 AND to_state = 'launching' ORDER BY id DESC LIMIT 1",
                [session],
            )
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get(0).unwrap()
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

async fn listed(client: &Connection, id: &str) -> SessionView {
    match request(client, Command::SessionList { ended: true }).await {
        Ok(Reply::Sessions { sessions }) => sessions
            .into_iter()
            .find(|session| session.id == id)
            .unwrap(),
        other => panic!("session-list: {other:?}"),
    }
}

struct RestartedAll {
    restarted: Vec<String>,
    deferred: Vec<SessionView>,
}

async fn restart_all(client: &Connection) -> RestartedAll {
    match request(client, Command::SessionRestartAll).await {
        Ok(Reply::RestartedAll {
            restarted,
            deferred,
            failed,
        }) => {
            assert!(failed.is_empty(), "{failed:?}");
            RestartedAll {
                restarted: restarted.into_iter().map(|session| session.id).collect(),
                deferred,
            }
        }
        other => panic!("session-restart-all: {other:?}"),
    }
}

/// Waits until the session has `count` launches and runs the latest.
async fn wait_for_launches(home: &Home, client: &Connection, id: &str, count: usize) {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let session = listed(client, id).await;
        if home.logged_launches_of(id).len() == count && session.state == SessionStateView::Running
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "session {id} has {} launches and is {:?}",
            home.logged_launches_of(id).len(),
            session.state
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn session_list_names_the_start_files_that_changed_since_the_launch() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    assert_eq!(started.changed_start_files, Vec::<String>::new());

    home.edit_start_file();

    let session = listed(&client, &started.id).await;
    assert_eq!(session.changed_start_files, vec![START_FILE.to_owned()]);
    assert!(!session.restart_pending);
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_all_restarts_only_the_sessions_whose_start_files_changed() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let old = new_session(&client, &root).await;
    home.edit_start_file();
    let current = new_session(&client, &root).await;
    home.report_as_agent(
        &home.launches_of(&old.id, 1).await[0],
        AgentState::Waiting,
        true,
    )
    .await;

    let outcome = restart_all(&client).await;

    assert_eq!(outcome.restarted, vec![old.id.clone()]);
    assert!(outcome.deferred.is_empty());
    let restarted = listed(&client, &old.id).await;
    assert_eq!(restarted.launch.unwrap().kind, LaunchKindView::Restart);
    assert_eq!(restarted.changed_start_files, Vec::<String>::new());
    assert_eq!(home.launches_of(&old.id, 2).await[1].mode, "resume");
    common::still_logged(1, || home.logged_launches_of(&current.id)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_all_with_every_start_file_current_restarts_nothing() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;

    let outcome = restart_all(&client).await;

    assert!(outcome.restarted.is_empty());
    assert!(outcome.deferred.is_empty());
    common::still_logged(1, || home.logged_launches_of(&started.id)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_session_restarts_once_its_agent_reports_waiting() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    let first_launch = home.launches_of(&started.id, 1).await[0].clone();
    home.report_as_agent(&first_launch, AgentState::Working, true)
        .await;
    home.edit_start_file();

    let outcome = restart_all(&client).await;

    assert!(outcome.restarted.is_empty());
    assert_eq!(outcome.deferred.len(), 1);
    assert!(outcome.deferred[0].restart_pending);
    // Periodic passes go by while it works: its turn is not cut.
    common::still_logged(1, || home.logged_launches_of(&started.id)).await;
    assert!(listed(&client, &started.id).await.restart_pending);

    home.report_as_agent(&first_launch, AgentState::Waiting, false)
        .await;

    wait_for_launches(&home, &client, &started.id, 2).await;
    let restarted = listed(&client, &started.id).await;
    assert_eq!(restarted.launch.unwrap().kind, LaunchKindView::Restart);
    assert!(!restarted.restart_pending);
    assert_eq!(restarted.changed_start_files, Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_whose_agent_never_reported_is_deferred_not_restarted() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    home.edit_start_file();

    let outcome = restart_all(&client).await;

    assert!(outcome.restarted.is_empty());
    assert_eq!(outcome.deferred[0].id, started.id);
    common::still_logged(1, || home.logged_launches_of(&started.id)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wake_path_wakes_an_asleep_session_and_records_its_cause() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    home.launches_of(&started.id, 1).await;
    request(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await
    .unwrap();
    let id = SessionId::parse(&started.id).unwrap();

    let woken = home
        .sessions()
        .ensure_awake(&id, WakeCause::Message)
        .await
        .unwrap();

    assert_eq!(woken.state, cockpit_sessions::SessionState::Running);
    assert_eq!(home.launches_of(&started.id, 2).await[1].mode, "resume");
    assert_eq!(
        home.latest_launching_reason(&started.id).await.as_deref(),
        Some("woken by a message")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wake_path_leaves_a_running_session_as_it_is() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    let id = SessionId::parse(&started.id).unwrap();

    let awake = home
        .sessions()
        .ensure_awake(&id, WakeCause::Open)
        .await
        .unwrap();

    assert_eq!(awake.revision, started.revision);
    common::still_logged(1, || home.logged_launches_of(&started.id)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owners_wake_records_the_owner_as_its_cause() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    request(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await
    .unwrap();

    request(
        &client,
        Command::SessionWake {
            session: started.id.clone(),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        home.latest_launching_reason(&started.id).await.as_deref(),
        Some("woken by the owner")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ended_session_keeps_no_pending_restart() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    home.edit_start_file();
    assert_eq!(restart_all(&client).await.deferred.len(), 1);

    request(
        &client,
        Command::SessionEnd {
            session: started.id.clone(),
        },
    )
    .await
    .unwrap();

    assert!(!listed(&client, &started.id).await.restart_pending);
}

/// Reports carry the launch they were sent from, not only a time in seconds: a report sent
/// in the same second as the next launch still belongs to the previous agent.
#[tokio::test(flavor = "multi_thread")]
async fn a_waiting_report_from_the_previous_launch_does_not_restart_the_new_agent() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    home.report_as_agent(
        &home.launches_of(&started.id, 1).await[0],
        AgentState::Waiting,
        true,
    )
    .await;
    request(
        &client,
        Command::SessionRestart {
            session: started.id.clone(),
        },
    )
    .await
    .unwrap();
    home.edit_start_file();

    let outcome = restart_all(&client).await;

    assert!(outcome.restarted.is_empty());
    assert_eq!(outcome.deferred[0].id, started.id);
    assert_eq!(
        outcome.deferred[0].changed_start_files,
        vec![START_FILE.to_owned()]
    );
}
