//! Durable step ownership. No expiry grants another writer access to a running syscall.
use super::{PruneEvidence, PrunePolicy, PruneStatus, filesystem, quarantine, refused, store};
use crate::{CheckoutKind, CheckoutState, Error, Git, store::StoredCheckout};
use cockpit_turso::{Database, WriteTransaction, sql::params};
use std::sync::Arc;

/// One exclusively reserved step. Only its returned receipt may checkpoint its effects.
pub struct PruneStep {
    plan: store::StoredPlan,
    checkout: StoredCheckout,
    main: StoredCheckout,
    policy: PrunePolicy,
    evidence: PruneEvidence,
}

pub struct PruneStepReceipt {
    epoch: String,
    operation: i64,
    generation: i64,
    phase: String,
    cursor: usize,
    result: Result<Effect, String>,
}

enum Effect {
    Created(String),
    Detached,
    Removed(quarantine::Slice),
    Finished,
}

async fn guards(write: &WriteTransaction<'_>, plan: &store::StoredPlan) -> Result<(), Error> {
    for key in [
        "global".to_owned(),
        format!("checkout:{}", plan.status.checkout.0),
    ] {
        let mut rows = write
            .query(
                "SELECT operation,generation,epoch FROM project_target_prune_guards WHERE key=?1",
                [key],
            )
            .await?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| store::stale("cleanup guard is absent"))?;
        if row.get::<i64>(0)? != plan.status.operation
            || row.get::<i64>(1)? != plan.generation
            || row.get::<String>(2)? != plan.epoch
        {
            return Err(store::stale("cleanup guard belongs to another generation"));
        }
    }
    Ok(())
}

async fn current(
    write: &WriteTransaction<'_>,
    epoch: &str,
    plan: &store::StoredPlan,
    policy: &PrunePolicy,
    evidence: &PruneEvidence,
) -> Result<StoredCheckout, Error> {
    if plan.epoch != epoch || plan.policy != format!("{policy:?}") {
        return Err(store::stale("cleanup epoch or owner policy changed"));
    }
    if let Some(reason) = &evidence.busy {
        return Err(refused(reason.clone()));
    }
    let checkout = crate::store::stored_checkout(write, plan.status.checkout).await?;
    if checkout.checkout.incarnation != plan.incarnation
        || checkout.checkout.state != CheckoutState::Live
        || matches!(checkout.checkout.kind, CheckoutKind::Main)
        || checkout.checkout.occupancy.is_some()
    {
        return Err(store::stale(
            "cleanup checkout identity, state or occupancy changed",
        ));
    }
    if plan
        .status
        .eligibility
        .starts_with("observed ended sessions:")
        && evidence
            .ended_sessions
            .as_ref()
            .map(|proof| format!("observed ended sessions: {proof}"))
            .as_deref()
            != Some(plan.status.eligibility.as_str())
    {
        return Err(store::stale("ended-session termination evidence changed"));
    }
    Ok(checkout)
}

/// Evidence must be composed on THIS writer by the daemon's capability owners.
pub async fn begin_prune_step(
    write: &WriteTransaction<'_>,
    epoch: &str,
    operation: i64,
    policy: &PrunePolicy,
    evidence: &PruneEvidence,
) -> Result<PruneStep, Error> {
    let plan = store::load(write, operation).await?;
    if plan.worker_active {
        return Err(refused(
            "cleanup step is already active; inspect its status",
        ));
    }
    if !matches!(
        plan.status.phase.as_str(),
        "reserved" | "detaching" | "deleting" | "finishing"
    ) {
        return Err(refused("cleanup operation is not runnable"));
    }
    guards(write, &plan).await?;
    let checkout = current(write, epoch, &plan, policy, evidence).await?;
    let project = crate::store::project_named(write, checkout.checkout.project.as_str()).await?;
    let main = crate::store::main_checkout(write, project.id).await?;
    write
        .execute(
            "UPDATE project_target_prunes SET worker_active=1 WHERE id=?1",
            [operation],
        )
        .await?;
    Ok(PruneStep {
        plan,
        checkout,
        main,
        policy: policy.clone(),
        evidence: evidence.clone(),
    })
}

