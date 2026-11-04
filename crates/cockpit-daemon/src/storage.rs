//! Storage orchestration; projects owns cleanup state and filesystem rules.
use crate::routes::{Route, WorkClass};
use cockpit_projects::{
    CheckoutId, CloneStorage, PruneEvidence, PrunePolicy, PruneStatus, PruneVerdict, TargetSize,
};
use cockpit_protocol::{
    CloneStorageView, Command, ErrorCode, Outcome, ProtocolError, PruneVerdictView, Reply,
    StoragePrunePlanView, StoragePrunePreviewView, StoragePruneStatusView, TargetSizeView,
};
use cockpit_turso::{Database, sql::Connection};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) database: Arc<Database>,
    pub(crate) git: cockpit_projects::Git,
    pub(crate) policy: Arc<PrunePolicy>,
    pub(crate) stopping: Arc<AtomicBool>,
    pub(crate) supervisor: cockpit_processes::Supervisor,
    pub(crate) outputs: Arc<crate::output_retention::OutputPolicy>,
}

pub(crate) async fn settings(
    environment: cockpit_config::Environment,
) -> Result<cockpit_config::StorageSettings, crate::StartError> {
    tokio::task::spawn_blocking(move || {
        cockpit_config::load(&environment, None, &[]).map(|config| config.storage)
    })
    .await
    .map_err(|error| crate::StartError::Runtime(std::io::Error::other(error)))?
    .map_err(crate::StartError::Config)
}

pub(crate) fn prune_policy(settings: &cockpit_config::StorageSettings) -> PrunePolicy {
    PrunePolicy {
        automatic: settings.automatic.value,
        projects: settings.projects.value.clone(),
        confirm_above_bytes: settings.confirm_above_mib.value * 1024 * 1024,
        interval_minutes: settings.interval_minutes.value,
        base_ref: settings.base_ref.value.clone(),
    }
}

fn internal(error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(ErrorCode::Internal, error.to_string())
}
fn project(error: cockpit_projects::Error) -> ProtocolError {
    ProtocolError::new(crate::projects::error_code(&error), error.to_string())
}

/// Every caller supplies its own read/write connection; no hidden second snapshot.
pub(crate) async fn evidence(
    connection: &Connection,
    checkout: CheckoutId,
) -> Result<PruneEvidence, ProtocolError> {
    let mut result = PruneEvidence::default();
    let settings = cockpit_resource_admission::settings(connection)
        .await
        .map_err(internal)?;
    if settings.queue_paused || settings.profile == cockpit_resource_admission::Profile::Foreground
    {
        result.busy =
            Some("target cleanup deferred by paused queue or foreground resource profile".into());
        return Ok(result);
    }
    let sessions = cockpit_sessions::maintenance_sessions(connection)
        .await
        .map_err(internal)?;
    if sessions.len() > 4096 {
        result.busy = Some("session evidence bound reached".into());
        return Ok(result);
    }
    if let Some(session) = sessions.iter().find(|s| {
        matches!(
            s.state,
            cockpit_sessions::SessionState::Launching
                | cockpit_sessions::SessionState::Running
                | cockpit_sessions::SessionState::Stopping { .. }
        ) || s.restart_pending
    }) {
        result.busy = Some(format!(
            "session {} has active or pending launch work",
            session.id
        ));
        return Ok(result);
    }
    if let Some(execution) = cockpit_processes::occupying_executions_in(connection)
        .await
        .map_err(internal)?
        .first()
    {
        result.busy = Some(format!(
            "execution {} has not been observed ended",
            execution.id.0
        ));
        return Ok(result);
    }
    let (pending, mut executions) = cockpit_approvals::maintenance_runs(connection)
        .await
        .map_err(internal)?;
    if pending.is_some() {
        result.busy = pending;
        return Ok(result);
    }
    executions.extend(
        sessions
            .iter()
            .filter_map(|s| s.launch.as_ref().map(|l| l.execution)),
    );
    if !cockpit_processes::observed_ended(connection, &executions)
        .await
        .map_err(internal)?
    {
        result.busy = Some("a session/run execution identity has unknown termination".into());
        return Ok(result);
    }
    // Placement charges remain live until their owner releases them; uncertainty is busy.
    if !cockpit_scheduling::placements(connection)
        .await
        .map_err(internal)?
        .is_empty()
    {
        result.busy = Some("scheduled work still holds a placement".into());
        return Ok(result);
    }
    let associated: Vec<_> = sessions.iter().filter(|s| s.checkout == checkout).collect();
    if !associated.is_empty()
        && associated
            .iter()
            .all(|s| s.state == cockpit_sessions::SessionState::Ended && s.launch.is_some())
    {
        result.ended_sessions = Some(
            associated
                .iter()
                .map(|s| format!("{}@{}:{:?}", s.id, s.revision, s.launch))
                .collect::<Vec<_>>()
                .join(";"),
        );
    }
    Ok(result)
}

