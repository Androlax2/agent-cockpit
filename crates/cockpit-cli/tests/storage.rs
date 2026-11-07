//! #406's first part through the CLI and real daemon, in a fixture home only.
mod common;
mod scripted;

use cockpit_protocol::{
    CloneStorageView, Command, EndSweepView, LogFolderStorageView, Outcome, PruneVerdictView,
    Reply, SessionTempStorageView, StorageAreasView, TargetSizeView,
};
use common::TestHome;
use std::os::unix::fs::DirBuilderExt;

#[test]
fn storage_measures_only_registered_clone_targets_without_changing_them() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let created = home.cockpit_ok(&["checkout", "clone", "app", "feature"]);
    let id = created.split('\t').next().unwrap();
    home.cockpit_ok(&["checkout", "claim", id, "live-session"]);
    let clone = home.home().join("projects/.parallel/app/feature");
    std::fs::create_dir_all(clone.join("target/debug")).unwrap();
    std::fs::write(clone.join("target/a"), "abc").unwrap();
    std::fs::write(clone.join("target/debug/b"), "12345").unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("target/not-a-clone"), [0; 100]).unwrap();
    let unknown = home
        .home()
        .join("projects/.parallel/app/unregistered/target");
    std::fs::create_dir_all(&unknown).unwrap();
    std::fs::write(unknown.join("not-registered"), [0; 100]).unwrap();

    let output = home.cockpit_ok(&["storage"]);

    assert!(output.contains("apparent bytes"), "{output}");
    assert!(
        output.contains(&format!("{id}\tapp\t1\t8\tcomplete\tlive-session\t")),
        "{output}"
    );
    assert!(!output.contains("unregistered"), "{output}");
    assert_eq!(
        output
            .lines()
            .filter(|line| line.starts_with(&format!("{id}\t")))
            .count(),
        1
    );
    assert_eq!(
        std::fs::read(clone.join("target/debug/b")).unwrap(),
        b"12345"
    );
    assert!(
        home.cockpit_ok(&["checkout", "list"])
            .contains("live-session")
    );
}

#[test]
fn storage_does_not_accept_a_prune_argument() {
    let home = TestHome::new();
    let output = home.cockpit(&["storage", "prune"]);
    assert_eq!(output.status.code(), Some(2));
}

#[tokio::test]
async fn storage_preserves_measurement_status_and_exits_unsuccessfully_on_partial_reports() {
    for (measurement, truncated, status, row) in [
        (
            TargetSizeView::Complete { apparent_bytes: 0 },
            false,
            0,
            "\t0\tcomplete\t",
        ),
        (TargetSizeView::Missing, false, 0, "\t0\tmissing\t"),
        (
            TargetSizeView::Incomplete {
                apparent_bytes: 8,
                reason: "entry limit".to_owned(),
            },
            false,
            1,
            "\t8\tincomplete\t",
        ),
        (
            TargetSizeView::Unavailable {
                reason: "permission denied".to_owned(),
            },
            false,
            1,
            "\t-\tunavailable\t",
        ),
        (
            TargetSizeView::Complete { apparent_bytes: 8 },
            true,
            1,
            "\t8\tcomplete\t",
        ),
    ] {
        let home = TestHome::new();
        let socket = home.home().join("run/torlek-dev/daemon.sock");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(socket.parent().unwrap())
            .unwrap();
        let daemon = scripted::scripted_daemon(
            &socket,
            Outcome::Ok(Reply::Storage {
                clones: vec![CloneStorageView {
                    checkout: 2,
                    project: "app".to_owned(),
                    incarnation: 1,
                    target: "/registered/target".to_owned(),
                    holder: None,
                    measurement,
                    prune: Some(PruneVerdictView::Kept {
                        reason: "fixture".to_owned(),
                    }),
                }],
                truncated,
                end_sweeps: Vec::new(),
                session_temps: Some(Vec::new()),
                session_temps_truncated: false,
                session_temps_unavailable: None,
                areas: Some(Box::new(complete_areas())),
            }),
        );
        let output = tokio::task::spawn_blocking(move || home.cockpit(&["storage"]))
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(status),
            "{}",
            scripted::text(&output.stderr)
        );
        let stdout = scripted::text(&output.stdout);
        assert!(stdout.contains(row), "{stdout}");
        assert_eq!(
            stdout.contains("further registered clones omitted"),
            truncated,
            "{stdout}"
        );
        assert_eq!(daemon.await.unwrap(), Command::StorageReport);
    }
}

