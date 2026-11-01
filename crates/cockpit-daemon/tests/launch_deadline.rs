//! Issue #272: a launch whose step never returns fails once the step timeout passes, so the
//! owner can end the session instead of being refused `conflict` until the daemon restarts.
//! The real socket and routes; the fixture provider's `launch` never returns while its gate
//! is closed, as a hung provider or tmux step would hold the launch.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{Command, ErrorCode, Reply, SessionStateView};
use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{
    Capabilities, LaunchCommand, LaunchRequest, Provider, ProviderError, ProviderName,
};
use common::GENEROUS;

/// Short, so a hung launch fails within the test.
const STEP_TIMEOUT: Duration = Duration::from_millis(500);

/// The fake provider, whose `launch` blocks while `is_closed` holds: it breaks the provider
/// contract on purpose, as a provider that hangs would.
struct HangingProvider {
    inner: FakeProvider,
    is_closed: Arc<AtomicBool>,
}

impl Provider for HangingProvider {
    fn name(&self) -> &ProviderName {
        self.inner.name()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        while self.is_closed.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.inner.launch(request)
    }
}

struct Home {
    _dir: tempfile::TempDir,
    root: PathBuf,
    is_closed: Arc<AtomicBool>,
    /// The tmux the daemon runs: the real one unless a test hangs it.
    tmux: PathBuf,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::tempdir().unwrap();
        Home {
            root: dir.path().canonicalize().unwrap(),
            _dir: dir,
            is_closed: Arc::new(AtomicBool::new(true)),
            tmux: PathBuf::from("tmux"),
        }
    }

    /// A home whose provider answers at once and whose tmux never answers `new-session`, as
    /// a hung tmux server would hold a terminal open.
    fn with_a_tmux_that_hangs_opening_a_terminal() -> Home {
        let mut home = Home::new();
        home.is_closed.store(false, Ordering::SeqCst);
        let tmux = home.root.join("hanging-tmux");
        std::fs::write(
            &tmux,
            "#!/bin/sh\nfor word; do [ \"$word\" = new-session ] && exec sleep 3; done\nexec tmux \"$@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&tmux, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        home.tmux = tmux;
        home
    }

    async fn start(&self) -> Daemon {
        let mut wiring = common::wiring_in(
            &self.root,
            vec![Arc::new(HangingProvider {
                inner: FakeProvider::new(
                    ProviderName::new("fake").unwrap(),
                    self.root.join("agent.log"),
                ),
                is_closed: Arc::clone(&self.is_closed),
            })],
        );
        wiring.sessions.step_timeout = STEP_TIMEOUT;
        wiring.tmux.tmux = self.tmux.clone();
        let options = Options {
            runtime_dir: self.root.join("run/cockpit"),
            state_dir: self.root.join("state/cockpit"),
            limits: Limits::default(),
        };
        Instance::acquire(options)
            .unwrap()
            .start(wiring, |capabilities| {
                Arc::new(Routes::new(
                    capabilities,
                    cockpit_config::Environment {
                        home: Some(self.root.join("home").into()),
                        ..cockpit_config::Environment::default()
                    },
                ))
            })
            .await
            .unwrap()
    }

    async fn project(&self, client: &Connection) -> PathBuf {
        let root = self.root.join("projects/app");
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
}

impl Drop for Home {
    fn drop(&mut self) {
        self.is_closed.store(false, Ordering::SeqCst);
        let _ = Process::new("tmux")
            .arg("-S")
            .arg(self.root.join("tmux.sock"))
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

async fn request(client: &Connection, command: Command) -> Result<Reply, ClientError> {
    client.request(command, GENEROUS).await
}

/// Starts a session whose launch hangs and returns the client and the daemon's answer.
async fn a_hung_launch(home: &Home) -> (Daemon, Connection, Result<Reply, ClientError>) {
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    let answer = request(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
        },
    )
    .await;
    (daemon, client, answer)
}

async fn the_only_session(client: &Connection) -> cockpit_protocol::SessionView {
    match request(client, Command::SessionList { ended: true }).await {
        Ok(Reply::Sessions { mut sessions }) if sessions.len() == 1 => sessions.remove(0),
        other => panic!("session-list: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_launch_whose_step_never_returns_is_answered_with_the_step_timeout() {
    let home = Home::new();
    let (_daemon, _client, answer) = a_hung_launch(&home).await;

    let error = common::refused(answer);

    assert_eq!(error.code, ErrorCode::Internal, "{error:?}");
    assert!(error.message.contains("did not finish within"), "{error:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_launch_whose_step_never_returns_leaves_its_session_failed() {
    let home = Home::new();
    let (_daemon, client, _answer) = a_hung_launch(&home).await;

    let session = the_only_session(&client).await;

    assert_eq!(session.state, SessionStateView::Failed, "{session:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_whose_launch_hung_can_be_ended() {
    let home = Home::new();
    let (_daemon, client, _answer) = a_hung_launch(&home).await;
    let id = the_only_session(&client).await.id;

    let ended = request(&client, Command::SessionEnd { session: id }).await;

    match ended {
        Ok(Reply::Session { session }) => assert_eq!(session.state, SessionStateView::Ended),
        other => panic!("session-end: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_launch_whose_terminal_never_opens_fails_and_its_session_can_be_ended() {
    let home = Home::with_a_tmux_that_hangs_opening_a_terminal();
    let (_daemon, client, answer) = a_hung_launch(&home).await;
    let error = common::refused(answer);
    assert!(error.message.contains("open the terminal"), "{error:?}");
    let id = the_only_session(&client).await.id;

    let ended = request(&client, Command::SessionEnd { session: id }).await;

    match ended {
        Ok(Reply::Session { session }) => assert_eq!(session.state, SessionStateView::Ended),
        other => panic!("session-end: {other:?}"),
    }
}
