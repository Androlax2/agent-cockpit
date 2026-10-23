//! Protected shared-work identity and schema; no link grants team membership.

use super::{ErrorCode, TaskKey, TeamError};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedWork {
    pub task: TaskKey,
    pub title: String,
    pub status: String,
}

/// Enroll new work inside the proposal's transaction. Existing closure evidence is never reset.
pub fn enroll_work(transaction: &Transaction<'_>, task: &TaskKey) -> Result<(SharedWork, i64), TeamError> {
    lock_work(transaction)?;
    let work = crate::todo::read_shared_work(task)?;
    let task_key = serde_json::to_string(task)?;
    let lifecycle = work_generation(transaction, task)?;
    if let Some((generation, state)) = lifecycle {
        if state != "open" { return Err(TeamError::new(ErrorCode::Conflict, "shared work is closed; explicitly reopen the TODO first")); }
        return Ok((work, generation));
    }
    refuse_lost_history(transaction, &task_key)?;
    transaction.execute("INSERT INTO link_work VALUES (?1,?2,?3,0,'open',?4)",
        params![task_key, task.project, task.task_id, crate::util::now_secs() * 1000])?;
    Ok((work, 0))
}

/// Approval, intake and final dispatch must call this in their committing transaction.
/// It never enrolls missing lifecycle rows or trusts an earlier TODO snapshot.
pub fn require_open_work(transaction: &Transaction<'_>, task: &TaskKey, generation: i64) -> Result<SharedWork, TeamError> {
    lock_work(transaction)?;
    match work_generation(transaction, task)? {
        Some((current, state)) if current == generation && state == "open" => crate::todo::read_shared_work(task),
        _ => Err(TeamError::new(ErrorCode::Conflict, "shared work lifecycle is closed, missing or stale")),
    }
}

fn lock_work(transaction: &Transaction<'_>) -> Result<(), TeamError> {
    // Also acquire the writer boundary if a future caller accidentally begins a deferred transaction.
    transaction.execute("UPDATE link_work SET updated_at=updated_at WHERE 0", [])?;
    Ok(())
}

fn work_generation(connection: &Connection, task: &TaskKey) -> Result<Option<(i64, String)>, TeamError> {
    Ok(connection.query_row("SELECT generation,state FROM link_work WHERE task_key=?1 AND project=?2 AND task_id=?3",
        params![serde_json::to_string(task)?, task.project, task.task_id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?)
}

fn refuse_lost_history(connection: &Connection, task_key: &str) -> Result<(), TeamError> {
    let has_history: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM link_proposals WHERE task_key=?1)
        OR EXISTS(SELECT 1 FROM links WHERE task_key=?1)", [task_key], |row| row.get(0))?;
    if has_history { return Err(TeamError::new(ErrorCode::Storage, "shared work lifecycle history is missing; approvals cannot be restored")); }
    Ok(())
}

/// Standalone host lifecycle closure. Use TodoList for JSON mutations so its guard spans both writes.
pub fn invalidate_work(connection: &mut Connection, task: &TaskKey, reason: &str) -> Result<(), TeamError> {
    let guard = crate::todo::lock_mutations()?;
    invalidate_work_guarded(connection, task, reason, &guard)
}

pub(crate) fn invalidate_work_guarded(connection: &mut Connection, task: &TaskKey, reason: &str, _guard: &crate::todo::MutationGuard) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let task_key = serde_json::to_string(task)?;
    if work_generation(&transaction, task)?.is_none() { refuse_lost_history(&transaction, &task_key)?; }
    let state = match reason { "done" => "done", "deleted" => "deleted", _ => "blocked" };
    let now = crate::util::now_secs() * 1000;
    transaction.execute("INSERT INTO link_work (task_key,project,task_id,generation,state,updated_at) VALUES (?1,?2,?3,1,?4,?5)
        ON CONFLICT(task_key) DO UPDATE SET generation=generation+1,state=excluded.state,updated_at=excluded.updated_at",
        params![task_key, task.project, task.task_id, state, now])?;
    transaction.execute("UPDATE links SET state='closed',closure_reason=?2,closed_at=?3 WHERE task_key=?1 AND state='active'",
        params![task_key, reason, now])?;
    transaction.execute("UPDATE link_proposals SET status='closed',closure_reason=?2 WHERE task_key=?1 AND status='pending'", params![task_key, reason])?;
    transaction.execute("UPDATE messages SET state='cancelled',last_error=?2 WHERE scope_kind='link'
        AND link_id IN (SELECT id FROM links WHERE task_key=?1) AND state IN ('queued','ready')", params![task_key, reason])?;
    transaction.commit()?;
    Ok(())
}

/// Recheck already durable open JSON. TodoList holds the same guard across its explicit reopen.
pub fn reopen_work(connection: &mut Connection, task: &TaskKey) -> Result<(), TeamError> {
    let guard = crate::todo::lock_mutations()?;
    reopen_work_guarded(connection, task, &guard)
}

pub(crate) fn reopen_work_guarded(connection: &mut Connection, task: &TaskKey, _guard: &crate::todo::MutationGuard) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    crate::todo::read_shared_work(task)?;
    let changed = transaction.execute("UPDATE link_work SET state='open',updated_at=?2 WHERE task_key=?1 AND project=?3 AND task_id=?4",
        params![serde_json::to_string(task)?, crate::util::now_secs() * 1000, task.project, task.task_id])?;
    if changed != 1 { return Err(TeamError::new(ErrorCode::Storage, "shared work lifecycle is missing")); }
    transaction.commit()?;
    Ok(())
}