pub(crate) fn route(command: Command, context: Context) -> Route {
    let class = if matches!(
        command,
        Command::StoragePruneStatus { .. } | Command::StoragePruneAbandon { .. }
    ) {
        WorkClass::Control
    } else {
        WorkClass::Bulk
    };
    Route::Run {
        class,
        job: Box::pin(async move {
            let answer = match command {
                Command::StorageReport => report(&context).await,
                Command::StorageOutputs { apply } => {
                    crate::output_retention::pass(&context, apply, None).await
                }
                Command::StoragePrunePreview { checkout, prepare } => {
                    preview(&context, checkout, prepare).await
                }
                Command::StoragePruneStatus { operation } => match context.database.read().await {
                    Ok(read) => cockpit_projects::prune_status(&read, operation)
                        .await
                        .map(|status| status_reply(status, context.database.epoch()))
                        .map_err(project),
                    Err(error) => Err(internal(error)),
                },
                Command::StoragePruneApply {
                    plan,
                    revision,
                    epoch,
                } => apply(&context, plan, revision, &epoch, false)
                    .await
                    .map(|status| status_reply(status, context.database.epoch())),
                Command::StoragePruneAbandon {
                    operation,
                    revision,
                    epoch,
                } => {
                    async {
                        let write = context.database.write().await.map_err(internal)?;
                        let status = cockpit_projects::abandon_target_prune(
                            &write,
                            context.database.epoch(),
                            operation,
                            revision,
                            &epoch,
                        )
                        .await
                        .map_err(project)?;
                        write.commit().await.map_err(internal)?;
                        Ok(status_reply(status, context.database.epoch()))
                    }
                    .await
                }
                _ => Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "not a storage command",
                )),
            };
            match answer {
                Ok(reply) => Outcome::Ok(reply),
                Err(error) => Outcome::Error(error),
            }
        }),
    }
}

async fn apply(
    context: &Context,
    operation: i64,
    revision: i64,
    epoch: &str,
    automatic: bool,
) -> Result<PruneStatus, ProtocolError> {
    // A restore can reuse numeric IDs. The owner must carry the displayed epoch;
    // never refresh stale intent into the current database before validating it.
    if epoch != context.database.epoch() {
        return Err(ProtocolError::new(
            ErrorCode::StaleRevision,
            "cleanup database epoch changed; inspect a fresh plan",
        ));
    }
    if context.stopping.load(Ordering::Acquire) {
        return Err(ProtocolError::new(
            ErrorCode::Busy,
            "daemon is stopping; cleanup remains recorded",
        ));
    }
    let initial = {
        let read = context.database.read().await.map_err(internal)?;
        cockpit_projects::prune_status(&read, operation)
            .await
            .map_err(project)?
    };
    if initial.revision != revision || initial.epoch != epoch {
        return Err(ProtocolError::new(
            ErrorCode::StaleRevision,
            "cleanup plan revision or epoch changed",
        ));
    }
    match initial.phase.as_str() {
        "prepared" => {
            let observed = {
                let read = context.database.read().await.map_err(internal)?;
                evidence(&read, initial.checkout).await?
            };
            let preview = cockpit_projects::preview_target_prune(
                &context.database,
                &context.git,
                initial.checkout,
                &context.policy,
                &observed,
            )
            .await
            .map_err(project)?;
            let write = context.database.write().await.map_err(internal)?;
            let actual = cockpit_projects::prune_status(&write, operation)
                .await
                .map_err(project)?;
            if actual.phase != "prepared" {
                return Ok(actual);
            }
            let current = evidence(&write, initial.checkout).await?;
            let reserved = cockpit_projects::reserve_target_prune(
                &write,
                context.database.epoch(),
                operation,
                revision,
                &preview,
                &current,
                automatic,
            )
            .await
            .map_err(project)?;
            if reserved.phase != "reserved" {
                return Ok(reserved);
            }
            write.commit().await.map_err(internal)?;
        }
        "paused" | "needs-owner" => {
            let write = context.database.write().await.map_err(internal)?;
            let actual = cockpit_projects::prune_status(&write, operation)
                .await
                .map_err(project)?;
            if !matches!(actual.phase.as_str(), "paused" | "needs-owner") {
                return Ok(actual);
            }
            let current = evidence(&write, initial.checkout).await?;
            cockpit_projects::resume_target_prune(
                &write,
                context.database.epoch(),
                operation,
                revision,
                &context.policy,
                &current,
            )
            .await
            .map_err(project)?;
            write.commit().await.map_err(internal)?;
        }
        _ => return Ok(initial),
    }
    drive(context, operation).await
}

