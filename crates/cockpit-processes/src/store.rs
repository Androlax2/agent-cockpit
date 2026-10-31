//! The executions table: every launch intent, identity and observed end.

use std::time::{SystemTime, UNIX_EPOCH};

use cockpit_turso::sql::Row;
use cockpit_turso::{Database, Migration, MigrationSet};

use crate::model::{PROCESS_GROUP, SYSTEMD_SCOPE};
use crate::{Containment, EndReason, Error, Execution, ExecutionId, LaunchContainment, Phase};

/// One row per execution. A row is never deleted: an ended execution keeps its identity and
/// reason, so a late report can be traced to the execution it came from.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "processes",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE process_executions (
                id INTEGER PRIMARY KEY,
                containment TEXT NOT NULL
                    CHECK (containment IN ('process-group', 'systemd-scope')),
                unit TEXT CHECK (unit <> '')
                    CHECK ((containment = 'systemd-scope') = (unit IS NOT NULL)),
                boot_id TEXT NOT NULL CHECK (boot_id <> ''),
                phase TEXT NOT NULL CHECK (phase IN ('launching', 'running', 'ended')),
                process_group INTEGER CHECK (process_group > 1),
                leader_start TEXT CHECK (leader_start <> ''),
                intended_at INTEGER NOT NULL,
                cancel_requested_at INTEGER,
                ended_at INTEGER,
                ended_reason TEXT CHECK (ended_reason IN
                    ('exited', 'terminated', 'never-started', 'host-restarted')),
                CHECK ((phase = 'ended') = (ended_reason IS NOT NULL)),
                CHECK ((phase = 'ended') = (ended_at IS NOT NULL)),
                CHECK (phase <> 'running'
                    OR (process_group IS NOT NULL AND leader_start IS NOT NULL))
            );
            CREATE INDEX process_executions_occupying
                ON process_executions (id) WHERE phase <> 'ended';",
    }],
};

/// Commits the launch intent: the reservation exists before any process does.
pub(crate) async fn record_intent(
    database: &Database,
    launch_containment: LaunchContainment,
    boot_id: &str,
) -> Result<(ExecutionId, Containment), Error> {
    let write = database.write().await?;
    // The unit name embeds the row id, so a restart finds the unit from the row alone; ids
    // are never reused because rows are never deleted.
    let next = write
        .prepare("SELECT coalesce(max(id), 0) + 1 FROM process_executions")
        .await?
        .query_row(())
        .await?;
    let id = ExecutionId(next.get(0)?);
    let (containment, containment_kind, unit) = match launch_containment {
        LaunchContainment::SystemdScope => {
            let unit = format!("cockpit-{}-{}.scope", database.epoch(), id.0);
            let containment = Containment::SystemdScope { unit: unit.clone() };
            (containment, SYSTEMD_SCOPE, Some(unit))
        }
        LaunchContainment::ProcessGroup => (Containment::ProcessGroup, PROCESS_GROUP, None),
    };
    write
        .execute(
            "INSERT INTO process_executions (id, containment, unit, boot_id, phase, intended_at)
             VALUES (?1, ?2, ?3, ?4, 'launching', ?5)",
            (id.0, containment_kind, unit, boot_id, now_ms()),
        )
        .await?;
    write.commit().await?;
    Ok((id, containment))
}

pub(crate) async fn record_process_group(
    database: &Database,
    id: ExecutionId,
    process_group: i32,
) -> Result<(), Error> {
    let write = database.write().await?;
    write
        .execute(
            "UPDATE process_executions SET process_group = ?1
             WHERE id = ?2 AND phase = 'launching'",
            (i64::from(process_group), id.0),
        )
        .await?;
    Ok(write.commit().await?)
}

pub(crate) async fn record_running(
    database: &Database,
    id: ExecutionId,
    process_group: i32,
    leader_start: &str,
) -> Result<(), Error> {
    let write = database.write().await?;
    write
        .execute(
            "UPDATE process_executions
             SET phase = 'running', process_group = ?1, leader_start = ?2
             WHERE id = ?3 AND phase = 'launching'",
            (i64::from(process_group), leader_start, id.0),
        )
        .await?;
    Ok(write.commit().await?)
}