impl PruneStep {
    /// The database stays owned by the blocking closure until all filesystem work ends,
    /// even if its async caller is aborted. A new daemon cannot recover its guard early.
    pub async fn run(self, database: Arc<Database>, git: Git) -> Result<PruneStepReceipt, Error> {
        if database.epoch() != self.plan.epoch {
            return Err(store::stale("cleanup worker database epoch changed"));
        }
        // A reserved capability can outlive a caller that never started it. Recheck
        // under the current writer before ANY filesystem work, not merely when its
        // eventual receipt arrives. The owned database then stays alive through join.
        {
            let write = database.write().await?;
            let mut rows = write.query("SELECT worker_active,epoch,generation,phase,cursor FROM project_target_prunes WHERE id=?1", [self.plan.status.operation]).await?;
            let row = rows
                .next()
                .await?
                .ok_or_else(|| store::stale("cleanup operation disappeared"))?;
            if row.get::<i64>(0)? != 1
                || row.get::<String>(1)? != self.plan.epoch
                || row.get::<i64>(2)? != self.plan.generation
                || row.get::<String>(3)? != self.plan.status.phase
                || usize::try_from(row.get::<i64>(4)?).ok() != Some(self.plan.cursor)
            {
                return Err(store::stale("cleanup capability belongs to another step"));
            }
            drop(rows);
            guards(&write, &self.plan).await?;
            write.commit().await?;
        }
        let operation = self.plan.status.operation;
        let epoch = self.plan.epoch.clone();
        let generation = self.plan.generation;
        let phase = self.plan.status.phase.clone();
        let cursor = self.plan.cursor;
        let result = crate::blocking(move || {
            let result = self.effects(&git);
            drop(database);
            Ok(result)
        })
        .await
        .unwrap_or_else(|error| Err(format!("cleanup worker failed: {error}")));
        Ok(PruneStepReceipt {
            epoch,
            operation,
            generation,
            phase,
            cursor,
            result,
        })
    }

    fn effects(&self, git: &Git) -> Result<Effect, String> {
        let plan = &self.plan;
        if plan.status.eligibility.starts_with("LOCAL ancestry:") {
            let base = self
                .policy
                .base_ref
                .as_deref()
                .ok_or("LOCAL base ref is no longer configured")?;
            if filesystem::local_ancestry(git, &self.checkout, &self.main, base)?
                != plan.status.eligibility
            {
                return Err(
                    "LOCAL ancestry OIDs changed; preserve target and inspect the operation".into(),
                );
            }
        } else if !plan
            .status
            .eligibility
            .starts_with("observed ended sessions:")
        {
            return Err("unrecognized cleanup eligibility".into());
        }
        let name = plan
            .quarantine
            .as_deref()
            .ok_or("missing quarantine name")?;
        let proof = || {
            plan.quarantine_identity
                .as_deref()
                .ok_or_else(|| "missing quarantine identity".to_owned())
        };
        match plan.status.phase.as_str() {
            "reserved" => {
                self.original(git)?;
                quarantine::create(&self.checkout, &plan.snapshot, name).map(Effect::Created)
            }
            "detaching" => {
                // An earlier rename may have succeeded before its transaction receipt.
                // Only the exact detached full manifest permits recovery to deletion.
                if quarantine::verify_detached(&self.checkout, &plan.snapshot, name, proof()?)
                    .is_err()
                {
                    self.original(git)?;
                    quarantine::detach(&self.checkout, &plan.snapshot, name, proof()?)?;
                }
                Ok(Effect::Detached)
            }
            "deleting" => Ok(Effect::Removed(quarantine::remove_slice(
                &self.checkout,
                &plan.snapshot,
                name,
                proof()?,
                plan.cursor,
            ))),
            "finishing" => {
                quarantine::finish(&self.checkout, &plan.snapshot, name, proof()?)?;
                Ok(Effect::Finished)
            }
            _ => Err("unknown cleanup phase".into()),
        }
    }

    fn original(&self, git: &Git) -> Result<(), String> {
        let preview = filesystem::preview(
            self.checkout.clone(),
            self.main.clone(),
            git,
            self.policy.clone(),
            self.evidence.clone(),
        );
        if preview.snapshot.as_ref() != Some(&self.plan.snapshot)
            || preview.eligibility.as_deref() != Some(self.plan.status.eligibility.as_str())
        {
            return Err(preview
                .kept_reason
                .unwrap_or_else(|| "target contents or eligibility changed".into()));
        }
        Ok(())
    }
}