async fn drive(context: &Context, operation: i64) -> Result<PruneStatus, ProtocolError> {
    loop {
        let step = {
            let write = context.database.write().await.map_err(internal)?;
            let status = cockpit_projects::prune_status(&write, operation)
                .await
                .map_err(project)?;
            if matches!(
                status.phase.as_str(),
                "complete" | "abandoned" | "paused" | "needs-owner"
            ) {
                return Ok(status);
            }
            if context.stopping.load(Ordering::Acquire) {
                let paused = cockpit_projects::pause_target_prune(
                    &write,
                    operation,
                    "daemon stopping; resume this exact operation",
                )
                .await
                .map_err(project)?;
                write.commit().await.map_err(internal)?;
                return Ok(paused);
            }
            let current = evidence(&write, status.checkout).await?;
            let step = match cockpit_projects::begin_prune_step(
                &write,
                context.database.epoch(),
                operation,
                &context.policy,
                &current,
            )
            .await
            {
                Ok(step) => step,
                Err(error) => {
                    let paused =
                        cockpit_projects::pause_target_prune(&write, operation, &error.to_string())
                            .await
                            .map_err(project)?;
                    write.commit().await.map_err(internal)?;
                    return Ok(paused);
                }
            };
            write.commit().await.map_err(internal)?;
            step
        };
        let receipt = step
            .run(Arc::clone(&context.database), context.git.clone())
            .await
            .map_err(project)?;
        let write = context.database.write().await.map_err(internal)?;
        let status =
            cockpit_projects::complete_prune_step(&write, context.database.epoch(), receipt)
                .await
                .map_err(project)?;
        write.commit().await.map_err(internal)?;
        if matches!(
            status.phase.as_str(),
            "complete" | "abandoned" | "paused" | "needs-owner"
        ) {
            return Ok(status);
        }
        tokio::task::yield_now().await;
    }
}

/// Each clone's verdict uses the evidence and policy prune itself would use, applied to the
/// report's measurement. Prune measures again with its own limits (mount boundaries, manifest
/// size), so it can still refuse a clone the report called eligible.
async fn report(context: &Context) -> Result<Reply, ProtocolError> {
    let report = cockpit_projects::clone_storage(&context.database)
        .await
        .map_err(project)?;
    let observed = {
        let read = context.database.read().await.map_err(internal)?;
        let mut observed = Vec::with_capacity(report.clones.len());
        for clone in &report.clones {
            observed.push(evidence(&read, clone.checkout.id).await);
        }
        observed
    };
    let mut clones = Vec::with_capacity(report.clones.len());
    for (clone, observed) in report.clones.into_iter().zip(observed) {
        let undecided = |error: String| PruneVerdict::Kept {
            reason: format!("could not decide: {error}"),
        };
        let prune = match observed {
            Ok(observed) => cockpit_projects::prune_verdict(
                &context.database,
                &context.git,
                &clone,
                &context.policy,
                &observed,
            )
            .await
            .unwrap_or_else(|error| undecided(error.to_string())),
            Err(error) => undecided(error.message),
        };
        clones.push(clone_view(clone, prune));
    }
    Ok(Reply::Storage {
        clones,
        truncated: report.truncated,
    })
}

