//! The daemon opens a session's provider socket only when the session's own process tree
//! listens on it (#442): the session's directory is the agent's to write, so its `codex.sock`
//! may be a link the agent planted. Needs `python3` and `sleep` on PATH.

use std::io::Read;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use cockpit_daemon::SessionTreeSockets;
use cockpit_provider_api::SessionSockets;

struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn tree_of(child: &Child) -> SessionTreeSockets {
    SessionTreeSockets {
        root: cockpit_agent_channel::identity_of(child.id()).unwrap(),
    }
}

/// A listener in its own process, the way Codex's app-server listens for the session.
fn listening_process(socket: &Path) -> Reaped {
    let child = Command::new("python3")
        .args([
            "-c",
            "import socket, sys, time\n\
             server = socket.socket(socket.AF_UNIX)\n\
             server.bind(sys.argv[1])\n\
             server.listen()\n\
             time.sleep(60)",
        ])
        .arg(socket)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "python3 never listened");
        std::thread::sleep(Duration::from_millis(20));
    }
    Reaped(child)
}

#[test]
fn a_codex_sock_the_agent_replaced_with_a_link_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("session");
    std::fs::create_dir(&session_dir).unwrap();
    let elsewhere = dir.path().join("owners-app-server.sock");
    let outside = UnixListener::bind(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, session_dir.join("codex.sock")).unwrap();
    let agent = Reaped(Command::new("sleep").arg("60").spawn().unwrap());

    let refused = tree_of(&agent.0)
        .connect(&session_dir.join("codex.sock"))
        .unwrap_err();

    assert!(
        refused
            .reason
            .contains("is not in the session's process tree"),
        "{}",
        refused.reason
    );
    let (mut accepted, _) = outside.accept().unwrap();
    let mut received = Vec::new();
    accepted.read_to_end(&mut received).unwrap();
    assert!(
        received.is_empty(),
        "the refused socket received {received:?}"
    );
}

#[test]
fn the_app_server_the_sessions_agent_runs_is_opened() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("codex.sock");
    let app_server = listening_process(&socket);

    let opened = tree_of(&app_server.0).connect(&socket);

    assert!(opened.is_ok(), "{:?}", opened.err());
}

#[test]
fn a_socket_nobody_listens_on_is_refused_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Reaped(Command::new("sleep").arg("60").spawn().unwrap());
    let socket = dir.path().join("codex.sock");

    let refused = tree_of(&agent.0).connect(&socket).unwrap_err();

    assert!(
        refused
            .reason
            .starts_with(&format!("{} was not opened: ", socket.display())),
        "{}",
        refused.reason
    );
}