/// Checkpoint only after the slice returned. A failed/paused slice releases the GLOBAL
/// guard, retaining its checkout guard; no second writer is admitted during a syscall.
pub async fn complete_prune_step(
    write: &WriteTransaction<'_>,
    epoch: &str,
    receipt: PruneStepReceipt,
) -> Result<PruneStatus, Error> {
    let plan = store::load(write, receipt.operation).await?;
    if receipt.epoch != epoch || plan.epoch != epoch {
        return Err(store::stale(
            "cleanup receipt belongs to another database epoch",
        ));
    }
    guards(write, &plan).await?;
    if !plan.worker_active
        || plan.generation != receipt.generation
        || plan.status.phase != receipt.phase
        || plan.cursor != receipt.cursor
    {
        return Err(store::stale("cleanup receipt belongs to another step"));
    }
    let mut phase = plan.status.phase.clone();
    let mut reason = None;
    let mut identity = plan.quarantine_identity;
    let mut cursor = plan.cursor;
    let mut removed = plan.status.removed_apparent_bytes;
    let mut absent = plan.status.absent_unconfirmed_bytes;
    match receipt.result {
        Ok(Effect::Created(proof)) => {
            identity = Some(proof);
            phase = "detaching".into();
        }
        Ok(Effect::Detached) => phase = "deleting".into(),
        Ok(Effect::Removed(slice)) => {
            cursor = slice.cursor;
            removed = removed
                .checked_add(slice.removed)
                .ok_or_else(|| refused("removed-byte overflow"))?;
            absent = absent
                .checked_add(slice.absent)
                .ok_or_else(|| refused("absent-byte overflow"))?;
            reason = slice.reason;
            if cursor == plan.snapshot.entries.len() {
                phase = "finishing".into();
            }
        }
        Ok(Effect::Finished) => phase = "complete".into(),
        Err(problem) => reason = Some(problem),
    }
    let resume = reason.as_ref().map(|_| phase.clone());
    if reason.is_some() {
        phase = "needs-owner".into();
    }
    write.execute("UPDATE project_target_prunes SET phase=?2,worker_active=0,resume_phase=?3,
        quarantine_identity=?4,cursor=?5,removed_bytes=?6,absent_bytes=?7,reason=?8,changed_at=?9 WHERE id=?1",
        params![receipt.operation, phase.as_str(), resume, identity,
            i64::try_from(cursor).map_err(|_| refused("cleanup cursor overflow"))?,
            i64::try_from(removed).map_err(|_| refused("removed-byte range"))?,
            i64::try_from(absent).map_err(|_| refused("absent-byte range"))?, reason, crate::store::now()]).await?;
    if phase == "complete" {
        // The bounded work manifest is no longer needed after all effects are receipted.
        // Keep the operation identity, decision, summary and accounting for status.
        write
            .execute(
                "UPDATE project_target_prunes SET manifest=X'' WHERE id=?1",
                [receipt.operation],
            )
            .await?;
        write
            .execute(
                "DELETE FROM project_target_prune_guards WHERE operation=?1 AND generation=?2",
                params![receipt.operation, receipt.generation],
            )
            .await?;
    } else if phase == "needs-owner" {
        write.execute("DELETE FROM project_target_prune_guards WHERE key='global' AND operation=?1 AND generation=?2",
            params![receipt.operation, receipt.generation]).await?;
    }
    store::prune_status(write, receipt.operation).await
}

/// Same immutable decision; no auto-refresh to a different plan or changed policy.
pub async fn resume_target_prune(
    write: &WriteTransaction<'_>,
    epoch: &str,
    operation: i64,
    revision: i64,
    policy: &PrunePolicy,
    evidence: &PruneEvidence,
) -> Result<PruneStatus, Error> {
    let status = store::prune_status(write, operation).await?;
    if status.revision != revision {
        return Err(store::stale("cleanup revision changed"));
    }
    if matches!(status.phase.as_str(), "complete" | "abandoned") {
        return Ok(status);
    }
    let plan = store::load(write, operation).await?;
    if plan.status.revision != revision {
        return Err(store::stale("cleanup revision changed"));
    }
    if !matches!(plan.status.phase.as_str(), "paused" | "needs-owner") {
        return Ok(plan.status);
    }
    if plan.worker_active {
        return Err(refused("cleanup worker has not stopped"));
    }
    current(write, epoch, &plan, policy, evidence).await?;
    store::require_execution_start_allowed(write).await?;
    let phase = plan
        .resume_phase
        .as_deref()
        .ok_or_else(|| refused("cleanup has no resumable phase"))?;
    let key = format!("checkout:{}", plan.status.checkout.0);
    let count = write
        .execute(
            "UPDATE project_target_prune_guards SET generation=generation+1
        WHERE key=?1 AND operation=?2 AND generation=?3 AND epoch=?4",
            params![key, operation, plan.generation, epoch],
        )
        .await?;
    if count != 1 {
        return Err(store::stale("candidate guard changed before resume"));
    }
    let generation = plan
        .generation
        .checked_add(1)
        .ok_or_else(|| refused("cleanup generation overflow"))?;
    write.execute("INSERT INTO project_target_prune_guards(key,operation,generation,epoch) VALUES ('global',?1,?2,?3)",
        params![operation, generation, epoch]).await?;
    write.execute("UPDATE project_target_prunes SET generation=?2,phase=?3,resume_phase=NULL,reason=NULL,changed_at=?4 WHERE id=?1",
        params![operation, generation, phase, crate::store::now()]).await?;
    store::prune_status(write, operation).await
}

