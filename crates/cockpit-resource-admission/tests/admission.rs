//! Admission on a real state database: the shared budget, the foreground share, the paused
//! queue, required controls, and reservations that outlive their caller.

mod common;

use cockpit_processes::ExecutionId;
use cockpit_resource_admission::{
    Admission, AdmissionRequest, Availability, Capacity, Control, ControlSupport, Demand, Error,
    Hold, Limits, Profile, ReservationId, ReservationState, Resource, Swept, Verdict, Workload,
};
use cockpit_turso::Database;
use common::*;

const NOW: i64 = 1_000_000;

async fn admit(database: &Database, request: &AdmissionRequest) -> Admission {
    admit_with(database, request, &[]).await
}

async fn admit_with(
    database: &Database,
    request: &AdmissionRequest,
    support: &[ControlSupport],
) -> Admission {
    let write = database.write().await.unwrap();
    let admission = cockpit_resource_admission::admit(&write, request, &policy(), support, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();
    admission
}

async fn admitted(database: &Database, request: &AdmissionRequest) -> ReservationId {
    match admit(database, request).await {
        Admission::Admitted(id) => id,
        Admission::Held(hold) => panic!("held: {hold}"),
    }
}

async fn set_profile(database: &Database, profile: Profile) {
    let write = database.write().await.unwrap();
    cockpit_resource_admission::set_profile(&write, profile, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();
}

async fn set_queue_paused(database: &Database, paused: bool) {
    let write = database.write().await.unwrap();
    cockpit_resource_admission::set_queue_paused(&write, paused, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();
}

#[tokio::test]
async fn work_that_does_not_fit_the_shared_budget_yet_waits_for_room() {
    let home = Home::new();
    let database = home.open_database().await;
    admitted(&database, &request(Workload::AgentTurn, 6000, 1024)).await;

    let admission = admit(&database, &request(Workload::Build, 3000, 1024)).await;

    assert_eq!(
        admission,
        Admission::Held(Hold::NoRoom {
            resource: Resource::Cpu,
            demand: 3000,
            available: 2000,
            limited_by: Capacity::Budget,
        })
    );
}

#[tokio::test]
async fn work_larger_than_the_whole_budget_is_held_as_never_fitting() {
    let home = Home::new();
    let database = home.open_database().await;

    let admission = admit(&database, &request(Workload::Test, 1000, 32 * 1024)).await;

    assert_eq!(
        admission,
        Admission::Held(Hold::Infeasible {
            resource: Resource::Memory,
            demand: 32 * 1024,
            capacity: 16 * 1024,
            limited_by: Capacity::Budget,
        })
    );
}

#[tokio::test]
async fn work_that_can_never_fit_is_held_as_never_fitting_even_while_the_budget_is_full() {
    let home = Home::new();
    let database = home.open_database().await;
    admitted(&database, &request(Workload::AgentTurn, 8000, 1024)).await;

    let admission = admit(&database, &request(Workload::Build, 1000, 20 * 1024)).await;

    assert_eq!(
        admission,
        Admission::Held(Hold::Infeasible {
            resource: Resource::Memory,
            demand: 20 * 1024,
            capacity: 16 * 1024,
            limited_by: Capacity::Budget,
        })
    );
}

#[tokio::test]
async fn the_foreground_profile_holds_heavy_jobs_beyond_its_share() {
    let home = Home::new();
    let database = home.open_database().await;
    set_profile(&database, Profile::Foreground).await;
    admitted(&database, &request(Workload::Build, 1500, 1024)).await;

    let admission = admit(&database, &request(Workload::Container, 1000, 1024)).await;

    assert_eq!(
        admission,
        Admission::Held(Hold::NoRoom {
            resource: Resource::Cpu,
            demand: 1000,
            available: 500,
            limited_by: Capacity::ForegroundShare,
        })
    );
}

#[tokio::test]
async fn back_in_the_normal_profile_heavy_jobs_get_the_whole_budget_again() {
    let home = Home::new();
    let database = home.open_database().await;
    set_profile(&database, Profile::Foreground).await;
    set_profile(&database, Profile::Normal).await;

    let admission = admit(&database, &request(Workload::Build, 6000, 1024)).await;

    assert!(matches!(admission, Admission::Admitted(_)), "{admission:?}");
}

#[tokio::test]
async fn the_foreground_profile_never_holds_agent_turns_deliveries_or_approvals() {
    for workload in [Workload::AgentTurn, Workload::Delivery, Workload::Approval] {
        let home = Home::new();
        let database = home.open_database().await;
        set_profile(&database, Profile::Foreground).await;
        admitted(&database, &request(Workload::Build, 2000, 1024)).await;

        let admission = admit(&database, &request(workload, 4000, 4096)).await;

        assert!(
            matches!(admission, Admission::Admitted(_)),
            "{workload:?}: {admission:?}"
        );
    }
}

#[test]
fn the_foreground_profile_adds_ceilings_to_heavy_jobs_and_leaves_the_others_alone() {
    let policy = policy();
    let demand = Demand {
        cpu_millis: 1500,
        memory_mib: 1024,
    };
    let asked = Limits {
        memory_max_mib: Some(2048),
        ..Limits::default()
    };

    let build = policy.enforced_limits(Workload::Build, demand, asked, Profile::Foreground);
    let turn = policy.enforced_limits(Workload::AgentTurn, demand, asked, Profile::Foreground);

    assert_eq!(
        build,
        Limits {
            cpu_weight: Some(1),
            cpu_quota_percent: Some(150),
            memory_max_mib: Some(2048),
            tasks_max: None,
            idle_scheduling: true,
            nice: Some(19),
        }
    );
    assert_eq!(turn, asked);
}

#[tokio::test]
async fn a_paused_queue_admits_nothing_new_until_it_resumes() {
    let home = Home::new();
    let database = home.open_database().await;
    set_queue_paused(&database, true).await;

    let paused = admit(&database, &request(Workload::AgentTurn, 100, 100)).await;
    set_queue_paused(&database, false).await;
    let resumed = admit(&database, &request(Workload::AgentTurn, 100, 100)).await;

    assert_eq!(paused, Admission::Held(Hold::QueuePaused));
    assert!(matches!(resumed, Admission::Admitted(_)), "{resumed:?}");
}

#[tokio::test]
async fn a_required_control_the_host_cannot_enforce_holds_the_work_with_the_backends_reason() {
    let home = Home::new();
    let database = home.open_database().await;
    let backend = process_group_backend().await;
    let mut build = request(Workload::Build, 1000, 1024);
    build.required = vec![Control::CpuQuota];

    let admission = admit_with(&database, &build, &backend.support()).await;

    let Admission::Held(Hold::ControlUnsupported { control, why }) = &admission else {
        panic!("not held for its control: {admission:?}");
    };
    assert_eq!(*control, Control::CpuQuota);
    assert!(why.contains("process group"), "{why}");
}

#[tokio::test]
async fn a_required_control_the_host_enforces_admits_the_work() {
    let home = Home::new();
    let database = home.open_database().await;
    let support = [ControlSupport {
        control: Control::MemoryMax,
        availability: Availability::Supported {
            how: "a fixture".to_owned(),
        },
    }];
    let mut build = request(Workload::Build, 1000, 1024);
    build.required = vec![Control::MemoryMax];

    let admission = admit_with(&database, &build, &support).await;

    assert!(matches!(admission, Admission::Admitted(_)), "{admission:?}");
}

#[tokio::test]
async fn a_check_answers_without_reserving_anything() {
    let home = Home::new();
    let database = home.open_database().await;
    let build = request(Workload::Build, 6000, 1024);

    let read = database.read().await.unwrap();
    let first = cockpit_resource_admission::check(&read, &build, &policy(), &[], NOW)
        .await
        .unwrap();
    let reservations = cockpit_resource_admission::reservations(&read)
        .await
        .unwrap();

    assert_eq!(first, Verdict::Fits);
    assert_eq!(reservations, vec![]);
}

#[tokio::test]
async fn a_malformed_request_is_refused_before_any_decision() {
    let home = Home::new();
    let database = home.open_database().await;
    let mut build = request(Workload::Build, 1000, 1024);
    build.limits.cpu_weight = Some(0);

    let write = database.write().await.unwrap();
    let refused = cockpit_resource_admission::admit(&write, &build, &policy(), &[], NOW).await;

    assert!(
        matches!(refused, Err(Error::InvalidRequest { .. })),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_reservation_survives_a_reopen_and_still_charges_the_budget() {
    let home = Home::new();
    {
        let database = home.open_database().await;
        admitted(&database, &request(Workload::Build, 7000, 1024)).await;
    }
    let database = home.open_database().await;

    let admission = admit(&database, &request(Workload::Test, 2000, 1024)).await;

    assert!(
        matches!(admission, Admission::Held(Hold::NoRoom { .. })),
        "{admission:?}"
    );
}

#[tokio::test]
async fn an_unstarted_reservation_is_released_at_its_expiry_and_not_before() {
    let home = Home::new();
    let database = home.open_database().await;
    let id = admitted(&database, &request(Workload::Build, 1000, 1024)).await;
    let expiry = i64::try_from(policy().unstarted_expiry.as_secs()).unwrap();

    let early = cockpit_resource_admission::sweep(&database, &home.supervisor(), NOW + expiry - 1)
        .await
        .unwrap();
    let due = cockpit_resource_admission::sweep(&database, &home.supervisor(), NOW + expiry)
        .await
        .unwrap();

    assert_eq!(early, Swept::default());
    assert_eq!(
        due,
        Swept {
            expired: vec![id],
            ..Swept::default()
        }
    );
}

#[tokio::test]
async fn an_expired_reservation_stops_charging_the_budget_before_the_sweep_releases_it() {
    let home = Home::new();
    let database = home.open_database().await;
    admitted(&database, &request(Workload::Build, 7000, 1024)).await;
    let expiry = i64::try_from(policy().unstarted_expiry.as_secs()).unwrap();
    let build = request(Workload::Build, 7000, 1024);

    let read = database.read().await.unwrap();
    let verdict = cockpit_resource_admission::check(&read, &build, &policy(), &[], NOW + expiry)
        .await
        .unwrap();

    assert_eq!(verdict, Verdict::Fits);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_reservation_whose_execution_exits_on_its_own_is_released_by_the_sweep() {
    let home = Home::new();
    let database = home.open_database().await;
    let id = admitted(&database, &request(Workload::Build, 1000, 1024)).await;
    let mut quick = home.sleeper();
    quick.arguments = vec!["0.2".into()];
    let execution = home.launch(&database, &quick).await;
    let write = database.write().await.unwrap();
    cockpit_resource_admission::bind_execution(&write, id, execution.id)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let mut swept = Swept::default();
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        swept = cockpit_resource_admission::sweep(&database, &home.supervisor(), NOW)
            .await
            .unwrap();
        if !swept.ended.is_empty() {
            break;
        }
    }

    assert_eq!(swept.ended, vec![id]);
}

#[tokio::test]
async fn an_execution_that_cannot_be_observed_stays_charged_and_the_rest_is_still_swept() {
    let home = Home::new();
    let database = home.open_database().await;
    let lost = admitted(&database, &request(Workload::Build, 1000, 1024)).await;
    let unstarted = admitted(&database, &request(Workload::Test, 1000, 1024)).await;
    let write = database.write().await.unwrap();
    cockpit_resource_admission::bind_execution(&write, lost, ExecutionId(999))
        .await
        .unwrap();
    write.commit().await.unwrap();
    let expiry = i64::try_from(policy().unstarted_expiry.as_secs()).unwrap();

    let swept = cockpit_resource_admission::sweep(&database, &home.supervisor(), NOW + expiry)
        .await
        .unwrap();

    assert_eq!(swept.expired, vec![unstarted]);
    assert_eq!(
        swept
            .unobserved
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        vec![lost]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_reservation_is_released_only_once_its_execution_is_observed_ended() {
    let home = Home::new();
    let database = home.open_database().await;
    let id = admitted(&database, &request(Workload::Build, 1000, 1024)).await;
    let execution = home.launch(&database, &home.sleeper()).await;
    let write = database.write().await.unwrap();
    cockpit_resource_admission::bind_execution(&write, id, execution.id)
        .await
        .unwrap();
    write.commit().await.unwrap();

    // Long past the unstarted expiry: a running reservation does not expire.
    let while_running =
        cockpit_resource_admission::sweep(&database, &home.supervisor(), NOW + 1_000_000)
            .await
            .unwrap();
    home.stop(&database, &execution).await;
    let after_stop =
        cockpit_resource_admission::sweep(&database, &home.supervisor(), NOW + 1_000_001)
            .await
            .unwrap();

    assert_eq!(while_running, Swept::default());
    assert_eq!(
        after_stop,
        Swept {
            ended: vec![id],
            ..Swept::default()
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_at_checkpoint_is_recorded_on_running_work_and_refused_on_unstarted_work() {
    let home = Home::new();
    let database = home.open_database().await;
    let unstarted = admitted(&database, &request(Workload::Test, 1000, 1024)).await;
    let running = admitted(&database, &request(Workload::Build, 1000, 1024)).await;
    let execution = home.launch(&database, &home.sleeper()).await;
    let write = database.write().await.unwrap();
    cockpit_resource_admission::bind_execution(&write, running, execution.id)
        .await
        .unwrap();

    let refused = cockpit_resource_admission::request_checkpoint(&write, unstarted, NOW).await;
    let recorded = cockpit_resource_admission::request_checkpoint(&write, running, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert!(
        matches!(
            refused,
            Err(Error::WrongState {
                needed: "running",
                ..
            })
        ),
        "{refused:?}"
    );
    assert!(recorded.checkpoint_requested);
    assert_eq!(
        recorded.state,
        ReservationState::Running {
            execution: execution.id
        }
    );
    home.stop(&database, &execution).await;
}

#[tokio::test]
async fn a_withdrawn_reservation_gives_its_capacity_back() {
    let home = Home::new();
    let database = home.open_database().await;
    let id = admitted(&database, &request(Workload::Build, 7000, 1024)).await;
    let write = database.write().await.unwrap();
    cockpit_resource_admission::withdraw(&write, id, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let admission = admit(&database, &request(Workload::Build, 7000, 1024)).await;

    assert!(matches!(admission, Admission::Admitted(_)), "{admission:?}");
}