/// Version 11 is atomic both standalone and within store initialization's transaction.
pub fn migrate(connection: &Connection) -> Result<(), TeamError> {
    let transaction = if connection.is_autocommit() { Some(connection.unchecked_transaction()?) } else { None };
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if !(10..=11).contains(&version) {
        return Err(TeamError::new(ErrorCode::Storage, "unsupported link database version"));
    }
    if version == 10 {
        connection.execute_batch("CREATE TABLE link_work (
            task_key TEXT PRIMARY KEY, project TEXT NOT NULL, task_id TEXT NOT NULL,
            generation INTEGER NOT NULL CHECK(generation>=0), state TEXT NOT NULL CHECK(state IN ('open','blocked','done','deleted')),
            updated_at INTEGER NOT NULL, UNIQUE(project,task_id));
        CREATE TABLE link_proposals (
            id TEXT PRIMARY KEY, sender TEXT NOT NULL, submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
            task_key TEXT NOT NULL REFERENCES link_work(task_key), purpose TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK(revision>0), mapping_hash TEXT NOT NULL,
            work_generation INTEGER NOT NULL CHECK(work_generation>=0),
            status TEXT NOT NULL CHECK(status IN ('pending','approved','closed','rejected')),
            closure_reason TEXT, created_at INTEGER NOT NULL, parent_id TEXT REFERENCES link_proposals(id),
            UNIQUE(sender,submission_key), UNIQUE(id,revision,mapping_hash));
        CREATE TABLE link_participants (
            proposal_id TEXT NOT NULL REFERENCES link_proposals(id), slot INTEGER NOT NULL CHECK(slot IN (0,1)),
            session_id TEXT NOT NULL, agent TEXT NOT NULL, project TEXT NOT NULL, runtime TEXT NOT NULL CHECK(length(runtime)>0),
            PRIMARY KEY(proposal_id,slot), UNIQUE(proposal_id,session_id));
        CREATE TABLE links (
            id TEXT PRIMARY KEY, proposal_id TEXT NOT NULL UNIQUE, revision INTEGER NOT NULL, mapping_hash TEXT NOT NULL,
            task_key TEXT NOT NULL REFERENCES link_work(task_key), work_generation INTEGER NOT NULL CHECK(work_generation>=0),
            session_low TEXT NOT NULL, session_high TEXT NOT NULL CHECK(session_low<session_high), epoch TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('active','closed')), closure_reason TEXT, created_at INTEGER NOT NULL, closed_at INTEGER,
            FOREIGN KEY(proposal_id,revision,mapping_hash) REFERENCES link_proposals(id,revision,mapping_hash));
        CREATE UNIQUE INDEX one_active_link_pair_work ON links(session_low,session_high,task_key) WHERE state='active';
        CREATE TRIGGER immutable_link_proposal BEFORE UPDATE OF id,sender,submission_key,fingerprint,task_key,purpose,revision,mapping_hash,work_generation,created_at,parent_id ON link_proposals
            BEGIN SELECT RAISE(ABORT,'link proposal revisions are immutable'); END;
        CREATE TRIGGER immutable_link_participant BEFORE UPDATE ON link_participants
            BEGIN SELECT RAISE(ABORT,'link participant identities are immutable'); END;
        CREATE TRIGGER retain_link_participant BEFORE DELETE ON link_participants
            BEGIN SELECT RAISE(ABORT,'link participant identities are durable'); END;
        CREATE TRIGGER sealed_link_participants BEFORE INSERT ON link_participants WHEN EXISTS(SELECT 1 FROM links WHERE proposal_id=NEW.proposal_id)
            BEGIN SELECT RAISE(ABORT,'approved link participants are sealed'); END;
        CREATE TRIGGER exact_link_approval BEFORE INSERT ON links WHEN NOT EXISTS (
            SELECT 1 FROM link_proposals p WHERE p.id=NEW.proposal_id AND p.revision=NEW.revision AND p.mapping_hash=NEW.mapping_hash
            AND p.task_key=NEW.task_key AND p.work_generation=NEW.work_generation
            AND (SELECT count(*) FROM link_participants WHERE proposal_id=p.id)=2
            AND EXISTS(SELECT 1 FROM link_participants WHERE proposal_id=p.id AND session_id=NEW.session_low)
            AND EXISTS(SELECT 1 FROM link_participants WHERE proposal_id=p.id AND session_id=NEW.session_high))
            BEGIN SELECT RAISE(ABORT,'link approval must bind its exact proposal and pair'); END;
        CREATE TRIGGER immutable_link_approval BEFORE UPDATE OF id,proposal_id,revision,mapping_hash,task_key,work_generation,session_low,session_high,epoch,created_at ON links
            BEGIN SELECT RAISE(ABORT,'link approval bindings are immutable'); END;
        CREATE TRIGGER retain_link_work BEFORE DELETE ON link_work
            BEGIN SELECT RAISE(ABORT,'shared work lifecycle is durable'); END;
        CREATE TRIGGER monotonic_link_work_generation BEFORE UPDATE OF generation ON link_work WHEN NEW.generation<OLD.generation
            BEGIN SELECT RAISE(ABORT,'shared work generation cannot decrease'); END;
        ALTER TABLE messages ADD COLUMN scope_kind TEXT NOT NULL DEFAULT 'team' CHECK(scope_kind IN ('team','link'));
        ALTER TABLE messages ADD COLUMN link_id TEXT REFERENCES links(id);
        PRAGMA user_version=11;")?;
    }
    if let Some(transaction) = transaction { transaction.commit()?; }
    Ok(())
}
