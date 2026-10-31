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

use cockpit_client::Connection;

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
        }
    }

    /// A home with nothing in it and only `HOME` in the environment.
    pub fn bare() -> TestHome {
        TestHome {
            root: tempfile::tempdir().expect("a temporary home"),
            runtime_base: None,
            sets_xdg_dirs: false,
            log_level: None,
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
        self.socket_dir().join("daemon.sock")
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

    pub async fn connect(&self) -> Connection {
        Connection::open(&self.socket_path(), GENEROUS)
            .await
            .expect("the daemon accepts the handshake")
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
