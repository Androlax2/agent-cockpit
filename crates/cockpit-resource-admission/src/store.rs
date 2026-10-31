//! The reservations and the owner's admission settings. A reservation is a row, never an
//! in-memory permit: it outlives the Rust task that asked for it and a daemon restart
//! (report 03), and only an expiry, a withdrawal or an observed end releases it.

use cockpit_processes::{ExecutionId, Phase};
use cockpit_turso::sql::{Connection, Row};
use cockpit_turso::{Database, Migration, MigrationSet, WriteTransaction};

use crate::model::{InUse, decide};
use crate::{
    Admission, AdmissionRequest, Control, ControlSupport, Demand, Error, Limits, Policy, Profile,
    ReleaseReason, Reservation, ReservationId, ReservationState, Settings, Swept, Verdict,
    Workload,
};

pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "resource-admission",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE resource_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                profile TEXT NOT NULL CHECK (profile IN ('normal', 'foreground')),
                queue_paused INTEGER NOT NULL CHECK (queue_paused IN (0, 1)),
                changed_at INTEGER NOT NULL
            );
            INSERT INTO resource_settings (id, profile, queue_paused, changed_at)
                VALUES (1, 'normal', 0, 0);
            CREATE TABLE resource_reservations (
                id INTEGER PRIMARY KEY,
                label TEXT NOT NULL CHECK (label <> ''),
                workload TEXT NOT NULL CHECK (workload IN
                    ('agent-turn', 'delivery', 'approval', 'build', 'test', 'container')),
                cpu_millis INTEGER NOT NULL CHECK (cpu_millis >= 0),
                memory_mib INTEGER NOT NULL CHECK (memory_mib >= 0),
                cpu_weight INTEGER CHECK (cpu_weight BETWEEN 1 AND 10000),
                cpu_quota_percent INTEGER CHECK (cpu_quota_percent > 0),
                memory_max_mib INTEGER CHECK (memory_max_mib > 0),
                tasks_max INTEGER CHECK (tasks_max > 0),
                idle_scheduling INTEGER NOT NULL CHECK (idle_scheduling IN (0, 1)),
                nice INTEGER CHECK (nice BETWEEN 0 AND 19),
                required TEXT NOT NULL,
                state TEXT NOT NULL CHECK (state IN ('reserved', 'running', 'released')),
                execution INTEGER UNIQUE,
                reserved_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                checkpoint_requested_at INTEGER,
                released_at INTEGER,
                released_reason TEXT CHECK (released_reason IN ('expired', 'withdrawn', 'ended')),
                CHECK ((state = 'released') = (released_reason IS NOT NULL)),
                CHECK ((state = 'released') = (released_at IS NOT NULL)),
                CHECK (state <> 'running' OR execution IS NOT NULL),
                CHECK (state <> 'reserved' OR execution IS NULL)
            );
            CREATE INDEX resource_reservations_occupying
                ON resource_reservations (id) WHERE state <> 'released';",
    }],
};

/// Decides `request` and, when it fits, commits its reservation in `write`, so the decision
/// and the charge are one change: two callers can never both take the last capacity.
pub async fn admit(
    write: &WriteTransaction<'_>,
    request: &AdmissionRequest,
    policy: &Policy,
    support: &[ControlSupport],
    now: i64,
) -> Result<Admission, Error> {
    if let Verdict::Held(hold) = check(write, request, policy, support).await? {
        return Ok(Admission::Held(hold));
    }
    let limits = request.limits;
    let expires_at =
        now.saturating_add(i64::try_from(policy.unstarted_expiry.as_secs()).unwrap_or(i64::MAX));
    let next = write
        .prepare("SELECT coalesce(max(id), 0) + 1 FROM resource_reservations")
        .await?
        .query_row(())
        .await?;
    let id = ReservationId(next.get(0)?);
    write
        .execute(
            "INSERT INTO resource_reservations (id, label, workload, cpu_millis, memory_mib,
                 cpu_weight, cpu_quota_percent, memory_max_mib, tasks_max, idle_scheduling, nice,
                 required, state, reserved_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'reserved', ?13, ?14)",
            cockpit_turso::sql::params![
                id.0,
                request.label.as_str(),
                request.workload.as_str(),
                stored(request.demand.cpu_millis)?,
                stored(request.demand.memory_mib)?,
                limits.cpu_weight.map(stored).transpose()?,
                limits.cpu_quota_percent.map(stored).transpose()?,
                limits.memory_max_mib.map(stored).transpose()?,
                limits.tasks_max.map(stored).transpose()?,
                i64::from(limits.idle_scheduling),
                limits.nice.map(stored).transpose()?,
                required_text(&request.required),
                now,
                expires_at,
            ],
        )
        .await?;
    Ok(Admission::Admitted(id))
}

