//! The enforcement backend: delegated controllers through a fake `systemctl` and a fake
//! cgroup tree (Linux), the launch shims on real processes, and freeze/thaw of a process group.

mod common;

use std::ffi::OsString;
use std::path::PathBuf;

use cockpit_processes::{Containment, Execution, ExecutionId, LaunchSpec, Phase};
use cockpit_resource_admission::{
    Availability, Backend, Control, Effective, Limits, LimitReport, Observed,
};
use common::*;

fn availability(backend: &Backend, control: Control) -> Availability {
    backend
        .support()
        .into_iter()
        .find(|support| support.control == control)
        .expect("every control is reported")
        .availability
}

fn scope_execution(unit: &str) -> Execution {
    Execution {
        id: ExecutionId(7),
        containment: Containment::SystemdScope {
            unit: unit.to_owned(),
        },
        boot_id: "boot".to_owned(),
        phase: Phase::Running,
        process_group: Some(i32::MAX),
        leader_start: Some("1".to_owned()),
        cancel_requested: false,
    }
}

fn report(reports: &[LimitReport], control: Control) -> &LimitReport {
    reports
        .iter()
        .find(|report| report.control == control)
        .expect("every control is reported")
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_controllers_delegated_to_the_user_manager_decide_which_cgroup_limits_are_supported() {
    let host = FakeHost::new("memory pids\n");

    let backend = host.scope_backend().await;

    assert_eq!(
        availability(&backend, Control::CpuQuota),
        Availability::Unsupported {
            why: format!(
                "the cpu controller is not delegated to {MANAGER_GROUP} (delegated: memory pids)"
            )
        }
    );
    assert!(matches!(
        availability(&backend, Control::MemoryMax),
        Availability::Supported { .. }
    ));
    assert!(matches!(
        availability(&backend, Control::TasksMax),
        Availability::Supported { .. }
    ));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cpu_weight_and_cpu_quota_are_reported_as_separate_mechanisms() {
    let host = FakeHost::new("cpu memory pids\n");

    let backend = host.scope_backend().await;

    let Availability::Supported { how: weight } = availability(&backend, Control::CpuWeight) else {
        panic!("cpu weight is supported");
    };
    let Availability::Supported { how: quota } = availability(&backend, Control::CpuQuota) else {
        panic!("cpu quota is supported");
    };
    assert!(weight.starts_with("CPUWeight="), "{weight}");
    assert!(quota.starts_with("CPUQuota="), "{quota}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_manager_that_does_not_answer_supports_no_cgroup_limit_and_says_why() {
    let host = FakeHost::new("cpu memory pids\n");
    host.fail_systemctl();

    let backend = host.scope_backend().await;

    let Availability::Unsupported { why } = availability(&backend, Control::MemoryMax) else {
        panic!("memory max is unsupported");
    };
    assert!(why.contains("cannot ask the systemd user manager"), "{why}");
    assert!(why.contains("Failed to connect to bus"), "{why}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn enforcing_sets_the_supported_limits_on_the_running_scope_and_resets_the_unrequested() {
    let host = FakeHost::new("memory pids\n");
    let backend = host.scope_backend().await;
    let limits = Limits {
        cpu_quota_percent: Some(50),
        memory_max_mib: Some(512),
        ..Limits::default()
    };

    backend
        .enforce(&scope_execution("cockpit-e-7.scope"), &limits)
        .await
        .unwrap();

    assert_eq!(
        host.calls().last().unwrap(),
        "--user set-property --runtime cockpit-e-7.scope MemoryMax=512M TasksMax=infinity"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_report_reads_effective_limits_back_from_the_scopes_cgroup_files() {
    let host = FakeHost::new("cpu memory pids\n");
    let unit = "cockpit-e-7.scope";
    host.scope(
        unit,
        &[
            ("cpu.weight", "1\n"),
            ("cpu.max", "50000 100000\n"),
            ("memory.max", "536870912\n"),
            ("pids.max", "max\n"),
            ("cgroup.freeze", "0\n"),
            ("memory.current", "1048576\n"),
            ("pids.current", "3\n"),
        ],
    );
    let backend = host.scope_backend().await;
    let limits = Limits {
        cpu_weight: Some(1),
        cpu_quota_percent: Some(50),
        memory_max_mib: Some(512),
        ..Limits::default()
    };

    let inspected = backend.inspect(&scope_execution(unit), &limits).await;

    let effective = |control| report(&inspected.limits, control).effective.clone();
    assert_eq!(effective(Control::CpuWeight), Effective::Value("1".to_owned()));
    assert_eq!(effective(Control::CpuQuota), Effective::Value("50%".to_owned()));
    assert_eq!(effective(Control::MemoryMax), Effective::Value("512M".to_owned()));
    assert_eq!(effective(Control::TasksMax), Effective::Value("none".to_owned()));
    assert_eq!(effective(Control::Freeze), Effective::Value("thawed".to_owned()));
    assert_eq!(
        report(&inspected.limits, Control::CpuQuota).requested,
        Some("50%".to_owned())
    );
    assert_eq!(
        inspected.observed,
        Observed {
            memory_bytes: Some(1_048_576),
            tasks: Some(3),
        }
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_limit_whose_cgroup_file_is_absent_is_reported_unknown_not_enforced() {
    let host = FakeHost::new("cpu memory pids\n");
    let unit = "cockpit-e-7.scope";
    host.scope(unit, &[("memory.max", "max\n")]);
    let backend = host.scope_backend().await;

    let inspected = backend
        .inspect(&scope_execution(unit), &Limits::default())
        .await;

    let Effective::Unknown { why } = &report(&inspected.limits, Control::CpuQuota).effective
    else {
        panic!("cpu quota has no effective value");
    };
    assert!(why.contains("cpu.max is absent"), "{why}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn freezing_a_scope_uses_the_cgroup_freezer() {
    let host = FakeHost::new("cpu memory pids\n");
    let backend = host.scope_backend().await;
    let execution = scope_execution("cockpit-e-7.scope");

    backend.freeze(&execution).await.unwrap();
    backend.thaw(&execution).await.unwrap();

    let calls = host.calls();
    assert_eq!(
        calls[calls.len() - 2..],
        [
            "--user freeze cockpit-e-7.scope",
            "--user thaw cockpit-e-7.scope"
        ]
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_shims_wrap_the_program_so_every_child_inherits_nice_and_idle_scheduling() {
    let host = FakeHost::new("cpu memory pids\n");
    let backend = host.scope_backend().await;
    let spec = LaunchSpec {
        program: PathBuf::from("/usr/bin/make"),
        arguments: vec![OsString::from("test")],
        working_dir: PathBuf::from("/"),
        environment: vec![],
    };
    let limits = Limits {
        idle_scheduling: true,
        nice: Some(19),
        ..Limits::default()
    };

    let wrapped = backend.wrap(spec, &limits);

    assert_eq!(wrapped.program, PathBuf::from("/usr/bin/nice"));
    assert_eq!(
        wrapped.arguments,
        ["-n", "19", "/usr/bin/chrt", "--idle", "0", "/usr/bin/make", "test"]
            .map(OsString::from)
    );
}

#[tokio::test]
async fn a_launch_that_asks_for_no_shim_runs_the_program_itself() {
    let backend = process_group_backend().await;
    let home = Home::new();

    let wrapped = backend.wrap(home.sleeper(), &Limits::default());

    assert_eq!(wrapped.program, PathBuf::from("/bin/sleep"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_niced_execution_reports_the_niceness_the_kernel_shows() {
    let home = Home::new();
    let database = home.open_database().await;
    let backend = process_group_backend().await;
    let limits = Limits {
        nice: Some(19),
        ..Limits::default()
    };
    let execution = home
        .launch(&database, &backend.wrap(home.sleeper(), &limits))
        .await;

    let inspected = backend.inspect(&execution, &limits).await;

    assert_eq!(
        report(&inspected.limits, Control::Nice).effective,
        Effective::Value("19".to_owned())
    );
    home.stop(&database, &execution).await;
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_execution_reports_sched_idle_from_the_kernel() {
    let home = Home::new();
    let database = home.open_database().await;
    let backend = process_group_backend().await;
    let limits = Limits {
        idle_scheduling: true,
        ..Limits::default()
    };
    let execution = home
        .launch(&database, &backend.wrap(home.sleeper(), &limits))
        .await;

    let inspected = backend.inspect(&execution, &limits).await;

    assert_eq!(
        report(&inspected.limits, Control::IdleScheduling).effective,
        Effective::Value("idle".to_owned())
    );
    home.stop(&database, &execution).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn freezing_a_process_group_stops_it_and_thawing_resumes_it() {
    let home = Home::new();
    let database = home.open_database().await;
    let backend = process_group_backend().await;
    let execution = home.launch(&database, &home.sleeper()).await;
    let frozen_state = || async {
        report(
            &backend.inspect(&execution, &Limits::default()).await.limits,
            Control::Freeze,
        )
        .effective
        .clone()
    };

    backend.freeze(&execution).await.unwrap();
    let frozen = eventually(&frozen_state, "frozen").await;
    backend.thaw(&execution).await.unwrap();
    let thawed = eventually(&frozen_state, "thawed").await;

    assert_eq!(frozen, Effective::Value("frozen".to_owned()));
    assert_eq!(thawed, Effective::Value("thawed".to_owned()));
    home.stop(&database, &execution).await;
}

/// A signal is delivered asynchronously: the state is read until it shows `wanted`, for at
/// most a few seconds.
async fn eventually<Read, Reading>(read: &Read, wanted: &str) -> Effective
where
    Read: Fn() -> Reading,
    Reading: std::future::Future<Output = Effective>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let effective = read().await;
        if effective == Effective::Value(wanted.to_owned()) || std::time::Instant::now() > deadline
        {
            return effective;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
