//! Hosted executions: a host (tmux, in the product) runs the command `prepare_hosted`
//! returns. Here the test plays the host, starting the command as a process-group leader
//! with an environment of its own that must not reach the program.

mod common;

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use cockpit_processes::{EndReason, Error, Phase, Termination};
use common::*;

/// Runs `command` the way a tmux pane does: its own process group, the host's environment,
/// reaped by the host when it exits. Answers the leader's pid and the reaper.
fn host(
    command: &[std::ffi::OsString],
    working_dir: &std::path::Path,
) -> (u32, std::thread::JoinHandle<()>) {
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .current_dir(working_dir)
        .env("HOST_ONLY_SECRET", "leaked")
        .env("HOST_TERMINAL", "the host's terminal")
        .env_remove("UNSET_ON_HOST")
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("the host runs the command");
    let pid = child.id();
    (pid, std::thread::spawn(move || drop(child.wait())))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hosted_program_is_confirmed_running_as_its_own_process_group() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let launch = supervisor
        .prepare_hosted(&database, &shell(&home, "exec sleep 30"), &[])
        .await
        .unwrap();

    let (leader, reaper) = host(&launch.command, home.path());
    let execution = supervisor
        .confirm_hosted(&database, launch.execution)
        .await
        .unwrap();

    assert_running(&execution);
    let identity = execution.identity().unwrap();
    assert_eq!(identity.process_group, i32::try_from(leader).unwrap());
    let termination = supervisor
        .terminate(&database, execution.id, QUICK_ESCALATION)
        .await
        .unwrap();
    assert!(matches!(termination, Termination::Confirmed { .. }));
    reaper.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hosted_program_gets_its_specs_environment_and_only_the_named_host_variables() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let printed = home.path().join("environment");
    let mut spec = shell(&home, &format!("env > {}", printed.display()));
    spec.environment.push(("SPEC_VALUE".into(), "kept".into()));
    let launch = supervisor
        .prepare_hosted(&database, &spec, &["HOST_TERMINAL", "UNSET_ON_HOST"])
        .await
        .unwrap();

    let (_, reaper) = host(&launch.command, home.path());
    reaper.join().unwrap();

    let environment = std::fs::read_to_string(&printed).unwrap();
    assert!(environment.contains("SPEC_VALUE=kept"), "{environment}");
    assert!(!environment.contains("HOST_ONLY_SECRET"), "{environment}");
    assert!(
        environment.contains("HOST_TERMINAL=the host's terminal"),
        "{environment}"
    );
    assert!(!environment.contains("UNSET_ON_HOST"), "{environment}");
    let ended = wait_until_ended(&supervisor, &database, launch.execution).await;
    assert_eq!(ended.phase, Phase::Ended(EndReason::Exited));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hosted_programs_environment_values_never_appear_in_its_command() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let printed = home.path().join("environment");
    let mut spec = shell(&home, &format!("env > {}", printed.display()));
    let secret = "it's a \"token\" $HOME `x`";
    spec.environment
        .push(("LAUNCH_SECRET".into(), secret.into()));
    let launch = supervisor
        .prepare_hosted(&database, &spec, &[])
        .await
        .unwrap();

    let words_hold_it = launch
        .command
        .iter()
        .any(|word| word.to_string_lossy().contains("token"));
    let (_, reaper) = host(&launch.command, home.path());
    reaper.join().unwrap();

    assert!(!words_hold_it, "{:?}", launch.command);
    let environment = std::fs::read_to_string(&printed).unwrap();
    assert!(
        environment.contains(&format!("LAUNCH_SECRET={secret}")),
        "{environment}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hosted_launch_its_host_never_ran_releases_its_reservation() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let launch = supervisor
        .prepare_hosted(&database, &shell(&home, "exec sleep 30"), &[])
        .await
        .unwrap();

    supervisor
        .abandon_hosted(&database, launch.execution)
        .await
        .unwrap();

    let execution = cockpit_processes::execution(&database, launch.execution)
        .await
        .unwrap();
    assert_eq!(execution.phase, Phase::Ended(EndReason::NeverStarted));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_name_env_would_misread_is_refused_before_any_reservation() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let mut spec = shell(&home, "true");
    spec.environment.push(("A=B".into(), "c".into()));

    let refused = supervisor.prepare_hosted(&database, &spec, &[]).await;

    assert!(matches!(refused, Err(Error::InvalidEnvironment { .. })));
    assert_eq!(
        cockpit_processes::occupying_executions(&database)
            .await
            .unwrap(),
        []
    );
}
