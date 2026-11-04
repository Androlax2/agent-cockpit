//! The low-disk guard (#406): below the floor, agent launches and heavy jobs are held with
//! the reason; deliveries and approvals are not; above it nothing changes.

mod common;

use cockpit_resource_admission::{Admission, Backend, DiskSpace, Hold, HostTools, Workload};
use common::*;

const NOW: i64 = 1_000_000;
const LOW: DiskSpace = DiskSpace {
    free_mib: 2_000,
    floor_mib: 10_240,
};

async fn admission_with(space: Option<DiskSpace>, workload: Workload) -> Admission {
    let home = Home::new();
    let database = home.open_database().await;
    let mut policy = policy();
    policy.low_disk = space;
    let write = database.write().await.unwrap();
    let admission =
        cockpit_resource_admission::admit(&write, &request(workload, 100, 64), &policy, &[], NOW)
            .await
            .unwrap();
    write.commit().await.unwrap();
    admission
}

#[tokio::test]
async fn agent_launches_and_heavy_jobs_wait_while_disk_space_is_low() {
    for workload in [
        Workload::AgentTurn,
        Workload::Build,
        Workload::Test,
        Workload::Container,
    ] {
        assert_eq!(
            admission_with(Some(LOW), workload).await,
            Admission::Held(Hold::LowDisk(LOW)),
            "{}",
            workload.as_str()
        );
    }
}

#[tokio::test]
async fn deliveries_and_approvals_keep_going_while_disk_space_is_low() {
    for workload in [Workload::Delivery, Workload::Approval] {
        assert!(
            matches!(
                admission_with(Some(LOW), workload).await,
                Admission::Admitted(_)
            ),
            "{}",
            workload.as_str()
        );
    }
}

#[tokio::test]
async fn work_is_admitted_once_disk_space_is_back() {
    assert!(matches!(
        admission_with(None, Workload::Build).await,
        Admission::Admitted(_)
    ));
}

#[test]
fn the_hold_names_free_space_and_the_floor() {
    assert_eq!(
        Hold::LowDisk(LOW).to_string(),
        "free disk space is 2000 MiB, below the 10240 MiB floor: new launches and heavy jobs wait"
    );
}

#[test]
fn a_floor_of_zero_is_never_low() {
    let off = DiskSpace {
        free_mib: 0,
        floor_mib: 0,
    };
    assert!(!off.is_low());
    assert!(LOW.is_low());
}

#[tokio::test]
async fn the_backend_policy_follows_the_guards_reading() {
    let backend = Backend::detect(
        HostTools::from_host(&cockpit_processes::SystemdTools::from_path()),
        cockpit_processes::LaunchContainment::ProcessGroup,
    )
    .await
    .unwrap();
    assert_eq!(backend.policy().low_disk, None);
    backend.set_disk_space(Some(LOW));
    assert_eq!(backend.policy().low_disk, Some(LOW));
    backend.set_disk_space(Some(DiskSpace {
        free_mib: 20_000,
        ..LOW
    }));
    assert_eq!(backend.policy().low_disk, None);
}

#[test]
fn space_equal_to_the_floor_is_not_low() {
    let at_floor = DiskSpace {
        free_mib: 10_240,
        floor_mib: 10_240,
    };
    assert!(!at_floor.is_low());
}
