//! A clean home in a temporary directory and the `cockpit-daemon` binary started in it.
#![allow(
    dead_code,
    reason = "each test file uses a different part of the fixture"
)]

use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use cockpit_agent_channel::{LaunchToken, ProcessIdentity};
use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{ProtocolError, Reply};

/// Long enough never to fire in a passing test, short enough to fail a hung one quickly.
pub const GENEROUS: Duration = Duration::from_secs(10);

pub struct TestHome {
    pub root: tempfile::TempDir,
    /// `XDG_RUNTIME_DIR`, or `None` to leave it unset as on macOS.
    pub runtime_base: Option<PathBuf>,
    /// `false` leaves `XDG_STATE_HOME` and `XDG_CONFIG_HOME` unset, so the daemon derives
    /// them from `HOME`; [`TestHome::bare`] also leaves `XDG_RUNTIME_DIR` unset.
    pub sets_xdg_dirs: bool,
    pub log_level: Option<String>,
    /// The daemon's `PATH`; unset by default, so it finds no agent program.
    pub path: Option<std::ffi::OsString>,
}

impl TestHome {
    pub fn new() -> TestHome {
        let root = tempfile::tempdir().expect("a temporary home");
        let runtime_base = root.path().join("run");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&runtime_base)
            .expect("the runtime base is creatable");
        TestHome {
            root,
            runtime_base: Some(runtime_base),
            sets_xdg_dirs: true,
            log_level: None,
            path: None,
        }
    }

    /// A home with nothing in it and only `HOME` in the environment.
    pub fn bare() -> TestHome {
        TestHome {
            root: tempfile::tempdir().expect("a temporary home"),
            runtime_base: None,
            sets_xdg_dirs: false,
            log_level: None,
            path: None,
        }
    }

    pub fn home(&self) -> &Path {
        self.root.path()
    }

    pub fn state_home(&self) -> PathBuf {
        if self.sets_xdg_dirs {
            self.home().join("state")
        } else {
            self.home().join(".local/state")
        }
    }

    pub fn socket_dir(&self) -> PathBuf {
        match &self.runtime_base {
            Some(base) => base.join("cockpit"),
            None => self.state_home().join("cockpit/run"),
        }
    }

    pub fn socket_path(&self) -> PathBuf {
        self.socket_dir().join(cockpit_config::DAEMON_SOCKET)
    }

    pub fn agent_socket_path(&self) -> PathBuf {
        self.socket_dir()
            .join(cockpit_config::AGENT_SOCKET_DIR)
            .join(cockpit_config::AGENT_SOCKET)
    }

    /// The state database the daemon opens, for a test to prepare while no daemon runs.
    pub async fn open_state(&self) -> cockpit_turso::Database {
        let state_dir = self.state_home().join("cockpit");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&state_dir)
            .expect("the state directory is creatable");
        cockpit_turso::Database::open(&state_dir.join("state.db"), &cockpit_daemon::MIGRATION_SETS)
            .await
            .expect("the state database opens")
    }

    pub fn log_file(&self) -> PathBuf {
        self.state_home().join("cockpit/logs/daemon/cockpit.log")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cockpit-daemon"));
        command
            .env_clear()
            .env("HOME", self.home())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if self.sets_xdg_dirs {
            command
                .env("XDG_STATE_HOME", self.state_home())
                .env("XDG_CONFIG_HOME", self.home().join("config"));
        }
        if let Some(base) = &self.runtime_base {
            command.env("XDG_RUNTIME_DIR", base);
        }
        if let Some(level) = &self.log_level {
            command.env("COCKPIT_LOG_LEVEL", level);
        }
        if let Some(path) = &self.path {
            command.env("PATH", path);
        }
        command
    }

    /// Starts the daemon and waits for its `ready` line.
    pub fn start(&self) -> RunningDaemon {
        let mut child = self.command().spawn().expect("the daemon binary runs");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (lines, ready) = mpsc::channel();
        std::thread::spawn(move || {
            let mut first = String::new();
            let _ = BufReader::new(stdout).read_line(&mut first);
            let _ = lines.send(first);
        });
        let line = ready.recv_timeout(GENEROUS).unwrap_or_default();
        let mut daemon = RunningDaemon { child };
        if line.trim() != format!("ready {}", self.socket_path().display()) {
            let mut stderr = String::new();
            let _ = daemon.child.kill();
            let _ = daemon
                .child
                .stderr
                .take()
                .map(|mut pipe| pipe.read_to_string(&mut stderr));
            panic!("the daemon did not report ready: stdout {line:?}, stderr {stderr:?}");
        }
        daemon
    }

    /// Runs the daemon expecting it to exit on its own.
    pub fn run_to_exit(&self) -> Output {
        let child = self.command().spawn().expect("the daemon binary runs");
        let (done, exited) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(child.wait_with_output());
        });
        exited
            .recv_timeout(GENEROUS)
            .expect("the daemon exits")
            .expect("its output is readable")
    }

    /// The daemon's log once it holds `needle`, or as it is after [`GENEROUS`]. Records
    /// reach the file through the log writer thread, in the order they were made.
    pub fn log_once_it_contains(&self, needle: &str) -> String {
        let deadline = std::time::Instant::now() + GENEROUS;
        loop {
            let log = std::fs::read_to_string(self.log_file()).unwrap_or_default();
            if log.contains(needle) || std::time::Instant::now() > deadline {
                return log;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Launches `session` rooted at `root` while no daemon runs, granted `permission` over
    /// itself when one is given.
    pub async fn launch(
        &self,
        session: &str,
        root: &ProcessIdentity,
        permission: Option<Permission>,
    ) -> LaunchToken {
        let session = SessionId::new(session).expect("a valid session id");
        let database = self.open_state().await;
        let write = database.write().await.unwrap();
        let token = cockpit_agent_channel::mint(&write, &session).await.unwrap();
        cockpit_agent_channel::record_root(&write, &session, root)
            .await
            .unwrap();
        if let Some(permission) = permission {
            cockpit_authorization::grant(
                &write,
                &session,
                permission,
                &Scope::Session(session.clone()),
            )
            .await
            .unwrap();
        }
        write.commit().await.unwrap();
        token
    }

    pub async fn connect(&self) -> Connection {
        Connection::open(&self.socket_path(), GENEROUS)
            .await
            .expect("the daemon accepts the handshake")
    }

    pub async fn connect_agent(&self) -> Connection {
        Connection::open(&self.agent_socket_path(), GENEROUS)
            .await
            .expect("the daemon accepts the handshake on the agent socket")
    }
}

pub struct RunningDaemon {
    pub child: Child,
}

impl RunningDaemon {
    /// `kill -9`: no cleanup runs, as after a crash.
    pub fn kill(mut self) {
        self.child.kill().expect("the daemon is killable");
        self.child.wait().expect("the daemon is reaped");
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What an in-process daemon is wired with in tests: a private tmux server in `dir`, never
/// the user's, and `providers`.
pub fn wiring_in(
    dir: &Path,
    providers: Vec<std::sync::Arc<dyn cockpit_provider_api::Provider>>,
) -> cockpit_daemon::Wiring {
    cockpit_daemon::Wiring {
        tmux: cockpit_tmux::ServerSpec {
            tmux: PathBuf::from("tmux"),
            socket: cockpit_tmux::Socket::Path(dir.join("tmux.sock")),
            config_path: dir.join("tmux.conf"),
        },
        git: cockpit_projects::Git::from_path(),
        providers,
        sessions: cockpit_sessions::Settings::default(),
        session_tick: Duration::from_secs(3600),
        run_settings: cockpit_daemon::run_settings(Vec::new()).unwrap(),
    }
}

/// The daemon's refusal of a request expected to be refused.
pub fn refused(reply: Result<Reply, ClientError>) -> ProtocolError {
    match reply {
        Err(ClientError::Daemon(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}