/// Owner-only orchestration supplies the explicit current database epoch. This changes
/// only the inactive operation's ledger; it never reads, removes or restores a path.
pub async fn abandon_target_prune(
    write: &WriteTransaction<'_>,
    current_epoch: &str,
    operation: i64,
    revision: i64,
    expected_epoch: &str,
) -> Result<PruneStatus, Error> {
    if current_epoch != expected_epoch || current_epoch.is_empty() {
        return Err(store::stale(
            "cleanup database epoch changed; inspect current status",
        ));
    }
    let status = store::prune_status(write, operation).await?;
    if status.revision != revision {
        return Err(store::stale("cleanup revision changed"));
    }
    if matches!(status.phase.as_str(), "complete" | "abandoned") {
        return Ok(status);
    }
    let mut rows = write
        .query(
            "SELECT generation,worker_active FROM project_target_prunes WHERE id=?1",
            [operation],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| store::stale("cleanup operation disappeared"))?;
    let generation: i64 = row.get(0)?;
    if row.get::<i64>(1)? != 0 {
        return Err(refused(
            "cleanup filesystem step is active or unreceipted; wait for its joined result or startup reconciliation",
        ));
    }
    drop(rows);
    // A restored plan retains its historical epoch. Only its exact guards may be
    // released, even when an unrelated operation currently owns the global guard.
    let mut rows = write
        .query(
            "SELECT key,generation,epoch FROM project_target_prune_guards WHERE operation=?1",
            [operation],
        )
        .await?;
    let checkout_key = format!("checkout:{}", status.checkout.0);
    while let Some(row) = rows.next().await? {
        let key: String = row.get(0)?;
        if !matches!(key.as_str(), "global") && key != checkout_key
            || row.get::<i64>(1)? != generation
            || row.get::<String>(2)? != status.epoch
        {
            return Err(store::stale(
                "cleanup guard identity changed; retained for inspection",
            ));
        }
    }
    drop(rows);
    let next = generation
        .checked_add(1)
        .ok_or_else(|| refused("cleanup generation overflow"))?;
    let reason = format!(
        "owner abandoned cleanup; all remaining target/quarantine/source files retained, with no restoration or deletion; owner must arrange any manual disposition separately. Previous {}: {}",
        status.phase,
        status.reason.as_deref().unwrap_or("no recorded error")
    );
    write.execute("UPDATE project_target_prunes SET phase='abandoned',generation=?2,resume_phase=NULL,manifest=X'',reason=?3,changed_at=?4 WHERE id=?1",
        params![operation, next, reason, crate::store::now()]).await?;
    write.execute("DELETE FROM project_target_prune_guards WHERE operation=?1 AND generation=?2 AND epoch=?3",
        params![operation, generation, status.epoch]).await?;
    store::prune_status(write, operation).await
}

/// Startup only, after exclusive database ownership/process reconciliation and before
/// session restore. Blocking workers retain that database ownership until they stop.
pub async fn recover_target_prunes(write: &WriteTransaction<'_>) -> Result<(), Error> {
    write.execute("UPDATE project_target_prunes SET resume_phase=phase,phase='paused',worker_active=0,
        reason='daemon interrupted cleanup; revalidate and resume this exact operation',changed_at=?1
        WHERE phase IN ('reserved','detaching','deleting','finishing')", [crate::store::now()]).await?;
    write
        .execute(
            "DELETE FROM project_target_prune_guards WHERE key='global' AND operation IN
        (SELECT id FROM project_target_prunes WHERE phase='paused' AND worker_active=0)",
            (),
        )
        .await?;
    Ok(())
}

/// A safe between-step deferral. An active syscall cannot be paused by changing rows.
pub async fn pause_target_prune(
    write: &WriteTransaction<'_>,
    operation: i64,
    reason: &str,
) -> Result<PruneStatus, Error> {
    let plan = store::load(write, operation).await?;
    if plan.worker_active {
        return Err(refused(
            "cleanup step must stop before its guard is released",
        ));
    }
    if !matches!(
        plan.status.phase.as_str(),
        "reserved" | "detaching" | "deleting" | "finishing"
    ) {
        return Err(refused("cleanup is not between active steps"));
    }
    guards(write, &plan).await?;
    write.execute("UPDATE project_target_prunes SET resume_phase=phase,phase='paused',reason=?2,changed_at=?3 WHERE id=?1",
        params![operation, reason, crate::store::now()]).await?;
    write.execute("DELETE FROM project_target_prune_guards WHERE key='global' AND operation=?1 AND generation=?2",
        params![operation, plan.generation]).await?;
    store::prune_status(write, operation).await
}
