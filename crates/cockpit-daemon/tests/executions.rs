//! Restart contract, steps 2 and 3 (report 03), through the daemon binary: executions left
//! by a killed daemon are reconciled before the new one accepts requests.

mod common;

use std::ffi::OsString;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

use cockpit_processes::{
    EndReason, Escalation, Execution, LaunchContainment, LaunchSpec, Phase, Supervisor,
    SystemdTools, Termination,
};
use cockpit_turso::Database;
use common::{GENEROUS, TestHome};

fn state_dir(home: &TestHome) -> PathBuf {
    home.state_home().join("cockpit")
}

async fn open_database(home: &TestHome) -> Database {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(state_dir(home))
        .unwrap();
    Database::open(
        &state_dir(home).join("state.db"),
        &cockpit_daemon::MIGRATION_SETS,
    )
    .await
    .unwrap()
}

/// The supervisor a daemon on this home uses, with process-group containment.
fn supervisor(home: &TestHome) -> Supervisor {
    let options = cockpit_daemon::Options {
        runtime_dir: home.socket_dir(),
        state_dir: state_dir(home),
        limits: cockpit_daemon::Limits::default(),
    };
    Supervisor::new(
        options.executions_dir(),
        LaunchContainment::ProcessGroup,
        SystemdTools::from_path(),
    )
}

async fn launch_sleeper(home: &TestHome) -> Execution {
    let database = open_database(home).await;
    let spec = LaunchSpec {
        program: PathBuf::from("/bin/sh"),
        arguments: vec![OsString::from("-c"), OsString::from("exec sleep 300")],
        working_dir: home.home().to_path_buf(),
        environment: vec![],
        terminal: None,
    };
    supervisor(home).launch(&database, &spec).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_reattaches_an_execution_that_kept_running() {
    let home = TestHome::new();
    let execution = launch_sleeper(&home).await;

    let daemon = home.start();
    // The log is written in the background: wait for the line before killing the daemon.
    let deadline = std::time::Instant::now() + GENEROUS;
    let reattached_logged = || {
        std::fs::read_to_string(home.log_file())
            .is_ok_and(|log| log.contains("reattached a running execution"))
    };
    while !reattached_logged() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    daemon.kill();

    assert!(
        reattached_logged(),
        "{:?}",
        std::fs::read_to_string(home.log_file())
    );
    let database = open_database(&home).await;
    let reattached = cockpit_processes::execution(&database, execution.id)
        .await
        .unwrap();
    assert_eq!(reattached.phase, Phase::Running);
    let termination = supervisor(&home)
        .terminate(
            &database,
            execution.id,
            Escalation {
                grace: GENEROUS,
                kill_wait: GENEROUS,
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(termination, Termination::Confirmed { after: Some(_) }),
        "the execution was still running: {termination:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_daemon_releases_an_execution_that_ended_while_it_was_down() {
    let home = TestHome::new();
    let execution = launch_sleeper(&home).await;
    let group = execution.process_group.unwrap();
    std::process::Command::new("/bin/kill")
        .args(["-s", "KILL", "--", &format!("-{group}")])
        .status()
        .unwrap();
    let deadline = std::time::Instant::now() + GENEROUS;
    let group_exists = || {
        std::process::Command::new("/bin/kill")
            .args(["-s", "0", "--", &format!("-{group}")])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    while group_exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "group {group} survived SIGKILL"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    home.start().kill();

    let database = open_database(&home).await;
    let released = cockpit_processes::execution(&database, execution.id)
        .await
        .unwrap();
    assert_eq!(released.phase, Phase::Ended(EndReason::Exited));
}