/// Whether `request` fits now, without reserving anything: what a caller would be told.
pub async fn check(
    connection: &Connection,
    request: &AdmissionRequest,
    policy: &Policy,
    support: &[ControlSupport],
) -> Result<Verdict, Error> {
    validate(request)?;
    let settings = settings(connection).await?;
    let in_use = in_use(connection).await?;
    Ok(decide(request, settings, in_use, policy, support))
}

fn validate(request: &AdmissionRequest) -> Result<(), Error> {
    let invalid = |reason: String| Err(Error::InvalidRequest { reason });
    if request.label.trim().is_empty() {
        return invalid("a reservation needs a label".to_owned());
    }
    if let Err(reason) = request.limits.validate() {
        return invalid(reason);
    }
    Ok(())
}

/// Every reservation still charged, oldest first.
pub async fn reservations(connection: &Connection) -> Result<Vec<Reservation>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {COLUMNS} FROM resource_reservations WHERE state <> 'released' ORDER BY id"
            ),
            (),
        )
        .await?;
    let mut reservations = Vec::new();
    while let Some(row) = rows.next().await? {
        reservations.push(reservation_from_row(&row)?);
    }
    Ok(reservations)
}

pub async fn reservation(connection: &Connection, id: ReservationId) -> Result<Reservation, Error> {
    let mut rows = connection
        .query(
            &format!("SELECT {COLUMNS} FROM resource_reservations WHERE id = ?1"),
            [id.0],
        )
        .await?;
    match rows.next().await? {
        Some(row) => reservation_from_row(&row),
        None => Err(Error::UnknownReservation { reservation: id }),
    }
}

/// The reservation's work started as `execution`: from now on only that execution's observed
/// end releases it.
pub async fn bind_execution(
    write: &WriteTransaction<'_>,
    id: ReservationId,
    execution: ExecutionId,
) -> Result<(), Error> {
    require_state(write, id, "reserved").await?;
    write
        .execute(
            "UPDATE resource_reservations SET state = 'running', execution = ?1
             WHERE id = ?2 AND state = 'reserved'",
            (execution.0, id.0),
        )
        .await?;
    Ok(())
}

/// Gives back a reservation whose work never started.
pub async fn withdraw(
    write: &WriteTransaction<'_>,
    id: ReservationId,
    now: i64,
) -> Result<(), Error> {
    require_state(write, id, "reserved").await?;
    write
        .execute(
            "UPDATE resource_reservations
             SET state = 'released', released_reason = 'withdrawn', released_at = ?1
             WHERE id = ?2 AND state = 'reserved'",
            (now, id.0),
        )
        .await?;
    Ok(())
}

/// Pause at checkpoint: records the owner's request that running work stop at its next
/// supported boundary. The work's owner reads it; nothing is signalled here.
pub async fn request_checkpoint(
    write: &WriteTransaction<'_>,
    id: ReservationId,
    now: i64,
) -> Result<Reservation, Error> {
    require_state(write, id, "running").await?;
    write
        .execute(
            "UPDATE resource_reservations SET checkpoint_requested_at = ?1
             WHERE id = ?2 AND checkpoint_requested_at IS NULL",
            (now, id.0),
        )
        .await?;
    reservation(write, id).await
}

