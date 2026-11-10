//! Routes the resource commands to `cockpit-resource-admission`, converts its types to the
//! wire at this boundary, applies a profile change to running heavy executions and running
//! sessions, stops an execution through `cockpit-processes`, and runs the periodic sweep.
//! The rules live in the admission crate; this file maps.

use std::sync::Arc;
use std::time::Duration;

use cockpit_processes::{Escalation, Supervisor, Termination};
use cockpit_processes::{Execution, Phase};
use cockpit_protocol::{
    Command, ControlSupportView, DemandView, EffectiveView, ErrorCode, LimitView, ObservedView,
    Outcome, ProfileView, ProtocolError, Reply, ReservationView, ResourceStatusView,
    SessionLimitsView, VerdictView,
};
use cockpit_resource_admission::{
    AdmissionRequest, Availability, Backend, Control, ControlSupport, Demand, Effective, Error,
    ExecutionReport, Limits, Policy, Profile, Reservation, ReservationId, ReservationState,
    Verdict, Workload,
};
use cockpit_turso::Database;

use crate::approvals::now;
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
    /// cannot leave running work under the limits of the profile that lost; a session's
    /// launch holds it while it reads the profile and limits its agent.
    pub(crate) profile_change: Arc<tokio::sync::Mutex<()>>,
    /// Wakes the scheduling pass that rewrites what `queue explain` says (#646).
    pub(crate) queue_changed: Arc<tokio::sync::Notify>,
}

impl Resources {
    fn policy(&self) -> Policy {
        self.backend.policy()
    }
}

