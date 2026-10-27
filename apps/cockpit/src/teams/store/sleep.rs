//! Host-only continuations: callers verify the sleep note and old-process death before binding.

use super::*;
use rusqlite::TransactionBehavior;

const LIFETIME_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const WAKE_TIMEOUT_MS: i64 = 90_000;
/// A restart's exit and relaunch take seconds; the column gives up on a report after 90 s.
const RESTART_LIFETIME_MS: i64 = 5 * 60 * 1000;

#[derive(Clone)]
pub struct SleepPermit {
    pub token: String,
    pub session_id: String,
    pub agent: String,
    pub project: String,
    pub checkout: String,
    pub old_runtime: String,
    pub task_key: String,
    pub role: Role,
    pub epoch: String,
    pub expires_at: i64,
    pub state: String,
    pub replacement_runtime: Option<String>,
    pub note_recorded: bool,
    /// "sleep", or "restart": a `cockpit restart` the host itself drives, with no sleep note.
    pub kind: String,
}

pub(super) fn migrate(connection: &Connection) -> Result<(), TeamError> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS sleep_permits (
        token TEXT PRIMARY KEY, session_id TEXT NOT NULL, agent TEXT NOT NULL, project TEXT NOT NULL,
        checkout TEXT NOT NULL, old_runtime TEXT NOT NULL, task_key TEXT NOT NULL, role TEXT NOT NULL,
        epoch TEXT NOT NULL, expires_at INTEGER NOT NULL, state TEXT NOT NULL,
        replacement_runtime TEXT, started_at INTEGER, error TEXT NOT NULL DEFAULT '', note_recorded INTEGER NOT NULL DEFAULT 0);
        CREATE UNIQUE INDEX IF NOT EXISTS one_sleep_continuation ON sleep_permits(session_id)
            WHERE state IN ('prepared','launch_requested','waking','failed');")?;
    let columns = connection.prepare("PRAGMA table_info(sleep_permits)")?.query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|name| name == "note_recorded") { connection.execute("ALTER TABLE sleep_permits ADD COLUMN note_recorded INTEGER NOT NULL DEFAULT 0", [])?; }
    if !columns.iter().any(|name| name == "kind") { connection.execute("ALTER TABLE sleep_permits ADD COLUMN kind TEXT NOT NULL DEFAULT 'sleep'", [])?; }
    Ok(())
}

fn read_permit(row: &rusqlite::Row<'_>) -> rusqlite::Result<SleepPermit> {
    let role: String = row.get(7)?;
    let role = role_of(&role).ok_or_else(|| rusqlite::Error::InvalidColumnType(7, "role".into(), rusqlite::types::Type::Text))?;
    Ok(SleepPermit { token: row.get(0)?, session_id: row.get(1)?, agent: row.get(2)?, project: row.get(3)?,
        checkout: row.get(4)?, old_runtime: row.get(5)?, task_key: row.get(6)?, role, epoch: row.get(8)?,
        expires_at: row.get(9)?, state: row.get(10)?, replacement_runtime: row.get(11)?, note_recorded: row.get(12)?, kind: row.get(13)? })
}

pub fn permit(connection: &Connection, token: &str) -> Result<Option<SleepPermit>, TeamError> {
    Ok(connection.query_row("SELECT token,session_id,agent,project,checkout,old_runtime,task_key,role,epoch,expires_at,state,replacement_runtime,note_recorded,kind
        FROM sleep_permits WHERE token=?1", [token], read_permit).optional()?)
}

fn membership_matches(connection: &Connection, permit: &SleepPermit, runtime: &str) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM members m JOIN teams t ON t.task_key=m.task_key
        WHERE m.session_id=?1 AND m.agent=?2 AND t.project=?3 AND m.runtime=?4 AND m.task_key=?5 AND m.role=?6 AND m.epoch=?7)",
        params![permit.session_id, permit.agent, permit.project, runtime, permit.task_key, role_name(permit.role), permit.epoch], |row| row.get(0))?)
}

