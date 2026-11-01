//! A private tmux server on a socket in a temporary directory, never the user's server.
#![allow(
    dead_code,
    reason = "each test file uses a different part of the fixture"
)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use cockpit_tmux::{PaneId, Server, ServerSpec, SessionLaunch, SessionName, Socket};

/// Long enough never to fire in a passing test, short enough to fail a hung one quickly.
pub const GENEROUS: Duration = Duration::from_secs(10);

pub struct TestServer {
    pub home: tempfile::TempDir,
    pub server: Server,
}

impl TestServer {
    pub fn new() -> TestServer {
        let home = tempfile::tempdir().expect("a temporary directory");
        let server =
            Server::open(Self::spec_in(home.path())).expect("tmux is installed and recent");
        TestServer { home, server }
    }

    pub fn spec_in(directory: &Path) -> ServerSpec {
        ServerSpec {
            tmux: PathBuf::from("tmux"),
            socket: Socket::Path(directory.join("tmux.sock")),
            config_path: directory.join("tmux.conf"),
        }
    }

    /// Another handle on the same server, as a restarted daemon would open it.
    pub fn reopen(&self) -> Server {
        Server::open(Self::spec_in(self.home.path())).expect("the server reopens")
    }

    pub fn socket_path(&self) -> PathBuf {
        self.home.path().join("tmux.sock")
    }

    /// Runs plain `tmux` against this server to check what the crate did.
    pub fn tmux(&self, arguments: &[&str]) -> String {
        let output = Command::new("tmux")
            .arg("-S")
            .arg(self.socket_path())
            .args(arguments)
            .env_remove("TMUX")
            .output()
            .expect("tmux runs");
        assert!(
            output.status.success(),
            "tmux {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("tmux prints UTF-8")
    }

    /// A detached session running `sh -c script` in the temporary directory.
    pub fn start_shell(&self, name: &str, script: &str) -> (SessionName, PaneId) {
        self.start_shell_through(&self.server, name, script)
    }

    /// [`TestServer::start_shell`] through another handle on this server.
    pub fn start_shell_through(
        &self,
        server: &Server,
        name: &str,
        script: &str,
    ) -> (SessionName, PaneId) {
        let name = SessionName::new(name).expect("a valid name");
        let pane = server
            .new_session(&SessionLaunch {
                name: name.clone(),
                working_dir: self.home.path().to_path_buf(),
                command: shell(script),
                environment: Vec::new(),
                columns: 100,
                rows: 30,
            })
            .expect("the session starts");
        (name, pane)
    }

    /// A file the session scripts wait for, so output starts only once the test is ready.
    pub fn release(&self, gate: &str) {
        std::fs::write(self.home.path().join(gate), "").expect("the gate file is writable");
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(self.socket_path())
            .arg("kill-server")
            .output();
    }
}

pub fn shell(script: &str) -> Vec<OsString> {
    vec!["sh".into(), "-c".into(), script.into()]
}

/// A shell line that waits until the test calls [`TestServer::release`] with `gate`.
pub fn wait_for(gate: &str) -> String {
    format!("while [ ! -e {gate} ]; do sleep 0.02; done")
}

/// Polls `condition` until it holds, failing the test after [`GENEROUS`].
pub fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + GENEROUS;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