fn scripted_row(checkout: i64, apparent_bytes: u64, prune: PruneVerdictView) -> CloneStorageView {
    CloneStorageView {
        checkout,
        project: "app".to_owned(),
        incarnation: 1,
        target: format!("/registered/{checkout}/target"),
        holder: None,
        measurement: TargetSizeView::Complete { apparent_bytes },
        prune: Some(prune),
    }
}

#[tokio::test]
async fn storage_shows_each_clone_prune_verdict_and_the_eligible_total() {
    let home = TestHome::new();
    let socket = home.home().join("run/torlek-dev/daemon.sock");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(socket.parent().unwrap())
        .unwrap();
    let daemon = scripted::scripted_daemon(
        &socket,
        Outcome::Ok(Reply::Storage {
            clones: vec![
                scripted_row(
                    2,
                    8,
                    PruneVerdictView::Eligible {
                        confirmation_required: true,
                    },
                ),
                scripted_row(
                    3,
                    16,
                    PruneVerdictView::Eligible {
                        confirmation_required: false,
                    },
                ),
                scripted_row(
                    4,
                    1000,
                    PruneVerdictView::Kept {
                        reason: "the clone has unpushed commits".to_owned(),
                    },
                ),
            ],
            truncated: false,
            end_sweeps: Vec::new(),
            session_temps: Some(Vec::new()),
            session_temps_truncated: false,
            session_temps_unavailable: None,
            areas: Some(Box::new(complete_areas())),
        }),
    );

    let output = tokio::task::spawn_blocking(move || home.cockpit(&["storage"]))
        .await
        .unwrap();

    let stdout = scripted::text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("\tholder\tprune\ttarget\treason"),
        "{stdout}"
    );
    assert!(
        stdout.contains("2\tapp\t1\t8\tcomplete\t-\teligible\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains("3\tapp\t1\t16\tcomplete\t-\teligible-auto\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains("4\tapp\t1\t1000\tcomplete\t-\tkept\t/registered/4/target\tthe clone has unpushed commits"),
        "{stdout}"
    );
    assert!(
        stdout.contains("eligible for prune: 2 of 3 clones, 24 apparent bytes"),
        "{stdout}"
    );
    assert_eq!(daemon.await.unwrap(), Command::StorageReport);
}

#[tokio::test]
async fn storage_lists_what_each_session_end_removed_or_kept_and_why() {
    let home = TestHome::new();
    let socket = home.home().join("run/torlek-dev/daemon.sock");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(socket.parent().unwrap())
        .unwrap();
    let sweep = |checkout: i64, kept: Option<&str>, target: Option<&str>| EndSweepView {
        checkout,
        incarnation: 1,
        session: format!("s{checkout}"),
        project: "app".to_owned(),
        path: format!("/clones/{checkout}"),
        kept: kept.map(str::to_owned),
        target: target.map(str::to_owned),
        swept_at: 0,
    };
    let daemon = scripted::scripted_daemon(
        &socket,
        Outcome::Ok(Reply::Storage {
            clones: Vec::new(),
            truncated: false,
            session_temps: Some(Vec::new()),
            session_temps_truncated: false,
            session_temps_unavailable: None,
            areas: Some(Box::new(complete_areas())),
            end_sweeps: vec![
                sweep(5, None, None),
                sweep(
                    6,
                    Some("uncommitted changes"),
                    Some("kept: needs a confirmation"),
                ),
            ],
        }),
    );

    let output = tokio::task::spawn_blocking(move || home.cockpit(&["storage"]))
        .await
        .unwrap();

    let stdout = scripted::text(&output.stdout);
    assert!(
        stdout.contains("swept at session end, newest first:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\t5\tapp\ts5\tremoved\t/clones/5\t-\t-\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "\t6\tapp\ts6\tkept\t/clones/6\tuncommitted changes\tkept: needs a confirmation\n"
        ),
        "{stdout}"
    );
    assert_eq!(daemon.await.unwrap(), Command::StorageReport);
}