pub fn paused(connection: &Connection, session: &str) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM sleep_permits p JOIN members m ON m.session_id=p.session_id
        AND m.task_key=p.task_key AND m.epoch=p.epoch AND m.runtime=p.old_runtime AND m.agent=p.agent AND m.role=p.role
        WHERE p.session_id=?1 AND p.state IN ('prepared','launch_requested','waking','failed'))", [session], |row| row.get(0))?)
}

pub(crate) fn guard_sender(connection: &Connection, session: &str) -> Result<(), TeamError> {
    if paused(connection, session)? { return Err(TeamError::new(ErrorCode::Conflict, "team member is prepared for sleep")); }
    Ok(())
}

pub fn prepare(connection: &mut Connection, member: &VerifiedSession, checkout: &str, now_ms: i64) -> Result<Option<String>, TeamError> {
    prepare_continuation(connection, member, checkout, now_ms, "sleep", LIFETIME_MS)
}

/// The daemon binds it only to a relaunch of the same session in `checkout`, once `old_runtime` died.
pub fn prepare_restart(connection: &mut Connection, member: &VerifiedSession, checkout: &str, now_ms: i64) -> Result<Option<String>, TeamError> {
    prepare_continuation(connection, member, checkout, now_ms, "restart", RESTART_LIFETIME_MS)
}

fn prepare_continuation(connection: &mut Connection, member: &VerifiedSession, checkout: &str, now_ms: i64, kind: &str, lifetime_ms: i64) -> Result<Option<String>, TeamError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let identity: Option<(String, String, String, String, String, String)> = transaction.query_row(
        "SELECT m.task_key,m.role,m.epoch,m.agent,m.runtime,t.project FROM members m JOIN teams t ON t.task_key=m.task_key WHERE m.session_id=?1",
        [&member.session_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))).optional()?;
    let Some((task_key, role, epoch, agent, runtime, project)) = identity else { return Ok(None) };
    if member.agent != agent || member.runtime != runtime || member.project != project || runtime.is_empty()
        || agents::of_session_id(&member.session_id).map(|known| known.name) != Some(agent.as_str()) {
        return Err(TeamError::new(ErrorCode::Unauthorized, "sleep identity does not match approved membership"));
    }
    if !checkout.starts_with('/') || checkout.chars().any(char::is_control) {
        return Err(TeamError::new(ErrorCode::Invalid, "sleep needs the verified original checkout"));
    }
    let existing: Option<String> = transaction.query_row("SELECT token FROM sleep_permits WHERE session_id=?1 AND state IN ('prepared','launch_requested','waking','failed')",
        [&member.session_id], |row| row.get(0)).optional()?;
    if let Some(token) = existing {
        let existing = permit(&transaction, &token)?.expect("selected permit");
        if existing.state == "prepared" && existing.kind == kind && existing.expires_at > now_ms && existing.checkout == checkout
            && membership_matches(&transaction, &existing, &runtime)? { return Ok(Some(token)); }
        return Err(TeamError::new(ErrorCode::Conflict, "sleep continuation already exists or expired; cancel it first"));
    }
    let pending: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE (scope_kind='link' OR (scope_kind='team' AND task_key=?1)) AND
        (sender=?2 OR recipient=?2) AND state IN ('queued','ready','submitting','uncertain'))
        OR EXISTS(SELECT 1 FROM chief_inbox WHERE recipient=?2 AND state IN ('queued','submitting','uncertain'))",
        params![task_key, member.session_id], |row| row.get(0))?;
    if pending { return Err(TeamError::new(ErrorCode::Conflict, "pending team delivery prevents sleep")); }
    let token = identifier()?;
    let expires_at = now_ms.checked_add(lifetime_ms).ok_or_else(|| TeamError::new(ErrorCode::Invalid, "invalid sleep time"))?;
    transaction.execute("INSERT INTO sleep_permits (token,session_id,agent,project,checkout,old_runtime,task_key,role,epoch,expires_at,state,kind)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'prepared',?11)",
        params![token, member.session_id, agent, project, checkout, runtime, task_key, role, epoch, expires_at, kind])?;
    transaction.commit()?;
    Ok(Some(token))
}

