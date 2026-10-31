//! `cockpit resources` through the daemon binary (#42): the host's enforcement report, the
//! foreground profile, the three pause operations, and reservations across a restart.

mod common;

use std::ffi::OsString;
use std::path::PathBuf;

use cockpit_client::{ClientError, Connection};
use cockpit_processes::{Execution, LaunchContainment, LaunchSpec, Supervisor, SystemdTools};
use cockpit_protocol::{
    Command, EffectiveView, ErrorCode, LimitView, ProfileView, Reply, ResourceStatusView,
    VerdictView,
};
use cockpit_resource_admission::{
    Admission, AdmissionRequest, Backend, Demand, HostTools, Limits, Policy, ReservationId,
    Workload,
};
use common::{GENEROUS, TestHome};

fn state_dir(home: &TestHome) -> PathBuf {
    home.state_home().join("cockpit")
}

/// The supervisor a daemon on this home uses, with process-group containment.
fn supervisor(home: &TestHome) -> Supervisor {
    let options = cockpit_daemon::Options {
        runtime_dir: home.socket_dir(),
        state_dir: state_dir(home),
        limits: cockpit_daemon::Limits::default(),
    };
    Supervisor::new(
        options.executions_dir(),
        LaunchContainment::ProcessGroup,
        SystemdTools::from_path(),
    )
}

async fn backend() -> Backend {
    Backend::detect(
        HostTools::from_host(&SystemdTools::from_path()),
        LaunchContainment::ProcessGroup,
    )
    .await
    .unwrap()
}

