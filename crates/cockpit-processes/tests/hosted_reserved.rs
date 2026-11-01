//! A hosted execution reserved in the caller's transaction (#256): a capability commits the
//! launch intent with its own record of what the execution is for, then prepares the host's
//! command for it, so no execution is ever reserved that the capability does not name.

mod common;

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use cockpit_processes::{EndReason, Error, ExecutionId, Phase, Supervisor};
use cockpit_turso::Database;
use common::*;

async fn reserve(database: &Database, supervisor: &Supervisor) -> ExecutionId {
    let write = database.write().await.unwrap();
    let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
    write.commit().await.unwrap();
    id
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reserved_hosted_launch_runs_as_the_execution_the_caller_reserved() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let reserved = reserve(&database, &supervisor).await;

    let launch = supervisor
        .prepare_reserved_hosted(&database, reserved, &shell(&home, "exec sleep 30"), &[])
        .await
        .unwrap();
    // Reaped at once, as a tmux pane's exit is, so no zombie keeps its group alive.
    let mut host = Command::new(&launch.command[0])
        .args(&launch.command[1..])
        .current_dir(home.path())
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let reaper = std::thread::spawn(move || drop(host.wait()));
    let execution = supervisor
        .confirm_hosted(&database, launch.execution)
        .await
        .unwrap();

    assert_eq!(launch.execution, reserved);
    assert_running(&execution);
    supervisor
        .terminate(&database, reserved, QUICK_ESCALATION)
        .await
        .unwrap();
    reaper.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_environment_ends_the_reserved_execution_as_never_started() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let reserved = reserve(&database, &supervisor).await;
    let mut spec = shell(&home, "true");
    spec.environment.push(("A=B".into(), "c".into()));

    let refused = supervisor
        .prepare_reserved_hosted(&database, reserved, &spec, &[])
        .await;

    assert!(matches!(refused, Err(Error::InvalidEnvironment { .. })));
    let execution = cockpit_processes::execution(&database, reserved)
        .await
        .unwrap();
    assert_eq!(execution.phase, Phase::Ended(EndReason::NeverStarted));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_execution_that_is_no_longer_reserved_is_refused() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let reserved = reserve(&database, &supervisor).await;
    supervisor
        .abandon_hosted(&database, reserved)
        .await
        .unwrap();

    let refused = supervisor
        .prepare_reserved_hosted(&database, reserved, &shell(&home, "true"), &[])
        .await;

    assert!(
        matches!(refused, Err(Error::NotReserved { execution }) if execution == reserved),
        "{refused:?}"
    );
}