#[test]
#[cfg(target_os = "linux")]
fn storage_keeps_a_clone_with_unpushed_commits_and_counts_only_eligible_clones() {
    let home = TestHome::new();
    std::fs::create_dir_all(home.home().join("config/torlek-dev")).unwrap();
    std::fs::write(
        home.home().join("config/torlek-dev/config.toml"),
        "[storage]\ntarget_prune_base_ref = 'refs/heads/main'\n",
    )
    .unwrap();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let mut ids = Vec::new();
    for branch in ["merged", "local-work"] {
        let created = home.cockpit_ok(&["checkout", "clone", "app", branch]);
        ids.push(created.split('\t').next().unwrap().to_owned());
        let clone = home.home().join("projects/.parallel/app").join(branch);
        std::fs::create_dir(clone.join("target")).unwrap();
        std::fs::write(clone.join(".git/info/exclude"), "/target/\n").unwrap();
        std::fs::write(clone.join("target/output"), b"12345678").unwrap();
    }
    common::real_git(
        &home.home().join("projects/.parallel/app/local-work"),
        &["commit", "--quiet", "--allow-empty", "-m", "on no remote"],
    );

    let output = home.cockpit_ok(&["storage"]);

    let row = |id: &str| {
        output
            .lines()
            .find(|line| line.starts_with(&format!("{id}\t")))
            .unwrap_or_else(|| panic!("no row for {id}: {output}"))
            .to_owned()
    };
    assert!(row(&ids[0]).contains("\teligible\t"), "{output}");
    assert!(row(&ids[1]).contains("\tkept\t"), "{output}");
    assert!(
        row(&ids[1]).contains("\tthe clone has unpushed commits"),
        "{output}"
    );
    assert!(
        output.contains("eligible for prune: 1 of 2 clones, 8 apparent bytes"),
        "{output}"
    );
    assert_eq!(
        std::fs::read(
            home.home()
                .join("projects/.parallel/app/local-work/target/output")
        )
        .unwrap(),
        b"12345678"
    );
}

fn output_row(retired: u64, failures: &[&str]) -> cockpit_protocol::OutputRetentionView {
    cockpit_protocol::OutputRetentionView {
        project: "app".to_owned(),
        outputs: 3,
        apparent_bytes: 1800,
        unmeasured: 0,
        protected: 1,
        selected: 2,
        selected_bytes: 1200,
        retired,
        retired_bytes: retired * 600,
        failures: failures
            .iter()
            .map(|failure| (*failure).to_owned())
            .collect(),
    }
}

/// Runs `torlek ARGUMENTS` against a daemon that answers `reply`; the command it sent, its
/// exit code and stdout.
async fn against_scripted(
    arguments: &'static [&'static str],
    reply: Reply,
) -> (Command, Option<i32>, String) {
    let home = TestHome::new();
    let socket = home.home().join("run/torlek-dev/daemon.sock");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(socket.parent().unwrap())
        .unwrap();
    let daemon = scripted::scripted_daemon(&socket, Outcome::Ok(reply));
    let output = tokio::task::spawn_blocking(move || home.cockpit(arguments))
        .await
        .unwrap();
    (
        daemon.await.unwrap(),
        output.status.code(),
        scripted::text(&output.stdout),
    )
}

#[tokio::test]
async fn storage_outputs_reports_what_retention_selects_without_retiring_it() {
    let (sent, code, stdout) = against_scripted(
        &["storage", "outputs"],
        Reply::StorageOutputs {
            projects: vec![output_row(0, &[])],
            applied: false,
            truncated: false,
        },
    )
    .await;

    assert_eq!(sent, Command::StorageOutputs { apply: false });
    assert_eq!(code, Some(0), "{stdout}");
    assert!(
        stdout.contains("app\t3\t1800\t0\t1\t2\t1200\t0\t0"),
        "{stdout}"
    );
    assert!(stdout.contains("torlek storage outputs prune"), "{stdout}");
}

