//! Routes the resource commands to `cockpit-resource-admission`, converts its types to the
//! wire at this boundary, applies a profile change to running heavy executions, stops an
//! execution through `cockpit-processes`, and runs the periodic sweep. The rules live in
//! the admission crate; this file maps.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cockpit_processes::{Escalation, Supervisor, Termination};
use cockpit_protocol::{
    Command, ControlSupportView, DemandView, EffectiveView, ErrorCode, LimitView, ObservedView,
    Outcome, ProfileView, ProtocolError, Reply, ReservationView, ResourceStatusView, VerdictView,
};
use cockpit_resource_admission::{
    AdmissionRequest, Availability, Backend, Control, Demand, Effective, Error, ExecutionReport,
    LimitReport, Limits, Observed, Policy, Profile, Reservation, ReservationId, ReservationState,
    Verdict, Workload,
};
use cockpit_turso::Database;

use crate::routes::{Route, WorkClass, refusal};

/// How long a stopped execution may take to end after `SIGTERM`, then after `SIGKILL`.
const STOP_ESCALATION: Escalation = Escalation {
    grace: Duration::from_secs(10),
    kill_wait: Duration::from_secs(10),
};

/// What the resource commands work with.
#[derive(Clone)]
pub(crate) struct Resources {
    pub(crate) database: Arc<Database>,
    pub(crate) backend: Arc<Backend>,
    pub(crate) supervisor: Supervisor,
    /// Held from a profile's commit until its limits are applied, so two concurrent changes
    /// cannot leave running work under the limits of the profile that lost.
    pub(crate) profile_change: Arc<tokio::sync::Mutex<()>>,
}

impl Resources {
    fn policy(&self) -> Policy {
        Policy::for_budget(self.backend.capacity())
    }
}

/// The route for one of the resource commands; `routes.rs` sends only those here. Reads and
/// setting changes run in the control lane: the owner must be able to pause or switch to the
/// foreground profile while the bulk lane is full. A stop may wait for the grace period, so
/// it runs in the bulk lane.
pub(crate) fn route(command: Command, resources: Resources) -> Route {
    let class = match command {
        Command::ResourceStop { .. } => WorkClass::Bulk,
        _ => WorkClass::Control,
    };
    let job: crate::Job = match command {
        Command::ResourceStatus => job(async move { status(&resources).await }),
        Command::ResourceProfile { profile } => job(async move {
            let profile = match profile {
                ProfileView::Normal => Profile::Normal,
                ProfileView::Foreground => Profile::Foreground,
                ProfileView::Unknown => {
                    return Err(Error::InvalidRequest {
                        reason: "the profile must be normal or foreground".to_owned(),
                    });
                }
            };
            let changing = resources.profile_change.lock().await;
            let write = resources.database.write().await?;
            cockpit_resource_admission::set_profile(&write, profile, now()).await?;
            write.commit().await?;
            apply_profile(&resources, profile).await?;
            drop(changing);
            status(&resources).await
        }),
        Command::ResourceQueue { paused } => job(async move {
            let write = resources.database.write().await?;
            cockpit_resource_admission::set_queue_paused(&write, paused, now()).await?;
            write.commit().await?;
            status(&resources).await
        }),
        Command::ResourceCheck {
            workload,
            cpu_millis,
            memory_mib,
            required,
        } => job(async move {
            let request = AdmissionRequest {
                label: "check".to_owned(),
                workload: Workload::parse(&workload).ok_or_else(|| Error::InvalidRequest {
                    reason: format!("{workload:?} is not a workload"),
                })?,
                demand: Demand {
                    cpu_millis,
                    memory_mib,
                },
                limits: Limits::default(),
                required: required
                    .iter()
                    .map(|name| {
                        Control::parse(name).ok_or_else(|| Error::InvalidRequest {
                            reason: format!("{name:?} is not a control"),
                        })
                    })
                    .collect::<Result<_, _>>()?,
            };
            let read = resources.database.read().await?;
            let verdict = cockpit_resource_admission::check(
                &read,
                &request,
                &resources.policy(),
                &resources.backend.support(),
                now(),
            )
            .await?;
            Ok(Reply::ResourceVerdict {
                verdict: match verdict {
                    Verdict::Fits => VerdictView::Fits,
                    Verdict::Held(hold) => VerdictView::Held {
                        reason: hold.to_string(),
                    },
                },
            })
        }),
        Command::ResourceCheckpoint { reservation } => job(async move {
            let write = resources.database.write().await?;
            let recorded = cockpit_resource_admission::request_checkpoint(
                &write,
                ReservationId(reservation),
                now(),
            )
            .await?;
            write.commit().await?;
            let profile = settings_profile(&resources).await?;
            Ok(Reply::Reservation {
                reservation: Box::new(reservation_view(&resources, &recorded, profile).await?),
            })
        }),
        Command::ResourceStop { reservation } => job(async move {
            let id = ReservationId(reservation);
            let read = resources.database.read().await?;
            let found = cockpit_resource_admission::reservation(&read, id).await?;
            drop(read);
            let ReservationState::Running { execution } = found.state else {
                return Err(Error::WrongState {
                    reservation: id,
                    state: "not running",
                    needed: "running",
                });
            };
            let termination = resources
                .supervisor
                .terminate(&resources.database, execution, STOP_ESCALATION)
                .await
                .map_err(Error::Processes)?;
            let observed_ended = matches!(termination, Termination::Confirmed { .. });
            if observed_ended {
                sweep(&resources.database, &resources.supervisor).await?;
            }
            Ok(Reply::ResourceStopped {
                reservation,
                observed_ended,
            })
        }),
        other => {
            tracing::error!(
                ?other,
                "a non-resource command was routed to the resource handler"
            );
            return Route::Answer(refusal(
                ErrorCode::Internal,
                "the daemon routed this command to the wrong handler",
            ));
        }
    };
    Route::Run { class, job }
}

