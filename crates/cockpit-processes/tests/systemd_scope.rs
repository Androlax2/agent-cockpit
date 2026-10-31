//! The systemd scope containment: through fake `systemd-run` and `systemctl` everywhere,
//! and against this user's real manager where one answers.

mod common;

use cockpit_processes::{
    Containment, EndReason, Error, LaunchContainment, Phase, Signal, Supervisor, SystemdTools,
    Termination,
};
use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_scope_launch_names_its_unit_after_the_database_epoch_and_the_execution() {
    let home = Home::new();
    let fakes = FakeSystemd::new();
    let database = home.open_database().await;
    let supervisor = home.scope_supervisor(&fakes);

    let execution = supervisor
        .launch(&database, &shell(&home, "exec sleep 300"))
        .await
        .unwrap();

    let unit = format!("cockpit-{}-{}.scope", database.epoch(), execution.id.0);
    assert_eq!(
        execution.containment,
        Containment::SystemdScope { unit: unit.clone() }
    );
    assert_running(&execution);
    assert_eq!(fakes.calls(), [format!("systemd-run {unit}")]);
    kill_group(execution.process_group.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn termination_signals_the_whole_scope_through_systemctl() {
    let home = Home::new();
    let fakes = FakeSystemd::new();
    let database = home.open_database().await;
    let supervisor = home.scope_supervisor(&fakes);
    let execution = supervisor
        .launch(&database, &shell(&home, "exec sleep 300"))
        .await
        .unwrap();
    let Containment::SystemdScope { unit } = &execution.containment else {
        panic!("a scope execution");
    };

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
        fakes.calls().contains(&format!(
            "systemctl --user kill --kill-who=all --signal=SIGTERM {unit}"
        )),
        "{:?}",
        fakes.calls()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launcher_that_cannot_reach_the_manager_fails_loud_and_releases_its_reservation() {
    let home = Home::new();
    let fakes = FakeSystemd::new();
    fakes.fail_launch();
    let database = home.open_database().await;
    let supervisor = home.scope_supervisor(&fakes);

    let error = supervisor
        .launch(&database, &shell(&home, "exit 0"))
        .await
        .unwrap_err();

    let Error::LaunchFailed {
        execution,
        launcher_output,
    } = error
    else {
        panic!("expected a launch failure, got {error}");
    };
    assert!(
        launcher_output.contains("Failed to connect to bus"),
        "{launcher_output}"
    );
    let ended = cockpit_processes::execution(&database, execution)
        .await
        .unwrap();
    assert_eq!(ended.phase, Phase::Ended(EndReason::NeverStarted));
}

#[tokio::test(flavor = "multi_thread")]
async fn detection_falls_back_to_process_groups_without_a_user_manager() {
    let home = Home::new();
    let fakes = FakeSystemd::new();
    fakes.without_manager();

    let supervisor = Supervisor::detect(home.executions_dir(), fakes.tools()).await;

    assert_eq!(
        supervisor.launch_containment(),
        LaunchContainment::ProcessGroup
    );
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn detection_picks_a_scope_when_the_user_manager_answers() {
    let home = Home::new();
    let fakes = FakeSystemd::new();

    let supervisor = Supervisor::detect(home.executions_dir(), fakes.tools()).await;

    assert_eq!(
        supervisor.launch_containment(),
        LaunchContainment::SystemdScope
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scope_launch_on_systemd_before_254_omits_the_switch_it_does_not_know() {
    let home = Home::new();
    let fakes = FakeSystemd::new();
    fakes.report_version(252);
    let database = home.open_database().await;
    let supervisor = home.scope_supervisor(&fakes);

    let execution = supervisor
        .launch(&database, &shell(&home, "exec sleep 300"))
        .await
        .unwrap();

    assert_running(&execution);
    kill_group(execution.process_group.unwrap());
}

/// The reason for scopes: a descendant that leaves the process group with `setsid` is still
/// in the scope's cgroup, so termination reaches it. Needs this user's real systemd manager,
/// which sandboxes and CI runners lack: run it with `--run-ignored only` on a desktop session.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a reachable systemd user manager"]
async fn a_real_user_scope_terminates_a_descendant_that_left_the_process_group() {
    let tools = SystemdTools::from_path();
    assert!(
        tools.manager_reachable().await,
        "no reachable systemd user manager (`systemctl --user show-environment` failed)"
    );
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = Supervisor::new(
        home.executions_dir(),
        LaunchContainment::SystemdScope,
        tools,
    );
    let escaped_pid_file = home.path().join("escaped.pid");
    let script = format!(
        "setsid sh -c 'echo $$ > {}; exec sleep 300' & exec sleep 300",
        escaped_pid_file.display()
    );
    let execution = supervisor
        .launch(&database, &shell(&home, &script))
        .await
        .unwrap();
    let escaped = read_pid(&escaped_pid_file).await;

    let termination = supervisor
        .terminate(&database, execution.id, QUICK_ESCALATION)
        .await
        .unwrap();

    assert!(
        matches!(termination, Termination::Confirmed { .. }),
        "{termination:?}"
    );
    assert!(
        eventually(|| !process_exists(escaped)).await,
        "{escaped} escaped the scope"
    );
}
