//! #406: `storage-database` reports and, owner-only, applies the database retention rules
//! through the daemon's socket; the daemon backs its database up when it starts.

#![cfg(target_os = "linux")]

mod candidate;
mod common;

use std::time::Duration;

use cockpit_approvals::{Output, RunAsk, RunResult};
use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_client::ClientError;
use cockpit_processes::ExecutionId;
use cockpit_protocol::{
    Command, DatabaseStorageView, ErrorCode, PayloadRetentionView, Reply, TargetSizeView,
};
use common::{GENEROUS, RunningDaemon, TestHome, unix_now};

const SESSION: &str = "s000000000001";
const DAY: i64 = 24 * 60 * 60;

/// A daemon over a state holding session [`SESSION`], ended 40 days ago with one launch whose
/// delivery recorded a program and an argument, and one command run it asked that finished
/// 40 days ago with `twelve bytes` as its output.
async fn started_with_old_history(owner_settings: &str) -> (TestHome, RunningDaemon) {
    let home = TestHome::new();
    let config = home.home().join("config").join(common::BUILD_PROFILE_DIR);
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.toml"), owner_settings).unwrap();
    let old = unix_now() - 40 * DAY;
    let database = home.open_state().await;
    let write = database.write().await.unwrap();
    for sql in [
        "INSERT INTO sessions (id, provider, checkout_id, holds_checkout, working_dir, state,
            revision, created_at, changed_at)
         VALUES ('s000000000001', 'claude', 1, 0, '/work', 'ended', 3, ?1, ?1)",
        "INSERT INTO session_launches (id, session_id, kind, execution_id, launched_at)
         VALUES (1, 's000000000001', 'start', 1, ?1)",
        "INSERT INTO session_transitions (session_id, from_state, to_state, at)
         VALUES ('s000000000001', 'running', 'ended', ?1)",
    ] {
        write.execute(sql, [old]).await.unwrap();
    }
    for sql in [
        "INSERT INTO session_delivery (execution_id, provider_program, reach)
         VALUES (1, X'2F62696E', 'direct')",
        "INSERT INTO session_delivery_arguments (execution_id, position, value)
         VALUES (1, 0, X'2D61')",
    ] {
        write.execute(sql, ()).await.unwrap();
    }
    write.commit().await.unwrap();
    finish_an_old_run(&home, &database, old).await;
    drop(database);
    let daemon = home.start();
    (home, daemon)
}

async fn finish_an_old_run(home: &TestHome, database: &cockpit_turso::Database, asked_at: i64) {
    let session = SessionId::new(SESSION).unwrap();
    let write = database.write().await.unwrap();
    cockpit_authorization::grant(
        &write,
        &session,
        Permission::RunCommands,
        &Scope::Session(session.clone()),
    )
    .await
    .unwrap();
    let ask = RunAsk {
        command: "ls".to_owned(),
        cwd: home.home().canonicalize().unwrap(),
        reason: "the retention test asks".to_owned(),
        project: None,
    };
    let run = cockpit_approvals::ask_run(&write, &session, ask, asked_at)
        .await
        .unwrap();
    let decided_at = asked_at + cockpit_approvals::APPROVAL_GRACE_SECONDS;
    let (approved, _) = cockpit_approvals::approve_run(
        &write,
        None,
        run.id,
        run.revision,
        &run.digest(),
        decided_at,
    )
    .await
    .unwrap();
    cockpit_approvals::start_run(&write, &approved, ExecutionId(approved.id.0))
        .await
        .unwrap();
    let result = RunResult {
        exit_code: 0,
        output: Output {
            shown: "twelve bytes".to_owned(),
            omitted_lines: 0,
            truncated_bytes: 0,
            redacted: 0,
        },
        output_retired: false,
        stopped_by_owner: false,
    };
    cockpit_approvals::finish_run(&write, &approved, &result, decided_at + 1)
        .await
        .unwrap();
    write.commit().await.unwrap();
}

async fn database_report(home: &TestHome, apply: bool) -> (DatabaseStorageView, bool) {
    let client = home.connect().await;
    match client
        .request(Command::StorageDatabase { apply }, GENEROUS)
        .await
        .unwrap()
    {
        Reply::StorageDatabase { database, applied } => (*database, applied),
        other => panic!("expected a database report, got {other:?}"),
    }
}

/// The session's delivery program (4 bytes) and argument (2 bytes), in 2 rows.
const OLD_SESSION: PayloadRetentionView = PayloadRetentionView {
    selected: 1,
    selected_bytes: 6,
    retired: 0,
    retired_bytes: 0,
};

const OLD_RUN: PayloadRetentionView = PayloadRetentionView {
    selected: 1,
    selected_bytes: 12,
    retired: 0,
    retired_bytes: 0,
};

#[tokio::test]
async fn a_dry_run_selects_the_old_payloads_and_retires_nothing() {
    let (home, _daemon) = started_with_old_history("").await;

    let (report, applied) = database_report(&home, false).await;

    assert!(!applied);
    assert_eq!(
        (report.sessions, report.run_outputs, report.retention_days),
        (OLD_SESSION, OLD_RUN, 30)
    );
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(database_report(&home, false).await.0.sessions, OLD_SESSION);
}