fn clone_view(clone: CloneStorage, prune: PruneVerdict) -> CloneStorageView {
    CloneStorageView {
        checkout: clone.checkout.id.0,
        project: clone.checkout.project.to_string(),
        incarnation: clone.checkout.incarnation,
        target: clone
            .checkout
            .path
            .join("target")
            .to_string_lossy()
            .into_owned(),
        holder: clone
            .checkout
            .occupancy
            .map(|occupancy| occupancy.holder.to_string()),
        measurement: measurement(clone.measurement),
        prune: Some(match prune {
            PruneVerdict::Eligible {
                confirmation_required,
                ..
            } => PruneVerdictView::Eligible {
                confirmation_required,
            },
            PruneVerdict::Kept { reason } => PruneVerdictView::Kept { reason },
        }),
    }
}

async fn preview(context: &Context, checkout: i64, prepare: bool) -> Result<Reply, ProtocolError> {
    if checkout <= 0 {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "checkout must be positive",
        ));
    }
    let observed = {
        let read = context.database.read().await.map_err(internal)?;
        evidence(&read, CheckoutId(checkout)).await?
    };
    let preview = cockpit_projects::preview_target_prune(
        &context.database,
        &context.git,
        CheckoutId(checkout),
        &context.policy,
        &observed,
    )
    .await
    .map_err(project)?;
    let plan = if prepare {
        let write = context.database.write().await.map_err(internal)?;
        let current = evidence(&write, CheckoutId(checkout)).await?;
        let plan = cockpit_projects::prepare_target_prune(
            &write,
            context.database.epoch(),
            &preview,
            &current,
        )
        .await
        .map_err(project)?;
        write.commit().await.map_err(internal)?;
        Some(StoragePrunePlanView {
            operation: plan.operation,
            revision: plan.revision,
            epoch: plan.epoch,
        })
    } else {
        None
    };
    Ok(Reply::StoragePrunePreview {
        preview: Box::new(StoragePrunePreviewView {
            checkout,
            incarnation: preview.checkout.incarnation,
            target: preview
                .checkout
                .path
                .join("target")
                .to_string_lossy()
                .into_owned(),
            measurement: measurement(preview.measurement),
            eligibility: preview.eligibility,
            kept_reason: preview.kept_reason,
            confirmation_required: preview.confirmation_required,
            plan,
            dry_run: !prepare,
        }),
    })
}

fn measurement(size: TargetSize) -> TargetSizeView {
    match size {
        TargetSize::Complete { apparent_bytes } => TargetSizeView::Complete { apparent_bytes },
        TargetSize::Missing => TargetSizeView::Missing,
        TargetSize::Incomplete {
            apparent_bytes,
            reason,
        } => TargetSizeView::Incomplete {
            apparent_bytes,
            reason,
        },
        TargetSize::Unavailable { reason } => TargetSizeView::Unavailable { reason },
    }
}
fn status_reply(status: PruneStatus, database_epoch: &str) -> Reply {
    Reply::StoragePruneStatus {
        status: Box::new(StoragePruneStatusView {
            target: status.target,
            quarantine: status.quarantine,
            operation: status.operation,
            revision: status.revision,
            epoch: status.epoch,
            database_epoch: database_epoch.to_owned(),
            checkout: status.checkout.0,
            phase: status.phase,
            apparent_bytes: status.apparent_bytes,
            removed_apparent_bytes: status.removed_apparent_bytes,
            absent_unconfirmed_bytes: status.absent_unconfirmed_bytes,
            eligibility: status.eligibility,
            reason: status.reason,
        }),
    }
}

/// Each scan/continuation uses the same bounded Bulk admission as owner requests.
/// No process, shell, owner socket impersonation or separate filesystem worker pool.
pub(crate) async fn maintain_forever(context: Context, shared: Arc<crate::server::Shared>) {
    if !context.policy.automatic {
        std::future::pending::<()>().await;
        return;
    }
    let mut ticks = tokio::time::interval(std::time::Duration::from_secs(
        u64::from(context.policy.interval_minutes) * 60,
    ));
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut after = CheckoutId(0);
    loop {
        ticks.tick().await;
        if let Err(error) = automatic_pass(&context, &shared, &mut after).await {
            tracing::warn!(error = %error.message, "automatic target pruning deferred");
        }
    }
}

