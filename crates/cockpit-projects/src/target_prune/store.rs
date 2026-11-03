//! Projects owns cleanup intent, decisions, manifests and exclusion; no foreign SQL.
use super::{
    PruneEvidence, PrunePreview, PruneStatus,
    manifest::{self, Snapshot},
    refused,
};
use crate::{CheckoutId, CheckoutKind, CheckoutState, Error};
use cockpit_turso::sql::{Connection, params};
use cockpit_turso::{Migration, WriteTransaction};

pub(crate) const MIGRATION: Migration = Migration {
    version: 2,
    sql: "CREATE TABLE project_target_prunes (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
        generation INTEGER NOT NULL DEFAULT 0 CHECK(generation >= 0),
        checkout_id INTEGER NOT NULL, incarnation INTEGER NOT NULL, epoch TEXT NOT NULL,
        policy TEXT NOT NULL, eligibility TEXT NOT NULL, target TEXT NOT NULL,
        root_identity TEXT NOT NULL, target_identity TEXT NOT NULL, manifest BLOB NOT NULL,
        apparent_bytes INTEGER NOT NULL CHECK(apparent_bytes >= 0),
        phase TEXT NOT NULL CHECK(phase IN ('prepared','reserved','detaching','deleting','finishing','paused','needs-owner','complete','abandoned')),
        worker_active INTEGER NOT NULL DEFAULT 0 CHECK(worker_active IN (0,1)),
        resume_phase TEXT CHECK(resume_phase IN ('reserved','detaching','deleting','finishing')),
        decision TEXT, quarantine TEXT, quarantine_identity TEXT,
        cursor INTEGER NOT NULL DEFAULT 0 CHECK(cursor >= 0),
        removed_bytes INTEGER NOT NULL DEFAULT 0 CHECK(removed_bytes >= 0),
        absent_bytes INTEGER NOT NULL DEFAULT 0 CHECK(absent_bytes >= 0),
        reason TEXT, created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL
    );
    CREATE TABLE project_target_prune_guards (
        key TEXT PRIMARY KEY, operation INTEGER NOT NULL, generation INTEGER NOT NULL,
        epoch TEXT NOT NULL
    );",
};

pub(super) fn stale(reason: &str) -> Error {
    Error::StalePrune {
        reason: reason.to_owned(),
    }
}

/// Call from the SAME writer that reserves a launch or approves a future execution.
pub async fn require_execution_start_allowed(connection: &Connection) -> Result<(), Error> {
    require_no_guard(connection, "global").await
}

pub async fn require_checkout_available(
    connection: &Connection,
    checkout: CheckoutId,
) -> Result<(), Error> {
    require_no_guard(connection, &format!("checkout:{}", checkout.0)).await
}

/// Approved commands/checks may start without claiming a session checkout. A paused
/// cleanup still excludes working directories inside (or containing) its candidate.
/// This is managed admission, not a sandbox for arbitrary shell path access.
pub async fn require_working_directory_available(
    connection: &Connection,
    path: &std::path::Path,
) -> Result<(), Error> {
    let mut rows = connection
        .query(
            "SELECT g.operation, c.path FROM project_target_prune_guards g
        JOIN project_checkouts c ON g.key = 'checkout:' || c.id",
            (),
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let root = std::path::PathBuf::from(row.get::<String>(1)?);
        if !root.is_absolute() {
            return Err(refused("invalid guarded checkout path"));
        }
        if path.starts_with(&root) || root.starts_with(path) {
            let operation: i64 = row.get(0)?;
            return Err(refused(format!(
                "target maintenance operation {operation} protects this working directory; inspect torlek storage prune status {operation}"
            )));
        }
    }
    Ok(())
}

async fn require_no_guard(connection: &Connection, key: &str) -> Result<(), Error> {
    let mut rows = connection
        .query(
            "SELECT operation FROM project_target_prune_guards WHERE key = ?1",
            [key],
        )
        .await?;
    if let Some(row) = rows.next().await? {
        let operation: i64 = row.get(0)?;
        return Err(refused(format!(
            "target maintenance operation {operation} holds {key}; inspect torlek storage prune status {operation}"
        )));
    }
    Ok(())
}

