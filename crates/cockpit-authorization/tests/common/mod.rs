//! Fixtures shared by the authorization tests: a scratch state database and grant shortcuts.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::path::PathBuf;

use cockpit_authorization::{Grant, Permission, Scope, SessionId, TaskId};
use cockpit_turso::Database;

/// An empty directory unique to this test process and `name`, removed first if a previous run
/// left it behind.
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("cockpit-authorization-tests")
        .join(format!("{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn database_path(name: &str) -> PathBuf {
    scratch_dir(name).join("state.db")
}

pub async fn open(name: &str) -> Database {
    Database::open(&database_path(name), &[cockpit_authorization::MIGRATIONS])
        .await
        .unwrap()
}

pub fn session(id: &str) -> SessionId {
    SessionId::new(id).unwrap()
}

pub fn task(id: &str) -> TaskId {
    TaskId::new(id).unwrap()
}

/// Grants `permission` over `scope` to `holder` in its own committed write transaction.
pub async fn grant(
    database: &Database,
    holder: &SessionId,
    permission: Permission,
    scope: &Scope,
) -> Grant {
    let write = database.write().await.unwrap();
    let grant = cockpit_authorization::grant(&write, holder, permission, scope)
        .await
        .unwrap();
    write.commit().await.unwrap();
    grant
}