async fn automatic_pass(
    context: &Context,
    shared: &crate::server::Shared,
    after: &mut CheckoutId,
) -> Result<(), ProtocolError> {
    let (paused, candidates) = {
        let read = context.database.read().await.map_err(internal)?;
        let paused = cockpit_projects::paused_automatic_prunes(&read)
            .await
            .map_err(project)?;
        let mut candidates = cockpit_projects::target_prune_candidates(&read, *after)
            .await
            .map_err(project)?;
        if candidates.is_empty() && after.0 != 0 {
            *after = CheckoutId(0);
            candidates = cockpit_projects::target_prune_candidates(&read, *after)
                .await
                .map_err(project)?;
        }
        (paused, candidates)
    };
    for status in paused {
        let context = context.clone();
        let work = Box::pin(async move {
            match apply(
                &context,
                status.operation,
                status.revision,
                &status.epoch,
                true,
            )
            .await
            {
                Ok(status) => Outcome::Ok(status_reply(status, context.database.epoch())),
                Err(error) => Outcome::Error(error),
            }
        });
        let admitted = shared
            .lanes
            .admit(WorkClass::Bulk, "storage-prune-recovery".into(), work);
        let outcome = match admitted {
            Ok(work) => work.await,
            Err(outcome) => outcome,
        };
        if let Outcome::Error(error) = outcome {
            if matches!(error.code, ErrorCode::Busy | ErrorCode::Internal) {
                return Err(error);
            }
            tracing::warn!(error = %error.message, "automatic cleanup recovery needs owner attention");
        }
    }
    for candidate in candidates {
        if !context
            .policy
            .projects
            .iter()
            .any(|project| project == candidate.project.as_str())
        {
            *after = candidate.id;
            continue;
        }
        let worker = context.clone();
        let checkout = candidate.id;
        let work = Box::pin(async move {
            match automatic_one(&worker, checkout).await {
                Ok(()) => Outcome::Ok(Reply::Pong),
                Err(error) => Outcome::Error(error),
            }
        });
        let admitted = shared
            .lanes
            .admit(WorkClass::Bulk, "storage-prune-automatic".into(), work);
        let outcome = match admitted {
            Ok(work) => work.await,
            Err(outcome) => outcome,
        };
        if let Outcome::Error(error) = outcome {
            if matches!(error.code, ErrorCode::Busy | ErrorCode::Internal) {
                return Err(error);
            }
            tracing::warn!(checkout = checkout.0, error = %error.message, "automatic target cleanup kept this candidate");
        }
        *after = checkout;
    }
    Ok(())
}

async fn automatic_one(context: &Context, checkout: CheckoutId) -> Result<(), ProtocolError> {
    if context.stopping.load(Ordering::Acquire) {
        return Err(ProtocolError::new(ErrorCode::Busy, "daemon is stopping"));
    }
    let observed = {
        let read = context.database.read().await.map_err(internal)?;
        evidence(&read, checkout).await?
    };
    if let Some(reason) = observed.busy {
        return Err(ProtocolError::new(ErrorCode::Busy, reason));
    }
    let preview = cockpit_projects::preview_target_prune(
        &context.database,
        &context.git,
        checkout,
        &context.policy,
        &observed,
    )
    .await
    .map_err(project)?;
    if let Some(reason) = &preview.kept_reason {
        tracing::info!(checkout = checkout.0, %reason, "automatic target pruning kept the target");
        return Ok(());
    }
    if preview.confirmation_required {
        tracing::info!(checkout = checkout.0, size = ?preview.measurement,
            "target pruning needs exact-plan owner confirmation: torlek storage prune CHECKOUT");
        return Ok(());
    }
    let plan = {
        let write = context.database.write().await.map_err(internal)?;
        let current = evidence(&write, checkout).await?;
        let plan = cockpit_projects::prepare_target_prune(
            &write,
            context.database.epoch(),
            &preview,
            &current,
        )
        .await
        .map_err(project)?;
        write.commit().await.map_err(internal)?;
        plan
    };
    tracing::info!(
        checkout = checkout.0,
        operation = plan.operation,
        apparent_bytes = plan.apparent_bytes,
        "recorded automatic target-prune size before consuming owner policy"
    );
    apply(context, plan.operation, plan.revision, &plan.epoch, true).await?;
    Ok(())
}