pub(super) async fn current_preview(
    write: &WriteTransaction<'_>,
    preview: &PrunePreview,
    evidence: &PruneEvidence,
) -> Result<(), Error> {
    let stored = crate::store::stored_checkout(write, preview.checkout.id).await?;
    if stored.checkout != preview.checkout
        || stored.checkout.state != CheckoutState::Live
        || matches!(stored.checkout.kind, CheckoutKind::Main)
        || stored.checkout.occupancy.is_some()
    {
        return Err(stale("checkout identity, state or occupancy changed"));
    }
    if let Some(reason) = &evidence.busy {
        return Err(refused(reason.clone()));
    }
    let proof = preview.eligibility.as_deref().ok_or_else(|| {
        refused(
            preview
                .kept_reason
                .clone()
                .unwrap_or_else(|| "target is not eligible".to_owned()),
        )
    })?;
    if proof.starts_with("observed ended sessions:")
        && evidence
            .ended_sessions
            .as_ref()
            .map(|ended| format!("observed ended sessions: {ended}"))
            .as_deref()
            != Some(proof)
    {
        return Err(stale(
            "ended-session identity or termination evidence changed",
        ));
    }
    Ok(())
}

pub(super) async fn prepare(
    write: &WriteTransaction<'_>,
    epoch: &str,
    preview: &PrunePreview,
    evidence: &PruneEvidence,
) -> Result<PruneStatus, Error> {
    current_preview(write, preview, evidence).await?;
    require_checkout_available(write, preview.checkout.id).await?;
    let snapshot = preview
        .snapshot
        .as_ref()
        .ok_or_else(|| refused("a complete target manifest is required"))?;
    let encoded = manifest::encode(&snapshot.entries)?;
    // Replace only unconsumed previews; ids are unique within the caller-bound epoch.
    write
        .execute(
            "DELETE FROM project_target_prunes WHERE checkout_id = ?1 AND phase = 'prepared'",
            [preview.checkout.id.0],
        )
        .await?;
    let mut rows = write.query("SELECT COUNT(*), COALESCE(SUM(length(manifest)),0) FROM project_target_prunes WHERE phase NOT IN ('complete','abandoned')", ()).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| refused("missing plan bound"))?;
    let count: i64 = row.get(0)?;
    let bytes: i64 = row.get(1)?;
    if count >= 8 || bytes.saturating_add(encoded.len() as i64) > 64 * 1024 * 1024 {
        return Err(refused(
            "prepared or unfinished cleanup plan bound reached; resolve existing operations",
        ));
    }
    drop(rows);
    let now = crate::store::now();
    let mut rows = write.query("INSERT INTO project_target_prunes
        (checkout_id,incarnation,epoch,policy,eligibility,root_identity,target_identity,manifest,apparent_bytes,phase,created_at,changed_at,target)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'prepared',?10,?10,?11) RETURNING id", params![
            preview.checkout.id.0, preview.checkout.incarnation, epoch,
            format!("{:?}", preview.policy), preview.eligibility.as_deref().unwrap_or_default(),
            snapshot.root.as_str(), snapshot.target.as_str(), encoded,
            i64::try_from(snapshot.bytes).map_err(|_| refused("target byte count exceeds stored range"))?, now, preview.checkout.path.join("target").to_string_lossy().as_ref()
        ]).await?;
    let id = rows
        .next()
        .await?
        .ok_or_else(|| refused("plan insertion returned no identity"))?
        .get(0)?;
    drop(rows);
    prune_status(write, id).await
}

