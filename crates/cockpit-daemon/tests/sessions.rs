//! Issue #31 at the daemon's socket: every session lifecycle transition through the protocol,
//! against a fake provider that implements only the baseline `cockpit-provider-api`
//! contract, a real state database, real processes and a private tmux server.
//!
//! The daemon runs in this process (as in `overload.rs`) because the fake provider
//! (`cockpit-provider-api`'s `fake` feature) is a test type the binary does not link. Its
//! agent is a shell script that logs each launch, then sleeps until stopped.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{Capabilities, Daemon, Instance, Limits, Options, Routes, Wiring};
use cockpit_protocol::{Command, ErrorCode, LaunchKindView, Reply, SessionStateView, SessionView};
use cockpit_provider_api::ProviderName;
use cockpit_provider_api::fake::{FakeLaunch as AgentLaunch, FakeProvider};
use common::GENEROUS;

struct Home {
    dir: tempfile::TempDir,
    /// The state database of the running daemon, for the agent-report fixture.
    database: Mutex<Option<Arc<cockpit_turso::Database>>>,
}

#[derive(Clone, Copy)]
struct Variant {
    can_resume: bool,
    names_conversation: bool,
}

const BASELINE: Variant = Variant {
    can_resume: true,
    names_conversation: true,
};

/// A provider that resumes but never names the conversation, so a restore cannot reopen it.
const UNNAMED: Variant = Variant {
    can_resume: true,
    names_conversation: false,
};

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

    /// The owner's environment the daemon reads its configuration from.
    fn environment(&self) -> cockpit_config::Environment {
        cockpit_config::Environment {
            home: Some(self.path().join("home").into()),
            ..cockpit_config::Environment::default()
        }
    }

    fn agent_log(&self) -> PathBuf {
        self.path().join("agent.log")
    }

    /// Every launch the fake agent logged, once it has logged `count`. The agent logs from
    /// its own shell after tmux starts it, so a launch the daemon already reported can still
    /// be missing; an agent stopped before it logs never does, so a test waits for a launch
    /// before it stops that agent or restarts the daemon (#231, #235).
    async fn launches(&self, count: usize) -> Vec<AgentLaunch> {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let launches = self.logged_launches();
            assert!(
                launches.len() <= count,
                "expected {count} launches, the agent logged {launches:?}"
            );
            if launches.len() == count {
                return launches;
            }
            assert!(
                Instant::now() < deadline,
                "the agent logged {} of {count} launches",
                launches.len()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// The launches logged so far, without waiting: for checks that no further launch
    /// happened.
    fn logged_launches(&self) -> Vec<AgentLaunch> {
        FakeProvider::launches(&self.agent_log())
    }

    fn wiring(&self, variant: Variant, settings: cockpit_sessions::Settings) -> Wiring {
        let mut provider = FakeProvider::new(ProviderName::new("fake").unwrap(), self.agent_log());
        if !variant.can_resume {
            provider = provider.without_resume();
        }
        if !variant.names_conversation {
            provider = provider.unnamed_conversations();
        }
        let mut wiring = common::wiring_in(&self.path(), vec![Arc::new(provider)]);
        wiring.sessions = settings;
        wiring.session_tick = Duration::from_millis(100);
        wiring
    }

    /// Starts a daemon on this home's state. A daemon dropped just before releases its
    /// database once its aborted tasks are gone, so the start is retried until then.
    async fn start(&self, variant: Variant, settings: cockpit_sessions::Settings) -> Daemon {
        self.start_wired(|| self.wiring(variant, settings.clone()))
            .await
    }

    /// Starts a daemon whose tmux cannot be run.
    async fn start_without_tmux(&self) -> Daemon {
        self.start_wired(|| {
            let mut wiring = self.wiring(BASELINE, settings());
            wiring.tmux.tmux = self.path().join("no-tmux-here");
            wiring
        })
        .await
    }

    /// Starts a daemon on this home's state with the wiring `wiring` builds, once per try.
    async fn start_wired(&self, wiring: impl Fn() -> Wiring) -> Daemon {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let options = Options {
                runtime_dir: self.path().join("run/cockpit"),
                state_dir: self.path().join("state/cockpit"),
                limits: Limits::default(),
            };
            let started = match Instance::acquire(options) {
                Ok(instance) => {
                    instance
                        .start(wiring(), |capabilities| {
                            self.keep_database(&capabilities);
                            Arc::new(Routes::new(capabilities, self.environment()))
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

    fn keep_database(&self, capabilities: &Capabilities) {
        *self.database.lock().unwrap() = Some(Arc::clone(&capabilities.database));
    }

    /// Stops `daemon` as a crash would leave things: its agents keep running.
    fn stop(&self, daemon: Daemon) {
        drop(daemon);
        self.database.lock().unwrap().take();
    }

    /// A Git project registered through the daemon, returning its root.
    async fn project(&self, client: &Connection) -> PathBuf {
        let root = self.path().join("projects/app");
        std::fs::create_dir_all(root.join("src")).unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("src/README"), "fixture\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "--quiet", "-m", "initial"]);
        let origin = self.path().join("origin.git");
        git(
            &self.path(),
            &["init", "--quiet", "--bare", origin.to_str().unwrap()],
        );
        git(
            &root,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
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

    fn tmux_sessions(&self) -> Vec<String> {
        let output = Process::new("tmux")
            .arg("-S")
            .arg(self.path().join("tmux.sock"))
            .args(["list-sessions", "-F", "#{session_name}"])
            .env_remove("TMUX")
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The machine reboots, as far as the agents can tell: the tmux server and every
    /// agent in it are gone.
    fn lose_every_agent(&self) {
        let _ = Process::new("tmux")
            .arg("-S")
            .arg(self.path().join("tmux.sock"))
            .arg("kill-server")
            .env_remove("TMUX")
            .output();
    }

    /// Whether `session` may change its own state: the check the agent channel makes before
    /// it accepts a report.
    async fn may_report(&self, session: &str) -> bool {
        self.may(session, cockpit_authorization::Permission::ChangeState)
            .await
    }

    /// Whether `session` may ask the owner to run a command or follow a guide: the check
    /// `cockpit-approvals` makes before it files an ask.
    async fn may_ask(&self, session: &str) -> bool {
        self.may(session, cockpit_authorization::Permission::RunCommands)
            .await
    }

    async fn may(&self, session: &str, permission: cockpit_authorization::Permission) -> bool {
        let database = self.database.lock().unwrap().clone().unwrap();
        let read = database.read().await.unwrap();
        let session = cockpit_authorization::SessionId::new(session).unwrap();
        let decision = cockpit_authorization::evaluate(
            &read,
            &cockpit_authorization::Actor::Session(session.clone()),
            permission,
            &cockpit_authorization::Scope::Session(session),
        )
        .await
        .unwrap();
        matches!(decision.outcome, cockpit_authorization::Outcome::Allowed(_))
    }

    /// Asks the owner to run `command` as the agent `launch` would with `cockpit ask run`:
    /// identified by that agent's real process tree and launch token, then filed through
    /// `cockpit-approvals`, as the daemon's agent route does. The agent itself cannot dial
    /// the socket from here: its connection would come from this test process.
    async fn ask_as_agent(&self, launch: &AgentLaunch, command: &str, cwd: &Path) {
        let database = self.database.lock().unwrap().clone().unwrap();
        let write = database.write().await.unwrap();
        let agent = cockpit_agent_channel::identity_of(u32::try_from(launch.pid).unwrap());
        let peer = cockpit_agent_channel::ancestry_of(&agent.unwrap());
        let session = cockpit_agent_channel::identify(&write, &peer, Some(&launch.token))
            .await
            .unwrap();
        cockpit_approvals::ask_run(
            &write,
            &session,
            cockpit_approvals::RunAsk {
                command: command.to_owned(),
                cwd: cwd.to_owned(),
                reason: "the acceptance walk asks".to_owned(),
                project: None,
            },
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .try_into()
                .unwrap(),
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
    }

    /// Starts a session whose agent names no conversation, then restarts the daemon with
    /// every agent gone, so the session's restore fails. Returns the session's id and the
    /// restarted daemon.
    async fn fail_a_restore(&self) -> (String, Daemon) {
        let daemon = self.start(UNNAMED, settings()).await;
        let client = connect(&daemon).await;
        let root = self.project(&client).await;
        let started = new_session(&client, &root, None).await;
        self.launches(1).await;
        drop(client);
        self.stop(daemon);
        self.lose_every_agent();
        (started.id, self.start(UNNAMED, settings()).await)
    }

    /// Reports `report` as the agent `launch` would, through the agent channel's API with
    /// that agent's real process tree and launch token.
    async fn report_as_agent(&self, launch: &AgentLaunch, state: cockpit_sessions::AgentState) {
        let database = self.database.lock().unwrap().clone().unwrap();
        let write = database.write().await.unwrap();
        let agent = cockpit_agent_channel::identity_of(u32::try_from(launch.pid).unwrap());
        let peer = cockpit_agent_channel::ancestry_of(&agent.unwrap());
        cockpit_sessions::report(
            &write,
            &peer,
            Some(&launch.token),
            cockpit_sessions::Report {
                state: Some(state),
                ..cockpit_sessions::Report::default()
            },
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
    }

    /// Leaves `session` `stopping` toward `to`, as an error or a crash midway through a stop
    /// leaves it.
    async fn leave_stopping(&self, session: &str, to: &str) {
        let database = self.database.lock().unwrap().clone().unwrap();
        let write = database.write().await.unwrap();
        write
            .execute(
                "UPDATE sessions SET state = 'stopping', stopping_to = ?2 WHERE id = ?1",
                [session, to],
            )
            .await
            .unwrap();
        write.commit().await.unwrap();
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
    match request(client, command.clone()).await {
        Ok(Reply::Session { session }) => *session,
        other => panic!("{command:?}: {other:?}"),
    }
}

fn refusal(result: Result<Reply, ClientError>) -> cockpit_protocol::ProtocolError {
    match result {
        Err(ClientError::Daemon(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn refusal_code(result: Result<Reply, ClientError>) -> ErrorCode {
    refusal(result).code
}

async fn new_session(client: &Connection, dir: &Path, prompt: Option<&str>) -> SessionView {
    session_reply(
        client,
        Command::SessionNew {
            dir: dir.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: prompt.map(str::to_owned),
            clone: None,
        },
    )
    .await
}

async fn sessions(client: &Connection, ended: bool) -> Vec<SessionView> {
    match request(client, Command::SessionList { ended }).await {
        Ok(Reply::Sessions { sessions }) => sessions,
        other => panic!("session-list: {other:?}"),
    }
}

async fn wait_for_state(client: &Connection, id: &str, state: SessionStateView) -> SessionView {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let session = sessions(client, true)
            .await
            .into_iter()
            .find(|session| session.id == id)
            .unwrap();
        if session.state == state {
            return session;
        }
        assert!(
            Instant::now() < deadline,
            "session {id} stayed {:?}",
            session.state
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn is_alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
        && !std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .unwrap_or_default()
            .contains(") Z ")
}

fn settings() -> cockpit_sessions::Settings {
    cockpit_sessions::Settings {
        stop: cockpit_processes::Escalation {
            grace: Duration::from_secs(2),
            kill_wait: Duration::from_secs(5),
        },
        ..cockpit_sessions::Settings::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_session_runs_its_agent_in_place_with_its_first_message() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;

    let session = new_session(&client, &root.join("src"), Some("hello")).await;

    assert_eq!(session.state, SessionStateView::Running);
    assert_eq!(session.agent, "fake");
    assert_eq!(session.working_dir, root.join("src").display().to_string());
    assert!(!session.holds_checkout);
    assert_eq!(session.conversation.as_deref(), Some("conversation-1"));
    assert_eq!(session.launch.unwrap().kind, LaunchKindView::Start);
    let launches = home.launches(1).await;
    assert_eq!(
        (launches[0].session.as_str(), launches[0].mode.as_str()),
        (session.id.as_str(), "new")
    );
    assert_eq!(launches[0].message, "hello");
    assert!(is_alive(launches[0].pid));
    assert_eq!(home.tmux_sessions(), [session.id]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_session_in_a_clone_occupies_a_parallel_clone_on_its_branch() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;

    let session = session_reply(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: None,
            prompt: None,
            clone: Some("feature/x".to_owned()),
        },
    )
    .await;

    assert_eq!(session.state, SessionStateView::Running);
    assert!(session.holds_checkout);
    let clone = PathBuf::from(&session.working_dir);
    assert_ne!(clone, root);
    let checkouts = match request(&client, Command::CheckoutList { project: None }).await {
        Ok(Reply::Checkouts { checkouts }) => checkouts,
        other => panic!("{other:?}"),
    };
    let occupied = checkouts
        .iter()
        .find(|checkout| checkout.id == session.checkout)
        .unwrap();
    assert_eq!(occupied.occupant.as_deref(), Some(session.id.as_str()));
    assert_eq!(occupied.path, session.working_dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_directory_in_no_registered_project_is_refused() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;

    let refused = request(
        &client,
        Command::SessionNew {
            dir: home.path().display().to_string(),
            agent: None,
            prompt: None,
            clone: None,
        },
    )
    .await;

    assert_eq!(refusal_code(refused), ErrorCode::NotFound);
    assert!(home.logged_launches().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_stops_the_agent_and_resumes_the_same_conversation_in_place() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    home.launches(1).await;

    let restarted = session_reply(
        &client,
        Command::SessionRestart {
            session: started.id.clone(),
        },
    )
    .await;

    assert_eq!(restarted.id, started.id);
    assert_eq!(restarted.state, SessionStateView::Running);
    assert_eq!(restarted.launch.unwrap().kind, LaunchKindView::Restart);
    let launches = home.launches(2).await;
    assert_eq!(
        (launches[1].mode.as_str(), launches[1].conversation.as_str()),
        ("resume", "conversation-1")
    );
    assert!(!is_alive(launches[0].pid));
    assert!(is_alive(launches[1].pid));
    assert_ne!(launches[0].token, launches[1].token);
}

#[tokio::test(flavor = "multi_thread")]
async fn sleep_stops_the_agent_and_wake_resumes_its_conversation() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    let first = home.launches(1).await;

    let asleep = session_reply(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await;
    assert_eq!(asleep.state, SessionStateView::Asleep);
    assert!(!is_alive(first[0].pid));
    assert!(home.tmux_sessions().is_empty());

    let awake = session_reply(
        &client,
        Command::SessionWake {
            session: started.id.clone(),
        },
    )
    .await;
    assert_eq!(awake.state, SessionStateView::Running);
    assert_eq!(awake.launch.unwrap().kind, LaunchKindView::Wake);
    let launches = home.launches(2).await;
    assert_eq!(
        (launches[1].mode.as_str(), launches[1].conversation.as_str()),
        ("resume", "conversation-1")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_whose_provider_cannot_resume_is_refused_sleep_and_keeps_running() {
    let home = Home::new();
    let variant = Variant {
        can_resume: false,
        names_conversation: true,
    };
    let daemon = home.start(variant, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    let first = home.launches(1).await;

    let refused = request(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await;

    assert_eq!(refusal_code(refused), ErrorCode::Conflict);
    assert!(is_alive(first[0].pid));
}

#[tokio::test(flavor = "multi_thread")]
async fn waking_a_running_session_is_refused_as_a_conflict() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    home.launches(1).await;

    let refused = request(
        &client,
        Command::SessionWake {
            session: started.id,
        },
    )
    .await;

    assert_eq!(refusal_code(refused), ErrorCode::Conflict);
    assert_eq!(home.logged_launches().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_waits_at_its_prompt_past_the_idle_window_is_put_to_sleep() {
    let home = Home::new();
    let idle_at_once = cockpit_sessions::Settings {
        idle_sleep_after: Duration::ZERO,
        ..settings()
    };
    let daemon = home.start(BASELINE, idle_at_once).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;

    // Without a report that it waits, the agent is not ready: passes go by, it runs on.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        sessions(&client, false).await[0].state,
        SessionStateView::Running
    );
    let first = home.launches(1).await;
    home.report_as_agent(&first[0], cockpit_sessions::AgentState::Waiting)
        .await;

    wait_for_state(&client, &started.id, SessionStateView::Asleep).await;
    assert!(!is_alive(first[0].pid));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_working_agent_is_not_put_to_sleep_however_long_it_works() {
    let home = Home::new();
    let idle_at_once = cockpit_sessions::Settings {
        idle_sleep_after: Duration::ZERO,
        ..settings()
    };
    let daemon = home.start(BASELINE, idle_at_once).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    new_session(&client, &root, None).await;

    home.report_as_agent(
        &home.launches(1).await[0],
        cockpit_sessions::AgentState::Working,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        sessions(&client, false).await[0].state,
        SessionStateView::Running
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_report_from_the_previous_launch_does_not_put_the_restarted_agent_to_sleep() {
    let home = Home::new();
    let idle_at_once = cockpit_sessions::Settings {
        idle_sleep_after: Duration::ZERO,
        ..settings()
    };
    let daemon = home.start(BASELINE, idle_at_once).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    home.report_as_agent(
        &home.launches(1).await[0],
        cockpit_sessions::AgentState::Working,
    )
    .await;
    session_reply(
        &client,
        Command::SessionRestart {
            session: started.id.clone(),
        },
    )
    .await;
    // The report is rewritten as waiting, dated before the restart's launch.
    let database = home.database.lock().unwrap().clone().unwrap();
    let write = database.write().await.unwrap();
    write
        .execute(
            "UPDATE session_reports SET state = 'waiting', reported_at = 0",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        sessions(&client, false).await[0].state,
        SessionStateView::Running
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn end_finishes_a_stop_an_error_left_unfinished() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    let first = home.launches(1).await;
    home.leave_stopping(&started.id, "asleep").await;

    let ended = session_reply(
        &client,
        Command::SessionEnd {
            session: started.id.clone(),
        },
    )
    .await;

    assert_eq!(ended.state, SessionStateView::Ended);
    assert!(!is_alive(first[0].pid));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_asleep_session_past_the_expiry_ends() {
    let home = Home::new();
    let expire_at_once = cockpit_sessions::Settings {
        asleep_expiry: Duration::ZERO,
        ..settings()
    };
    let daemon = home.start(BASELINE, expire_at_once).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    session_reply(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await;

    let ended = wait_for_state(&client, &started.id, SessionStateView::Ended).await;

    assert!(ended.reason.unwrap().contains("expiry"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_exits_ends_its_session_and_resume_reopens_the_conversation() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;

    Process::new("kill")
        .arg(home.launches(1).await[0].pid.to_string())
        .status()
        .unwrap();
    let ended = wait_for_state(&client, &started.id, SessionStateView::Ended).await;
    assert!(ended.reason.unwrap().contains("exited"));
    assert!(sessions(&client, false).await.is_empty());

    let resumed = session_reply(
        &client,
        Command::SessionResume {
            session: started.id.clone(),
        },
    )
    .await;
    assert_eq!(resumed.state, SessionStateView::Running);
    assert_eq!(resumed.launch.unwrap().kind, LaunchKindView::Resume);
    assert_eq!(home.launches(2).await[1].conversation, "conversation-1");
}

#[tokio::test(flavor = "multi_thread")]
async fn without_tmux_the_daemon_starts_and_refuses_new_sessions_naming_tmux() {
    let home = Home::new();
    let daemon = home.start_without_tmux().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;

    let refused = request(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: None,
            prompt: None,
            clone: None,
        },
    )
    .await;

    let error = refusal(refused);
    assert_eq!(error.code, ErrorCode::Refused);
    assert!(error.message.contains("tmux"), "{}", error.message);
    assert!(home.logged_launches().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn without_tmux_waking_is_refused_and_the_session_stays_asleep() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    home.launches(1).await;
    session_reply(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await;
    drop(client);
    home.stop(daemon);
    let daemon = home.start_without_tmux().await;
    let client = connect(&daemon).await;

    let refused = request(
        &client,
        Command::SessionWake {
            session: started.id.clone(),
        },
    )
    .await;

    assert_eq!(refusal_code(refused), ErrorCode::Refused);
    assert_eq!(
        sessions(&client, false).await[0].state,
        SessionStateView::Asleep
    );
    assert_eq!(home.logged_launches().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_tmux_a_running_session_is_left_unrestored_until_tmux_is_back() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    let first = home.launches(1).await;
    drop(client);
    home.stop(daemon);

    let daemon = home.start_without_tmux().await;
    let client = connect(&daemon).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let left = sessions(&client, false).await;
    assert_eq!(left[0].launch, started.launch);
    assert_eq!(left[0].state, SessionStateView::Running);
    assert_eq!(home.logged_launches().len(), 1);
    assert!(is_alive(first[0].pid));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launched_session_holds_change_state_over_itself() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;

    let started = new_session(&client, &root, None).await;

    assert!(home.may_report(&started.id).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_session_revokes_its_change_state_over_itself() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;

    session_reply(
        &client,
        Command::SessionEnd {
            session: started.id.clone(),
        },
    )
    .await;

    assert!(!home.may_report(&started.id).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_exits_loses_change_state_over_its_session() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;

    Process::new("kill")
        .arg(home.launches(1).await[0].pid.to_string())
        .status()
        .unwrap();
    wait_for_state(&client, &started.id, SessionStateView::Ended).await;

    assert!(!home.may_report(&started.id).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_that_fails_to_restore_loses_change_state_over_itself() {
    let home = Home::new();

    let (session, daemon) = home.fail_a_restore().await;

    let client = connect(&daemon).await;
    wait_for_state(&client, &session, SessionStateView::Failed).await;
    assert!(!home.may_report(&session).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launched_sessions_ask_run_reaches_the_owners_queue_without_a_manual_grant() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    let launch = home.launches(1).await.remove(0);

    home.ask_as_agent(&launch, "make test", &root).await;

    let queued = match request(&client, Command::ApprovalQueue).await {
        Ok(Reply::ApprovalQueue { runs, .. }) => runs,
        other => panic!("expected the queue, got {other:?}"),
    };
    let asked: Vec<_> = queued
        .iter()
        .map(|run| (run.session.as_str(), run.command.as_str()))
        .collect();
    assert_eq!(asked, [(started.id.as_str(), "make test")]);
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_session_revokes_its_run_commands_over_itself() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    assert!(home.may_ask(&started.id).await);

    session_reply(
        &client,
        Command::SessionEnd {
            session: started.id.clone(),
        },
    )
    .await;

    assert!(!home.may_ask(&started.id).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_that_fails_to_restore_loses_run_commands_over_itself() {
    let home = Home::new();

    let (session, daemon) = home.fail_a_restore().await;

    let client = connect(&daemon).await;
    wait_for_state(&client, &session, SessionStateView::Failed).await;
    assert!(!home.may_ask(&session).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn end_stops_the_agent_and_releases_the_clone() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = session_reply(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: None,
            prompt: None,
            clone: Some("work".to_owned()),
        },
    )
    .await;
    let first = home.launches(1).await;

    let ended = session_reply(
        &client,
        Command::SessionEnd {
            session: started.id.clone(),
        },
    )
    .await;

    assert_eq!(ended.state, SessionStateView::Ended);
    assert!(!is_alive(first[0].pid));
    let checkouts = match request(&client, Command::CheckoutList { project: None }).await {
        Ok(Reply::Checkouts { checkouts }) => checkouts,
        other => panic!("{other:?}"),
    };
    let clone = checkouts
        .iter()
        .find(|checkout| checkout.id == started.checkout)
        .unwrap();
    assert_eq!(clone.occupant, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn on_daemon_start_a_session_whose_agent_is_gone_is_reopened_without_a_request() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    home.launches(1).await;
    drop(client);
    home.stop(daemon);
    home.lose_every_agent();

    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;

    let restored = sessions(&client, false).await;
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].id, started.id);
    assert_eq!(restored[0].state, SessionStateView::Running);
    assert_eq!(
        restored[0].launch.as_ref().unwrap().kind,
        LaunchKindView::Restore
    );
    let launches = home.launches(2).await;
    assert_eq!(
        (launches[1].mode.as_str(), launches[1].conversation.as_str()),
        ("resume", "conversation-1")
    );
    assert!(is_alive(launches[1].pid));
}

#[tokio::test(flavor = "multi_thread")]
async fn on_daemon_start_a_session_whose_agent_still_runs_keeps_it() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    let first = home.launches(1).await;
    drop(client);
    home.stop(daemon);

    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;

    let kept = sessions(&client, false).await;
    assert_eq!(kept[0].launch, started.launch);
    assert_eq!(kept[0].state, SessionStateView::Running);
    assert_eq!(home.logged_launches().len(), 1);
    assert!(is_alive(first[0].pid));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_restore_is_reported_and_not_retried() {
    let home = Home::new();

    let (session, daemon) = home.fail_a_restore().await;

    let client = connect(&daemon).await;
    let failed = sessions(&client, false).await;
    assert_eq!(failed[0].id, session);
    assert_eq!(failed[0].state, SessionStateView::Failed);
    assert!(
        failed[0].reason.as_ref().unwrap().contains("restore"),
        "{failed:?}"
    );
    drop(client);
    home.stop(daemon);

    let daemon = home.start(UNNAMED, settings()).await;
    let client = connect(&daemon).await;
    assert_eq!(
        sessions(&client, false).await[0].state,
        SessionStateView::Failed
    );
    assert_eq!(home.logged_launches().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_interrupted_by_a_daemon_crash_is_finished_on_start() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    let first = home.launches(1).await;
    drop(client);
    home.leave_stopping(&started.id, "asleep").await;
    home.stop(daemon);

    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;

    let finished = sessions(&client, false).await;
    assert_eq!(finished[0].state, SessionStateView::Asleep);
    assert!(!is_alive(first[0].pid));
}

/// #205: the stop of an interrupted restart is finished, and a relaunch that then fails is
/// a failed reopen, as for any restore: `failed`, not retried on the next start.
#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_restart_whose_relaunch_fails_on_start_is_left_failed() {
    let home = Home::new();
    let daemon = home.start(UNNAMED, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    let first = home.launches(1).await;
    drop(client);
    home.leave_stopping(&started.id, "restart").await;
    home.stop(daemon);

    let daemon = home.start(UNNAMED, settings()).await;
    let client = connect(&daemon).await;
    let failed = sessions(&client, false).await;
    assert_eq!(failed[0].state, SessionStateView::Failed, "{failed:?}");
    assert!(!is_alive(first[0].pid), "the old agent was stopped");
    drop(client);
    home.stop(daemon);

    let daemon = home.start(UNNAMED, settings()).await;
    let client = connect(&daemon).await;
    assert_eq!(
        sessions(&client, false).await[0].state,
        SessionStateView::Failed
    );
    assert_eq!(home.logged_launches().len(), 1, "never launched again");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_design_link_is_kept_on_the_session_and_anything_but_a_url_is_refused() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;

    let linked = session_reply(
        &client,
        Command::SessionLink {
            session: started.id.clone(),
            url: "https://claude.ai/design/canvas-1".to_owned(),
        },
    )
    .await;
    let refused = request(
        &client,
        Command::SessionLink {
            session: started.id.clone(),
            url: "file:///etc/passwd".to_owned(),
        },
    )
    .await;

    assert_eq!(
        linked.design_link.as_deref(),
        Some("https://claude.ai/design/canvas-1")
    );
    assert!(linked.revision > started.revision);
    assert_eq!(refusal_code(refused), ErrorCode::InvalidArgument);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_socket_refuses_every_session_command() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let agent = Connection::open(daemon.agent_socket_path(), GENEROUS)
        .await
        .unwrap();
    let session = "s000000000000".to_owned();

    for command in [
        Command::SessionNew {
            dir: "/".to_owned(),
            agent: None,
            prompt: None,
            clone: None,
        },
        Command::SessionList { ended: false },
        Command::SessionRestart {
            session: session.clone(),
        },
        Command::SessionRestartAll,
        Command::SessionSleep {
            session: session.clone(),
        },
        Command::SessionWake {
            session: session.clone(),
        },
        Command::SessionResume {
            session: session.clone(),
        },
        Command::SessionEnd {
            session: session.clone(),
        },
        Command::SessionLink {
            session: session.clone(),
            url: "https://example.com".to_owned(),
        },
    ] {
        assert_eq!(
            refusal_code(request(&agent, command).await),
            ErrorCode::NotAuthorized
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_configured_idle_window_decides_when_a_waiting_agent_sleeps() {
    let home = Home::new();
    let config_dir = home.path().join("home/.config/cockpit");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "[sessions]\nidle_sleep_after_minutes = 45\n",
    )
    .unwrap();
    let config = cockpit_config::load(&home.environment(), None, &[]).unwrap();
    let configured = cockpit_sessions::Settings {
        stop: settings().stop,
        ..cockpit_daemon::session_settings(&config)
    };
    let daemon = home.start(BASELINE, configured).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    home.report_as_agent(
        &home.launches(1).await[0],
        cockpit_sessions::AgentState::Waiting,
    )
    .await;

    // 46 minutes at the prompt: past the configured 45, short of the product's 60. The
    // launch moves back too, so the report still belongs to this agent.
    let waiting_since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 46 * 60;
    let waiting_since = i64::try_from(waiting_since).unwrap();
    let database = home.database.lock().unwrap().clone().unwrap();
    let write = database.write().await.unwrap();
    for statement in [
        "UPDATE session_launches SET launched_at = ?1",
        "UPDATE session_reports SET reported_at = ?1",
    ] {
        write.execute(statement, [waiting_since]).await.unwrap();
    }
    write.commit().await.unwrap();

    wait_for_state(&client, &started.id, SessionStateView::Asleep).await;
}

#[test]
fn a_clean_owner_configuration_keeps_the_sessions_product_defaults() {
    let home = Home::new();
    let config = cockpit_config::load(&home.environment(), None, &[]).unwrap();

    let configured = cockpit_daemon::session_settings(&config);
    let product = cockpit_sessions::Settings::default();

    assert_eq!(configured.idle_sleep_after, product.idle_sleep_after);
    assert_eq!(configured.asleep_expiry, product.asleep_expiry);
}