#[tokio::test]
async fn the_storage_report_counts_the_old_payloads_and_retires_nothing() {
    let (home, _daemon) = started_with_old_history("").await;
    let client = home.connect().await;

    let areas = match client
        .request(Command::StorageReport, GENEROUS)
        .await
        .unwrap()
    {
        Reply::Storage {
            areas: Some(areas), ..
        } => areas,
        other => panic!("expected a storage report with its areas, got {other:?}"),
    };

    let database = areas.database.expect("the database area");
    assert_eq!(
        (database.sessions, database.run_outputs),
        (OLD_SESSION, OLD_RUN)
    );
    assert_eq!(database_report(&home, false).await.0.sessions, OLD_SESSION);
}

#[tokio::test]
async fn the_storage_report_measures_the_daemon_log_folder() {
    let (home, _daemon) = started_with_old_history("").await;
    let client = home.connect().await;
    let deadline = std::time::Instant::now() + GENEROUS;
    let logged = loop {
        let length = std::fs::metadata(home.log_file()).map_or(0, |file| file.len());
        if length > 0 {
            break length;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the daemon logged nothing"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let areas = match client
        .request(Command::StorageReport, GENEROUS)
        .await
        .unwrap()
    {
        Reply::Storage {
            areas: Some(areas), ..
        } => areas,
        other => panic!("expected a storage report with its areas, got {other:?}"),
    };

    let daemon_logs = areas
        .logs
        .iter()
        .find(|folder| folder.process == "daemon")
        .unwrap_or_else(|| panic!("no daemon log folder in {:?}", areas.logs));
    assert!(
        matches!(
            daemon_logs.measurement,
            TargetSizeView::Complete { apparent_bytes } if apparent_bytes >= logged
        ),
        "{daemon_logs:?}"
    );
    assert_eq!(areas.logs_unavailable, None);
}

#[tokio::test]
async fn pruning_retires_the_payloads_and_keeps_the_session_and_its_audit_rows() {
    let (home, daemon) = started_with_old_history("").await;

    let (report, applied) = database_report(&home, true).await;

    assert!(applied);
    assert_eq!(
        (report.sessions.retired, report.sessions.retired_bytes),
        (1, 6)
    );
    assert_eq!(
        (report.run_outputs.retired, report.run_outputs.retired_bytes),
        (1, 12)
    );
    let (after, _) = database_report(&home, false).await;
    assert_eq!(
        (after.sessions.selected, after.run_outputs.selected),
        (0, 0)
    );
    daemon.kill();
    let database = home.open_state().await;
    let read = database.read().await.unwrap();
    for (table, kept) in [
        ("sessions", 1),
        ("session_launches", 1),
        ("session_transitions", 1),
        ("session_delivery", 0),
        ("session_delivery_arguments", 0),
        (
            "approvals_runs WHERE output = '' AND output_retired_at IS NOT NULL",
            1,
        ),
        (
            "turso_cdc WHERE table_name LIKE 'session_delivery%' AND before IS NOT NULL",
            0,
        ),
    ] {
        let mut rows = read
            .query(&format!("SELECT count(*) FROM {table}"), ())
            .await
            .unwrap();
        let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(count, kept, "{table}");
    }
}

#[tokio::test]
async fn a_longer_retention_keeps_the_payloads() {
    let (home, _daemon) =
        started_with_old_history("[storage]\ndatabase_retention_days = 60\n").await;

    let (report, _) = database_report(&home, true).await;

    assert_eq!(report.retention_days, 60);
    assert_eq!(
        (report.sessions, report.run_outputs),
        (
            PayloadRetentionView::default(),
            PayloadRetentionView::default()
        )
    );
}

#[tokio::test]
async fn the_feed_history_cap_is_the_owner_setting() {
    let (home, _daemon) =
        started_with_old_history("[storage]\nfeed_history_changes = 5000\n").await;

    let (report, _) = database_report(&home, false).await;

    assert_eq!(report.feed_changes_kept, 5000);
}

#[tokio::test]
async fn the_daemon_backs_its_database_up_when_it_starts() {
    let (home, _daemon) = started_with_old_history("[storage]\nlow_disk_floor_mib = 0\n").await;
    let backups = backups_dir(&home);
    let deadline = std::time::Instant::now() + GENEROUS;
    while cockpit_turso::backups(&backups).unwrap().is_empty()
        && std::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let (report, _) = database_report(&home, false).await;

    assert_eq!(report.backups.len(), 1, "{:?}", report.backups);
    assert_eq!(report.backups_kept, 7);
    assert!(report.backups[0].bytes > 0);
}

/// A floor no disk meets: the low-disk guard holds the backup.
#[tokio::test]
async fn no_backup_is_taken_while_free_disk_space_is_below_the_floor() {
    let (home, _daemon) =
        started_with_old_history("[storage]\nlow_disk_floor_mib = 1000000000000\n").await;

    let log = home.log_once_it_contains("database backup waits");

    assert!(log.contains("database backup waits"), "{log}");

    assert!(
        cockpit_turso::backups(&backups_dir(&home))
            .unwrap()
            .is_empty()
    );
}

fn backups_dir(home: &TestHome) -> std::path::PathBuf {
    home.state_home()
        .join(common::BUILD_PROFILE_DIR)
        .join("backups")
}

#[tokio::test]
async fn database_retention_refuses_the_agent_socket() {
    let (home, _daemon) = started_with_old_history("").await;
    let agent = home.connect_agent().await;

    let refusal = match agent
        .request(Command::StorageDatabase { apply: true }, GENEROUS)
        .await
    {
        Err(ClientError::Daemon(error)) => error,
        other => panic!("expected owner-only refusal, got {other:?}"),
    };

    assert_eq!(refusal.code, ErrorCode::NotAuthorized);
    assert_eq!(database_report(&home, false).await.0.sessions, OLD_SESSION);
}