#[tokio::test]
async fn storage_outputs_prune_retires_and_reports_the_bytes_removed() {
    let (sent, code, stdout) = against_scripted(
        &["storage", "outputs", "prune"],
        Reply::StorageOutputs {
            projects: vec![output_row(2, &[])],
            applied: true,
            truncated: false,
        },
    )
    .await;

    assert_eq!(sent, Command::StorageOutputs { apply: true });
    assert_eq!(code, Some(0), "{stdout}");
    assert!(
        stdout.contains("app\t3\t1800\t0\t1\t2\t1200\t2\t1200"),
        "{stdout}"
    );
    assert!(
        stdout.contains("retired 2 outputs, 1200 apparent bytes"),
        "{stdout}"
    );
}

#[tokio::test]
async fn storage_outputs_prune_fails_when_it_kept_a_selected_output() {
    let (_, code, stdout) = against_scripted(
        &["storage", "outputs", "prune"],
        Reply::StorageOutputs {
            projects: vec![output_row(1, &["execution 9: kept: being read"])],
            applied: true,
            truncated: false,
        },
    )
    .await;

    assert_eq!(code, Some(1), "{stdout}");
    assert!(
        stdout.contains("app: execution 9: kept: being read"),
        "{stdout}"
    );
}

#[tokio::test]
async fn storage_outputs_fails_when_the_pass_left_outputs_out() {
    let (_, code, stdout) = against_scripted(
        &["storage", "outputs"],
        Reply::StorageOutputs {
            projects: vec![output_row(0, &[])],
            applied: false,
            truncated: true,
        },
    )
    .await;

    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("incomplete"), "{stdout}");
}

/// #766: each session's temporary folder is a row of its own, and a partial measurement of
/// one makes the report incomplete as a clone's does.
#[tokio::test]
async fn storage_lists_each_sessions_temp_folder_size() {
    for (measurement, truncated, status, row) in [
        (
            TargetSizeView::Complete {
                apparent_bytes: 9_200,
            },
            false,
            0,
            "s1\t9200\tcomplete\t/state/tmp/s1\t",
        ),
        (
            TargetSizeView::Incomplete {
                apparent_bytes: 8,
                reason: "symbolic links are not followed".to_owned(),
            },
            false,
            1,
            "s1\t8\tincomplete\t/state/tmp/s1\tsymbolic links are not followed",
        ),
        (
            TargetSizeView::Complete { apparent_bytes: 0 },
            true,
            1,
            "s1\t0\tcomplete\t/state/tmp/s1\t",
        ),
    ] {
        let home = TestHome::new();
        let socket = home.home().join("run/torlek-dev/daemon.sock");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(socket.parent().unwrap())
            .unwrap();
        let daemon = scripted::scripted_daemon(
            &socket,
            Outcome::Ok(Reply::Storage {
                clones: Vec::new(),
                truncated: false,
                end_sweeps: Vec::new(),
                session_temps: Some(vec![SessionTempStorageView {
                    session: "s1".to_owned(),
                    path: "/state/tmp/s1".to_owned(),
                    measurement,
                }]),
                session_temps_truncated: truncated,
                session_temps_unavailable: None,
                areas: Some(Box::new(complete_areas())),
            }),
        );
        let output = tokio::task::spawn_blocking(move || home.cockpit(&["storage"]))
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(status),
            "{}",
            scripted::text(&output.stderr)
        );
        let stdout = scripted::text(&output.stdout);
        assert!(stdout.lines().any(|line| line == row), "{stdout}");
        assert_eq!(
            stdout.contains("further session temp folders omitted"),
            truncated,
            "{stdout}"
        );
        assert_eq!(daemon.await.unwrap(), Command::StorageReport);
    }
}