/// Re-applies the limits of `profile` to every running heavy execution, so switching to the
/// foreground profile adds ceilings to builds already running and switching back removes
/// them. A failure on one execution is logged and the others still change.
async fn apply_profile(resources: &Resources, profile: Profile) -> Result<(), Error> {
    let read = resources.database.read().await?;
    let reservations = cockpit_resource_admission::reservations(&read).await?;
    drop(read);
    let policy = resources.policy();
    for reservation in reservations
        .iter()
        .filter(|reservation| reservation.workload.is_heavy())
    {
        let ReservationState::Running { execution } = reservation.state else {
            continue;
        };
        let limits = policy.enforced_limits(
            reservation.workload,
            reservation.demand,
            reservation.limits,
            profile,
        );
        let applied = match cockpit_processes::execution(&resources.database, execution).await {
            Ok(execution) => resources.backend.enforce(&execution, &limits).await,
            Err(error) => Err(Error::Processes(error)),
        };
        if let Err(error) = applied {
            tracing::warn!(
                reservation = reservation.id.0,
                %error,
                "could not apply the profile's limits to a running execution"
            );
        }
    }
    Ok(())
}

async fn settings_profile(resources: &Resources) -> Result<Profile, Error> {
    let read = resources.database.read().await?;
    Ok(cockpit_resource_admission::settings(&read).await?.profile)
}

async fn status(resources: &Resources) -> Result<Reply, Error> {
    let read = resources.database.read().await?;
    let settings = cockpit_resource_admission::settings(&read).await?;
    let in_use = cockpit_resource_admission::in_use(&read, now()).await?;
    let reservations = cockpit_resource_admission::reservations(&read).await?;
    drop(read);
    let policy = resources.policy();
    let mut views = Vec::with_capacity(reservations.len());
    for reservation in &reservations {
        views.push(reservation_view(resources, reservation, settings.profile).await?);
    }
    Ok(Reply::ResourceStatus {
        status: Box::new(ResourceStatusView {
            platform: resources.backend.platform().as_str().to_owned(),
            containment: match resources.backend.launch_containment() {
                cockpit_processes::LaunchContainment::SystemdScope => "systemd-scope",
                cockpit_processes::LaunchContainment::ProcessGroup => "process-group",
            }
            .to_owned(),
            profile: profile_view(settings.profile),
            queue_paused: settings.queue_paused,
            budget: demand_view(policy.budget),
            in_use: demand_view(in_use.all),
            foreground_heavy_percent: policy.foreground_heavy_percent,
            controls: resources
                .backend
                .support()
                .into_iter()
                .map(|support| {
                    let (supported, detail) = availability_parts(support.availability);
                    ControlSupportView {
                        control: support.control.as_str().to_owned(),
                        supported,
                        detail,
                    }
                })
                .collect(),
            reservations: views,
        }),
    })
}