pub async fn prune_status(connection: &Connection, operation: i64) -> Result<PruneStatus, Error> {
    let mut rows = connection.query("SELECT id,revision,checkout_id,phase,apparent_bytes,removed_bytes,absent_bytes,eligibility,reason,epoch,target,quarantine
        FROM project_target_prunes WHERE id = ?1", [operation]).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| stale("unknown or superseded target-prune plan"))?;
    let unsigned = |index| -> Result<u64, Error> {
        u64::try_from(row.get::<i64>(index)?).map_err(|_| refused("invalid stored cleanup count"))
    };
    let target: String = row.get(10)?;
    let quarantine: Option<String> = row.get(11)?;
    let quarantine = quarantine.map(|name| {
        std::path::Path::new(&target)
            .with_file_name(name)
            .to_string_lossy()
            .into_owned()
    });
    Ok(PruneStatus {
        target,
        quarantine,
        operation: row.get(0)?,
        revision: row.get(1)?,
        checkout: CheckoutId(row.get(2)?),
        phase: row.get(3)?,
        apparent_bytes: unsigned(4)?,
        removed_apparent_bytes: unsigned(5)?,
        absent_unconfirmed_bytes: unsigned(6)?,
        eligibility: row.get(7)?,
        reason: row.get(8)?,
        epoch: row.get(9)?,
    })
}

/// Only already authorized automatic operations deferred between steps or by restart.
/// Filesystem uncertainty is needs-owner and is never retried automatically.
pub async fn paused_automatic_prunes(connection: &Connection) -> Result<Vec<PruneStatus>, Error> {
    let mut rows = connection
        .query(
            "SELECT id FROM project_target_prunes
        WHERE phase='paused' AND decision='owner-policy' AND worker_active=0 ORDER BY id LIMIT 8",
            (),
        )
        .await?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await? {
        ids.push(row.get::<i64>(0)?);
    }
    drop(rows);
    let mut statuses = Vec::new();
    for id in ids {
        statuses.push(prune_status(connection, id).await?);
    }
    Ok(statuses)
}

pub(super) struct StoredPlan {
    pub status: PruneStatus,
    pub generation: i64,
    pub incarnation: i64,
    pub epoch: String,
    pub policy: String,
    pub snapshot: Snapshot,
    pub quarantine: Option<String>,
    pub quarantine_identity: Option<String>,
    pub cursor: usize,
    pub worker_active: bool,
    pub resume_phase: Option<String>,
}

pub(super) async fn load(connection: &Connection, id: i64) -> Result<StoredPlan, Error> {
    let status = prune_status(connection, id).await?;
    let mut rows = connection.query("SELECT generation,incarnation,epoch,policy,root_identity,target_identity,manifest,
        quarantine,quarantine_identity,cursor,worker_active,resume_phase FROM project_target_prunes WHERE id = ?1", [id]).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| stale("cleanup operation disappeared"))?;
    let encoded: Vec<u8> = row.get(6)?;
    let plan = StoredPlan {
        generation: row.get(0)?,
        incarnation: row.get(1)?,
        epoch: row.get(2)?,
        policy: row.get(3)?,
        snapshot: Snapshot {
            root: row.get(4)?,
            target: row.get(5)?,
            entries: manifest::decode(&encoded)?,
            bytes: status.apparent_bytes,
        },
        quarantine: row.get(7)?,
        quarantine_identity: row.get(8)?,
        cursor: usize::try_from(row.get::<i64>(9)?)
            .map_err(|_| refused("invalid cleanup cursor"))?,
        worker_active: row.get::<i64>(10)? != 0,
        resume_phase: row.get(11)?,
        status,
    };
    let total = plan
        .snapshot
        .entries
        .iter()
        .try_fold(0_u64, |total, entry| total.checked_add(entry.bytes));
    let accounted = plan
        .status
        .removed_apparent_bytes
        .checked_add(plan.status.absent_unconfirmed_bytes);
    let processed = plan
        .snapshot
        .entries
        .iter()
        .rev()
        .take(plan.cursor)
        .try_fold(0_u64, |total, entry| total.checked_add(entry.bytes));
    let paused = matches!(plan.status.phase.as_str(), "paused" | "needs-owner");
    let phase = if paused {
        plan.resume_phase.as_deref().unwrap_or("")
    } else {
        plan.status.phase.as_str()
    };
    let phase_valid = match phase {
        "prepared" => {
            plan.generation == 0
                && plan.quarantine.is_none()
                && plan.quarantine_identity.is_none()
                && plan.cursor == 0
                && !plan.worker_active
        }
        "reserved" => {
            plan.generation > 0
                && plan.quarantine.is_some()
                && plan.quarantine_identity.is_none()
                && plan.cursor == 0
        }
        "detaching" => {
            plan.generation > 0
                && plan.quarantine.is_some()
                && plan.quarantine_identity.is_some()
                && plan.cursor == 0
        }
        "deleting" => {
            plan.generation > 0 && plan.quarantine.is_some() && plan.quarantine_identity.is_some()
        }
        "finishing" => {
            plan.generation > 0
                && plan.quarantine.is_some()
                && plan.quarantine_identity.is_some()
                && plan.cursor == plan.snapshot.entries.len()
        }
        _ => false,
    };
    if plan.cursor > plan.snapshot.entries.len()
        || total != Some(plan.status.apparent_bytes)
        || accounted != processed
        || accounted.is_none()
        || !phase_valid
        || paused && plan.worker_active
        || !paused && plan.resume_phase.is_some()
        || !plan.snapshot.root.starts_with("d:")
        || plan.snapshot.entries.first().map(|entry| &entry.identity) != Some(&plan.snapshot.target)
        || plan.status.phase == "prepared" && (plan.cursor != 0 || accounted != Some(0))
    {
        return Err(refused(
            "stored cleanup manifest, cursor or byte receipt is inconsistent",
        ));
    }
    Ok(plan)
}

pub(super) async fn reserve(
    write: &WriteTransaction<'_>,
    epoch: &str,
    id: i64,
    revision: i64,
    preview: &PrunePreview,
    evidence: &PruneEvidence,
    automatic: bool,
) -> Result<PruneStatus, Error> {
    let status = prune_status(write, id).await?;
    if status.revision != revision {
        return Err(stale("cleanup plan revision changed"));
    }
    if matches!(status.phase.as_str(), "complete" | "abandoned") {
        return Ok(status);
    }
    let plan = load(write, id).await?;
    if plan.status.revision != revision || plan.epoch != epoch {
        return Err(stale("cleanup plan revision or database epoch changed"));
    }
    if plan.status.phase != "prepared" {
        return Ok(plan.status);
    }
    current_preview(write, preview, evidence).await?;
    if plan.status.checkout != preview.checkout.id
        || plan.incarnation != preview.checkout.incarnation
        || plan.policy != format!("{:?}", preview.policy)
        || preview.eligibility.as_deref() != Some(plan.status.eligibility.as_str())
        || preview.snapshot.as_ref() != Some(&plan.snapshot)
    {
        return Err(stale(
            "target contents, eligibility or owner policy changed; prepare and confirm a new plan",
        ));
    }
    if automatic && preview.confirmation_required {
        return Err(refused("exact-plan owner confirmation is required"));
    }
    require_execution_start_allowed(write).await?;
    require_checkout_available(write, preview.checkout.id).await?;
    let generation = plan
        .generation
        .checked_add(1)
        .ok_or_else(|| refused("cleanup generation overflow"))?;
    for key in [
        "global".to_owned(),
        format!("checkout:{}", preview.checkout.id.0),
    ] {
        write.execute("INSERT INTO project_target_prune_guards(key,operation,generation,epoch) VALUES (?1,?2,?3,?4)",
            params![key, id, generation, epoch]).await?;
    }
    write.execute("UPDATE project_target_prunes SET phase='reserved', generation=?2,decision=?3,quarantine=?4,changed_at=?5 WHERE id=?1",
        params![id, generation, if automatic {"owner-policy"} else {"owner-confirmed"},
            format!(".torlek-target-prune-{epoch}-{id}"), crate::store::now()]).await?;
    prune_status(write, id).await
}