async fn require_state(
    connection: &Connection,
    id: ReservationId,
    needed: &'static str,
) -> Result<(), Error> {
    let found = reservation(connection, id).await?;
    let state = state_name(&found.state);
    if state != needed {
        return Err(Error::WrongState {
            reservation: id,
            state,
            needed,
        });
    }
    Ok(())
}

fn state_name(state: &ReservationState) -> &'static str {
    match state {
        ReservationState::Reserved { .. } => "reserved",
        ReservationState::Running { .. } => "running",
        ReservationState::Released { .. } => "released",
    }
}

/// Releases what is over: unstarted reservations past their expiry, and running ones whose
/// execution `cockpit-processes` observed ended. A running reservation whose execution is
/// not observed ended stays charged, whatever happened to the task that started it.
pub async fn sweep(database: &Database, now: i64) -> Result<Swept, Error> {
    let running: Vec<(ReservationId, ExecutionId)> = {
        let read = database.read().await?;
        let mut rows = read
            .query(
                "SELECT id, execution FROM resource_reservations WHERE state = 'running'
                 ORDER BY id",
                (),
            )
            .await?;
        let mut running = Vec::new();
        while let Some(row) = rows.next().await? {
            running.push((ReservationId(row.get(0)?), ExecutionId(row.get(1)?)));
        }
        running
    };
    let mut ended = Vec::new();
    for (reservation, execution) in running {
        if matches!(
            cockpit_processes::execution(database, execution)
                .await?
                .phase,
            Phase::Ended(_)
        ) {
            ended.push(reservation);
        }
    }
    let write = database.write().await?;
    let mut expired = Vec::new();
    let mut rows = write
        .query(
            "SELECT id FROM resource_reservations WHERE state = 'reserved' AND expires_at <= ?1
             ORDER BY id",
            [now],
        )
        .await?;
    while let Some(row) = rows.next().await? {
        expired.push(ReservationId(row.get(0)?));
    }
    drop(rows);
    for (ids, from, reason) in [
        (&expired, "reserved", ReleaseReason::Expired),
        (&ended, "running", ReleaseReason::Ended),
    ] {
        for id in ids {
            write
                .execute(
                    "UPDATE resource_reservations
                     SET state = 'released', released_reason = ?1, released_at = ?2
                     WHERE id = ?3 AND state = ?4",
                    (reason.as_str(), now, id.0, from),
                )
                .await?;
        }
    }
    write.commit().await?;
    Ok(Swept { expired, ended })
}

pub async fn settings(connection: &Connection) -> Result<Settings, Error> {
    let row = connection
        .prepare("SELECT profile, queue_paused FROM resource_settings WHERE id = 1")
        .await?
        .query_row(())
        .await?;
    let profile: String = row.get(0)?;
    Ok(Settings {
        profile: Profile::parse(&profile).ok_or_else(|| Error::InvalidRequest {
            reason: format!("the stored profile {profile:?} is unknown"),
        })?,
        queue_paused: row.get::<i64>(1)? != 0,
    })
}

/// Sets the profile. Only the owner's socket reaches this: agents cannot award themselves
/// foreground priority.
pub async fn set_profile(
    write: &WriteTransaction<'_>,
    profile: Profile,
    now: i64,
) -> Result<(), Error> {
    write
        .execute(
            "UPDATE resource_settings SET profile = ?1, changed_at = ?2 WHERE id = 1",
            (profile.as_str(), now),
        )
        .await?;
    Ok(())
}

/// Pause queue: while paused, nothing new is admitted. Running work is not touched.
pub async fn set_queue_paused(
    write: &WriteTransaction<'_>,
    paused: bool,
    now: i64,
) -> Result<(), Error> {
    write
        .execute(
            "UPDATE resource_settings SET queue_paused = ?1, changed_at = ?2 WHERE id = 1",
            (i64::from(paused), now),
        )
        .await?;
    Ok(())
}

