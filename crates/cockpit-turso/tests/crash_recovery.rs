//! Crash injection: a process that dies mid-transaction leaves its committed work, none of the
//! open transaction, a released single-opener lock and a consistent change capture.
mod common;

use std::path::Path;
use std::process::Command;

use cockpit_turso::Database;
use common::{TASK_MIGRATIONS, insert_task, scratch_dir, task_titles};

const CRASH_DATABASE_VARIABLE: &str = "COCKPIT_TURSO_CRASH_DATABASE";

/// Runs only as the child of the crash tests: commits two tasks, writes a third inside an open
/// transaction, then aborts the process without unwinding, closing or committing.
#[tokio::test]
async fn crashing_child() {
    let Some(path) = std::env::var_os(CRASH_DATABASE_VARIABLE) else {
        return;
    };
    let database = Database::open(Path::new(&path), &[TASK_MIGRATIONS])
        .await
        .unwrap();
    insert_task(&database, "committed one").await;
    insert_task(&database, "committed two").await;
    let write = database.write().await.unwrap();
    write
        .execute("INSERT INTO tasks (title) VALUES ('open at the crash')", ())
        .await
        .unwrap();
    std::process::abort();
}

fn crash_child_on(path: &Path) {
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["crashing_child", "--exact", "--nocapture"])
        .env(CRASH_DATABASE_VARIABLE, path)
        .status()
        .unwrap();
    assert!(!status.success(), "the child must abort, got {status}");
}

#[tokio::test]
async fn committed_work_survives_a_crash_and_the_open_transaction_does_not() {
    let path = scratch_dir("crash-committed").join("state.db");
    crash_child_on(&path);

    let recovered = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();

    assert_eq!(
        task_titles(&recovered).await,
        ["committed one", "committed two"]
    );
}

#[tokio::test]
async fn the_recovered_file_passes_the_integrity_check() {
    let path = scratch_dir("crash-integrity").join("state.db");
    crash_child_on(&path);

    let recovered = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();

    let read = recovered.read().await.unwrap();
    let mut rows = read.query("PRAGMA integrity_check", ()).await.unwrap();
    let report: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(report, "ok");
}

#[tokio::test]
async fn change_capture_after_a_crash_holds_only_committed_transactions() {
    let path = scratch_dir("crash-changes").join("state.db");
    crash_child_on(&path);
    let recovered = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    let read = recovered.read().await.unwrap();
    let revision = read.revision().await.unwrap();
    let task_commits: Vec<_> = read
        .commits_after(0, 1000)
        .await
        .unwrap()
        .into_iter()
        .filter(|commit| commit.changes.iter().any(|change| change.table == "tasks"))
        .collect();
    drop(read);

    insert_task(&recovered, "after recovery").await;

    assert_eq!(task_commits.len(), 2);
    let read = recovered.read().await.unwrap();
    let next = read.commits_after(revision, 10).await.unwrap();
    assert_eq!(next.len(), 1);
    assert!(next[0].revision > revision);
}
