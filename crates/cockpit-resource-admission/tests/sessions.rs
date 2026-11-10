//! #1351: the limits on what agents run in their sessions. Nice 19 and the idle IO class in
//! every profile; under `foreground` also the hard ceilings, set on every task of a real
//! process tree and taken off again, and never set where they could not be taken off.

mod common;

use std::ffi::OsString;
use std::path::PathBuf;

use cockpit_processes::{Containment, Execution, ExecutionId, LaunchSpec, Phase};
use cockpit_resource_admission::{
    Availability, Backend, Control, ControlSupport, Demand, Effective, HostTools, Limits, Policy,
    Profile,
};
use common::*;

/// The policy of a host whose budget is eight CPUs.
fn policy() -> Policy {
    Policy::for_budget(Demand {
        cpu_millis: 8_000,
        memory_mib: 16_384,
    })
}

fn session_availability(backend: &Backend, control: Control) -> Availability {
    backend
        .session_support()
        .into_iter()
        .find(|support: &ControlSupport| support.control == control)
        .expect("every session ceiling is reported")
        .availability
}

fn is_supported(backend: &Backend, control: Control) -> bool {
    matches!(
        session_availability(backend, control),
        Availability::Supported { .. }
    )
}

/// A shell and the sleeper it started, one process group: a tree, not one process.
fn tree(home: &Home) -> LaunchSpec {
    LaunchSpec {
        program: PathBuf::from("/bin/sh"),
        arguments: ["-c", "sleep 300 & wait"].map(OsString::from).to_vec(),
        working_dir: home.path().to_path_buf(),
        environment: vec![],
        terminal: None,
    }
}

/// Every process of the execution's group, once its shell started its child.
async fn processes_of(execution: &Execution) -> Vec<i32> {
    let group = execution.process_group.expect("a started execution");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let processes: Vec<i32> = std::fs::read_dir("/proc")
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
            .filter(|pid: &i32| {
                std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                    stat_fields(&stat).get(5 - 3) == Some(&group.to_string().as_str())
                })
            })
            .collect();
        if processes.len() >= 2 {
            return processes;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "group {group} never held two processes: {processes:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn stat_fields(stat: &str) -> Vec<&str> {
    stat.rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .collect()
}

/// Niceness and whether the policy is `SCHED_IDLE`, from `/proc/<pid>/stat`.
fn priority(pid: i32) -> (i32, bool) {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields = stat_fields(&stat);
    (fields[19 - 3].parse().unwrap(), fields[41 - 3] == "5")
}

