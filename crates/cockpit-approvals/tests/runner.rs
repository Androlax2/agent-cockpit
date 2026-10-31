//! `Runner` on a real state database and real processes (#163): what an approved run
//! reports when its session leader fails, and when a restart finds it still launching.
#![cfg(target_os = "linux")]

mod common;

use std::ffi::OsString;
use std::path::PathBuf;

use cockpit_approvals::{NoResult, RunSettings, Runner, approve_run, ended_after_restart};
use cockpit_processes::{EndReason, LaunchContainment, Supervisor, SystemdTools};
use cockpit_turso::Database;
use common::{NOW, session};

#[tokio::test(flavor = "multi_thread")]
async fn a_leader_that_fails_leaves_the_run_not_run_with_what_it_printed() {
    let dir = tempfile::tempdir().unwrap();
    let database = Database::open(
        &dir.path().join("state.db"),
        &[
            cockpit_authorization::MIGRATIONS,
            cockpit_processes::MIGRATIONS,
            cockpit_approvals::MIGRATIONS,
        ],
    )
    .await
    .unwrap();
    let fixture = common::Fixture {
        dir: tempfile::tempdir().unwrap(),
        database,
    };
    let asked = fixture.asked(&session("s1"), "true").await;
    let write = fixture.database.write().await.unwrap();
    let after_grace = NOW + cockpit_approvals::APPROVAL_GRACE_SECONDS;
    let (approved, _) = approve_run(
        &write,
        asked.id,
        asked.revision,
        &asked.digest(),
        after_grace,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    // A leader that prints why it failed and exits before the program could start.
    let settings = RunSettings::new(
        PathBuf::from("/bin/sh"),
        PathBuf::from("/bin/sh"),
        vec![
            OsString::from("-c"),
            OsString::from("echo cannot take the terminal; exit 127"),
        ],
    );
    let supervisor = Supervisor::new(
        dir.path().join("executions"),
        LaunchContainment::ProcessGroup,
        SystemdTools::from_path(),
    );
    let runner = Runner::new(settings, supervisor);

    let ended = runner.run(&fixture.database, &approved, &[]).await;

    let Err(NoResult::NotStarted(reason)) = ended else {
        panic!("expected a run that never started, got {ended:?}");
    };
    assert!(reason.contains("cannot take the terminal"), "{reason}");
}

#[test]
fn a_run_whose_launch_never_started_after_a_restart_is_not_run() {
    assert!(matches!(
        ended_after_restart(EndReason::NeverStarted),
        NoResult::NotStarted(_)
    ));
    assert!(matches!(
        ended_after_restart(EndReason::Exited),
        NoResult::Unobserved(_)
    ));
}
