//! Fixtures shared by the database tests: scratch directories and a capability's migrations.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::path::PathBuf;

use cockpit_turso::{Migration, MigrationSet};

/// An empty directory unique to this test process and `name`, removed first if a previous run
/// left it behind.
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("cockpit-turso-tests")
        .join(format!("{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub const TASK_MIGRATIONS: MigrationSet = MigrationSet {
    owner: "tasks",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL)",
    }],
};

pub const NOTE_MIGRATIONS: MigrationSet = MigrationSet {
    owner: "notes",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE notes (id INTEGER PRIMARY KEY, task_id INTEGER NOT NULL, body TEXT NOT NULL)",
    }],
};

pub async fn task_titles(database: &cockpit_turso::Database) -> Vec<String> {
    let read = database.read().await.unwrap();
    let mut rows = read
        .query("SELECT title FROM tasks ORDER BY id", ())
        .await
        .unwrap();
    let mut titles = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        titles.push(row.get::<String>(0).unwrap());
    }
    titles
}

pub async fn insert_task(database: &cockpit_turso::Database, title: &str) {
    let write = database.write().await.unwrap();
    write
        .execute("INSERT INTO tasks (title) VALUES (?1)", [title])
        .await
        .unwrap();
    write.commit().await.unwrap();
}
