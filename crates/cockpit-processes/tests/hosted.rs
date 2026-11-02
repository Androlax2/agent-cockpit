//! Hosted executions: a host (tmux, in the product) runs the command `prepare_hosted`
//! returns. Here the test plays the host, starting the command as a process-group leader
//! with an environment of its own that must not reach the program.

mod common;

#[cfg(target_os = "linux")]
#[path = "common/gated_host.rs"]
mod gated_host;

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

/// [`host`], then the start confirmation and the release the launcher gives a hosted program.
async fn host_released(
    supervisor: &cockpit_processes::Supervisor,
    database: &cockpit_turso::Database,
    launch: &cockpit_processes::HostedLaunch,
    working_dir: &std::path::Path,
) -> std::thread::JoinHandle<()> {
    let (_, reaper) = host(&launch.command, working_dir);
    supervisor
        .confirm_hosted(database, launch.execution)
        .await
        .unwrap();
    supervisor
        .release_hosted(database, launch.execution)
        .await
        .unwrap();
    reaper
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

    let reaper = host_released(&supervisor, &database, &launch, home.path()).await;
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
    let reaper = host_released(&supervisor, &database, &launch, home.path()).await;
    reaper.join().unwrap();

    assert!(!words_hold_it, "{:?}", launch.command);
    let environment = std::fs::read_to_string(&printed).unwrap();
    assert!(
        environment.contains(&format!("LAUNCH_SECRET={secret}")),
        "{environment}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confirmed_hosted_program_runs_only_once_released() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let ran = home.path().join("ran");
    let launch = supervisor
        .prepare_hosted(
            &database,
            &shell(&home, &format!("touch {}", ran.display())),
            &[],
        )
        .await
        .unwrap();
    let (_, reaper) = host(&launch.command, home.path());
    supervisor
        .confirm_hosted(&database, launch.execution)
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let ran_before_release = ran.exists();
    supervisor
        .release_hosted(&database, launch.execution)
        .await
        .unwrap();
    reaper.join().unwrap();

    assert!(!ran_before_release, "the program ran before its release");
    assert!(ran.exists(), "the released program did not run");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hosted_program_never_released_gives_up_without_running() {
    let home = Home::new();
    let database = home.open_database().await;
    let mut supervisor = home.process_group_supervisor();
    supervisor.release_window = std::time::Duration::from_millis(200);
    let ran = home.path().join("ran");
    let launch = supervisor
        .prepare_hosted(
            &database,
            &shell(&home, &format!("touch {}", ran.display())),
            &[],
        )
        .await
        .unwrap();
    let mut child = Command::new(&launch.command[0])
        .args(&launch.command[1..])
        .current_dir(home.path())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    supervisor
        .confirm_hosted(&database, launch.execution)
        .await
        .unwrap();

    let status = tokio::task::spawn_blocking(move || child.wait().unwrap())
        .await
        .unwrap();

    assert_eq!(status.code(), Some(126));
    assert!(!ran.exists(), "the program ran without its release");
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

#[cfg(target_os = "linux")]
const GATED_ESCALATION: cockpit_processes::Escalation = cockpit_processes::Escalation {
    grace: std::time::Duration::from_millis(100),
    kill_wait: std::time::Duration::from_millis(200),
};

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreaped_hosted_leader_keeps_its_reservation_until_reaped() {
    tokio::time::timeout(std::time::Duration::from_secs(25), async {
        let home = Home::new();
        let database = home.open_database().await;
        let supervisor = home.process_group_supervisor();
        let launch = supervisor
            .prepare_hosted(
                &database,
                &shell(&home, "trap '' TERM; echo ready > ready; exec sleep 300"),
                &[],
            )
            .await
            .unwrap();
        let mut host = gated_host::GatedHost::start(&launch.command, home.path()).await;
        supervisor
            .confirm_hosted(&database, launch.execution)
            .await
            .unwrap();
        supervisor
            .release_hosted(&database, launch.execution)
            .await
            .unwrap();
        assert!(eventually(|| home.path().join("ready").exists()).await);

        let termination = supervisor
            .terminate(&database, launch.execution, GATED_ESCALATION)
            .await
            .unwrap();
        assert_eq!(
            host.exited_without_reaping(host.leader)
                .await
                .terminating_signal(),
            Some(9)
        );
        assert!(
            std::fs::read_to_string(format!("/proc/{}/stat", host.leader.as_raw_nonzero()))
                .unwrap()
                .contains(") Z ")
        );
        let Termination::Unconfirmed { diagnostics } = termination else {
            panic!("unreaped leader must remain unconfirmed")
        };
        assert_eq!(
            diagnostics.reason,
            cockpit_processes::UnconfirmedReason::KillWaitElapsed
        );
        assert_eq!(
            diagnostics.term,
            Some(cockpit_processes::SignalOutcome::Sent)
        );
        assert_eq!(
            diagnostics.kill,
            Some(cockpit_processes::SignalOutcome::Sent)
        );
        assert!(
            diagnostics
                .snapshot
                .members
                .iter()
                .any(|member| member.pid == host.leader.as_raw_nonzero().get()
                    && member.state == 'Z')
        );
        assert!(
            diagnostics
                .snapshot
                .parents
                .iter()
                .any(|parent| parent.pid == i32::try_from(std::process::id()).unwrap())
        );
        let kept = supervisor
            .observe(&database, launch.execution)
            .await
            .unwrap();
        assert!(kept.phase.is_occupying() && kept.cancel_requested);
        assert_eq!(
            cockpit_processes::occupying_executions(&database)
                .await
                .unwrap(),
            [kept]
        );

        host.reap(host.leader);
        assert_eq!(
            supervisor
                .observe(&database, launch.execution)
                .await
                .unwrap()
                .phase,
            Phase::Ended(EndReason::Terminated)
        );
        assert_eq!(
            supervisor
                .terminate(&database, launch.execution, GATED_ESCALATION)
                .await
                .unwrap(),
            Termination::Confirmed { after: None }
        );
    })
    .await
    .expect("bounded hosted zombie characterization");
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unconfirmed_stop_retains_the_identity_recovered_from_its_leader_file() {
    tokio::time::timeout(std::time::Duration::from_secs(25), async {
        let home = Home::new();
        let database = home.open_database().await;
        let supervisor = home.process_group_supervisor();
        let launch = supervisor
            .prepare_hosted(
                &database,
                &shell(&home, "trap '' TERM; echo ready > ready; exec sleep 300"),
                &[],
            )
            .await
            .unwrap();
        let mut host = gated_host::GatedHost::start(&launch.command, home.path()).await;
        // Model a host that ran while confirmation was never committed.
        supervisor
            .release_hosted(&database, launch.execution)
            .await
            .unwrap();
        assert!(eventually(|| home.path().join("ready").exists()).await);
        let leader = std::fs::read_to_string(
            supervisor
                .execution_dir(&database, launch.execution)
                .join("leader"),
        )
        .unwrap();
        let expected_start = leader.lines().nth(1).unwrap();
        let before = cockpit_processes::execution(&database, launch.execution)
            .await
            .unwrap();
        assert_eq!(before.phase, Phase::Launching);
        assert_eq!(before.process_group, None);
        assert_eq!(before.leader_start, None);

        let Termination::Unconfirmed { diagnostics } = supervisor
            .terminate(&database, launch.execution, GATED_ESCALATION)
            .await
            .unwrap()
        else {
            panic!("unreaped recovered leader must remain unconfirmed");
        };
        assert_eq!(
            host.exited_without_reaping(host.leader)
                .await
                .terminating_signal(),
            Some(9)
        );
        assert_eq!(
            diagnostics.execution.process_group,
            Some(host.leader.as_raw_nonzero().get())
        );
        assert_eq!(
            diagnostics.execution.leader_start.as_deref(),
            Some(expected_start)
        );
        assert_eq!(diagnostics.execution.phase, Phase::Running);
        host.reap(host.leader);
    })
    .await
    .expect("bounded recovered-identity regression");
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reaped_leader_does_not_release_or_hide_its_live_descendant() {
    use rustix::process::{self, Pid, Signal};
    tokio::time::timeout(std::time::Duration::from_secs(25), async {
        let home = Home::new();
        let database = home.open_database().await;
        let supervisor = home.process_group_supervisor();
        let launch = supervisor.prepare_hosted(&database, &shell(&home,
            "trap '' TERM; (echo ready > ready; exec sleep 300) & echo $! > descendant; wait"), &[]).await.unwrap();
        let mut host = gated_host::GatedHost::start(&launch.command, home.path()).await;
        supervisor.confirm_hosted(&database, launch.execution).await.unwrap();
        supervisor.release_hosted(&database, launch.execution).await.unwrap();
        let descendant = Pid::from_raw(read_pid(&home.path().join("descendant")).await).unwrap();
        assert!(eventually(|| home.path().join("ready").exists()).await);
        process::kill_process(host.leader, Signal::KILL).unwrap();
        host.exited_without_reaping(host.leader).await;
        assert!(supervisor.observe(&database, launch.execution).await.unwrap().phase.is_occupying());
        host.reap(host.leader);
        assert!(supervisor.observe(&database, launch.execution).await.unwrap().phase.is_occupying());
        assert!(process::waitid(process::WaitId::Pid(descendant), process::WaitIdOptions::EXITED | process::WaitIdOptions::NOHANG | process::WaitIdOptions::NOWAIT).unwrap().is_none());

        assert!(matches!(supervisor.terminate(&database, launch.execution, GATED_ESCALATION).await.unwrap(), Termination::Unconfirmed { .. }));
        assert_eq!(host.exited_without_reaping(descendant).await.terminating_signal(), Some(9));
        assert!(supervisor.observe(&database, launch.execution).await.unwrap().phase.is_occupying());
        host.reap(descendant);
        assert_eq!(supervisor.observe(&database, launch.execution).await.unwrap().phase, Phase::Ended(EndReason::Terminated));
    }).await.expect("bounded descendant characterization");
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_that_never_reports_ready_is_killed_and_reaped_by_cleanup() {
    let home = Home::new();
    let command = ["/bin/sleep".into(), "300".into()];
    let mut host = gated_host::GatedHost::start(&command, home.path()).await;
    let ready = tokio::time::timeout(std::time::Duration::from_millis(50), async {
        while !home.path().join("ready").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(ready.is_err());
    host.cleanup();
    assert_eq!(
        rustix::process::test_kill_process_group(host.leader),
        Err(rustix::io::Errno::SRCH)
    );
}
