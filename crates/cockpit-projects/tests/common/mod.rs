//! Fixtures shared by the projects tests: a scratch state database and real Git repositories.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::path::{Path, PathBuf};
use std::process::Command;

use cockpit_projects::{Checkout, Git, Holder, Project, register_project};
use cockpit_turso::Database;

pub struct Scratch {
    pub dir: tempfile::TempDir,
}

impl Scratch {
    pub fn new() -> Scratch {
        Scratch {
            dir: tempfile::tempdir().expect("a scratch directory"),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.dir
            .path()
            .canonicalize()
            .expect("the scratch directory resolves")
    }

    pub async fn open(&self) -> Database {
        Database::open(
            &self.path().join("state.db"),
            &[cockpit_projects::MIGRATIONS],
        )
        .await
        .expect("the state database opens")
    }

    /// A Git repository with one commit at `<scratch>/<name>`.
    pub fn repository(&self, name: &str) -> PathBuf {
        let root = self.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("README"), "fixture\n").unwrap();
        git(&root, &["add", "README"]);
        git(&root, &["commit", "--quiet", "-m", "initial"]);
        root
    }
}

/// Runs the test machine's Git with a fixed identity, failing the test when it fails.
pub fn git(dir: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
        ])
        .args(arguments)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

pub async fn register(database: &Database, root: &Path) -> (Project, Checkout) {
    register_project(database, &Git::from_path(), root, None)
        .await
        .expect("the project registers")
}

pub fn holder(id: &str) -> Holder {
    Holder::new(id).unwrap()
}