pub fn cancel(connection: &mut Connection, session: &str, token: &str) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(permit) = permit(&transaction, token)? else { return Ok(()) };
    if permit.session_id != session { return Ok(()); }
    let changed = transaction.execute("UPDATE sleep_permits SET state='cancelled' WHERE session_id=?1 AND token=?2
        AND state IN ('prepared','launch_requested','waking','failed')", params![session, token])?;
    if changed == 1 && membership_matches(&transaction, &permit, &permit.old_runtime)? {
        schedule_qa(&transaction, &permit.task_key, (crate::util::now() * 1000.0) as i64)?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn bind(connection: &mut Connection, token: &str, replacement: &VerifiedSession, checkout: &str, now_ms: i64) -> Result<bool, TeamError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(permit) = permit(&transaction, token)? else { return Ok(false) };
    if replacement.session_id != permit.session_id || replacement.agent != permit.agent || replacement.project != permit.project
        || checkout != permit.checkout || replacement.runtime.is_empty() || replacement.runtime == permit.old_runtime
        || agents::of_session_id(&replacement.session_id).map(|known| known.name) != Some(permit.agent.as_str()) { return Ok(false); }
    if permit.state == "bound" {
        return Ok(permit.replacement_runtime.as_deref() == Some(&replacement.runtime)
            && membership_matches(&transaction, &permit, &replacement.runtime)?);
    }
    if !matches!(permit.state.as_str(), "prepared" | "launch_requested" | "waking") || permit.expires_at <= now_ms
        || !membership_matches(&transaction, &permit, &permit.old_runtime)? { return Ok(false); }
    transaction.execute("UPDATE members SET runtime=?2,last_idle_turn=NULL WHERE session_id=?1 AND epoch=?3 AND runtime=?4",
        params![permit.session_id, replacement.runtime, permit.epoch, permit.old_runtime])?;
    transaction.execute("UPDATE messages SET recipient_runtime=?2 WHERE scope_kind='team' AND recipient=?1 AND task_key=?3 AND recipient_epoch=?4
        AND recipient_runtime=?5 AND state='queued'", params![permit.session_id, replacement.runtime, permit.task_key, permit.epoch, permit.old_runtime])?;
    transaction.execute("UPDATE sleep_permits SET state='bound',replacement_runtime=?2 WHERE token=?1", params![token, replacement.runtime])?;
    schedule_qa(&transaction, &permit.task_key, now_ms)?;
    transaction.commit()?;
    Ok(true)
}

/// Protected records for host note reconciliation; failed launches are included, never retried.
pub fn prepared_records(connection: &Connection) -> Result<Vec<SleepPermit>, TeamError> {
    let mut statement = connection.prepare("SELECT token,session_id,agent,project,checkout,old_runtime,task_key,role,epoch,expires_at,state,replacement_runtime,note_recorded,kind
        FROM sleep_permits WHERE state IN ('prepared','launch_requested','waking','failed') ORDER BY expires_at,token")?;
    Ok(statement.query_map([], read_permit)?.collect::<Result<_, _>>()?)
}

/// Host acknowledgement that the matching protected sleep note was durably written before exit.
pub fn record_note(connection: &mut Connection, session: &str, token: &str) -> Result<(), TeamError> {
    let changed = connection.execute("UPDATE sleep_permits SET note_recorded=1 WHERE session_id=?1 AND token=?2 AND state='prepared'", params![session, token])?;
    if changed != 1 { return Err(TeamError::new(ErrorCode::Conflict, "sleep preparation is no longer recordable")); }
    Ok(())
}

pub fn pending_wakes(connection: &Connection, now_ms: i64) -> Result<Vec<SleepPermit>, TeamError> {
    let mut wakes = Vec::new();
    for permit in prepared_records(connection)? {
        if permit.kind != "sleep" || permit.state != "prepared" || permit.expires_at <= now_ms || !membership_matches(connection, &permit, &permit.old_runtime)? { continue; }
        let deliverable: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM messages q
            JOIN members s ON s.task_key=q.task_key AND s.session_id=q.sender AND s.epoch=q.sender_epoch
            WHERE q.scope_kind='team' AND q.recipient=?1 AND q.task_key=?2 AND q.recipient_epoch=?3 AND q.recipient_runtime=?4
            AND q.state='queued' AND q.next_at<=?5 AND NOT EXISTS(SELECT 1 FROM messages u
                WHERE u.recipient=q.recipient AND u.recipient_runtime=q.recipient_runtime AND u.state IN ('submitting','uncertain')))",
            params![permit.session_id, permit.task_key, permit.epoch, permit.old_runtime, now_ms], |row| row.get(0))?;
        if deliverable { wakes.push(permit); }
    }
    Ok(wakes)
}

/// Reserve one automatic launch; false means already reserved or invalid.
pub fn reserve_wake(connection: &mut Connection, session: &str, token: &str, now_ms: i64) -> Result<bool, TeamError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(permit) = permit(&transaction, token)? else { return Ok(false) };
    if permit.session_id != session || permit.state != "prepared" || permit.expires_at <= now_ms
        || !membership_matches(&transaction, &permit, &permit.old_runtime)? { return Ok(false); }
    // Recheck at the writer boundary: a prior host snapshot is not wake authorization.
    if !pending_wakes(&transaction, now_ms)?.iter().any(|pending| pending.token == token) { return Ok(false); }
    transaction.execute("UPDATE sleep_permits SET state='launch_requested',started_at=?2 WHERE token=?1", params![token, now_ms])?;
    transaction.commit()?;
    Ok(true)
}

