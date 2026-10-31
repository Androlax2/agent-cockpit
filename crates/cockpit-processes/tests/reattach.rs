//! Restart reconciliation (report 03's restart contract): a "restart" drops the database and
//! the supervisor and opens new ones over the same files, as a new daemon process would.

mod common;

use cockpit_processes::{EndReason, Error, Phase, Reconciled, Signal, Termination};
use common::*;

/// Issue #29's acceptance: the launcher times out, the process still starts and runs, the
/// reservation is kept, and the next daemon reconciles it.
#[tokio::test(flavor = "multi_thread")]
async fn a_timed_out_launcher_keeps_its_reservation_and_is_reconciled_after_restart() {
    let home = Home::new();
    let fakes = FakeSystemd::new();
    fakes.delay_launch(1);
    let pid_file = home.path().join("program.pid");
    let spec = shell(
        &home,
        &format!("echo $$ > {}; exec sleep 300", pid_file.display()),
    );
    let execution_id = {
        let database = home.open_database().await;
        let mut supervisor = home.scope_supervisor(&fakes);
        supervisor.launch_timeout = std::time::Duration::from_millis(100);

        let error = supervisor.launch(&database, &spec).await.unwrap_err();

        let Error::LaunchTimedOut { execution } = error else {
            panic!("expected a launch timeout, got {error}");
        };
        let occupying = cockpit_processes::occupying_executions(&database)
            .await
            .unwrap();
        assert_eq!(occupying.len(), 1);
        assert_eq!(occupying[0].phase, Phase::Launching);
        execution
    };
    let program = read_pid(&pid_file).await;

    let database = home.open_database().await;
    let supervisor = home.scope_supervisor(&fakes);
    let reconciled = supervisor.reconcile(&database).await.unwrap();

    assert_eq!(reconciled, [Reconciled::Reattached(execution_id)]);
    let execution = cockpit_processes::execution(&database, execution_id)
        .await
        .unwrap();
    assert_running(&execution);
    assert_eq!(execution.process_group, Some(program));
    let termination = supervisor
        .terminate(&database, execution_id, QUICK_ESCALATION)
        .await
        .unwrap();
    assert_eq!(
        termination,
        Termination::Confirmed {
            after: Some(Signal::Term)
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_execution_is_reattached_and_still_terminable_after_restart() {
    let home = Home::new();
    let execution = {
        let database = home.open_database().await;
        home.process_group_supervisor()
            .launch(&database, &shell(&home, "exec sleep 300"))
            .await
            .unwrap()
    };

    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let reconciled = supervisor.reconcile(&database).await.unwrap();
    let termination = supervisor
        .terminate(&database, execution.id, QUICK_ESCALATION)
        .await
        .unwrap();

    assert_eq!(reconciled, [Reconciled::Reattached(execution.id)]);
    assert_eq!(
        termination,
        Termination::Confirmed {
            after: Some(Signal::Term)
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_execution_that_ended_while_the_daemon_was_down_is_released() {
    let home = Home::new();
    let execution = {
        let database = home.open_database().await;
        home.process_group_supervisor()
            .launch(&database, &shell(&home, "exec sleep 300"))
            .await
            .unwrap()
    };
    let group = execution.process_group.unwrap();
    kill_group(group);
    assert!(eventually(|| !process_exists(group)).await);

    let database = home.open_database().await;
    let reconciled = home
        .process_group_supervisor()
        .reconcile(&database)
        .await
        .unwrap();

    assert_eq!(
        reconciled,
        [Reconciled::Ended(execution.id, EndReason::Exited)]
    );
    assert_eq!(
        cockpit_processes::occupying_executions(&database)
            .await
            .unwrap(),
        []
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_execution_from_an_earlier_boot_is_released_as_host_restarted() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let execution = supervisor
        .launch(&database, &shell(&home, "exec sleep 300"))
        .await
        .unwrap();
    set_column(&database, execution.id, "boot_id = 'an-earlier-boot'").await;

    let reconciled = supervisor.reconcile(&database).await.unwrap();

    assert_eq!(
        reconciled,
        [Reconciled::Ended(execution.id, EndReason::HostRestarted)]
    );
    kill_group(execution.process_group.unwrap());
}

/// Never PID alone: a live leader whose start token differs is a reused pid, not ours.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_process_group_with_another_leader_start_is_not_reattached() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let execution = supervisor
        .launch(&database, &shell(&home, "exec sleep 300"))
        .await
        .unwrap();
    set_column(&database, execution.id, "leader_start = 'another-start'").await;

    let reconciled = supervisor.reconcile(&database).await.unwrap();

    assert_eq!(
        reconciled,
        [Reconciled::Ended(execution.id, EndReason::Exited)]
    );
    kill_group(execution.process_group.unwrap());
}

/// A daemon that crashed between the spawn and committing the identity still finds the
/// execution: the program wrote its own leader file before it started.
#[tokio::test(flavor = "multi_thread")]
async fn a_crash_before_the_identity_was_committed_is_recovered_from_the_leader_file() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let execution = supervisor
        .launch(&database, &shell(&home, "exec sleep 300"))
        .await
        .unwrap();
    set_column(
        &database,
        execution.id,
        "phase = 'launching', process_group = NULL, leader_start = NULL",
    )
    .await;

    let reconciled = supervisor.reconcile(&database).await.unwrap();

    assert_eq!(reconciled, [Reconciled::Reattached(execution.id)]);
    let recovered = cockpit_processes::execution(&database, execution.id)
        .await
        .unwrap();
    assert_eq!(recovered, execution);
    kill_group(execution.process_group.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_intent_with_no_identity_and_no_leader_file_stays_charged() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let execution = supervisor
        .launch(&database, &shell(&home, "exec sleep 300"))
        .await
        .unwrap();
    set_column(
        &database,
        execution.id,
        "phase = 'launching', process_group = NULL, leader_start = NULL",
    )
    .await;
    std::fs::remove_file(
        supervisor
            .execution_dir(&database, execution.id)
            .join("leader"),
    )
    .unwrap();

    let reconciled = supervisor.reconcile(&database).await.unwrap();
    let termination = supervisor
        .terminate(&database, execution.id, QUICK_ESCALATION)
        .await
        .unwrap();

    assert_eq!(reconciled, [Reconciled::Uncertain(execution.id)]);
    assert_eq!(termination, Termination::Unconfirmed);
    let kept = cockpit_processes::execution(&database, execution.id)
        .await
        .unwrap();
    assert!(
        kept.phase.is_occupying() && kept.cancel_requested,
        "{kept:?}"
    );
    kill_group(execution.process_group.unwrap());
}

/// A stop that races the launch waits for the start confirmation instead of being lost.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_requested_while_the_launcher_is_slow_terminates_once_it_started() {
    let home = Home::new();
    let fakes = FakeSystemd::new();
    fakes.delay_launch(1);
    let database = home.open_database().await;
    let mut supervisor = home.scope_supervisor(&fakes);
    supervisor.launch_timeout = std::time::Duration::from_millis(100);
    let error = supervisor
        .launch(&database, &shell(&home, "exec sleep 300"))
        .await
        .unwrap_err();
    let Error::LaunchTimedOut { execution } = error else {
        panic!("expected a launch timeout, got {error}");
    };

    let observed = supervisor.observe(&database, execution).await.unwrap();
    supervisor.launch_timeout = GENEROUS;
    let termination = supervisor
        .terminate(&database, execution, QUICK_ESCALATION)
        .await
        .unwrap();

    assert_eq!(
        observed.phase,
        Phase::Launching,
        "an unregistered scope is not gone"
    );
    assert_eq!(
        termination,
        Termination::Confirmed {
            after: Some(Signal::Term)
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_execution_that_cannot_be_checked_stays_charged_without_stopping_reconciliation() {
    let home = Home::new();
    let fakes = FakeSystemd::new();
    let database = home.open_database().await;
    let supervisor = home.scope_supervisor(&fakes);
    let execution = supervisor
        .launch(&database, &shell(&home, "exec sleep 300"))
        .await
        .unwrap();
    fakes.break_manager();

    let reconciled = supervisor.reconcile(&database).await.unwrap();

    let [Reconciled::Unchecked(id, reason)] = reconciled.as_slice() else {
        panic!("expected one unchecked execution, got {reconciled:?}");
    };
    assert_eq!(*id, execution.id);
    assert!(reason.contains("Failed to connect to bus"), "{reason}");
    let kept = cockpit_processes::execution(&database, execution.id)
        .await
        .unwrap();
    assert!(kept.phase.is_occupying());
    kill_group(execution.process_group.unwrap());
}