pub(crate) async fn record_cancel_requested(
    database: &Database,
    id: ExecutionId,
) -> Result<(), Error> {
    let write = database.write().await?;
    write
        .execute(
            "UPDATE process_executions SET cancel_requested_at = ?1
             WHERE id = ?2 AND cancel_requested_at IS NULL",
            (now_ms(), id.0),
        )
        .await?;
    Ok(write.commit().await?)
}

/// Releases the reservation. Only an observation calls this, never a dropped task.
pub(crate) async fn record_ended(
    database: &Database,
    id: ExecutionId,
    reason: EndReason,
) -> Result<(), Error> {
    let write = database.write().await?;
    write
        .execute(
            "UPDATE process_executions
             SET phase = 'ended', ended_reason = ?1, ended_at = ?2
             WHERE id = ?3 AND phase <> 'ended'",
            (reason.as_str(), now_ms(), id.0),
        )
        .await?;
    Ok(write.commit().await?)
}

const COLUMNS: &str = "id, containment, unit, boot_id, phase, process_group, leader_start,
    cancel_requested_at, ended_reason";

pub async fn execution(database: &Database, id: ExecutionId) -> Result<Execution, Error> {
    let read = database.read().await?;
    let mut rows = read
        .query(
            &format!("SELECT {COLUMNS} FROM process_executions WHERE id = ?1"),
            [id.0],
        )
        .await?;
    match rows.next().await? {
        Some(row) => execution_from_row(&row),
        None => Err(Error::UnknownExecution { execution: id }),
    }
}

/// Every execution that still holds its reservation, oldest first.
pub async fn occupying_executions(database: &Database) -> Result<Vec<Execution>, Error> {
    let read = database.read().await?;
    let mut rows = read
        .query(
            &format!("SELECT {COLUMNS} FROM process_executions WHERE phase <> 'ended' ORDER BY id"),
            (),
        )
        .await?;
    let mut executions = Vec::new();
    while let Some(row) = rows.next().await? {
        executions.push(execution_from_row(&row)?);
    }
    Ok(executions)
}

fn execution_from_row(row: &Row) -> Result<Execution, Error> {
    let id = ExecutionId(row.get(0)?);
    let invalid = |reason: String| Error::InvalidStoredExecution {
        execution: id,
        reason,
    };
    let containment_kind: String = row.get(1)?;
    let unit: Option<String> = row.get(2)?;
    let containment = match (containment_kind.as_str(), unit) {
        (SYSTEMD_SCOPE, Some(unit)) => Containment::SystemdScope { unit },
        (PROCESS_GROUP, None) => Containment::ProcessGroup,
        (kind, unit) => {
            return Err(invalid(format!("containment {kind:?} with unit {unit:?}")));
        }
    };
    let phase_name: String = row.get(4)?;
    let ended_reason: Option<String> = row.get(8)?;
    let phase = match (phase_name.as_str(), ended_reason.as_deref()) {
        ("launching", None) => Phase::Launching,
        ("running", None) => Phase::Running,
        ("ended", Some(reason)) => Phase::Ended(
            EndReason::parse(reason).ok_or_else(|| invalid(format!("end reason {reason:?}")))?,
        ),
        (phase, reason) => {
            return Err(invalid(format!(
                "phase {phase:?} with end reason {reason:?}"
            )));
        }
    };
    let process_group = row
        .get::<Option<i64>>(5)?
        .map(|group| i32::try_from(group).map_err(|_| invalid(format!("process group {group}"))))
        .transpose()?;
    Ok(Execution {
        id,
        containment,
        boot_id: row.get(3)?,
        phase,
        process_group,
        leader_start: row.get(6)?,
        cancel_requested: row.get::<Option<i64>>(7)?.is_some(),
    })
}

fn now_ms() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    i64::try_from(elapsed.as_millis()).expect("milliseconds since 1970 fit in i64")
}