pub fn begin_wake(connection: &mut Connection, session: &str, token: &str, now_ms: i64) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let permit = permit(&transaction, token)?.ok_or_else(|| TeamError::new(ErrorCode::NotFound, "sleep continuation not found"))?;
    if permit.session_id != session || permit.expires_at <= now_ms || !matches!(permit.state.as_str(), "prepared" | "launch_requested")
        || !membership_matches(&transaction, &permit, &permit.old_runtime)? {
        return Err(TeamError::new(ErrorCode::Conflict, "sleep continuation is no longer launchable"));
    }
    transaction.execute("UPDATE sleep_permits SET state='waking',started_at=COALESCE(started_at,?2) WHERE token=?1", params![token, now_ms])?;
    transaction.commit()?;
    Ok(())
}

pub fn mark_failed(connection: &mut Connection, session: &str, token: &str, reason: &str) -> Result<(), TeamError> {
    connection.execute("UPDATE sleep_permits SET state='failed',error=?3 WHERE session_id=?1 AND token=?2
        AND state IN ('prepared','launch_requested','waking')", params![session, token, reason])?;
    Ok(())
}

pub fn expire_wakes(connection: &mut Connection, now_ms: i64) -> Result<(), TeamError> {
    connection.execute("UPDATE sleep_permits SET state='failed',error='wake acknowledgement timed out'
        WHERE state IN ('launch_requested','waking') AND started_at<=?1", [now_ms.saturating_sub(WAKE_TIMEOUT_MS)])?;
    Ok(())
}

pub fn status(connection: &Connection, session: &str, token: &str) -> Result<Value, TeamError> {
    let result: Option<Value> = connection.query_row("SELECT state,error,replacement_runtime FROM sleep_permits WHERE session_id=?1 AND token=?2",
        params![session, token], |row| Ok(json!({"state": row.get::<_, String>(0)?, "error": row.get::<_, String>(1)?,
            "replacement_runtime": row.get::<_, Option<String>>(2)?}))).optional()?;
    result.ok_or_else(|| TeamError::new(ErrorCode::NotFound, "sleep continuation not found"))
}