/// Capacity charged now: every reservation not released, and the heavy ones among them.
pub async fn in_use(connection: &Connection) -> Result<InUse, Error> {
    let mut rows = connection
        .query(
            "SELECT workload, coalesce(sum(cpu_millis), 0), coalesce(sum(memory_mib), 0)
             FROM resource_reservations WHERE state <> 'released' GROUP BY workload",
            (),
        )
        .await?;
    let mut in_use = InUse::default();
    while let Some(row) = rows.next().await? {
        let workload: String = row.get(0)?;
        let demand = Demand {
            cpu_millis: loaded(row.get(1)?)?,
            memory_mib: loaded(row.get(2)?)?,
        };
        in_use.all = in_use.all.plus(demand);
        if Workload::parse(&workload).is_some_and(Workload::is_heavy) {
            in_use.heavy = in_use.heavy.plus(demand);
        }
    }
    Ok(in_use)
}

const COLUMNS: &str = "id, label, workload, cpu_millis, memory_mib, cpu_weight, cpu_quota_percent,
    memory_max_mib, tasks_max, idle_scheduling, nice, required, state, execution, expires_at,
    released_reason, checkpoint_requested_at";

fn reservation_from_row(row: &Row) -> Result<Reservation, Error> {
    let id = ReservationId(row.get(0)?);
    let invalid = |reason: String| Error::InvalidStoredReservation {
        reservation: id,
        reason,
    };
    let workload_name: String = row.get(2)?;
    let workload = Workload::parse(&workload_name)
        .ok_or_else(|| invalid(format!("workload {workload_name:?}")))?;
    let optional = |index: usize| -> Result<Option<u64>, Error> {
        row.get::<Option<i64>>(index)?.map(loaded).transpose()
    };
    let required_text: String = row.get(11)?;
    let required = required_text
        .split_whitespace()
        .map(|name| Control::parse(name).ok_or_else(|| invalid(format!("control {name:?}"))))
        .collect::<Result<Vec<_>, _>>()?;
    let state_name: String = row.get(12)?;
    let execution: Option<i64> = row.get(13)?;
    let released_reason: Option<String> = row.get(15)?;
    let state = match (state_name.as_str(), execution, released_reason.as_deref()) {
        ("reserved", None, None) => ReservationState::Reserved {
            expires_at: row.get(14)?,
        },
        ("running", Some(execution), None) => ReservationState::Running {
            execution: ExecutionId(execution),
        },
        ("released", _, Some(reason)) => ReservationState::Released {
            reason: ReleaseReason::parse(reason)
                .ok_or_else(|| invalid(format!("release reason {reason:?}")))?,
        },
        (state, execution, reason) => {
            return Err(invalid(format!(
                "state {state:?} with execution {execution:?} and reason {reason:?}"
            )));
        }
    };
    Ok(Reservation {
        id,
        label: row.get(1)?,
        workload,
        demand: Demand {
            cpu_millis: loaded(row.get(3)?)?,
            memory_mib: loaded(row.get(4)?)?,
        },
        limits: Limits {
            cpu_weight: optional(5)?,
            cpu_quota_percent: optional(6)?,
            memory_max_mib: optional(7)?,
            tasks_max: optional(8)?,
            idle_scheduling: row.get::<i64>(9)? != 0,
            nice: optional(10)?,
        },
        required,
        state,
        checkpoint_requested: row.get::<Option<i64>>(16)?.is_some(),
    })
}

fn required_text(required: &[Control]) -> String {
    required
        .iter()
        .map(|control| control.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn stored(value: u64) -> Result<i64, Error> {
    i64::try_from(value).map_err(|_| Error::InvalidRequest {
        reason: format!("{value} is too large"),
    })
}

fn loaded(value: i64) -> Result<u64, Error> {
    u64::try_from(value).map_err(|_| Error::InvalidRequest {
        reason: format!("the stored value {value} is negative"),
    })
}
