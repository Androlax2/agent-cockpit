//! This host's own enforcement report, on whichever platform runs the tests: CI runs it on
//! Linux, macOS and WSL 2 (report 10: advertise only what the backend verified).

mod common;

use cockpit_resource_admission::{Availability, Control, Effective, Limits, Platform};
use common::*;

#[tokio::test]
async fn every_control_is_reported_with_its_mechanism_or_the_missing_piece() {
    let backend = process_group_backend().await;

    let support = backend.support();

    for line in &support {
        println!(
            "{:?} {:?}: {:?}",
            backend.platform(),
            line.control,
            line.availability
        );
    }
    assert_eq!(
        support.iter().map(|line| line.control).collect::<Vec<_>>(),
        Control::ALL
    );
    assert!(support.iter().all(|line| match &line.availability {
        Availability::Supported { how } => !how.is_empty(),
        Availability::Unsupported { why } => !why.is_empty(),
    }));
}

#[tokio::test]
async fn a_process_group_never_claims_a_cgroup_limit() {
    let backend = process_group_backend().await;

    let claimed: Vec<Control> = backend
        .support()
        .into_iter()
        .filter(|line| matches!(line.availability, Availability::Supported { .. }))
        .map(|line| line.control)
        .filter(|control| {
            matches!(
                control,
                Control::CpuWeight | Control::CpuQuota | Control::MemoryMax | Control::TasksMax
            )
        })
        .collect();

    assert_eq!(claimed, vec![]);
}

#[tokio::test]
async fn idle_scheduling_is_claimed_only_where_the_kernel_has_it() {
    let backend = process_group_backend().await;

    let idle = backend
        .support()
        .into_iter()
        .find(|line| line.control == Control::IdleScheduling)
        .unwrap();

    if backend.platform() == Platform::MacOs {
        assert_eq!(
            idle.availability,
            Availability::Unsupported {
                why: "macOS has no SCHED_IDLE; nice is its subset".to_owned()
            }
        );
    } else {
        assert!(
            matches!(idle.availability, Availability::Supported { .. }),
            "{idle:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_niced_execution_on_this_host_shows_the_niceness_it_asked_for() {
    let home = Home::new();
    let database = home.open_database().await;
    let backend = process_group_backend().await;
    let limits = Limits {
        nice: Some(19),
        ..Limits::default()
    };
    let execution = home
        .launch_past_its_shims(&database, &backend.wrap(home.sleeper(), &limits))
        .await;

    let report = backend.inspect(&execution, &limits).await;

    for line in &report.limits {
        println!(
            "{:?}: requested {:?}, effective {:?}",
            line.control, line.requested, line.effective
        );
    }
    let nice = report
        .limits
        .iter()
        .find(|line| line.control == Control::Nice)
        .unwrap();
    assert_eq!(nice.requested.as_deref(), Some("19"));
    assert_eq!(nice.effective, Effective::Value("19".to_owned()));
    home.stop(&database, &execution).await;
}

#[tokio::test]
async fn the_budget_is_the_hosts_cpus_and_memory() {
    let backend = process_group_backend().await;

    let capacity = backend.capacity();

    let cpus = std::thread::available_parallelism().unwrap().get() as u64;
    assert_eq!(capacity.cpu_millis, cpus * 1000);
    assert!(capacity.memory_mib > 0);
}
