mod common;

use cockpit_processes::{Phase, Termination, UnconfirmedReason};
use common::*;

#[tokio::test]
async fn an_unidentified_stop_reports_no_signal_attempts_and_retains_occupancy() {
    let home = Home::new();
    let database = home.open_database().await;
    let mut supervisor = home.process_group_supervisor();
    supervisor.launch_timeout = std::time::Duration::from_millis(20);
    let launch = supervisor
        .prepare_hosted(&database, &shell(&home, "exit 0"), &[])
        .await
        .unwrap();

    let Termination::Unconfirmed { diagnostics } = supervisor
        .terminate(&database, launch.execution, QUICK_ESCALATION)
        .await
        .unwrap()
    else {
        panic!("an unstarted hosted execution is not identified");
    };
    assert_eq!(diagnostics.reason, UnconfirmedReason::Unidentified);
    assert_eq!(diagnostics.term, None);
    assert_eq!(diagnostics.kill, None);
    let occupying = cockpit_processes::occupying_executions(&database)
        .await
        .unwrap();
    assert_eq!(occupying.len(), 1);
    assert_eq!(occupying[0].id, launch.execution);
    assert_eq!(occupying[0].phase, Phase::Launching);
    assert!(occupying[0].cancel_requested);
}