/// A temp folder parent that cannot be listed is said, and the report is incomplete, while
/// the clones are still shown.
#[tokio::test]
async fn storage_says_when_the_session_temp_folders_cannot_be_listed() {
    let home = TestHome::new();
    let socket = home.home().join("run/torlek-dev/daemon.sock");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(socket.parent().unwrap())
        .unwrap();
    let daemon = scripted::scripted_daemon(
        &socket,
        Outcome::Ok(Reply::Storage {
            clones: vec![scripted_row(
                2,
                8,
                PruneVerdictView::Kept {
                    reason: "fixture".to_owned(),
                },
            )],
            truncated: false,
            end_sweeps: Vec::new(),
            session_temps: Some(Vec::new()),
            session_temps_truncated: false,
            session_temps_unavailable: Some("cannot open /state/tmp: EACCES".to_owned()),
            areas: Some(Box::new(complete_areas())),
        }),
    );

    let output = tokio::task::spawn_blocking(move || home.cockpit(&["storage"]))
        .await
        .unwrap();

    let stdout = scripted::text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("2\tapp\t1\t8\tcomplete\t"), "{stdout}");
    assert!(
        stdout.contains("session temp folders\t-\t-\tunavailable\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "incomplete report: session temp folders unavailable: cannot open /state/tmp: EACCES"
        ),
        "{stdout}"
    );
    assert_eq!(daemon.await.unwrap(), Command::StorageReport);
}

#[tokio::test]
async fn storage_from_a_daemon_without_session_temp_folders_is_incomplete() {
    let home = TestHome::new();
    let socket = home.home().join("run/torlek-dev/daemon.sock");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(socket.parent().unwrap())
        .unwrap();
    let daemon = scripted::scripted_daemon(
        &socket,
        Outcome::Ok(Reply::Storage {
            clones: Vec::new(),
            truncated: false,
            end_sweeps: Vec::new(),
            session_temps: None,
            session_temps_truncated: false,
            session_temps_unavailable: None,
            areas: Some(Box::new(complete_areas())),
        }),
    );

    let output = tokio::task::spawn_blocking(move || home.cockpit(&["storage"]))
        .await
        .unwrap();

    let stdout = scripted::text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("incomplete report: the daemon reported no session temp folders"),
        "{stdout}"
    );
    assert_eq!(daemon.await.unwrap(), Command::StorageReport);
}

fn database_report(retired: u64, failures: &[&str]) -> Reply {
    let rule = |bytes| cockpit_protocol::PayloadRetentionView {
        selected: 2,
        selected_bytes: bytes,
        retired,
        retired_bytes: if retired > 0 { bytes } else { 0 },
    };
    Reply::StorageDatabase {
        database: Box::new(cockpit_protocol::DatabaseStorageView {
            file_bytes: 8192,
            wal_bytes: 0,
            retention_days: 30,
            sessions: rule(300),
            run_outputs: rule(40),
            feed_changes: 12,
            feed_changes_kept: 100_000,
            backups: vec![cockpit_protocol::BackupView {
                path: "/s/backups/state-9.db".to_owned(),
                taken_at: 9,
                bytes: 8192,
            }],
            backups_kept: 7,
            truncated: false,
            wal_checkpoint_busy: false,
            failures: failures
                .iter()
                .map(|failure| (*failure).to_owned())
                .collect(),
        }),
        applied: retired > 0,
    }
}

#[tokio::test]
async fn storage_database_reports_each_rule_the_feed_and_backups_without_retiring() {
    let (sent, code, stdout) =
        against_scripted(&["storage", "database"], database_report(0, &[])).await;

    assert_eq!(sent, Command::StorageDatabase { apply: false });
    assert_eq!(code, Some(0), "{stdout}");
    for line in [
        "database: 8192 bytes, WAL 0 bytes",
        "ended sessions' launch context\t2\t300\t0\t0",
        "finished command runs' output\t2\t40\t0\t0",
        "change feed: 12 captured rows kept now; the hourly trim keeps the newest 100000",
        "backups: 1 of the newest 7 kept",
        "/s/backups/state-9.db\t9\t8192",
        "torlek storage database prune",
    ] {
        assert!(stdout.contains(line), "missing {line}: {stdout}");
    }
}

