//! The daemon opens a session's provider socket only when the session's own process tree
//! listens on it (#442): the session's directory is the agent's to write, so its `codex.sock`
//! may be a link the agent planted. Needs `python3` and `sleep` on PATH.

use std::io::Read;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use cockpit_daemon::SessionTreeSockets;
use cockpit_provider_api::SessionSockets;
use cockpit_sandbox::{FolderMount, ProviderReach};

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
        reach: ProviderReach::Direct,
    }
}

/// The session's folder in the daemon's mount sources, which the sandbox shows at
/// `/tmp/codex-daemon-4242`.
fn mounted(mount_dir: &Path) -> ProviderReach {
    ProviderReach::Mounted(vec![FolderMount {
        host: mount_dir.join("codex-daemon"),
        sandbox: PathBuf::from("/tmp/codex-daemon-4242"),
    }])
}

/// A listener in its own process, the way Codex's app-server listens for the session. It
/// binds from inside the socket's folder, so the path may be longer than a socket address.
fn listening_process(socket: &Path) -> Reaped {
    let child = Command::new("python3")
        .args([
            "-c",
            "import os, socket, sys, time\n\
             os.chdir(sys.argv[1])\n\
             server = socket.socket(socket.AF_UNIX)\n\
             server.bind(sys.argv[2])\n\
             server.listen()\n\
             time.sleep(60)",
        ])
        .arg(socket.parent().unwrap())
        .arg(socket.file_name().unwrap())
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

/// Codex leaves `codex.sock` linking to `/tmp/codex-daemon-<uid>/<hash>` as the sandbox sees
/// it; the daemon reads that as the session's folder in its mount sources, never the host's
/// (#446), though the entry's full path is longer than a unix socket address.
#[test]
fn a_link_into_codexs_socket_folder_reaches_the_sessions_own_folder() {
    let session_dir = tempfile::tempdir().unwrap();
    let mount_sources = tempfile::tempdir().unwrap();
    let mount_dir = mount_sources.path().join("mounts/s000000000001");
    std::fs::create_dir_all(mount_dir.join("codex-daemon")).unwrap();
    let entry = "95315c63f246391f03e71667597da1286d1fbc2cada8689510e30233d82fdd2d";
    let real_socket = mount_dir.join("codex-daemon").join(entry);
    assert!(real_socket.as_os_str().len() > 108);
    let app_server = listening_process(&real_socket);
    std::os::unix::fs::symlink(
        format!("/tmp/codex-daemon-4242/{entry}"),
        session_dir.path().join("codex.sock"),
    )
    .unwrap();
    let sockets = SessionTreeSockets {
        reach: mounted(&mount_dir),
        ..tree_of(&app_server.0)
    };

    let opened = sockets.connect(&session_dir.path().join("codex.sock"));

    assert!(opened.is_ok(), "{:?}", opened.err());
}

#[test]
fn a_link_outside_the_mounted_folders_is_refused_without_connecting() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("owners-app-server.sock");
    let outside = UnixListener::bind(&elsewhere).unwrap();
    outside.set_nonblocking(true).unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.path().join("codex.sock")).unwrap();
    let agent = Reaped(Command::new("sleep").arg("60").spawn().unwrap());
    let sockets = SessionTreeSockets {
        reach: mounted(dir.path()),
        ..tree_of(&agent.0)
    };

    let refused = sockets.connect(&dir.path().join("codex.sock")).unwrap_err();

    assert!(
        refused
            .reason
            .contains("outside the folders the sandbox mounts"),
        "{}",
        refused.reason
    );
    assert!(outside.accept().is_err(), "the daemon connected");
}

/// Without greywall's `--mount`, the sandboxed app-server's socket is out of reach: the
/// delivery is refused naming the missing feature, and nothing is opened (#446).
#[test]
fn without_folder_mounts_delivery_is_refused_naming_the_feature() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("codex.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let agent = Reaped(Command::new("sleep").arg("60").spawn().unwrap());
    let sockets = SessionTreeSockets {
        reach: ProviderReach::Unreachable {
            feature: cockpit_sandbox::MOUNT_FLAG,
            reason: "greywall's --help lists no --mount".to_owned(),
        },
        ..tree_of(&agent.0)
    };

    let refused = sockets.connect(&socket).unwrap_err();

    assert!(
        refused
            .reason
            .ends_with("greywall's --help lists no --mount"),
        "{}",
        refused.reason
    );
    assert!(listener.accept().is_err(), "the daemon connected");
}
