//! Terminal executions (#163) on real processes, a real pseudo-terminal and a real state
//! database: the execution is the terminal's session, led by the program, and its exit
//! status comes back. util-linux `setsid --ctty` plays the session leader the daemon is in
//! production (`cockpit_processes::run_session_leader`, proven by `cockpit-daemon`'s
//! approvals tests).
#![cfg(target_os = "linux")]

mod common;

use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::path::PathBuf;

use cockpit_processes::{EndReason, Error, LaunchSpec, Phase, Signal, Terminal, Termination};
use common::*;
use rustix::pty::OpenptFlags;

/// A new pseudo-terminal: the side the test reads, and the side the execution gets.
fn open_terminal() -> (File, OwnedFd) {
    let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
    rustix::pty::grantpt(&master).unwrap();
    rustix::pty::unlockpt(&master).unwrap();
    let name = rustix::pty::ptsname(&master, Vec::new()).unwrap();
    let device = File::options()
        .read(true)
        .write(true)
        .open(name.to_str().unwrap())
        .unwrap();
    (File::from(master), OwnedFd::from(device))
}

fn in_terminal(home: &Home, script: &str, device: OwnedFd) -> LaunchSpec {
    let mut spec = shell(home, script);
    spec.terminal = Some(Terminal {
        device,
        leader: PathBuf::from("/usr/bin/setsid"),
        leader_arguments: vec![OsString::from("--ctty")],
    });
    spec
}

/// Everything the terminal shows until every process closed it.
fn read_to_end(mut master: File) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut shown = Vec::new();
        let mut buffer = [0u8; 4096];
        // EIO once every process closed the terminal.
        while let Ok(read @ 1..) = master.read(&mut buffer) {
            shown.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8_lossy(&shown).into_owned()
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_execution_is_the_terminals_session_led_by_the_program() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let (master, device) = open_terminal();
    let shown = read_to_end(master);

    // Fields 1, 5, 6 and 7 of /proc/<pid>/stat: pid, process group, session, terminal.
    let execution = supervisor
        .launch(
            &database,
            &in_terminal(
                &home,
                "read -r stat < /proc/$$/stat; set -- ${stat##*) }; echo \"$$ $3 $4 $5\"; exit 7",
                device,
            ),
        )
        .await
        .unwrap();

    let identity = execution.identity().unwrap();
    let shown = tokio::task::spawn_blocking(move || shown.join().unwrap())
        .await
        .unwrap();
    let fields: Vec<i64> = shown
        .split_whitespace()
        .map(|field| field.parse().unwrap())
        .collect();
    let [pid, group, session, terminal] = fields[..] else {
        panic!("expected four fields, the terminal showed {shown:?}");
    };
    assert_eq!(pid, i64::from(identity.process_group));
    assert_eq!((group, session), (pid, pid));
    assert_ne!(
        terminal, 0,
        "the terminal is the program's controlling terminal"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_execution_reports_its_programs_exit_status() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let (_master, device) = open_terminal();
    let write = database.write().await.unwrap();
    let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
    write.commit().await.unwrap();

    let launched = supervisor
        .launch_reserved(&database, id, &in_terminal(&home, "exit 7", device))
        .await
        .unwrap();
    let status = launched.exit.await.unwrap().unwrap();

    assert_eq!(status.code(), Some(7));
    assert_eq!(
        wait_until_ended(&supervisor, &database, id).await.phase,
        Phase::Ended(EndReason::Exited)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reserved_execution_launches_once() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let write = database.write().await.unwrap();
    let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
    write.commit().await.unwrap();
    supervisor
        .launch_reserved(&database, id, &shell(&home, "exit 0"))
        .await
        .unwrap();

    let again = supervisor
        .launch_reserved(&database, id, &shell(&home, "exit 0"))
        .await;

    assert!(matches!(again, Err(Error::NotReserved { execution }) if execution == id));
}

#[tokio::test(flavor = "multi_thread")]
async fn terminating_a_terminal_execution_reaches_its_whole_session() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let (_master, device) = open_terminal();
    let execution = supervisor
        .launch(
            &database,
            &in_terminal(&home, "sleep 300 & exec sleep 300", device),
        )
        .await
        .unwrap();

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
    assert!(!process_exists(execution.process_group.unwrap()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_leader_that_cannot_take_the_terminal_never_starts_the_program() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let not_a_terminal = OwnedFd::from(File::open("/dev/null").unwrap());
    let marker = home.path().join("ran");

    let error = supervisor
        .launch(
            &database,
            &in_terminal(
                &home,
                &format!("touch {}", marker.display()),
                not_a_terminal,
            ),
        )
        .await
        .unwrap_err();

    let Error::LaunchFailed { execution, .. } = error else {
        panic!("expected a failed launch, got {error:?}");
    };
    assert_eq!(
        cockpit_processes::execution(&database, execution)
            .await
            .unwrap()
            .phase,
        Phase::Ended(EndReason::NeverStarted)
    );
    assert!(!marker.exists());
}

/// Between its spawn and its `setsid`, a terminal launcher's pid lives in the daemon's group
/// and the group it will lead has no member yet: an observation then must not end it.
#[tokio::test(flavor = "multi_thread")]
async fn a_launcher_not_yet_leading_its_group_is_not_observed_gone() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let write = database.write().await.unwrap();
    let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
    write.commit().await.unwrap();
    // In this test's process group, as the launcher is before it calls `setsid`.
    let mut launcher = std::process::Command::new("sleep")
        .arg("300")
        .spawn()
        .unwrap();
    set_column(&database, id, &format!("process_group = {}", launcher.id())).await;

    let observed = supervisor.observe(&database, id).await.unwrap();

    launcher.kill().unwrap();
    launcher.wait().unwrap();
    assert_eq!(observed.phase, Phase::Launching);
}
