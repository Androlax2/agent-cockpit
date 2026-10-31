//! Issue #35 at the daemon's socket: the provider list the new-session form offers, the
//! manager tmux session (the TUI's column and the viewer beside it) and showing a session in
//! the viewer, on a private tmux server with a fake provider.
//!
//! The daemon runs in this process because the fake provider is a test type the binary does
//! not link (as in `sessions.rs`, whose fixture this file does not share so the two can
//! change apart; both use `cockpit_provider_api::fake`).
#![cfg(target_os = "linux")]

mod common;

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{
    Command, ErrorCode, LaunchKindView, ProviderView, Reply, SessionStateView, SessionView,
};
use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{Provider, ProviderName};
use common::GENEROUS;

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    async fn start(&self) -> Daemon {
        self.start_with_tmux(PathBuf::from("tmux")).await
    }

    /// Starts the daemon with `tmux` as its tmux binary.
    async fn start_with_tmux(&self, tmux: PathBuf) -> Daemon {
        let log = self.path().join("agents.log");
        let providers: Vec<Arc<dyn Provider>> = vec![
            Arc::new(FakeProvider::new(
                ProviderName::new("fake").unwrap(),
                log.clone(),
            )),
            Arc::new(FakeProvider::new(ProviderName::new("plain").unwrap(), log).without_resume()),
        ];
        let mut wiring = common::wiring_in(&self.path(), providers);
        wiring.tmux.tmux = tmux;
        wiring.sessions.stop = cockpit_processes::Escalation {
            grace: Duration::from_secs(2),
            kill_wait: Duration::from_secs(5),
        };
        let options = Options {
            runtime_dir: self.path().join("run/cockpit"),
            state_dir: self.path().join("state/cockpit"),
            limits: Limits::default(),
        };
        Instance::acquire(options)
            .unwrap()
            .start(wiring, |capabilities| {
                Arc::new(Routes::new(
                    capabilities,
                    cockpit_config::Environment {
                        home: Some(self.path().join("home").into()),
                        ..cockpit_config::Environment::default()
                    },
                ))
            })
            .await
            .expect("the daemon starts")
    }

    fn tmux(&self, arguments: &[&str]) -> String {
        let output = Process::new("tmux")
            .arg("-S")
            .arg(self.path().join("tmux.sock"))
            .args(arguments)
            .env_remove("TMUX")
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
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

    async fn open_manager(&self, client: &Connection) -> Result<Reply, ClientError> {
        request(
            client,
            Command::ManagerOpen {
                dir: self.path().display().to_string(),
                column: vec!["/bin/sleep".to_owned(), "600".to_owned()],
                viewer: vec!["/bin/sleep".to_owned(), "601".to_owned()],
            },
        )
        .await
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
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

async fn request(client: &Connection, command: Command) -> Result<Reply, ClientError> {
    client.request(command, Duration::from_secs(60)).await
}

async fn session(client: &Connection, command: Command) -> SessionView {
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

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + GENEROUS;
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_provider_list_names_each_wired_provider_and_whether_it_resumes() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    let reply = request(&client, Command::ProviderList).await.unwrap();

    assert_eq!(
        reply,
        Reply::Providers {
            providers: vec![
                ProviderView {
                    name: "fake".to_owned(),
                    resume: true
                },
                ProviderView {
                    name: "plain".to_owned(),
                    resume: false
                },
            ]
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_opens_its_tmux_server_once_for_the_sessions_and_the_manager() {
    let home = Home::new();
    // Opening the server checks the version (`tmux -V`), then writes the config; nothing
    // else writes it. One check that found no config, and a config after, is one open.
    let checks = home.path().join("version-checks.log");
    let config = home.path().join("tmux.conf");
    let wrapper = home.path().join("tmux-wrapper");
    // The wrapper finds the config and its log beside itself, so no path is quoted into it.
    std::fs::write(
        &wrapper,
        "#!/bin/sh\n\
         here=$(dirname \"$0\")\n\
         if [ \"$1\" = -V ]; then\n\
         if [ -e \"$here/tmux.conf\" ]; then echo config-written >> \"$here/version-checks.log\"; \
         else echo no-config >> \"$here/version-checks.log\"; fi\n\
         fi\n\
         exec tmux \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, Permissions::from_mode(0o755)).unwrap();
    let daemon = home.start_with_tmux(wrapper).await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;

    home.open_manager(&client).await.unwrap();
    session(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
        },
    )
    .await;

    assert_eq!(std::fs::read_to_string(&checks).unwrap(), "no-config\n");
    assert!(config.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn without_tmux_the_manager_and_sessions_are_refused_naming_the_same_cause_and_fix() {
    let home = Home::new();
    let missing_tmux = home.path().join("no-tmux-here");
    let daemon = home.start_with_tmux(missing_tmux.clone()).await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;

    let manager = refusal(home.open_manager(&client).await);
    let sessions = refusal(
        request(
            &client,
            Command::SessionNew {
                dir: root.display().to_string(),
                agent: Some("fake".to_owned()),
                prompt: None,
                clone: None,
            },
        )
        .await,
    );

    assert_eq!(
        (manager.code, sessions.code),
        (ErrorCode::Refused, ErrorCode::Refused)
    );
    let reason = format!("`{}` could not be run", missing_tmux.display());
    for message in [&manager.message, &sessions.message] {
        assert!(message.contains(&reason), "{message}");
        assert!(message.contains("restart the daemon"), "{message}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_manager_opens_a_column_and_a_viewer_once_and_answers_how_to_attach() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    let first = home.open_manager(&client).await.unwrap();
    let second = home.open_manager(&client).await.unwrap();

    let socket = home.path().join("tmux.sock").display().to_string();
    let attach: Vec<String> = ["tmux", "-S", &socket, "attach-session", "-t", "=cockpit:"]
        .map(str::to_owned)
        .to_vec();
    assert_eq!(
        (first, second),
        (
            Reply::Manager {
                attach: attach.clone(),
                created: true
            },
            Reply::Manager {
                attach,
                created: false
            }
        )
    );
    let panes = home.tmux(&[
        "list-panes",
        "-t",
        "=cockpit:",
        "-F",
        "#{pane_left} #{pane_start_command}",
    ]);
    let commands: Vec<&str> = panes.lines().collect();
    assert_eq!(commands.len(), 2, "{panes}");
    assert!(
        commands[0].starts_with("0 ") && commands[0].contains("600"),
        "{panes}"
    );
    assert!(
        !commands[1].starts_with("0 ") && commands[1].contains("601"),
        "{panes}"
    );
}

/// The pane an attached client's keys go to: the window's active pane.
fn focused_pane_command(home: &Home) -> String {
    home.tmux(&[
        "display-message",
        "-p",
        "-t",
        "=cockpit:",
        "#{pane_start_command}",
    ])
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_manager_answers_with_its_column_focused() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    home.open_manager(&client).await.unwrap();

    let focused = focused_pane_command(&home);
    assert!(focused.contains("600"), "{focused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn reopening_the_manager_focuses_its_column_again() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.open_manager(&client).await.unwrap();
    home.tmux(&["select-pane", "-t", "=cockpit:.1"]);

    home.open_manager(&client).await.unwrap();

    let focused = focused_pane_command(&home);
    assert!(focused.contains("600"), "{focused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn showing_an_asleep_session_wakes_it_and_attaches_the_viewer_to_it() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    home.open_manager(&client).await.unwrap();
    let started = session(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
        },
    )
    .await;
    session(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await;

    let shown = session(
        &client,
        Command::SessionShow {
            session: started.id.clone(),
        },
    )
    .await;

    assert_eq!(shown.state, SessionStateView::Running);
    assert_eq!(shown.launch.unwrap().kind, LaunchKindView::Wake);
    eventually("a client in the viewer is attached to the session", || {
        home.tmux(&["list-clients", "-F", "#{session_name}"])
            .lines()
            .any(|attached| attached == started.id)
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn showing_a_session_before_the_manager_is_open_is_refused_naming_the_fix() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    let started = session(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
        },
    )
    .await;

    let error = refusal(
        request(
            &client,
            Command::SessionShow {
                session: started.id,
            },
        )
        .await,
    );

    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(
        error.message.contains("run `cockpit tui`"),
        "{}",
        error.message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_may_neither_open_the_manager_nor_list_providers() {
    let home = Home::new();
    let daemon = home.start().await;
    let agent = Connection::open(daemon.agent_socket_path(), GENEROUS)
        .await
        .unwrap();

    let codes = [
        refusal(home.open_manager(&agent).await).code,
        refusal(request(&agent, Command::ProviderList).await).code,
    ];

    assert_eq!(codes, [ErrorCode::NotAuthorized, ErrorCode::NotAuthorized]);
}

#[tokio::test(flavor = "multi_thread")]
async fn showing_an_asleep_session_without_a_manager_leaves_it_asleep() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    let started = session(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
        },
    )
    .await;
    session(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await;

    let error = refusal(
        request(
            &client,
            Command::SessionShow {
                session: started.id.clone(),
            },
        )
        .await,
    );

    assert_eq!(error.code, ErrorCode::NotFound);
    let listed = match request(&client, Command::SessionList { ended: false }).await {
        Ok(Reply::Sessions { sessions }) => sessions,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        listed
            .iter()
            .find(|listed| listed.id == started.id)
            .map(|listed| listed.state.clone()),
        Some(SessionStateView::Asleep)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_manager_that_lost_its_viewer_is_built_again_on_the_next_open() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    home.open_manager(&client).await.unwrap();
    home.tmux(&["kill-pane", "-t", "=cockpit:.1"]);

    let reopened = home.open_manager(&client).await.unwrap();

    assert!(
        matches!(reopened, Reply::Manager { created: true, .. }),
        "{reopened:?}"
    );
    let panes = home.tmux(&["list-panes", "-t", "=cockpit:", "-F", "#{pane_id}"]);
    assert_eq!(panes.lines().count(), 2, "{panes}");
}