/// A build admitted with `limits`, running as a sleeper started through the shims, as a
/// scheduler would leave it before the daemon starts.
async fn running_build(home: &TestHome, limits: Limits) -> (ReservationId, Execution) {
    let database = home.open_state().await;
    let backend = backend().await;
    let write = database.write().await.unwrap();
    let admission = cockpit_resource_admission::admit(
        &write,
        &AdmissionRequest {
            label: "make test".to_owned(),
            workload: Workload::Build,
            demand: Demand {
                cpu_millis: 100,
                memory_mib: 64,
            },
            limits,
            required: vec![],
        },
        &Policy::for_budget(backend.capacity()),
        &[],
        0,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    let Admission::Admitted(reservation) = admission else {
        panic!("held: {admission:?}");
    };
    let sleeper = LaunchSpec {
        program: PathBuf::from("/bin/sleep"),
        arguments: vec![OsString::from("300")],
        working_dir: home.home().to_path_buf(),
        environment: vec![],
        terminal: None,
    };
    let execution = supervisor(home)
        .launch(&database, &backend.wrap(sleeper, &limits))
        .await
        .unwrap();
    let write = database.write().await.unwrap();
    cockpit_resource_admission::bind_execution(&write, reservation, execution.id)
        .await
        .unwrap();
    write.commit().await.unwrap();
    (reservation, execution)
}

async fn status(owner: &Connection) -> ResourceStatusView {
    match owner.request(Command::ResourceStatus, GENEROUS).await {
        Ok(Reply::ResourceStatus { status }) => *status,
        other => panic!("not a status: {other:?}"),
    }
}

async fn check(
    owner: &Connection,
    workload: &str,
    cpu_millis: u64,
    required: &[&str],
) -> VerdictView {
    let command = Command::ResourceCheck {
        workload: workload.to_owned(),
        cpu_millis,
        memory_mib: 1,
        required: required.iter().map(|name| (*name).to_owned()).collect(),
    };
    match owner.request(command, GENEROUS).await {
        Ok(Reply::ResourceVerdict { verdict }) => verdict,
        other => panic!("not a verdict: {other:?}"),
    }
}

fn limit<'a>(limits: &'a [LimitView], control: &str) -> &'a LimitView {
    limits
        .iter()
        .find(|limit| limit.control == control)
        .unwrap_or_else(|| panic!("no {control} in {limits:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_status_reports_every_control_of_this_host_with_its_evidence() {
    let home = TestHome::new();
    let _daemon = home.start();
    let owner = home.connect().await;

    let status = status(&owner).await;

    // The daemon's environment has no user bus, so executions are process groups.
    assert_eq!(status.containment, "process-group");
    assert_eq!(
        status
            .controls
            .iter()
            .map(|control| control.control.as_str())
            .collect::<Vec<_>>(),
        [
            "cpu-weight",
            "cpu-quota",
            "memory-max",
            "tasks-max",
            "idle-scheduling",
            "nice",
            "freeze"
        ]
    );
    let quota = status
        .controls
        .iter()
        .find(|control| control.control == "cpu-quota")
        .unwrap();
    assert!(!quota.supported);
    assert!(quota.detail.contains("process group"), "{}", quota.detail);
}

#[tokio::test(flavor = "multi_thread")]
async fn work_that_requires_a_control_this_host_cannot_enforce_is_held_with_the_reason() {
    let home = TestHome::new();
    let _daemon = home.start();
    let owner = home.connect().await;

    let verdict = check(&owner, "build", 100, &["memory-max"]).await;

    let VerdictView::Held { reason } = verdict else {
        panic!("not held: {verdict:?}");
    };
    assert!(
        reason.starts_with("memory-max is required but cannot be enforced here: "),
        "{reason}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_foreground_profile_holds_heavy_work_beyond_its_share_and_never_agent_turns() {
    let home = TestHome::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let switched = owner
        .request(
            Command::ResourceProfile {
                profile: ProfileView::Foreground,
            },
            GENEROUS,
        )
        .await;
    let half_the_budget = status(&owner).await.budget.cpu_millis / 2;

    let build = check(&owner, "build", half_the_budget, &[]).await;
    let turn = check(&owner, "agent-turn", half_the_budget, &[]).await;

    assert!(
        matches!(&switched, Ok(Reply::ResourceStatus { status }) if status.profile == ProfileView::Foreground),
        "{switched:?}"
    );
    let VerdictView::Held { reason } = build else {
        panic!("a build of half the budget was not held: {build:?}");
    };
    assert!(
        reason.contains("the foreground profile's heavy-job share"),
        "{reason}"
    );
    assert_eq!(turn, VerdictView::Fits);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_cannot_award_itself_the_foreground_profile() {
    let home = TestHome::new();
    let _daemon = home.start();
    let agent = home.connect_agent().await;
    let owner = home.connect().await;

    let refused = agent
        .request(
            Command::ResourceProfile {
                profile: ProfileView::Foreground,
            },
            GENEROUS,
        )
        .await;

    assert!(
        matches!(&refused, Err(ClientError::Daemon(error)) if error.code == ErrorCode::NotAuthorized),
        "{refused:?}"
    );
    assert_eq!(status(&owner).await.profile, ProfileView::Normal);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_queue_holds_every_new_work_until_it_resumes() {
    let home = TestHome::new();
    let _daemon = home.start();
    let owner = home.connect().await;

    owner
        .request(Command::ResourceQueue { paused: true }, GENEROUS)
        .await
        .unwrap();
    let paused = check(&owner, "agent-turn", 1, &[]).await;
    owner
        .request(Command::ResourceQueue { paused: false }, GENEROUS)
        .await
        .unwrap();
    let resumed = check(&owner, "agent-turn", 1, &[]).await;

    assert_eq!(
        paused,
        VerdictView::Held {
            reason: "the queue is paused: nothing new starts".to_owned()
        }
    );
    assert_eq!(resumed, VerdictView::Fits);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_reservation_reports_its_limits_requested_supported_and_effective() {
    let home = TestHome::new();
    let limits = Limits {
        nice: Some(19),
        ..Limits::default()
    };
    let (reservation, execution) = running_build(&home, limits).await;
    let _daemon = home.start();
    let owner = home.connect().await;

    let status = status(&owner).await;

    let view = status
        .reservations
        .iter()
        .find(|view| view.id == reservation.0)
        .expect("the reservation is reported");
    assert_eq!(view.state, "running");
    assert_eq!(view.execution, Some(execution.id.0));
    assert_eq!(
        limit(&view.limits, "nice"),
        &LimitView {
            control: "nice".to_owned(),
            requested: Some("19".to_owned()),
            supported: true,
            effective: EffectiveView::Value {
                value: "19".to_owned()
            },
        }
    );
    let quota = limit(&view.limits, "cpu-quota");
    assert_eq!((quota.requested.as_deref(), quota.supported), (None, false));
    assert!(matches!(quota.effective, EffectiveView::Unknown { .. }));
    stop(&owner, reservation).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_at_checkpoint_is_recorded_on_the_running_reservation() {
    let home = TestHome::new();
    let (reservation, _execution) = running_build(&home, Limits::default()).await;
    let _daemon = home.start();
    let owner = home.connect().await;

    let recorded = owner
        .request(
            Command::ResourceCheckpoint {
                reservation: reservation.0,
            },
            GENEROUS,
        )
        .await;

    assert!(
        matches!(&recorded, Ok(Reply::Reservation { reservation }) if reservation.checkpoint_requested),
        "{recorded:?}"
    );
    stop(&owner, reservation).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_execution_ends_the_whole_execution_and_releases_its_reservation() {
    let home = TestHome::new();
    let (reservation, _execution) = running_build(&home, Limits::default()).await;
    let _daemon = home.start();
    let owner = home.connect().await;

    let stopped = stop(&owner, reservation).await;

    assert_eq!(
        stopped,
        Reply::ResourceStopped {
            reservation: reservation.0,
            observed_ended: true
        }
    );
    assert_eq!(status(&owner).await.reservations, vec![]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_of_work_that_never_started_is_refused_as_a_conflict() {
    let home = TestHome::new();
    let database = home.open_state().await;
    let write = database.write().await.unwrap();
    let Admission::Admitted(reservation) = cockpit_resource_admission::admit(
        &write,
        &AdmissionRequest {
            label: "make test".to_owned(),
            workload: Workload::Test,
            demand: Demand::default(),
            limits: Limits::default(),
            required: vec![],
        },
        &Policy::for_budget(backend().await.capacity()),
        &[],
        // Far in the future, so the start-up sweep does not expire it.
        i64::MAX / 2,
    )
    .await
    .unwrap() else {
        panic!("held");
    };
    write.commit().await.unwrap();
    drop(database);
    let _daemon = home.start();
    let owner = home.connect().await;

    let refused = owner
        .request(
            Command::ResourceStop {
                reservation: reservation.0,
            },
            GENEROUS,
        )
        .await;

    assert!(
        matches!(&refused, Err(ClientError::Daemon(error)) if error.code == ErrorCode::Conflict),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reservation_whose_execution_ended_while_the_daemon_was_down_is_released_at_start() {
    let home = TestHome::new();
    let (_reservation, execution) = running_build(&home, Limits::default()).await;
    supervisor(&home)
        .terminate(&home.open_state().await, execution.id, quick())
        .await
        .unwrap();
    let _daemon = home.start();
    let owner = home.connect().await;

    let status = status(&owner).await;

    assert_eq!(status.reservations, vec![]);
    assert_eq!(status.in_use.cpu_millis, 0);
}

async fn stop(owner: &Connection, reservation: ReservationId) -> Reply {
    owner
        .request(
            Command::ResourceStop {
                reservation: reservation.0,
            },
            GENEROUS,
        )
        .await
        .unwrap()
}

fn quick() -> cockpit_processes::Escalation {
    cockpit_processes::Escalation {
        grace: std::time::Duration::from_millis(500),
        kill_wait: GENEROUS,
    }
}