fn io_class(pid: i32) -> String {
    let output = std::process::Command::new("ionice")
        .args(["-p", &pid.to_string()])
        .output()
        .expect("ionice runs");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn sessions_run_at_nice_19_in_the_idle_io_class_in_every_profile() {
    assert_eq!(
        policy().session_limits(Profile::Normal),
        Limits {
            nice: Some(19),
            idle_io: true,
            ..Limits::default()
        }
    );
}

#[test]
fn the_foreground_profile_adds_the_heavy_jobs_ceilings_with_a_quota_of_its_share() {
    assert_eq!(
        policy().session_limits(Profile::Foreground),
        Limits {
            cpu_weight: Some(1),
            cpu_quota_percent: Some(200),
            idle_scheduling: true,
            nice: Some(19),
            idle_io: true,
            ..Limits::default()
        }
    );
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn a_running_session_gets_the_ceilings_on_every_process_and_keeps_only_nice_and_idle_io_on_normal()
 {
    let home = Home::new();
    let database = home.open_database().await;
    let backend = process_group_backend().await;
    let execution = home.launch(&database, &tree(&home)).await;
    let processes = processes_of(&execution).await;
    let before: Vec<(i32, bool)> = processes.iter().map(|pid| priority(*pid)).collect();

    backend
        .apply_to_session(&execution, &policy().session_limits(Profile::Foreground))
        .await
        .expect("every supported ceiling applies");

    for (pid, before) in processes.iter().zip(&before) {
        let (niceness, idle) = priority(*pid);
        assert_eq!(niceness, 19, "process {pid}");
        assert_eq!(
            idle,
            is_supported(&backend, Control::IdleScheduling) || before.1,
            "process {pid}"
        );
        if is_supported(&backend, Control::IdleIo) {
            assert_eq!(io_class(*pid), "idle", "process {pid}");
        }
    }
    let inspected = backend
        .inspect_session(&execution, &policy().session_limits(Profile::Foreground))
        .await;
    let controls: Vec<Control> = inspected.limits.iter().map(|line| line.control).collect();
    assert_eq!(
        controls,
        [
            Control::CpuWeight,
            Control::CpuQuota,
            Control::IdleScheduling,
            Control::Nice,
            Control::IdleIo
        ]
    );
    if is_supported(&backend, Control::IdleIo) {
        let idle_io = inspected
            .limits
            .iter()
            .find(|line| line.control == Control::IdleIo)
            .unwrap();
        assert_eq!(idle_io.effective, Effective::Value("idle".to_owned()));
    }

    backend
        .apply_to_session(&execution, &policy().session_limits(Profile::Normal))
        .await
        .expect("every supported ceiling comes off");

    for pid in &processes {
        let (niceness, idle) = priority(*pid);
        assert_eq!(niceness, 19, "process {pid}");
        if is_supported(&backend, Control::IdleScheduling) {
            assert!(!idle, "process {pid} is still SCHED_IDLE");
        }
        if is_supported(&backend, Control::IdleIo) {
            assert_eq!(io_class(*pid), "idle", "process {pid}");
        }
    }
    home.stop(&database, &execution).await;
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn idle_scheduling_the_daemon_could_not_take_off_again_is_never_set() {
    let home = Home::new();
    let database = home.open_database().await;
    let tools = HostTools {
        nice_rlimit: Some(0),
        ..HostTools::from_host(&cockpit_processes::SystemdTools::from_path())
    };
    let backend = Backend::detect(tools, cockpit_processes::LaunchContainment::ProcessGroup)
        .await
        .unwrap();
    let execution = home.launch(&database, &tree(&home)).await;
    let processes = processes_of(&execution).await;
    let before: Vec<(i32, bool)> = processes.iter().map(|pid| priority(*pid)).collect();

    backend
        .apply_to_session(&execution, &policy().session_limits(Profile::Foreground))
        .await
        .expect("nothing unsupported is attempted");

    let Availability::Unsupported { why } = session_availability(&backend, Control::IdleScheduling)
    else {
        panic!("SCHED_IDLE is claimed while it could not be undone");
    };
    assert!(why.contains("RLIMIT_NICE is 0"), "{why}");
    let after: Vec<(i32, bool)> = processes.iter().map(|pid| priority(*pid)).collect();
    let expected: Vec<(i32, bool)> = before.iter().map(|(_, idle)| (19, *idle)).collect();
    assert_eq!(after, expected, "nice 19 is set, SCHED_IDLE is not");
    home.stop(&database, &execution).await;
}

#[tokio::test]
async fn a_scope_sessions_cpu_weight_and_quota_are_set_and_reset_and_nothing_else() {
    let host = FakeHost::new("cpu memory pids\n");
    let backend = host.scope_backend().await;
    // A task that ended between the read of the scope and its change is skipped.
    host.scope("cockpit-e-7.scope", &[("cgroup.threads", "2147483646\n")]);
    let execution = Execution {
        id: ExecutionId(7),
        containment: Containment::SystemdScope {
            unit: "cockpit-e-7.scope".to_owned(),
        },
        boot_id: "boot".to_owned(),
        phase: Phase::Running,
        process_group: Some(2_147_483_646),
        leader_start: Some("1".to_owned()),
        cancel_requested: false,
    };

    for profile in [Profile::Foreground, Profile::Normal] {
        backend
            .apply_to_session(&execution, &policy().session_limits(profile))
            .await
            .expect("the scope's weight is set");
    }

    let properties: Vec<String> = host
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("--user set-property"))
        .map(|call| {
            call.split_whitespace()
                .filter(|word| word.contains('='))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    assert_eq!(
        properties,
        ["CPUWeight=1 CPUQuota=200%", "CPUWeight=100 CPUQuota="]
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_idle_io_shim_starts_the_program_in_the_idle_class() {
    let host = FakeHost::new("cpu memory pids\n");
    let backend = host.scope_backend().await;
    let launch = LaunchSpec {
        program: PathBuf::from("/usr/bin/make"),
        arguments: vec![OsString::from("test")],
        working_dir: PathBuf::from("/"),
        environment: vec![],
        terminal: None,
    };

    let wrapped = backend.wrap(
        launch,
        &Limits {
            idle_io: true,
            ..Limits::default()
        },
    );

    assert_eq!(wrapped.program, PathBuf::from("/usr/bin/ionice"));
    assert_eq!(
        wrapped.arguments,
        ["-c", "3", "/usr/bin/make", "test"].map(OsString::from)
    );
}
