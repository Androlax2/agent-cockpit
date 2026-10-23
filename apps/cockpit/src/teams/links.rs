//! Protected shared-work identity and schema; no link grants team membership.

use super::{ErrorCode, TaskKey, TeamError};
use rusqlite::Connection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedWork {
    pub task: TaskKey,
    pub title: String,
    pub status: String,
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
