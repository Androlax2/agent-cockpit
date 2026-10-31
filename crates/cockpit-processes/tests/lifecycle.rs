//! Launch, natural end and whole-group termination on real processes and a real state
//! database, process-group containment.

mod common;

use cockpit_processes::{
    Containment, EndReason, OutputKind, OutputRead, Phase, Signal, Termination,
};
use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_launched_program_leads_its_own_process_group_and_is_recorded_running() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();

    let execution = supervisor
        .launch(&database, &shell(&home, "exec sleep 30"))
        .await
        .unwrap();

    assert_running(&execution);
    assert_eq!(execution.containment, Containment::ProcessGroup);
    let identity = execution
        .identity()
        .expect("a running execution has its identity");
    let own_group = rustix::process::getpgid(None).unwrap().as_raw_pid();
    assert_ne!(identity.process_group, own_group);
    assert!(process_exists(identity.process_group));
    assert_eq!(
        cockpit_processes::occupying_executions(&database)
            .await
            .unwrap(),
        std::slice::from_ref(&execution)
    );
    kill_group(identity.process_group);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_exits_is_observed_ended_and_releases_its_reservation() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();

    let execution = supervisor
        .launch(&database, &shell(&home, "exit 3"))
        .await
        .unwrap();
    let ended = wait_until_ended(&supervisor, &database, execution.id).await;

    assert_eq!(ended.phase, Phase::Ended(EndReason::Exited));
    assert_eq!(
        cockpit_processes::occupying_executions(&database)
            .await
            .unwrap(),
        []
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_environment_is_exactly_the_launch_specs() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let mut spec = shell(&home, "env");
    spec.environment
        .push(("COCKPIT_TEST_ONLY".into(), "passed".into()));

    let execution = supervisor.launch(&database, &spec).await.unwrap();
    wait_until_ended(&supervisor, &database, execution.id).await;

    let printed = std::fs::read_to_string(supervisor.output_path(
        &database,
        execution.id,
        OutputKind::Stdout,
    ))
    .unwrap();
    assert!(printed.contains("COCKPIT_TEST_ONLY=passed"), "{printed}");
    assert!(!printed.contains("HOME="), "{printed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn termination_reaches_every_process_of_the_group() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let child_pid_file = home.path().join("child.pid");
    let script = format!("sleep 300 & echo $! > {}; wait", child_pid_file.display());

    let execution = supervisor
        .launch(&database, &shell(&home, &script))
        .await
        .unwrap();
    let child = read_pid(&child_pid_file).await;
    let termination = supervisor
        .terminate(&database, execution.id, QUICK_ESCALATION)
        .await
        .unwrap();

    assert_eq!(
        termination,
        Termination::Confirmed {
            after: Some(Signal::Term)
        }
    );
    assert!(
        eventually(|| !process_exists(child)).await,
        "child {child} survived"
    );
    let ended = cockpit_processes::execution(&database, execution.id)
        .await
        .unwrap();
    assert_eq!(ended.phase, Phase::Ended(EndReason::Terminated));
    assert!(ended.cancel_requested);
}

#[tokio::test(flavor = "multi_thread")]
async fn termination_escalates_to_sigkill_when_sigterm_is_ignored() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let ready = home.path().join("ready");
    // An ignored signal stays ignored across fork and exec, so the whole group ignores it.
    let script = format!(
        "trap '' TERM; sleep 300 & echo $! > {}; wait",
        ready.display()
    );

    let execution = supervisor
        .launch(&database, &shell(&home, &script))
        .await
        .unwrap();
    read_pid(&ready).await;
    let termination = supervisor
        .terminate(&database, execution.id, QUICK_ESCALATION)
        .await
        .unwrap();

    assert_eq!(
        termination,
        Termination::Confirmed {
            after: Some(Signal::Kill)
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn terminating_an_ended_execution_confirms_without_a_signal() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let execution = supervisor
        .launch(&database, &shell(&home, "exit 0"))
        .await
        .unwrap();
    wait_until_ended(&supervisor, &database, execution.id).await;

    let termination = supervisor
        .terminate(&database, execution.id, QUICK_ESCALATION)
        .await
        .unwrap();

    assert_eq!(termination, Termination::Confirmed { after: None });
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_cannot_be_spawned_releases_its_reservation() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let mut spec = shell(&home, "exit 0");
    spec.working_dir = home.path().join("missing");

    let error = supervisor.launch(&database, &spec).await.unwrap_err();

    assert!(
        matches!(error, cockpit_processes::Error::Io { .. }),
        "{error}"
    );
    assert_eq!(
        cockpit_processes::occupying_executions(&database)
            .await
            .unwrap(),
        []
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn output_is_drained_from_the_files_the_program_writes() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();

    let execution = supervisor
        .launch(&database, &shell(&home, "echo out; echo err >&2"))
        .await
        .unwrap();
    let stdout = supervisor
        .follow_output(&database, &execution, OutputKind::Stdout)
        .unwrap();
    let stderr = supervisor
        .follow_output(&database, &execution, OutputKind::Stderr)
        .unwrap();

    assert_eq!(
        stdout.read(0, 1024).await,
        OutputRead::Bytes {
            offset: 0,
            bytes: b"out\n".to_vec()
        }
    );
    assert_eq!(
        stderr.read(0, 1024).await,
        OutputRead::Bytes {
            offset: 0,
            bytes: b"err\n".to_vec()
        }
    );
    assert_eq!(stdout.read(4, 1024).await, OutputRead::Finished);
}