#[tokio::test]
async fn storage_database_does_not_bound_a_feed_that_grew_past_its_cap_between_trims() {
    let Reply::StorageDatabase {
        mut database,
        applied,
    } = database_report(0, &[])
    else {
        unreachable!("database_report builds a StorageDatabase reply");
    };
    database.feed_changes = 101_362;
    let (_, code, stdout) = against_scripted(
        &["storage", "database"],
        Reply::StorageDatabase { database, applied },
    )
    .await;

    assert_eq!(code, Some(0), "{stdout}");
    assert!(
        stdout.contains(
            "change feed: 101362 captured rows kept now; the hourly trim keeps the newest 100000"
        ),
        "{stdout}"
    );
    assert!(!stdout.contains("at most"), "{stdout}");
}

#[tokio::test]
async fn storage_database_prune_reports_what_it_retired() {
    let (sent, code, stdout) =
        against_scripted(&["storage", "database", "prune"], database_report(2, &[])).await;

    assert_eq!(sent, Command::StorageDatabase { apply: true });
    assert_eq!(code, Some(0), "{stdout}");
    assert!(
        stdout.contains("retired 2 sessions' launch context and 2 run outputs, 340 apparent bytes"),
        "{stdout}"
    );
}

#[tokio::test]
async fn storage_database_prune_fails_when_it_stopped_early() {
    let (_, code, stdout) = against_scripted(
        &["storage", "database", "prune"],
        database_report(1, &["run outputs: database is locked"]),
    )
    .await;

    assert_eq!(code, Some(1), "{stdout}");
    assert!(
        stdout.contains("run outputs: database is locked"),
        "{stdout}"
    );
}