/// The route for one of the resource commands; `routes.rs` sends only those here. Reads and
/// setting changes run in the control lane: the owner must be able to pause or switch to the
/// foreground profile while the bulk lane is full. A stop may wait for the grace period, so
/// it runs in the stop lane.
pub(crate) fn route(command: Command, resources: Resources) -> Route {
    let class = match command {
        Command::ResourceStop { .. } => WorkClass::Stop,
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
            resources.queue_changed.notify_one();
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
                    state: found.state.as_str(),
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
                let write = resources.database.write().await?;
                cockpit_resource_admission::release_ended(&write, id, now()).await?;
                write.commit().await?;
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

/// Re-applies the limits of `profile` to every running heavy execution and every running
/// session, so switching to the foreground profile adds ceilings to builds already running
/// and to what agents run in their sessions, and switching back removes them (sessions keep
/// nice 19 and the idle IO class). A failure on
/// one execution is logged and the others still change.
async fn apply_profile(resources: &Resources, profile: Profile) -> Result<(), Error> {
    apply_to_heavy_work(resources, profile).await?;
    let limits = resources.policy().session_limits(profile);
    for (session, execution) in running_sessions(&resources.database).await {
        if let Err(error) = resources
            .backend
            .apply_to_session(&execution, &limits)
            .await
        {
            tracing::warn!(
                session = %session,
                %error,
                "could not apply the profile's limits to a running session"
            );
        }
    }
    Ok(())
}

/// The step every session launch runs before its agent's program starts (#1351): the agent's
/// first process gets the session limits of the current profile, so all it starts inherits
/// them. Never fails the launch: a turn is never held, only made to yield.
pub(crate) fn before_agent_runs(
    database: Arc<Database>,
    backend: Arc<Backend>,
    profile_change: Arc<tokio::sync::Mutex<()>>,
) -> cockpit_sessions::BeforeAgentRuns {
    cockpit_sessions::BeforeAgentRuns(Arc::new(move |execution: Execution| {
        let database = Arc::clone(&database);
        let backend = Arc::clone(&backend);
        let profile_change = Arc::clone(&profile_change);
        Box::pin(async move {
            let _unchanged = profile_change.lock().await;
            let profile = async {
                let read = database.read().await?;
                Ok::<_, Error>(cockpit_resource_admission::settings(&read).await?.profile)
            };
            let limited = match profile.await {
                Ok(profile) => {
                    backend
                        .apply_to_session(&execution, &backend.policy().session_limits(profile))
                        .await
                }
                Err(error) => Err(error),
            };
            if let Err(error) = limited {
                tracing::warn!(
                    execution = execution.id.0,
                    %error,
                    "a session's agent starts without its profile's session limits"
                );
            }
        })
    }))
}

/// Every session whose agent runs now, with its execution. Sessions that cannot be read are
/// logged and left out: they must not hide the heavy work, or stop the others changing.
async fn running_sessions(database: &Database) -> Vec<(String, Execution)> {
    let sessions = match database.read().await {
        Ok(read) => cockpit_sessions::sessions(&read, false).await,
        Err(error) => Err(error.into()),
    };
    let sessions = match sessions {
        Ok(sessions) => sessions,
        Err(error) => {
            tracing::warn!(%error, "could not read the sessions to limit");
            return Vec::new();
        }
    };
    let mut running = Vec::new();
    for session in sessions {
        let Some(launch) = session.launch else {
            continue;
        };
        match cockpit_processes::execution(database, launch.execution).await {
            Ok(execution) if execution.phase == Phase::Running => {
                running.push((session.id.as_str().to_owned(), execution));
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(
                session = %session.id,
                %error,
                "could not read a session's execution"
            ),
        }
    }
    running
}

async fn apply_to_heavy_work(resources: &Resources, profile: Profile) -> Result<(), Error> {
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
    let session_limits = policy.session_limits(settings.profile);
    let mut sessions = Vec::new();
    for (session, execution) in running_sessions(&resources.database).await {
        let report = resources
            .backend
            .inspect_session(&execution, &session_limits)
            .await;
        sessions.push(SessionLimitsView {
            session,
            execution: execution.id.0,
            limits: limit_views(report),
        });
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
            controls: control_views(resources.backend.support()),
            session_controls: control_views(resources.backend.session_support()),
            reservations: views,
            sessions,
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
    let report = match reservation.state {
        ReservationState::Running { execution } => {
            match cockpit_processes::execution(&resources.database, execution).await {
                Ok(found) => resources.backend.inspect(&found, &limits).await,
                // One unreadable execution must not hide the rest of the status.
                Err(error) => resources.backend.unknown_report(
                    &limits,
                    &format!("its execution could not be read: {error}"),
                ),
            }
        }
        ReservationState::Reserved { .. } | ReservationState::Released { .. } => resources
            .backend
            .unknown_report(&limits, "the work has not started"),
        ReservationState::Held => resources.backend.unknown_report(
            &limits,
            "it runs inside a session's agent, which has no execution of its own to inspect",
        ),
    };
    let execution = match reservation.state {
        ReservationState::Running { execution } => Some(execution.0),
        _ => None,
    };
    let observed = ObservedView {
        memory_bytes: report.observed.memory_bytes,
        tasks: report.observed.tasks,
    };
    Ok(ReservationView {
        id: reservation.id.0,
        label: reservation.label.clone(),
        workload: reservation.workload.as_str().to_owned(),
        state: reservation.state.as_str().to_owned(),
        execution,
        reserved: demand_view(reservation.demand),
        required: reservation
            .required
            .iter()
            .map(|control| control.as_str().to_owned())
            .collect(),
        checkpoint_requested: reservation.checkpoint_requested,
        limits: limit_views(report),
        observed,
    })
}

fn limit_views(report: ExecutionReport) -> Vec<LimitView> {
    report
        .limits
        .into_iter()
        .map(|line| LimitView {
            control: line.control.as_str().to_owned(),
            requested: line.requested,
            supported: availability_parts(line.availability).0,
            effective: match line.effective {
                Effective::Value(value) => EffectiveView::Value { value },
                Effective::Unknown { why } => EffectiveView::Unknown { why },
            },
        })
        .collect()
}

fn control_views(support: Vec<ControlSupport>) -> Vec<ControlSupportView> {
    support
        .into_iter()
        .map(|support| {
            let (supported, detail) = availability_parts(support.availability);
            ControlSupportView {
                control: support.control.as_str().to_owned(),
                supported,
                detail,
            }
        })
        .collect()
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

pub(crate) fn demand_view(demand: Demand) -> DemandView {
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