/// A reservation with its limits as the current profile asks them. A running one reports
/// what the host enforces on its execution; one not started reports what new executions get.
async fn reservation_view(
    resources: &Resources,
    reservation: &Reservation,
    profile: Profile,
) -> Result<ReservationView, Error> {
    let limits = resources.policy().enforced_limits(
        reservation.workload,
        reservation.demand,
        reservation.limits,
        profile,
    );
    let (state, execution, limit_views, observed) = match reservation.state {
        ReservationState::Running { execution } => {
            let report = match cockpit_processes::execution(&resources.database, execution).await {
                Ok(found) => resources.backend.inspect(&found, &limits).await,
                // One unreadable execution must not hide the rest of the status.
                Err(error) => unreadable_execution_report(resources, &limits, &error),
            };
            let limit_views = report
                .limits
                .into_iter()
                .map(|line| {
                    let (supported, _) = availability_parts(line.availability);
                    LimitView {
                        control: line.control.as_str().to_owned(),
                        requested: line.requested,
                        supported,
                        effective: match line.effective {
                            Effective::Value(value) => EffectiveView::Value { value },
                            Effective::Unknown { why } => EffectiveView::Unknown { why },
                        },
                    }
                })
                .collect();
            let observed = ObservedView {
                memory_bytes: report.observed.memory_bytes,
                tasks: report.observed.tasks,
            };
            ("running", Some(execution.0), limit_views, observed)
        }
        ReservationState::Reserved { .. } | ReservationState::Released { .. } => {
            let limit_views = resources
                .backend
                .support()
                .into_iter()
                .map(|support| LimitView {
                    control: support.control.as_str().to_owned(),
                    requested: limits.requested(support.control),
                    supported: availability_parts(support.availability).0,
                    effective: EffectiveView::Unknown {
                        why: "the work has not started".to_owned(),
                    },
                })
                .collect();
            let state = match reservation.state {
                ReservationState::Reserved { .. } => "reserved",
                _ => "released",
            };
            (state, None, limit_views, ObservedView::default())
        }
    };
    Ok(ReservationView {
        id: reservation.id.0,
        label: reservation.label.clone(),
        workload: reservation.workload.as_str().to_owned(),
        state: state.to_owned(),
        execution,
        reserved: demand_view(reservation.demand),
        required: reservation
            .required
            .iter()
            .map(|control| control.as_str().to_owned())
            .collect(),
        checkpoint_requested: reservation.checkpoint_requested,
        limits: limit_views,
        observed,
    })
}

fn unreadable_execution_report(
    resources: &Resources,
    limits: &Limits,
    error: &cockpit_processes::Error,
) -> ExecutionReport {
    ExecutionReport {
        limits: resources
            .backend
            .support()
            .into_iter()
            .map(|support| LimitReport {
                control: support.control,
                requested: limits.requested(support.control),
                availability: support.availability,
                effective: Effective::Unknown {
                    why: format!("its execution could not be read: {error}"),
                },
            })
            .collect(),
        observed: Observed::default(),
    }
}

fn availability_parts(availability: Availability) -> (bool, String) {
    match availability {
        Availability::Supported { how } => (true, how),
        Availability::Unsupported { why } => (false, why),
    }
}

fn profile_view(profile: Profile) -> ProfileView {
    match profile {
        Profile::Normal => ProfileView::Normal,
        Profile::Foreground => ProfileView::Foreground,
    }
}

fn demand_view(demand: Demand) -> DemandView {
    DemandView {
        cpu_millis: demand.cpu_millis,
        memory_mib: demand.memory_mib,
    }
}

fn job(work: impl Future<Output = Result<Reply, Error>> + Send + 'static) -> crate::Job {
    Box::pin(async move {
        match work.await {
            Ok(reply) => Outcome::Ok(reply),
            Err(error) => Outcome::Error(protocol_error(&error)),
        }
    })
}

fn protocol_error(error: &Error) -> ProtocolError {
    let code = match error {
        Error::InvalidRequest { .. } => ErrorCode::InvalidArgument,
        Error::UnknownReservation { .. } => ErrorCode::NotFound,
        Error::WrongState { .. } | Error::NotStarted { .. } => ErrorCode::Conflict,
        _ => ErrorCode::Internal,
    };
    if code == ErrorCode::Internal {
        tracing::error!(%error, "a resource command failed");
    }
    ProtocolError::new(code, error.to_string())
}

/// Releases expired and ended reservations: at startup, before requests are accepted, and
/// then periodically.
pub(crate) async fn sweep(database: &Database, supervisor: &Supervisor) -> Result<(), Error> {
    let swept = cockpit_resource_admission::sweep(database, supervisor, now()).await?;
    for (id, why) in &swept.unobserved {
        tracing::warn!(
            reservation = id.0,
            why,
            "kept a reservation charged whose execution could not be observed"
        );
    }
    for id in &swept.expired {
        tracing::info!(
            reservation = id.0,
            "released a reservation that never started"
        );
    }
    for id in &swept.ended {
        tracing::info!(
            reservation = id.0,
            "released a reservation whose execution ended"
        );
    }
    Ok(())
}

pub(crate) async fn sweep_forever(
    database: Arc<Database>,
    supervisor: Supervisor,
    interval: Duration,
) {
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        if let Err(error) = sweep(&database, &supervisor).await {
            tracing::error!(%error, "the reservations' sweep failed");
        }
    }
}

/// Seconds since the Unix epoch.
fn now() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    i64::try_from(elapsed.as_secs()).expect("seconds since 1970 fit in i64")
}