#[test]
fn storage_database_refuses_an_unknown_argument() {
    let home = TestHome::new();
    let output = home.cockpit(&["storage", "database", "now"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

/// Areas a daemon read in full: one project's outputs, the database with two backups and
/// two log folders.
fn complete_areas() -> StorageAreasView {
    let Reply::StorageDatabase { database, .. } = database_report(0, &[]) else {
        unreachable!("database_report answers a database report")
    };
    let mut database = *database;
    database.backups.push(cockpit_protocol::BackupView {
        path: "/s/backups/state-8.db".to_owned(),
        taken_at: 8,
        bytes: 4096,
    });
    let log = |process: &str, apparent_bytes| LogFolderStorageView {
        process: process.to_owned(),
        path: format!("/s/logs/{process}"),
        measurement: TargetSizeView::Complete { apparent_bytes },
    };
    StorageAreasView {
        outputs: Some(vec![output_row(0, &[])]),
        outputs_truncated: false,
        outputs_unavailable: None,
        database: Some(database),
        database_unavailable: None,
        logs: vec![log("daemon", 64), log("tui", 16)],
        logs_truncated: false,
        logs_unavailable: None,
    }
}

fn composed_report(areas: Option<StorageAreasView>) -> Reply {
    Reply::Storage {
        clones: vec![
            scripted_row(
                2,
                100,
                PruneVerdictView::Eligible {
                    confirmation_required: true,
                },
            ),
            scripted_row(
                3,
                50,
                PruneVerdictView::Kept {
                    reason: "a session works in this clone".to_owned(),
                },
            ),
        ],
        truncated: false,
        end_sweeps: Vec::new(),
        session_temps: Some(vec![SessionTempStorageView {
            session: "s1".to_owned(),
            path: "/state/tmp/s1".to_owned(),
            measurement: TargetSizeView::Complete { apparent_bytes: 20 },
        }]),
        session_temps_truncated: false,
        session_temps_unavailable: None,
        areas: areas.map(Box::new),
    }
}

#[tokio::test]
async fn storage_totals_each_area_and_what_its_prune_would_retire() {
    let (sent, code, stdout) =
        against_scripted(&["storage"], composed_report(Some(complete_areas()))).await;

    assert_eq!(sent, Command::StorageReport);
    assert_eq!(code, Some(0), "{stdout}");
    for row in [
        "area\tbytes\tretires\tstatus\tfreed by",
        "build folders\t150\t100\tcomplete\ttorlek storage prune CHECKOUT",
        "session temp folders\t20\t0\tcomplete\tremoved when its session ends",
        "ended check outputs\t1800\t1200\tcomplete\ttorlek storage outputs prune",
        "database\t8192\t340\tcomplete\ttorlek storage database prune",
        "backups\t12288\t0\tcomplete\trotation keeps the newest 7",
        "logs\t80\t0\tcomplete\trotation by size and age",
        "total\t22530\t1300\tcomplete\t-",
    ] {
        assert!(
            stdout.lines().any(|line| line == row),
            "{row:?} in {stdout}"
        );
    }
}

#[tokio::test]
async fn storage_from_a_daemon_without_the_composed_areas_is_incomplete() {
    let (_, code, stdout) = against_scripted(&["storage"], composed_report(None)).await;

    assert_eq!(code, Some(1), "{stdout}");
    assert!(
        stdout.contains(
            "incomplete report: the daemon reported no check outputs, database, backups or logs"
        ),
        "{stdout}"
    );
    for row in [
        "ended check outputs\t-\t-\tunavailable\t",
        "database\t-\t-\tunavailable\t",
        "backups\t-\t-\tunavailable\t",
        "logs\t-\t-\tunavailable\t",
        "total\t170\t100\tincomplete\t-",
    ] {
        assert!(stdout.contains(row), "{row:?} in {stdout}");
    }
}

/// An area read only in part keeps the bytes it saw, says so, and makes the report fail.
#[tokio::test]
async fn an_area_read_in_part_marks_its_row_and_the_report_incomplete() {
    type Change = fn(&mut StorageAreasView);
    let cases: [(Change, &str, &str); 6] = [
        (
            |areas| areas.outputs.as_mut().unwrap()[0].unmeasured = 1,
            "ended check outputs\t1800\t1200\tincomplete\t",
            "incomplete report: 1 check outputs unmeasured",
        ),
        (
            |areas| areas.outputs_truncated = true,
            "ended check outputs\t1800\t1200\tincomplete\t",
            "incomplete report: further check outputs left out by the pass bound",
        ),
        (
            |areas| {
                areas.outputs = None;
                areas.outputs_unavailable = Some("no read slot".to_owned());
            },
            "ended check outputs\t-\t-\tunavailable\t",
            "incomplete report: check outputs unavailable: no read slot",
        ),
        (
            |areas| {
                areas.database = None;
                areas.database_unavailable = Some("database is locked".to_owned());
            },
            "backups\t-\t-\tunavailable\t",
            "incomplete report: database unavailable: database is locked",
        ),
        (
            |areas| {
                areas.database.as_mut().unwrap().failures =
                    vec!["backups: permission denied".to_owned()];
            },
            "backups\t12288\t0\tincomplete\t",
            "incomplete report: backups: permission denied",
        ),
        (
            |areas| {
                areas.logs[1].measurement = TargetSizeView::Unavailable {
                    reason: "permission denied".to_owned(),
                };
            },
            "logs\t64\t0\tincomplete\t",
            "incomplete report: log folder tui: permission denied",
        ),
    ];
    for (change, row, explanation) in cases {
        let mut areas = complete_areas();
        change(&mut areas);

        let (_, code, stdout) = against_scripted(&["storage"], composed_report(Some(areas))).await;

        assert_eq!(code, Some(1), "{stdout}");
        assert!(stdout.contains(row), "{row:?} in {stdout}");
        assert!(stdout.contains(explanation), "{explanation:?} in {stdout}");
        assert!(
            stdout
                .lines()
                .any(|line| line.starts_with("total\t") && line.contains("\tincomplete\t")),
            "{stdout}"
        );
    }
}

#[tokio::test]
async fn a_backup_listing_failure_marks_only_the_backups_row() {
    let mut areas = complete_areas();
    areas.database.as_mut().unwrap().failures = vec!["backups: permission denied".to_owned()];

    let (_, code, stdout) = against_scripted(&["storage"], composed_report(Some(areas))).await;

    assert_eq!(code, Some(1), "{stdout}");
    assert!(
        stdout.contains("database\t8192\t340\tcomplete\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains("backups\t12288\t0\tincomplete\t"),
        "{stdout}"
    );
}
