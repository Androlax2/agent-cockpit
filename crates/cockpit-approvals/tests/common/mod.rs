//! A scratch state database with the approvals and authorization tables, and sessions.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::path::{Path, PathBuf};

use cockpit_approvals::{RunAsk, RunRequest};
use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_turso::Database;

/// A moment far from any expiry, as seconds since 1970.
pub const NOW: i64 = 1_800_000_000;

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub database: Database,
}

impl Fixture {
    pub async fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(
            &dir.path().join("state.db"),
            &[
                cockpit_authorization::MIGRATIONS,
                cockpit_approvals::MIGRATIONS,
            ],
        )
        .await
        .unwrap();
        Fixture { dir, database }
    }

    /// A directory that exists, canonical, for a request to run in.
    pub fn cwd(&self) -> PathBuf {
        std::fs::canonicalize(self.dir.path()).unwrap()
    }

    pub async fn grant(&self, holder: &SessionId) {
        let write = self.database.write().await.unwrap();
        cockpit_authorization::grant(
            &write,
            holder,
            Permission::RunCommands,
            &Scope::Session(holder.clone()),
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
    }

    /// `session`, granted `run-commands`, asks to run `command` here.
    pub async fn asked(&self, session: &SessionId, command: &str) -> RunRequest {
        self.grant(session).await;
        let write = self.database.write().await.unwrap();
        let run = cockpit_approvals::ask_run(&write, session, self.ask(command), NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();
        run
    }

    /// The revision of the last commit: unchanged when nothing was written.
    pub async fn revision(&self) -> i64 {
        self.database
            .read()
            .await
            .unwrap()
            .revision()
            .await
            .unwrap()
    }

    pub fn ask(&self, command: &str) -> RunAsk {
        RunAsk {
            command: command.to_owned(),
            cwd: self.cwd(),
            reason: "Lists the files so the agent can see the layout.".to_owned(),
            project: None,
        }
    }
}

pub fn session(id: &str) -> SessionId {
    SessionId::new(id).unwrap()
}

pub fn path(text: &str) -> &Path {
    Path::new(text)
}
