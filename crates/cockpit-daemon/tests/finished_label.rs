//! The finished label across launches, at the daemon's socket.
//!
//! Issue #269 (decided in #312): only a launch made to give the session work drops its
//! label (start, resume, the owner's restart, every wake); the daemon's restore on start,
//! `restart --all` and the deferred start-file restart keep it.
//! Issue #273: a finish and a launch are ordered by their commits, not by the clock's
//! seconds, so a finish in the launch's own second counts.
//!
//! Same setting as `session_restarts.rs`: the daemon runs in this process with a fake
//! provider that can resume, a real state database, real processes and a private tmux
//! server. The periodic pass is left at the fixture's hour, so it never runs mid-test.
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
use cockpit_sessions::AgentState;
use common::GENEROUS;

const AGENT_SCRIPT: &str = r#"printf '%s pid=%s token=%s\n' "$COCKPIT_SESSION" "$$" "$COCKPIT_LAUNCH_TOKEN" >> "$FAKE_AGENT_LOG"
exec sleep 600"#;

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
        let conversation = match request.conversation {
            Conversation::New => {
                ConversationId::new(format!("conversation-{}", request.working_dir.display()))
                    .unwrap()
            }
            Conversation::Resume(id) => id.clone(),
        };
        Ok(LaunchCommand {
            program: PathBuf::from("/bin/sh"),
            arguments: vec!["-c".into(), AGENT_SCRIPT.into()],
            environment: vec![("FAKE_AGENT_LOG".into(), OsString::from(&self.log))],
            conversation: Some(conversation),
        })
    }
}

/// One line of the agent log: a launch's agent, with what identifies it to the channel.
struct AgentLaunch {
    session: String,
    pid: i32,
    token: String,
}

struct Home {
    dir: tempfile::TempDir,
    database: Mutex<Option<Arc<cockpit_turso::Database>>>,
}

impl Home {
    fn new() -> Home {
        let home = Home {
            dir: tempfile::tempdir().unwrap(),
            database: Mutex::new(None),
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

    fn edit_start_file(&self) {
        let previous = std::fs::read_to_string(self.start_file()).unwrap();
        std::fs::write(self.start_file(), previous + "one more rule\n").unwrap();
    }

    fn launches_of(&self, session: &str) -> Vec<AgentLaunch> {
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
                    pid: field("pid=").parse().unwrap(),
                    token: field("token="),
                }
            })
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

    /// Stops `daemon` and loses every agent, as a reboot would.
    fn reboot(&self, daemon: Daemon) {
        drop(daemon);
        self.database.lock().unwrap().take();
        self.kill_every_agent();
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

    /// `session` takes an item the owner files and finishes it, as its agent would; the
    /// finish carries `finished_at`, seconds since 1970.
    async fn session_finishes_an_item_at(
        &self,
        client: &Connection,
        session: &str,
        finished_at: i64,
    ) {
        let added = request(
            client,
            Command::TaskAdd {
                launch_token: None,
                project: Some("app".to_owned()),
                title: format!("Work for {session}"),
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
        let item = cockpit_tasks::ItemId(task.id);
        let database = self.database();
        let write = database.write().await.unwrap();
        cockpit_tasks::start(&write, &agent(session), item, finished_at)
            .await
            .unwrap();
        cockpit_tasks::finish(&write, &agent(session), item, Some("done"), finished_at)
            .await
            .unwrap();
        write.commit().await.unwrap();
    }

    /// A session labelled finished by an item it finished in its launch's second.
    async fn finished_session(&self, client: &Connection, root: &Path) -> SessionView {
        let started = new_session(client, root).await;
        let launched_at = started.launch.as_ref().unwrap().launched_at;
        self.session_finishes_an_item_at(client, &started.id, launched_at)
            .await;
        let finished = listed(client, &started.id).await;
        assert!(finished.finished, "{finished:?}");
        finished
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

fn agent(session: &str) -> cockpit_tasks::Actor {
    cockpit_tasks::Actor::Session {
        session: cockpit_authorization::SessionId::new(session).unwrap(),
        project: Some("app".to_owned()),
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
        },
    )
    .await
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

/// #273: the finish lands in the same clock second as the launch, after it.
#[tokio::test(flavor = "multi_thread")]
async fn a_finish_in_the_second_of_its_launch_labels_the_session_finished() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    let launched_at = started.launch.unwrap().launched_at;

    home.session_finishes_an_item_at(&client, &started.id, launched_at)
        .await;

    assert!(listed(&client, &started.id).await.finished);
}

/// #273: a finish committed before the launch never counts for it, whatever its time says.
#[tokio::test(flavor = "multi_thread")]
async fn a_finish_before_the_launch_does_not_label_the_relaunched_session_even_dated_later() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    let in_an_hour = started.launch.unwrap().launched_at + 3600;
    home.session_finishes_an_item_at(&client, &started.id, in_an_hour)
        .await;

    let restarted = session_reply(
        &client,
        Command::SessionRestart {
            session: started.id.clone(),
        },
    )
    .await;

    assert!(!restarted.finished, "{restarted:?}");
}

/// #269: a reboot's restore keeps the label.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_restored_on_daemon_start_keeps_its_finished_label() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let finished = home.finished_session(&client, &root).await;
    drop(client);
    home.reboot(daemon);

    let daemon = home.start().await;
    let client = connect(&daemon).await;

    let restored = listed(&client, &finished.id).await;
    assert_eq!(
        restored.launch.as_ref().unwrap().kind,
        LaunchKindView::Restore
    );
    assert!(restored.finished, "{restored:?}");
}

/// #269: `restart --all` after a start-file change keeps the label.
#[tokio::test(flavor = "multi_thread")]
async fn restart_all_keeps_a_finished_label() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let finished = home.finished_session(&client, &root).await;
    home.edit_start_file();
    home.report_as_agent(&home.launches_of(&finished.id)[0], AgentState::Waiting)
        .await;

    let outcome = request(&client, Command::SessionRestartAll).await;

    let Ok(Reply::RestartedAll { restarted, .. }) = outcome else {
        panic!("session-restart-all: {outcome:?}");
    };
    assert_eq!(restarted.len(), 1, "{restarted:?}");
    assert!(restarted[0].finished, "{:?}", restarted[0]);
    let listed = listed(&client, &finished.id).await;
    assert_eq!(
        listed.launch.as_ref().unwrap().kind,
        LaunchKindView::Restart
    );
    assert!(listed.finished, "{listed:?}");
}

/// #269: a wake is a working launch: someone is about to talk to the session.
#[tokio::test(flavor = "multi_thread")]
async fn a_woken_session_loses_its_finished_label() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let finished = home.finished_session(&client, &root).await;
    let asleep = session_reply(
        &client,
        Command::SessionSleep {
            session: finished.id.clone(),
        },
    )
    .await;
    assert_eq!(asleep.state, SessionStateView::Asleep);

    let woken = session_reply(
        &client,
        Command::SessionWake {
            session: finished.id.clone(),
        },
    )
    .await;

    assert!(!woken.finished, "{woken:?}");
}
