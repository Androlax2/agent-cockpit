//! Fixtures shared by the channel tests: a scratch state database, launches and real processes.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cockpit_agent_channel::{
    Error, LaunchToken, PeerAncestry, ProcessIdentity, Refusal, ancestry, identify, identity_of,
};
use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_turso::Database;

pub async fn open(dir: &Path) -> Database {
    Database::open(
        &dir.join("state.db"),
        &[
            cockpit_authorization::MIGRATIONS,
            cockpit_agent_channel::MIGRATIONS,
        ],
    )
    .await
    .unwrap()
}

pub fn session(id: &str) -> SessionId {
    SessionId::new(id).unwrap()
}

/// Mints a launch of `session` rooted at `root` and commits it.
pub async fn launch(
    database: &Database,
    session: &SessionId,
    root: &ProcessIdentity,
) -> LaunchToken {
    let write = database.write().await.unwrap();
    let token = cockpit_agent_channel::mint(&write, session).await.unwrap();
    cockpit_agent_channel::record_root(&write, session, root)
        .await
        .unwrap();
    write.commit().await.unwrap();
    token
}

/// Identifies the sender in a write transaction and commits it, as the daemon does.
pub async fn identify_committed(
    database: &Database,
    peer: &PeerAncestry,
    token: Option<&str>,
) -> Result<SessionId, Error> {
    let write = database.write().await.unwrap();
    let identified = identify(&write, peer, token).await;
    write.commit().await.unwrap();
    identified
}

pub fn refusal<T: std::fmt::Debug>(result: Result<T, Error>) -> Refusal {
    match result {
        Err(Error::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

pub async fn grant(database: &Database, holder: &SessionId, permission: Permission) {
    let write = database.write().await.unwrap();
    cockpit_authorization::grant(&write, holder, permission, &Scope::Session(holder.clone()))
        .await
        .unwrap();
    write.commit().await.unwrap();
}

/// This test process, as the daemon would see it on the agent socket.
pub fn this_process() -> PeerAncestry {
    ancestry(std::process::id())
}

pub fn this_process_identity() -> ProcessIdentity {
    identity_of(std::process::id()).unwrap()
}

/// A child process that lives until dropped.
pub struct Process(pub Child);

impl Process {
    /// `sh -c <script>`; the trailing `:` keeps the shell from exec'ing its last command, so
    /// the shell stays the parent of what it starts.
    pub fn shell(script: &str) -> Process {
        Process(
            Command::new("sh")
                .args(["-c", &format!("{script}; :")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("sh runs"),
        )
    }

    pub fn sleeper() -> Process {
        Process(
            Command::new("sleep")
                .arg("60")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("sleep runs"),
        )
    }

    pub fn pid(&self) -> u32 {
        self.0.id()
    }

    pub fn identity(&self) -> ProcessIdentity {
        identity_of(self.pid()).unwrap()
    }

    pub fn ancestry(&self) -> PeerAncestry {
        ancestry(self.pid())
    }

    /// The first child this process started, waiting for it to exist.
    pub fn first_child(&self) -> u32 {
        let pid = self.pid();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            // `pgrep` rather than `/proc/<pid>/task/<pid>/children`: macOS has no `/proc`.
            let children = Command::new("pgrep")
                .args(["-P", &pid.to_string()])
                .output()
                .expect("pgrep runs");
            if let Some(child) = String::from_utf8_lossy(&children.stdout)
                .split_whitespace()
                .next()
            {
                return child.parse().unwrap();
            }
            assert!(Instant::now() < deadline, "process {pid} started no child");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Process {
    /// Kills the process's children too, so no test leaves a process behind.
    fn drop(&mut self) {
        let _ = Command::new("pkill")
            .args(["-KILL", "-P", &self.pid().to_string()])
            .status();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
