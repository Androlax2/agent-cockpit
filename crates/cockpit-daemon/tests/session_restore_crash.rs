//! Reboot restore after a daemon crash, at the daemon's start (#251, #252, #256): what a
//! crash or a broken state database leaves midway through a stop or a launch is finished,
//! reported or refused there, before new work is accepted. Startup reports are served
//! during restore so a relaunched agent's first hook can reach the daemon (#1278).
//!
//! Same setting as `sessions.rs`: the daemon runs in this process with the fake provider, a
//! real state database, real processes and a private tmux server. Each fixture rewrites the
//! stored state the way the interrupted step leaves it.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes, StartError};
use cockpit_processes::{EndReason, ExecutionId, LaunchContainment, Phase, Supervisor};
use cockpit_protocol::{
    AgentReport, AgentState, Command, ErrorCode, LaunchKindView, Reply, SessionStateView,
    SessionView, TurnOutcome,
};
use cockpit_provider_api::fake::{FakeLaunch as AgentLaunch, FakeProvider};
use cockpit_provider_api::{
    Capabilities, CredentialHeader, Credentials, InvalidValue, LaunchCommand, LaunchRequest, Needs,
    Provider, ProviderError, ProviderName,
};
use common::GENEROUS;

struct Home {
    dir: tempfile::TempDir,
    /// The state database of the running daemon, for the fixtures.
    database: Mutex<Option<Arc<cockpit_turso::Database>>>,
    provider: Option<Arc<dyn Provider>>,
    settings: Option<cockpit_sessions::Settings>,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
            database: Mutex::new(None),
            provider: None,
            settings: None,
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
            let provider = self.provider.clone().unwrap_or_else(|| {
                Arc::new(FakeProvider::new(
                    ProviderName::new("fake").unwrap(),
                    self.path().join("agent.log"),
                ))
            });
            let mut wiring = common::wiring_in(&self.path(), vec![provider]);
            if let Some(settings) = &self.settings {
                wiring.sessions = settings.clone();
                wiring.session_tick = Duration::from_millis(100);
            }
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
                agent: Some(
                    self.provider
                        .as_ref()
                        .map_or("fake", |p| p.name().as_str())
                        .to_owned(),
                ),
                prompt: None,
                clone: None,
                model: None,
                effort: None,
                compaction_threshold: None,
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
    let error = match home.try_start().await {
        Ok(_) => panic!("the daemon started"),
        Err(error) => error,
    };
    let deadline = Instant::now() + GENEROUS;
    while tokio::net::UnixStream::connect(home.options().agent_socket_path())
        .await
        .is_ok()
    {
        assert!(
            Instant::now() < deadline,
            "a refused startup kept its agent listener running"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    error
}

/// The provider probes the socket synchronously in launch preparation: this cannot race
/// with restore finishing, unlike a child scheduled after its launch has returned.
struct ReportingProvider {
    inner: FakeProvider,
    socket: PathBuf,
    owner_socket: PathBuf,
    result: PathBuf,
    launches: AtomicUsize,
    is_restoring: AtomicBool,
    quota_barrier: Option<PathBuf>,
}

impl Provider for ReportingProvider {
    fn name(&self) -> &ProviderName {
        self.inner.name()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn reports_at_launch(&self) -> bool {
        true
    }

    fn needs(&self) -> Result<Needs, InvalidValue> {
        Needs::new(&["api.example.com"], &[], &[])
    }

    fn credentials(&self) -> Result<Credentials, InvalidValue> {
        Credentials::new(
            &[],
            &[(
                "FIXTURE_TOKEN",
                CredentialHeader::Bearer,
                &["api.example.com"],
            )],
            &self.needs()?,
        )
    }

    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        if !self.is_restoring.load(Ordering::Acquire) {
            return self.inner.launch(request);
        }
        let index = self.launches.fetch_add(1, Ordering::SeqCst);
        if index >= 2 {
            return self.inner.launch(request);
        }
        tokio::runtime::Handle::current()
            .block_on(async {
                assert!(tokio::net::UnixStream::connect(&self.owner_socket).await.is_err(),
                    "owner clients must not connect to an unserved socket during restore");
                let connection = Connection::open(&self.socket, GENEROUS).await?;
                let reply = connection.request(Command::AgentEnd {
                    launch_token: None,
                    session: "s1".to_owned(),
                }, GENEROUS).await;
                assert!(matches!(reply, Err(ClientError::Daemon(ref error)) if error.code == ErrorCode::Busy),
                    "new work must wait for restoration: {reply:?}");
                Ok::<_, ClientError>(())
            })
            .map_err(|error| ProviderError {
                reason: format!("the agent socket must answer before restore launches: {error}"),
            })?;
        if index == 1 {
            if let Some(barrier) = &self.quota_barrier {
                std::fs::write(barrier.with_extension("blocked"), "restoring").unwrap();
            }
            let deadline = Instant::now() + GENEROUS;
            while !self.result.exists() {
                assert!(
                    Instant::now() < deadline,
                    "the first agent must report during restore"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        if index == 1
            && let Some(barrier) = &self.quota_barrier
        {
            let deadline = Instant::now() + GENEROUS;
            while !barrier.exists() {
                assert!(Instant::now() < deadline, "restore was not released");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let mut launch = self.inner.launch(request)?;
        launch.program = std::env::current_exe().unwrap();
        launch.arguments = vec![
            "--exact".into(),
            "restored_agent_reports".into(),
            "--nocapture".into(),
        ];
        launch.environment.extend([
            ("RESTORE_AGENT_SOCKET".into(), self.socket.clone().into()),
            ("RESTORE_REPORT_RESULT".into(), self.result.clone().into()),
        ]);
        if index == 0
            && let Some(barrier) = &self.quota_barrier
        {
            launch
                .environment
                .push(("RESTORE_QUOTA_BARRIER".into(), barrier.clone().into()));
        }
        Ok(launch)
    }
}

/// Runs as the restored agent's real process, so its report exercises authentication too.
#[tokio::test]
async fn restored_agent_reports() {
    let Some(socket) = std::env::var_os("RESTORE_AGENT_SOCKET") else {
        return;
    };
    let connection = Connection::open(Path::new(&socket), GENEROUS)
        .await
        .unwrap();
    let quota_barrier = std::env::var_os("RESTORE_QUOTA_BARRIER");
    if let Some(barrier) = &quota_barrier {
        let deadline = Instant::now() + GENEROUS;
        while !Path::new(barrier).with_extension("blocked").exists() {
            assert!(Instant::now() < deadline, "second restore never started");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    let reply = connection
        .request(
            Command::AgentReport {
                launch_token: Some(std::env::var("TORLEK_LAUNCH_TOKEN").unwrap()),
                report: AgentReport {
                    state: Some(AgentState::Waiting),
                    outcome: quota_barrier.map(|_| TurnOutcome::QuotaStopped),
                    ..AgentReport::default()
                },
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(reply, Ok(Reply::AgentReported { .. })),
        "{reply:?}"
    );
    let brief = connection
        .request(
            Command::AgentBrief {
                launch_token: Some(std::env::var("TORLEK_LAUNCH_TOKEN").unwrap()),
            },
            GENEROUS,
        )
        .await;
    assert!(matches!(brief, Ok(Reply::AgentBrief { .. })), "{brief:?}");
    let inbox = connection
        .request(
            Command::AgentInbox {
                launch_token: Some(std::env::var("TORLEK_LAUNCH_TOKEN").unwrap()),
                waiter: None,
            },
            GENEROUS,
        )
        .await;
    assert!(matches!(inbox, Ok(Reply::AgentInbox { .. })), "{inbox:?}");
    let result = PathBuf::from(std::env::var_os("RESTORE_REPORT_RESULT").unwrap());
    let staged = result.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&staged, std::env::var("TORLEK_SESSION").unwrap()).unwrap();
    std::fs::rename(staged, result).unwrap();
    std::future::pending::<()>().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restored_agent_can_report_on_its_first_attempt() {
    let mut home = Home::new();
    let (daemon, started) = home.running_session().await;
    let client = connect(&daemon).await;
    let second = session_reply(
        &client,
        Command::SessionNew {
            dir: started.working_dir.clone(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
            model: None,
            effort: None,
            compaction_threshold: None,
        },
    )
    .await;
    home.wait_for_launches(2).await;
    drop(client);
    home.stop(daemon);
    home.lose_every_agent();
    let result = home.path().join("restore-report");
    home.provider = Some(Arc::new(ReportingProvider {
        inner: FakeProvider::new(
            ProviderName::new("fake").unwrap(),
            home.path().join("agent.log"),
        )
        .reporting_at_launch(),
        socket: home.options().agent_socket_path(),
        owner_socket: home.options().socket_path(),
        result: result.clone(),
        launches: AtomicUsize::new(0),
        is_restoring: AtomicBool::new(true),
        quota_barrier: None,
    }));

    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let restored = listed(&client, &started.id).await;
    assert_eq!(restored.state, SessionStateView::Running, "{restored:?}");
    assert_eq!(
        listed(&client, &second.id).await.state,
        SessionStateView::Running
    );
    let deadline = Instant::now() + GENEROUS;
    loop {
        let Reply::AgentReports { reports } = request(&client, Command::AgentReports).await else {
            panic!("expected agent reports");
        };
        if [&started.id, &second.id].iter().all(|id| {
            reports.iter().any(|report| {
                &report.session == *id
                    && report.state == Some(AgentState::Waiting)
                    && report.needs_input.is_none()
            })
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "not every restored agent reported: {reports:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Two real account selections, with only the sandbox launcher replaced: the test exercises
/// restore/report/tick ordering, not namespace enforcement. No live credential is read.
fn account_settings(home: &Home) -> cockpit_sessions::Settings {
    let environment = cockpit_config::Environment {
        home: Some(home.path().join("home").into()),
        ..Default::default()
    };
    for account in ["work", "spare"] {
        let dir = cockpit_config::account_credentials_dir(&environment, "claude", account).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let credential = dir.join("FIXTURE_TOKEN");
        std::fs::write(&credential, "fixture-only-token").unwrap();
        std::fs::set_permissions(credential, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let config = home.path().join("home/.config/torlek");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "[accounts]\nclaude = [\"work\", \"spare\"]\n",
    )
    .unwrap();
    let accounts = cockpit_config::load(&environment, None, &[])
        .unwrap()
        .accounts;
    let launcher = home.path().join("sandbox-fixture");
    std::fs::write(
        &launcher,
        r#"#!/usr/bin/python3
import json, os, sys
assert sys.argv[1] == "run"
with open(sys.argv[2]) as source:
    spec = json.load(source)
os.unlink(sys.argv[2])
os.chdir(spec["working_dir"])
os.execv(spec["program"], [spec["program"], *spec["arguments"]])
"#,
    )
    .unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
    let options = home.options();
    cockpit_sessions::Settings {
        owner: cockpit_config::SessionSettings {
            sandbox: cockpit_config::AgentSandbox::Required,
            ..cockpit_sessions::Settings::default().owner
        },
        accounts,
        base_environment: vec![
            ("HOME".into(), home.path().join("home").into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ],
        sandbox: cockpit_sandbox::Sandbox::new(
            cockpit_sandbox::Platform::Linux,
            cockpit_sandbox::HostPaths {
                read_only: Vec::new(),
                sockets: vec![options.agent_socket_path().parent().unwrap().to_path_buf()],
                denied: Vec::new(),
                denied_write: Vec::new(),
                owner_uid: std::fs::metadata(home.path()).unwrap().uid(),
                mount_sources: options.mount_sources(),
                homes: options.session_homes(),
                logins: options.account_logins(),
            },
            cockpit_sandbox::Backend::Native(cockpit_sandbox::NativeBackend {
                launcher,
                bubblewrap: "/unused/bwrap".into(),
                bubblewrap_version: "fixture".to_owned(),
                landlock_abi: 6,
            }),
        ),
        ..cockpit_sessions::Settings::default()
    }
}

/// Removing the report's restore guard must fail the held-restore assertion; removing the
/// periodic retry must fail the eventual switch. The agent sends no second quota report.
#[tokio::test(flavor = "multi_thread")]
async fn a_quota_stop_during_restore_waits_then_switches_accounts_once() {
    let mut home = Home::new();
    let barrier = home.path().join("finish-restore");
    let result = home.path().join("quota-reported");
    let provider = Arc::new(ReportingProvider {
        inner: FakeProvider::new(
            ProviderName::new("claude").unwrap(),
            home.path().join("agent.log"),
        )
        .reporting_at_launch(),
        socket: home.options().agent_socket_path(),
        owner_socket: home.options().socket_path(),
        result: result.clone(),
        launches: AtomicUsize::new(0),
        is_restoring: AtomicBool::new(false),
        quota_barrier: Some(barrier.clone()),
    });
    home.settings = Some(account_settings(&home));
    home.provider = Some(provider.clone());
    let (daemon, first) = home.running_session().await;
    let client = connect(&daemon).await;
    let second = session_reply(
        &client,
        Command::SessionNew {
            dir: first.working_dir.clone(),
            agent: Some("claude".to_owned()),
            prompt: None,
            clone: None,
            model: None,
            effort: None,
            compaction_threshold: None,
        },
    )
    .await;
    for session in [&first, &second] {
        assert_eq!(
            session.launch.as_ref().unwrap().account.as_deref(),
            Some("work")
        );
    }
    home.wait_for_launches(2).await;
    drop(client);
    home.stop(daemon);
    home.lose_every_agent();
    provider.is_restoring.store(true, Ordering::Release);

    let hold_restore = async {
        let deadline = Instant::now() + GENEROUS;
        while !result.exists() {
            assert!(Instant::now() < deadline, "no quota report during restore");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let id =
            cockpit_sessions::SessionId::parse(&std::fs::read_to_string(&result).unwrap()).unwrap();
        // A negative observation needs time for the erroneous background relaunch to run.
        // The barrier keeps restore open for the whole interval, independent of host speed.
        let until = Instant::now() + Duration::from_secs(1);
        loop {
            let database = home.database();
            let read = database.read().await.unwrap();
            let session = cockpit_sessions::session(&read, &id).await.unwrap();
            assert_eq!(
                session.state,
                cockpit_sessions::SessionState::Running,
                "quota report started a relaunch before restore finished: {session:?}"
            );
            let launch = session.launch.unwrap();
            assert_eq!(launch.kind, cockpit_sessions::LaunchKind::Restore);
            assert_eq!(launch.account.as_deref(), Some("work"));
            if Instant::now() >= until {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        std::fs::write(&barrier, "finish").unwrap();
        id
    };
    let (daemon, stopped) = tokio::join!(home.start(), hold_restore);
    let client = connect(&daemon).await;
    let deadline = Instant::now() + GENEROUS;
    let switched = loop {
        let session = listed(&client, stopped.as_str()).await;
        if session.state == SessionStateView::Running
            && session.launch.as_ref().unwrap().account.as_deref() == Some("spare")
        {
            break session;
        }
        assert!(
            Instant::now() < deadline,
            "periodic pass did not switch accounts: {session:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let launch = switched.launch.unwrap();
    assert_eq!(launch.kind, LaunchKindView::Restart);
    assert_eq!(launch.switched_from.as_deref(), Some("work"));
    // Several more periodic passes must not repeat the switch.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        listed(&client, stopped.as_str())
            .await
            .launch
            .unwrap()
            .number,
        launch.number
    );
    assert_eq!(provider.launches.load(Ordering::SeqCst), 3);
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

/// #251: the relaunch that finishes an interrupted restart fails on a database error
/// (the authorization grants a launch writes), and so does recording the session `failed`;
/// a failure that cannot be recorded stops the start, as on the reopen path.
#[tokio::test(flavor = "multi_thread")]
async fn a_relaunch_failure_that_cannot_be_recorded_stops_the_daemon_start() {
    let home = Home::new();
    let (daemon, started) = home.running_session().await;
    home.rewrite(
        "UPDATE sessions SET state = 'stopping', stopping_to = 'restart' WHERE id = ?1",
        &started.id,
    )
    .await;
    home.drop_table("authorization_grants").await;
    home.stop(daemon);

    let error = refused_start(&home).await;

    assert!(matches!(error, StartError::Restore(_)), "{error}");
    assert!(
        error.to_string().contains("authorization_grants"),
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
