//! Authoritative team state lives beside the protected audit log, never in a writable inbox.

use super::{AgentAction, Dispatch, ErrorCode, HostAction, MessageKind, Proposal, Role, TaskKey, TeamError, TeamReply, TransportOutcome, VerifiedSession};
use crate::{agents, audit};
use ring::rand::{SecureRandom, SystemRandom};
pub use rusqlite::Connection;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct BoundMember {
    session_id: String,
    role: Role,
    agent: String,
    runtime: String,
}

fn identifier() -> Result<String, TeamError> {
    let mut bytes = [0u8; 16];
    SystemRandom::new().fill(&mut bytes).map_err(|_| TeamError::new(ErrorCode::Unavailable, "secure random source unavailable"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn open(path: &Path) -> Result<Connection, TeamError> {
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| TeamError::new(ErrorCode::Invalid, "team database needs a protected directory"))?;
    for component in path.ancestors() {
        match fs::symlink_metadata(component) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(TeamError::new(ErrorCode::Storage, "symlinked team state is refused")),
            Ok(_) => {},
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error.into()),
        }
    }
    fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
    if fs::metadata(parent)?.permissions().mode() & 0o077 != 0 {
        return Err(TeamError::new(ErrorCode::Storage, "team state directory must be private (0700)"));
    }
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600)
        .custom_flags(libc::O_NOFOLLOW).open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.permissions().mode() & 0o077 != 0 {
        return Err(TeamError::new(ErrorCode::Storage, "team database must be a private regular file"));
    }
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_millis(250))?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if !(0..=2).contains(&version) { return Err(TeamError::new(ErrorCode::Storage, "unsupported team database version")); }
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE IF NOT EXISTS teams (task_key TEXT PRIMARY KEY, project TEXT NOT NULL, task_id TEXT NOT NULL, revision INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS proposals (id TEXT PRIMARY KEY, sender TEXT NOT NULL, submission_key TEXT NOT NULL,
            fingerprint TEXT NOT NULL, task_key TEXT NOT NULL, project TEXT NOT NULL, task_id TEXT NOT NULL,
            mapping TEXT NOT NULL, mapping_hash TEXT NOT NULL, reason TEXT NOT NULL, revision INTEGER NOT NULL,
            status TEXT NOT NULL, created_at INTEGER NOT NULL, parent_id TEXT, created_by TEXT NOT NULL DEFAULT 'agent', UNIQUE(sender, submission_key));
        CREATE TABLE IF NOT EXISTS members (task_key TEXT NOT NULL REFERENCES teams(task_key), session_id TEXT NOT NULL UNIQUE,
            role TEXT NOT NULL, agent TEXT NOT NULL, runtime TEXT NOT NULL, epoch TEXT NOT NULL, last_idle_turn TEXT, PRIMARY KEY(task_key, role));
        CREATE TABLE IF NOT EXISTS host_actions (request_id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, response TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
            task_key TEXT NOT NULL, sender TEXT NOT NULL, recipient TEXT NOT NULL, sender_epoch TEXT NOT NULL, recipient_epoch TEXT NOT NULL,
            submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL, envelope TEXT NOT NULL, state TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL, created_at INTEGER NOT NULL,
            last_error TEXT NOT NULL DEFAULT '', idle_turn TEXT, recipient_runtime TEXT NOT NULL, UNIQUE(sender,submission_key));
        CREATE TABLE IF NOT EXISTS attempts (id TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES messages(id),
            number INTEGER NOT NULL, state TEXT NOT NULL, native_id TEXT, finished_at INTEGER, started_at INTEGER NOT NULL DEFAULT 0,
            UNIQUE(message_id,number));
        CREATE TABLE IF NOT EXISTS threads (id TEXT PRIMARY KEY, task_key TEXT NOT NULL, asker TEXT NOT NULL, answerer TEXT NOT NULL,
            round INTEGER NOT NULL, status TEXT NOT NULL, created_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS escalations (thread_id TEXT PRIMARY KEY REFERENCES threads(id), task_key TEXT NOT NULL, reason TEXT NOT NULL,
            status TEXT NOT NULL, note TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL, resolved_at INTEGER);
        CREATE TABLE IF NOT EXISTS handoffs (id TEXT PRIMARY KEY, task_key TEXT NOT NULL, developer TEXT NOT NULL, developer_epoch TEXT NOT NULL,
            submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL, payload TEXT NOT NULL, status TEXT NOT NULL, created_at INTEGER NOT NULL,
            UNIQUE(developer, submission_key));
        CREATE TABLE IF NOT EXISTS spawns (token TEXT PRIMARY KEY, task_key TEXT NOT NULL, role TEXT NOT NULL, agent TEXT NOT NULL,
            directory TEXT NOT NULL, commit_id TEXT, brief TEXT NOT NULL, handoff_id TEXT, status TEXT NOT NULL, session_id TEXT,
            created_at INTEGER NOT NULL, started_at INTEGER);
        CREATE TABLE IF NOT EXISTS team_qa (task_key TEXT PRIMARY KEY, brief TEXT NOT NULL, agent TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS qa_exits (session_id TEXT PRIMARY KEY, status TEXT NOT NULL);
        CREATE UNIQUE INDEX IF NOT EXISTS one_inflight_delivery ON messages(recipient,recipient_epoch) WHERE state='submitting';")?;
    // Version 2: a proposal can carry recruits (teammates to start on approval) and the agent they run in.
    let columns: Vec<String> = connection.prepare("SELECT name FROM pragma_table_info('proposals')")?
        .query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
    // One transaction, each column checked on its own: a crash midway leaves nothing half done.
    let mut migration = String::from("BEGIN IMMEDIATE;");
    for (column, definition) in [("recruits", "TEXT NOT NULL DEFAULT '[]'"), ("spawn_agent", "TEXT NOT NULL DEFAULT ''")] {
        if !columns.iter().any(|existing| existing == column) {
            migration += &format!("ALTER TABLE proposals ADD COLUMN {column} {definition};");
        }
    }
    connection.execute_batch(&(migration + "PRAGMA user_version=2; COMMIT;"))?;
    Ok(connection)
}

fn bind(proposal: &Proposal, live: &[VerifiedSession]) -> Result<Vec<BoundMember>, TeamError> {
    let invalid = || TeamError::new(ErrorCode::Invalid, "invalid or unavailable team mapping");
    let issue = regex::Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+#[1-9][0-9]*$").expect("constant issue pattern");
    let typed = proposal.task.task_id.len() == 8 && proposal.task.task_id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if proposal.task.project.is_empty() || proposal.task.project.len() > 300 || proposal.task.project.chars().any(char::is_control)
        || (!typed && !issue.is_match(&proposal.task.task_id)) || proposal.task.task_id.len() > 256
        || proposal.members.is_empty() || proposal.members.len() > 3 || proposal.reason.trim().is_empty()
        || proposal.reason.chars().count() > 1000 || proposal.reason.chars().any(char::is_control)
        || proposal.submission_key.is_empty() || proposal.submission_key.len() > 128
        || !proposal.submission_key.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) {
        return Err(invalid());
    }
    let (mut roles, mut sessions, mut bound) = (BTreeSet::new(), BTreeSet::new(), Vec::new());
    for member in &proposal.members {
        if !roles.insert(member.role) || !sessions.insert(&member.session_id) { return Err(invalid()); }
        let matches: Vec<_> = live.iter().filter(|session| session.session_id == member.session_id).collect();
        let [session] = matches.as_slice() else { return Err(invalid()) };
        if session.project != proposal.task.project || session.runtime.is_empty()
            || agents::of_session_id(&session.session_id).map(|agent| agent.name) != Some(session.agent.as_str()) {
            return Err(invalid());
        }
        bound.push(BoundMember { session_id: session.session_id.clone(), role: member.role,
            agent: session.agent.clone(), runtime: session.runtime.clone() });
    }
    bound.sort_by_key(|member| member.role);
    Ok(bound)
}

fn role_name(role: Role) -> &'static str {
    match role { Role::Designer => "designer", Role::Developer => "developer", Role::Qa => "qa" }
}

pub fn propose(connection: &mut Connection, sender: &VerifiedSession, proposal: &Proposal, live: &[VerifiedSession], now_ms: i64) -> Result<TeamReply, TeamError> {
    if sender.project != proposal.task.project || !live.contains(sender) {
        return Err(TeamError::new(ErrorCode::Unauthorized, "team request is not from this task's project"));
    }
    let bound = bind(proposal, live)?;
    let mapping = serde_json::to_string(&bound)?;
    let fingerprint = audit::sha256_hex(serde_json::to_string(proposal)?.as_bytes());
    let transaction = connection.transaction()?;
    let previous: Option<(String, String, String, i64)> = transaction.query_row(
        "SELECT id, fingerprint, mapping_hash, revision FROM proposals WHERE sender=?1 AND submission_key=?2",
        params![sender.session_id, proposal.submission_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
    if let Some((id, old_fingerprint, mapping_hash, revision)) = previous {
        if old_fingerprint != fingerprint { return Err(TeamError::new(ErrorCode::Conflict, "submission key already names different contents")); }
        return Ok(TeamReply { request_id: proposal.submission_key.clone(), result: Ok(json!({"proposal_id": id, "revision": revision, "mapping_hash": mapping_hash})) });
    }
    let recent: i64 = transaction.query_row("SELECT count(*) FROM proposals WHERE sender=?1 AND created_at>?2", params![sender.session_id, now_ms - 60_000], |row| row.get(0))?;
    if recent >= 10 { return Err(TeamError::new(ErrorCode::Limit, "team request rate limit reached")); }
    let id = identifier()?;
    let task_key = serde_json::to_string(&proposal.task)?;
    let mapping_hash = audit::sha256_hex(format!("{task_key}\n{mapping}").as_bytes());
    transaction.execute("INSERT INTO proposals (id,sender,submission_key,fingerprint,task_key,project,task_id,mapping,mapping_hash,reason,revision,status,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,'pending',?11)",
        params![id, sender.session_id, proposal.submission_key, fingerprint, task_key, proposal.task.project, proposal.task.task_id, mapping, mapping_hash, proposal.reason, now_ms])?;
    transaction.commit()?;
    Ok(TeamReply { request_id: proposal.submission_key.clone(), result: Ok(json!({"proposal_id": id, "revision": 1, "mapping_hash": mapping_hash})) })
}

fn valid_brief(brief: &str) -> bool {
    !brief.trim().is_empty() && brief.chars().count() <= 2000 && !brief.chars().any(|character| character.is_control() && character != '\n')
}

/// An agent asks for missing teammates. Pending like any request: only the asker is bound now; the
/// recruits, their briefs and the agent they run in are part of what Theo approves (the hash).
pub fn recruit(connection: &mut Connection, sender: &VerifiedSession, recruitment: &super::Recruitment, agent: &str,
               live: &[VerifiedSession], now_ms: i64) -> Result<TeamReply, TeamError> {
    if sender.project != recruitment.task.project || !live.contains(sender) {
        return Err(TeamError::new(ErrorCode::Unauthorized, "team request is not from this task's project"));
    }
    let mut roles = BTreeSet::from([recruitment.role]);
    if recruitment.recruits.is_empty() || agent.is_empty() || agents::find(agent).is_none()
        || !recruitment.recruits.iter().all(|recruit| roles.insert(recruit.role) && valid_brief(&recruit.brief)) {
        return Err(TeamError::new(ErrorCode::Invalid, "a recruit request names new roles other than its own, each with a 1–2000 character brief"));
    }
    let task_key = serde_json::to_string(&recruitment.task)?;
    let own_task: Option<(String, String)> = connection.query_row("SELECT task_key, role FROM members WHERE session_id=?1", [&sender.session_id],
        |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    if own_task.as_ref().is_some_and(|(own_key, own_role)| *own_key != task_key || own_role != role_name(recruitment.role)) {
        return Err(TeamError::new(ErrorCode::Conflict, "this session already has another task or role in a team"));
    }
    let occupied: Vec<String> = connection.prepare("SELECT role FROM members WHERE task_key=?1 AND session_id<>?2")?
        .query_map(params![task_key, sender.session_id], |row| row.get(0))?.collect::<Result<_, _>>()?;
    if roles.iter().any(|role| occupied.iter().any(|taken| taken == role_name(*role))) {
        return Err(TeamError::new(ErrorCode::Conflict, "the team already has one of these roles; ask only for the missing ones"));
    }
    let asker = Proposal { submission_key: recruitment.submission_key.clone(), task: recruitment.task.clone(), reason: recruitment.reason.clone(),
        members: vec![super::Member { session_id: sender.session_id.clone(), role: recruitment.role }] };
    let mapping = serde_json::to_string(&bind(&asker, live)?)?;
    let recruits = serde_json::to_string(&recruitment.recruits)?;
    let fingerprint = audit::sha256_hex(format!("{}\n{agent}", serde_json::to_string(recruitment)?).as_bytes());
    let transaction = connection.transaction()?;
    let previous: Option<(String, String, String)> = transaction.query_row(
        "SELECT id, fingerprint, mapping_hash FROM proposals WHERE sender=?1 AND submission_key=?2",
        params![sender.session_id, recruitment.submission_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
    if let Some((id, old_fingerprint, mapping_hash)) = previous {
        if old_fingerprint != fingerprint { return Err(TeamError::new(ErrorCode::Conflict, "submission key already names different contents")); }
        return Ok(TeamReply { request_id: recruitment.submission_key.clone(), result: Ok(json!({"proposal_id": id, "revision": 1, "mapping_hash": mapping_hash})) });
    }
    let recent: i64 = transaction.query_row("SELECT count(*) FROM proposals WHERE sender=?1 AND created_at>?2", params![sender.session_id, now_ms - 60_000], |row| row.get(0))?;
    if recent >= 10 { return Err(TeamError::new(ErrorCode::Limit, "team request rate limit reached")); }
    let id = identifier()?;
    let mapping_hash = audit::sha256_hex(format!("{task_key}\n{mapping}\n{recruits}\n{agent}").as_bytes());
    transaction.execute("INSERT INTO proposals (id,sender,submission_key,fingerprint,task_key,project,task_id,mapping,mapping_hash,reason,revision,status,created_at,recruits,spawn_agent)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,'pending',?11,?12,?13)",
        params![id, sender.session_id, recruitment.submission_key, fingerprint, task_key, recruitment.task.project, recruitment.task.task_id,
            mapping, mapping_hash, recruitment.reason, now_ms, recruits, agent])?;
    transaction.commit()?;
    Ok(TeamReply { request_id: recruitment.submission_key.clone(), result: Ok(json!({"proposal_id": id, "revision": 1, "mapping_hash": mapping_hash})) })
}

pub fn control(connection: &mut Connection, action: &HostAction, live: &[VerifiedSession], now_ms: i64) -> Result<TeamReply, TeamError> {
    control_with(connection, action, live, &HashMap::new(), now_ms)
}

/// `directories`: the live sessions' folders (session id → cwd), where approved recruits start.
pub fn control_with(connection: &mut Connection, action: &HostAction, live: &[VerifiedSession], directories: &HashMap<String, String>,
                    now_ms: i64) -> Result<TeamReply, TeamError> {
    let request_id = match action {
        HostAction::Approve { request_id, .. } | HostAction::Edit { request_id, .. }
        | HostAction::Reject { request_id, .. } | HostAction::Unlink { request_id, .. } | HostAction::Resolve { request_id, .. }
        | HostAction::Propose { request_id, .. } | HostAction::Reconcile { request_id, .. } => request_id,
    };
    let fingerprint = audit::sha256_hex(serde_json::to_string(action)?.as_bytes());
    let transaction = connection.transaction()?;
    let prior: Option<(String, String)> = transaction.query_row("SELECT fingerprint, response FROM host_actions WHERE request_id=?1", [request_id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    if let Some((old_fingerprint, response)) = prior {
        if fingerprint != old_fingerprint { return Err(TeamError::new(ErrorCode::Conflict, "host request id already used")); }
        return Ok(TeamReply { request_id: request_id.clone(), result: Ok(serde_json::from_str(&response)?) });
    }
    let response = match action {
        HostAction::Approve { proposal_id, expected_revision, mapping_hash, .. } => approve_mapping(&transaction, proposal_id, *expected_revision, mapping_hash, live, directories, now_ms)?,
        HostAction::Edit { proposal_id, expected_revision, replacement, .. } => {
            let old: Option<(String, String, i64, String, String, String)> = transaction.query_row(
                "SELECT sender, project, revision, status, recruits, spawn_agent FROM proposals WHERE id=?1", [proposal_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))).optional()?;
            let Some((sender, project, revision, status, recruits, spawn_agent)) = old else { return Err(TeamError::new(ErrorCode::NotFound, "team request not found")); };
            if revision != *expected_revision || status != "pending" || replacement.task.project != project {
                return Err(TeamError::new(ErrorCode::Conflict, "team request changed or belongs to another project"));
            }
            let mapping = serde_json::to_string(&bind(replacement, live)?)?;
            let task_key = serde_json::to_string(&replacement.task)?;
            let mapping_hash = if spawn_agent.is_empty() { audit::sha256_hex(format!("{task_key}\n{mapping}").as_bytes()) }
                               else { audit::sha256_hex(format!("{task_key}\n{mapping}\n{recruits}\n{spawn_agent}").as_bytes()) };
            let id = identifier()?;
            transaction.execute("UPDATE proposals SET status='superseded' WHERE id=?1", [proposal_id])?;
            transaction.execute("INSERT INTO proposals (id,sender,submission_key,fingerprint,task_key,project,task_id,mapping,mapping_hash,reason,revision,status,created_at,parent_id,created_by,recruits,spawn_agent)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'pending',?12,?13,'host',?14,?15)",
                params![id,sender,format!("host-edit-{}",identifier()?),fingerprint,task_key,project,replacement.task.task_id,mapping,mapping_hash,replacement.reason,revision+1,now_ms,proposal_id,recruits,spawn_agent])?;
            json!({"proposal_id":id,"revision":revision+1,"mapping_hash":mapping_hash})
        },
        HostAction::Reject { proposal_id, expected_revision, .. } => {
            let changed = transaction.execute("UPDATE proposals SET status='rejected' WHERE id=?1 AND revision=?2 AND status='pending'", params![proposal_id,expected_revision])?;
            if changed != 1 { return Err(TeamError::new(ErrorCode::Conflict, "team request changed; review it again")); }
            json!({"proposal_id":proposal_id,"status":"rejected"})
        },
        HostAction::Unlink { task, session_id, .. } => {
            let task_key = serde_json::to_string(task)?;
            cancel_member_messages(&transaction, &task_key, session_id)?;
            if transaction.execute("DELETE FROM members WHERE task_key=?1 AND session_id=?2", params![task_key,session_id])? != 1 {
                return Err(TeamError::new(ErrorCode::NotFound, "that session is not a member of this task's team"));
            }
            transaction.execute("UPDATE teams SET revision=revision+1 WHERE task_key=?1", [&task_key])?;
            json!({"session_id":session_id,"status":"unlinked"})
        },
        HostAction::Resolve { thread_id, note, .. } => {
            if note.trim().is_empty() || note.chars().count() > 1000 || note.chars().any(|character| character.is_control() && character != '\n') {
                return Err(TeamError::new(ErrorCode::Invalid, "a resolution note is 1–1000 characters without controls"));
            }
            let changed = transaction.execute("UPDATE escalations SET status='resolved', note=?2, resolved_at=?3 WHERE thread_id=?1 AND status='open'",
                params![thread_id, note, now_ms])?;
            if changed != 1 { return Err(TeamError::new(ErrorCode::Conflict, "no open escalation for this thread")); }
            transaction.execute("UPDATE threads SET status='resolved' WHERE id=?1", [thread_id])?;
            json!({"thread_id": thread_id, "status": "resolved"})
        },
        HostAction::Propose { proposal, .. } => {
            let mapping = serde_json::to_string(&bind(proposal, live)?)?;
            let task_key = serde_json::to_string(&proposal.task)?;
            let mapping_hash = audit::sha256_hex(format!("{task_key}\n{mapping}").as_bytes());
            let id = identifier()?;
            transaction.execute("INSERT INTO proposals (id,sender,submission_key,fingerprint,task_key,project,task_id,mapping,mapping_hash,reason,revision,status,created_at,parent_id,created_by)
                VALUES (?1,'host',?2,?3,?4,?5,?6,?7,?8,?9,1,'pending',?10,NULL,'host')",
                params![id, format!("host-{id}"), fingerprint, task_key, proposal.task.project, proposal.task.task_id, mapping, mapping_hash, proposal.reason, now_ms])?;
            json!({"proposal_id": id, "revision": 1, "mapping_hash": mapping_hash})
        },
        HostAction::Reconcile { message_id, decision, .. } => {
            let state = match decision.as_str() { "handled" => "received", "cancel" => "cancelled", _ => {
                return Err(TeamError::new(ErrorCode::Invalid, "reconcile with handled or cancel"));
            } };
            let changed = transaction.execute("UPDATE messages SET state=?2, last_error='reconciled by Theo' WHERE id=?1 AND state='uncertain'",
                params![message_id, state])?;
            if changed != 1 { return Err(TeamError::new(ErrorCode::Conflict, "only an uncertain delivery can be reconciled")); }
            transaction.execute("UPDATE attempts SET state=?2, finished_at=?3 WHERE message_id=?1 AND state='uncertain'",
                params![message_id, if state == "received" { "received" } else { "not_submitted" }, now_ms])?;
            json!({"message_id": message_id, "state": state})
        },
    };
    transaction.execute("INSERT INTO host_actions VALUES (?1,?2,?3)", params![request_id, fingerprint, response.to_string()])?;
    transaction.commit()?;
    Ok(TeamReply { request_id: request_id.clone(), result: Ok(response) })
}

fn approve_mapping(transaction: &rusqlite::Transaction<'_>, proposal_id: &str, expected_revision: i64, mapping_hash: &str, live: &[VerifiedSession],
                   directories: &HashMap<String, String>, now_ms: i64) -> Result<Value, TeamError> {
    let stored: Option<(String, String, String, String, String, i64, String, String)> = transaction.query_row(
        "SELECT task_key, project, task_id, mapping, mapping_hash, revision, status, reason FROM proposals WHERE id=?1", [proposal_id],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?))).optional()?;
    let Some((task_key, project, task_id, mapping, stored_hash, revision, status, reason)) = stored else {
        return Err(TeamError::new(ErrorCode::NotFound, "team request not found"));
    };
    if status != "pending" || revision != expected_revision || stored_hash != mapping_hash {
        return Err(TeamError::new(ErrorCode::Conflict, "team request changed; review its exact mapping again"));
    }
    let bound: Vec<BoundMember> = serde_json::from_str(&mapping)?;
    let proposal = Proposal { submission_key: "host-validation".into(), task: TaskKey { project: project.clone(), task_id: task_id.clone() },
        reason, members: bound.iter().map(|member| super::Member { session_id: member.session_id.clone(), role: member.role }).collect() };
    if bind(&proposal, live)? != bound { return Err(TeamError::new(ErrorCode::Conflict, "a requested session runtime changed; request fresh approval")); }
    transaction.execute("INSERT INTO teams VALUES (?1,?2,?3,1) ON CONFLICT(task_key) DO UPDATE SET revision=revision+1", params![task_key, project, task_id])?;
    let existing: Vec<(String, String, String)> = {
        let mut statement = transaction.prepare("SELECT session_id, role, runtime FROM members WHERE task_key=?1")?;
        statement.query_map([&task_key], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?.collect::<Result<_,_>>()?
    };
    // A link request names the whole team; a recruit request names only its asker and adds to the team.
    let is_recruit: bool = transaction.query_row("SELECT spawn_agent<>'' FROM proposals WHERE id=?1", [proposal_id], |row| row.get(0))?;
    if is_recruit { recruit_roles_still_free(transaction, proposal_id, &task_key, &bound)?; }
    for (session, role, runtime) in existing {
        if !is_recruit && !bound.iter().any(|member| member.session_id == session && role_name(member.role) == role && member.runtime == runtime) {
            cancel_member_messages(transaction, &task_key, &session)?;
            transaction.execute("DELETE FROM members WHERE task_key=?1 AND session_id=?2", params![task_key, session])?;
        }
    }
    for member in bound {
        let old_task: Option<String> = transaction.query_row("SELECT task_key FROM members WHERE session_id=?1", [&member.session_id], |row| row.get(0)).optional()?;
        if old_task.as_ref().is_some_and(|old| old != &task_key) { return Err(TeamError::new(ErrorCode::Conflict, "a requested session already belongs to another task")); }
        transaction.execute("INSERT INTO members (task_key,session_id,role,agent,runtime,epoch) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(task_key,role) DO NOTHING",
            params![task_key, member.session_id, role_name(member.role), member.agent, member.runtime, identifier()?])?;
    }
    queue_recruits(transaction, proposal_id, &task_key, directories, now_ms)?;
    transaction.execute("UPDATE proposals SET status='approved' WHERE id=?1", [proposal_id])?;
    schedule_qa(transaction, &task_key, now_ms)?;
    Ok(json!({"proposal_id": proposal_id, "status": "approved"}))
}

/// Roles can be taken between the request and Theo's yes (another request, a link): review again then.
fn recruit_roles_still_free(transaction: &Connection, proposal_id: &str, task_key: &str, bound: &[BoundMember]) -> Result<(), TeamError> {
    let recruits: String = transaction.query_row("SELECT recruits FROM proposals WHERE id=?1", [proposal_id], |row| row.get(0))?;
    let recruits: Vec<super::Recruit> = serde_json::from_str(&recruits)?;
    let taken = |role: Role, except: Option<&str>| -> Result<bool, TeamError> {
        let holder: Option<String> = transaction.query_row("SELECT session_id FROM members WHERE task_key=?1 AND role=?2",
            params![task_key, role_name(role)], |row| row.get(0)).optional()?;
        let starting: i64 = transaction.query_row("SELECT count(*) FROM spawns WHERE task_key=?1 AND role=?2 AND status IN ('pending','started')",
            params![task_key, role_name(role)], |row| row.get(0))?;
        Ok(holder.is_some_and(|holder| Some(holder.as_str()) != except) || (role != Role::Qa && starting > 0))
    };
    let asker_taken = bound.iter().map(|member| taken(member.role, Some(&member.session_id))).collect::<Result<Vec<_>, _>>()?.into_iter().any(|is| is);
    let recruit_taken = recruits.iter().map(|recruit| taken(recruit.role, None)).collect::<Result<Vec<_>, _>>()?.into_iter().any(|is| is);
    if asker_taken || recruit_taken {
        return Err(TeamError::new(ErrorCode::Conflict, "a role of this request was taken meanwhile; the agent must ask again"));
    }
    Ok(())
}

/// What an approved recruit request starts: its Designer and Developer now, in the asker's folder;
/// a Qa recruit becomes the team's standing QA brief, a fresh QA starting at each handoff.
fn queue_recruits(transaction: &Connection, proposal_id: &str, task_key: &str, directories: &HashMap<String, String>, now_ms: i64) -> Result<(), TeamError> {
    let (sender, recruits, agent, mapping): (String, String, String, String) = transaction.query_row(
        "SELECT sender, recruits, spawn_agent, mapping FROM proposals WHERE id=?1", [proposal_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
    let asker_role = serde_json::from_str::<Vec<BoundMember>>(&mapping)?.first().map(|member| role_name(member.role)).unwrap_or("asking");
    let recruits: Vec<super::Recruit> = serde_json::from_str(&recruits)?;
    if recruits.is_empty() { return Ok(()); }
    let directory = directories.get(&sender).ok_or_else(|| TeamError::new(ErrorCode::Conflict, "the asking session is no longer live"))?;
    for recruit in recruits {
        if recruit.role == Role::Qa {
            transaction.execute("INSERT INTO team_qa VALUES (?1,?2,?3) ON CONFLICT(task_key) DO UPDATE SET brief=?2, agent=?3",
                params![task_key, recruit.brief, agent])?;
            continue;
        }
        transaction.execute("INSERT INTO spawns (token,task_key,role,agent,directory,brief,status,created_at) VALUES (?1,?2,?3,?4,?5,?6,'pending',?7)",
            params![identifier()?, task_key, role_name(recruit.role), agent, directory,
                format!("Brief written by your team's {asker_role} agent and approved by Theo:\n{}", recruit.brief), now_ms])?;
    }
    Ok(())
}

/// A session the cockpit is asked to start for an approved team.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawn {
    pub token: String,
    pub role: Role,
    pub agent: String,
    pub directory: String,
    pub commit_id: Option<String>,
    pub brief: String,
}

fn role_of(name: &str) -> Option<Role> {
    match name { "designer" => Some(Role::Designer), "developer" => Some(Role::Developer), "qa" => Some(Role::Qa), _ => None }
}

pub fn pending_spawns(connection: &Connection) -> Result<Vec<Spawn>, TeamError> {
    let mut statement = connection.prepare("SELECT token, role, agent, directory, commit_id, brief FROM spawns WHERE status='pending' ORDER BY created_at, token")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().filter_map(|(token, role, agent, directory, commit_id, brief)|
        Some(Spawn { token, role: role_of(&role)?, agent, directory, commit_id, brief })).collect())
}

pub fn mark_spawn_started(connection: &Connection, token: &str, now_ms: i64) -> Result<(), TeamError> {
    connection.execute("UPDATE spawns SET status='started', started_at=?2 WHERE token=?1 AND status='pending'", params![token, now_ms])?;
    Ok(())
}

/// A started session that never reported is Theo's to look at: never started a second time.
pub fn expire_spawns(connection: &Connection, now_ms: i64) -> Result<(), TeamError> {
    connection.execute("UPDATE spawns SET status='failed' WHERE status='started' AND started_at<?1", [now_ms - 300_000])?;
    Ok(())
}

/// The first verified session whose launch carried `token` takes its role, once. A session of another
/// project, or already in another team, takes nothing. A start that was superseded (a newer handoff)
/// or whose role was taken meanwhile is refused, and its session queued to close.
pub fn bind_spawn(connection: &mut Connection, token: &str, session: &VerifiedSession, now_ms: i64) -> Result<bool, TeamError> {
    let transaction = connection.transaction()?;
    let spawn: Option<(String, String, String)> = transaction.query_row("SELECT task_key, role, status FROM spawns WHERE token=?1 AND status IN ('started','superseded')",
        [token], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
    let Some((task_key, role, status)) = spawn else { return Ok(false) };
    let project: String = transaction.query_row("SELECT project FROM teams WHERE task_key=?1", [&task_key], |row| row.get(0))?;
    let elsewhere: Option<String> = transaction.query_row("SELECT task_key FROM members WHERE session_id=?1", [&session.session_id], |row| row.get(0)).optional()?;
    if project != session.project || elsewhere.is_some() || session.runtime.is_empty() { return Ok(false); }
    let is_bound = status == "started" && transaction.execute("INSERT INTO members (task_key,session_id,role,agent,runtime,epoch) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(task_key,role) DO NOTHING",
        params![task_key, session.session_id, role, session.agent, session.runtime, identifier()?])? == 1;
    if is_bound {
        transaction.execute("UPDATE spawns SET status='bound', session_id=?2 WHERE token=?1", params![token, session.session_id])?;
        transaction.execute("UPDATE teams SET revision=revision+1 WHERE task_key=?1", [&task_key])?;
        schedule_qa(&transaction, &task_key, now_ms)?;
    } else {
        transaction.execute("UPDATE spawns SET status=CASE status WHEN 'superseded' THEN 'superseded' ELSE 'refused' END, session_id=?2 WHERE token=?1",
            params![token, session.session_id])?;
        transaction.execute("INSERT INTO qa_exits VALUES (?1,'pending') ON CONFLICT(session_id) DO UPDATE SET status='pending'", [&session.session_id])?;
    }
    transaction.commit()?;
    Ok(is_bound)
}

/// Actionable rows (pending requests, open escalations, uncertain or failed deliveries) always, plus the
/// newest `limit` of each history.
pub fn snapshot(connection: &Connection, task: Option<&TaskKey>, after: Option<i64>, limit: usize) -> Result<Value, TeamError> {
    let task_key = task.map(serde_json::to_string).transpose()?;
    let mut members = connection.prepare("SELECT m.session_id,m.role,m.agent,m.runtime,m.epoch,t.project,t.task_id FROM members m JOIN teams t ON t.task_key=m.task_key WHERE (?1 IS NULL OR m.task_key=?1) ORDER BY t.task_key,m.role")?;
    let members = members.query_map(params![task_key], |row| Ok(json!({"session_id":row.get::<_,String>(0)?,"role":row.get::<_,String>(1)?,"agent":row.get::<_,String>(2)?,
        "runtime":row.get::<_,String>(3)?,"epoch":row.get::<_,String>(4)?,"project":row.get::<_,String>(5)?,"task_id":row.get::<_,String>(6)?})))?.collect::<Result<Vec<_>,_>>()?;
    let mut proposals = connection.prepare("SELECT id,project,task_id,mapping,mapping_hash,revision,status,reason,sender,created_by,parent_id,recruits,spawn_agent FROM proposals WHERE (?1 IS NULL OR task_key=?1)
        AND (status='pending' OR id IN (SELECT id FROM proposals WHERE (?1 IS NULL OR task_key=?1) ORDER BY created_at DESC, id DESC LIMIT ?2))
        ORDER BY created_at,id")?;
    let proposals = proposals.query_map(params![task_key, limit.min(100) as i64], |row| Ok(json!({"proposal_id":row.get::<_,String>(0)?,"project":row.get::<_,String>(1)?,"task_id":row.get::<_,String>(2)?,
        "mapping":row.get::<_,String>(3)?,"mapping_hash":row.get::<_,String>(4)?,"revision":row.get::<_,i64>(5)?,"status":row.get::<_,String>(6)?,"reason":row.get::<_,String>(7)?,"sender":row.get::<_,String>(8)?,"created_by":row.get::<_,String>(9)?,"parent_id":row.get::<_,Option<String>>(10)?,"recruits":row.get::<_,String>(11)?,"spawn_agent":row.get::<_,String>(12)?})))?.collect::<Result<Vec<_>,_>>()?;
    let mut messages = connection.prepare("SELECT seq,id,envelope,state,attempts,next_at,last_error FROM messages WHERE (?1 IS NULL OR task_key=?1) AND (?2 IS NULL OR seq>?2)
        AND (state IN ('uncertain','failed') OR seq IN (SELECT seq FROM messages WHERE (?1 IS NULL OR task_key=?1) AND (?2 IS NULL OR seq>?2) ORDER BY seq DESC LIMIT ?3))
        ORDER BY seq")?;
    let messages = messages.query_map(params![task_key, after, limit.min(100) as i64], |row| Ok(json!({"seq":row.get::<_,i64>(0)?,"message_id":row.get::<_,String>(1)?,
        "envelope":row.get::<_,String>(2)?,"state":row.get::<_,String>(3)?,"attempts":row.get::<_,i64>(4)?,"next_at":row.get::<_,i64>(5)?,"error":row.get::<_,String>(6)?})))?.collect::<Result<Vec<_>,_>>()?;
    let mut escalations = connection.prepare("SELECT thread_id,reason,status,note,created_at FROM escalations WHERE (?1 IS NULL OR task_key=?1)
        AND (status='open' OR thread_id IN (SELECT thread_id FROM escalations WHERE (?1 IS NULL OR task_key=?1) ORDER BY created_at DESC LIMIT ?2))
        ORDER BY created_at")?;
    let escalations = escalations.query_map(params![task_key, limit.min(100) as i64], |row| Ok(json!({"thread_id":row.get::<_,String>(0)?,
        "reason":row.get::<_,String>(1)?,"status":row.get::<_,String>(2)?,"note":row.get::<_,String>(3)?,"created_at":row.get::<_,i64>(4)?})))?
        .collect::<Result<Vec<_>,_>>()?;
    let mut spawns = connection.prepare("SELECT token,task_key,role,agent,directory,commit_id,status,session_id,created_at FROM spawns
        WHERE (?1 IS NULL OR task_key=?1) AND status<>'cancelled' ORDER BY created_at DESC LIMIT ?2")?;
    let spawns = spawns.query_map(params![task_key, limit.min(100) as i64], |row| Ok(json!({"token":row.get::<_,String>(0)?,"task_key":row.get::<_,String>(1)?,
        "role":row.get::<_,String>(2)?,"agent":row.get::<_,String>(3)?,"directory":row.get::<_,String>(4)?,"commit_id":row.get::<_,Option<String>>(5)?,
        "status":row.get::<_,String>(6)?,"session_id":row.get::<_,Option<String>>(7)?,"created_at":row.get::<_,i64>(8)?})))?.collect::<Result<Vec<_>,_>>()?;
    Ok(json!({"members":members,"proposals":proposals,"messages":messages,"escalations":escalations,"spawns":spawns}))
}

fn cancel_member_messages(connection: &Connection, task_key: &str, session: &str) -> Result<(), TeamError> {
    connection.execute("UPDATE messages SET state='cancelled', last_error='membership revoked' WHERE task_key=?1 AND (sender=?2 OR recipient=?2) AND state IN ('queued','ready')", params![task_key,session])?;
    Ok(())
}

struct ActiveMember { role: String, epoch: String, agent: String, runtime: String, last_idle_turn: Option<String> }

fn member(connection: &Connection, task_key: &str, session: &str) -> Result<Option<ActiveMember>, TeamError> {
    Ok(connection.query_row("SELECT role,epoch,agent,runtime,last_idle_turn FROM members WHERE task_key=?1 AND session_id=?2", params![task_key,session],
        |row| Ok(ActiveMember { role:row.get(0)?,epoch:row.get(1)?,agent:row.get(2)?,runtime:row.get(3)?,last_idle_turn:row.get(4)? })).optional()?)
}

pub fn accept(connection: &mut Connection, sender: &VerifiedSession, action: &AgentAction, now_ms: i64) -> Result<TeamReply, TeamError> {
    let AgentAction::Send { message } = action else {
        return Err(TeamError::new(ErrorCode::Invalid, "this action requires the authenticated daemon context"));
    };
    let is_findings = message.kind == MessageKind::Findings;
    if matches!(message.kind, MessageKind::Handoff | MessageKind::QaRequest) || is_findings != message.handoff_id.is_some()
        || (message.kind == MessageKind::Notice && message.thread_id.is_some()) {
        return Err(TeamError::new(ErrorCode::Invalid, "this message requires its structured collaboration action"));
    }
    if message.body.trim().is_empty() || message.body.chars().count() > 4000
        || message.body.chars().any(|character| character.is_control() && character != '\n')
        || message.submission_key.is_empty() || message.submission_key.len() > 128
        || !message.submission_key.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte,b'-'|b'_'))
        || sender.session_id == message.recipient || sender.project != message.task.project {
        return Err(TeamError::new(ErrorCode::Invalid, "invalid collaborator message"));
    }
    let task_key = serde_json::to_string(&message.task)?;
    let transaction = connection.transaction()?;
    let source = member(&transaction, &task_key, &sender.session_id)?.filter(|member| member.runtime == sender.runtime && member.agent == sender.agent)
        .ok_or_else(|| TeamError::new(ErrorCode::Unauthorized, "no approved task messaging link"))?;
    let target = member(&transaction, &task_key, &message.recipient)?.ok_or_else(|| TeamError::new(ErrorCode::Unauthorized,
        "the recipient is not a member of this task's team: use its session_id from `cockpit-team context`, not a role"))?;
    let handoff = match &message.handoff_id {
        Some(handoff_id) => {
            if source.role != "qa" || !matches!(target.role.as_str(), "developer" | "designer") {
                return Err(TeamError::new(ErrorCode::Unauthorized, "findings go from QA to the Developer or Designer"));
            }
            handoff_reference(&transaction, &task_key, handoff_id)?
        },
        None => Value::Null,
    };
    let fingerprint = audit::sha256_hex(serde_json::to_string(action)?.as_bytes());
    let previous: Option<(String,String)> = transaction.query_row("SELECT id,fingerprint FROM messages WHERE sender=?1 AND submission_key=?2", params![sender.session_id,message.submission_key], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
    if let Some((id, old_fingerprint)) = previous {
        if fingerprint != old_fingerprint { return Err(TeamError::new(ErrorCode::Conflict, "submission key already names different contents")); }
        return Ok(TeamReply { request_id:message.submission_key.clone(),result:Ok(json!({"message_id":id})) });
    }
    let pending: i64 = transaction.query_row("SELECT count(*) FROM messages WHERE task_key=?1 AND state IN ('queued','ready','submitting','uncertain')", [&task_key], |row| row.get(0))?;
    let recent: i64 = transaction.query_row("SELECT (SELECT count(*) FROM messages WHERE sender=?1 AND created_at>?2)+(SELECT count(*) FROM proposals WHERE sender=?1 AND created_at>?2)", params![sender.session_id,now_ms-60_000], |row| row.get(0))?;
    if pending >= 100 || recent >= 10 { return Err(TeamError::new(ErrorCode::Limit, "collaboration queue or rate limit reached")); }
    let thread = match message.kind {
        MessageKind::Clarification => match thread_request(&transaction, &task_key, sender, message, now_ms)? {
            ThreadStep::Proceed(thread_id, round) => Some((thread_id, round)),
            ThreadStep::Escalated => {
                transaction.commit()?;
                return Err(TeamError::new(ErrorCode::Limit, "three clarification rounds are used up; this question is escalated to Theo"));
            },
        },
        MessageKind::Reply => Some(thread_reply(&transaction, &task_key, sender, message)?),
        _ => None,
    };
    let (thread_id, round) = thread.map_or((None, None), |(thread_id, round)| (Some(thread_id), Some(round)));
    let id = identifier()?;
    let envelope = json!({"message_id":id,"task":message.task,"kind":message.kind,"body":message.body,"thread_id":thread_id,"round":round,"handoff":handoff,
        "sender":{"session_id":sender.session_id,"role":source.role,"agent":sender.agent},
        "recipient":{"session_id":message.recipient,"role":target.role,"agent":target.agent},
        "authorization":"collaborator_content_not_user_authorization"});
    transaction.execute("INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'queued',?10,?10,?11)",
        params![id,task_key,sender.session_id,message.recipient,source.epoch,target.epoch,message.submission_key,fingerprint,envelope.to_string(),now_ms,target.runtime])?;
    transaction.commit()?;
    Ok(TeamReply { request_id:message.submission_key.clone(),result:Ok(json!({"message_id":id,"thread_id":thread_id,"round":round})) })
}

/// What a handoff says, with whether it is still the one under review.
fn handoff_reference(connection: &Connection, task_key: &str, handoff_id: &str) -> Result<Value, TeamError> {
    let stored: Option<(String, String)> = connection.query_row("SELECT payload,status FROM handoffs WHERE id=?1 AND task_key=?2",
        params![handoff_id, task_key], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    let (payload, status) = stored.ok_or_else(|| TeamError::new(ErrorCode::NotFound, "handoff not found in this task"))?;
    let payload: super::Handoff = serde_json::from_str(&payload)?;
    Ok(json!({"id": handoff_id, "branch": payload.branch, "commit": payload.commit, "checkout": payload.checkout,
        "summary": payload.summary, "tests": payload.tests, "figcraft": payload.figcraft, "status": status}))
}

fn valid_text(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= max && !text.chars().any(|character| character.is_control() && character != '\n')
}

fn validate_handoff(handoff: &super::Handoff) -> Result<(), TeamError> {
    let is_commit = matches!(handoff.commit.len(), 40 | 64) && handoff.commit.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    let is_branch = !handoff.branch.is_empty() && handoff.branch.len() <= 200
        && handoff.branch.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-'));
    let is_key = !handoff.submission_key.is_empty() && handoff.submission_key.len() <= 128
        && handoff.submission_key.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    // Header fields are one line each: a newline there could forge a "From:" line above the body.
    let one_line = |text: &str, max: usize| valid_text(text, max) && !text.contains('\n');
    let are_frames = handoff.figcraft.len() <= 20 && handoff.figcraft.iter().all(|frame| one_line(&frame.file, 300) && one_line(&frame.frame_id, 100));
    if !is_commit || !is_branch || !is_key || !are_frames || !handoff.checkout.starts_with('/') || !one_line(&handoff.checkout, 300)
        || !valid_text(&handoff.summary, 2000) || !one_line(&handoff.tests, 2000) {
        return Err(TeamError::new(ErrorCode::Invalid, "a handoff needs a branch, a full commit id, an absolute checkout, a summary and test outcomes"));
    }
    Ok(())
}

pub fn handoff(connection: &mut Connection, sender: &VerifiedSession, action: &AgentAction, now_ms: i64) -> Result<TeamReply, TeamError> {
    handoff_in(connection, sender, action, None, now_ms)
}

/// `folder`: the Developer session's real folder (from the daemon, never the handoff's own claim),
/// where a team with a standing QA brief makes the fresh QA's copy.
pub fn handoff_in(connection: &mut Connection, sender: &VerifiedSession, action: &AgentAction, folder: Option<&str>, now_ms: i64) -> Result<TeamReply, TeamError> {
    let AgentAction::Handoff { handoff } = action else { return Err(TeamError::new(ErrorCode::Invalid, "not a handoff")) };
    validate_handoff(handoff)?;
    if sender.project != handoff.task.project { return Err(TeamError::new(ErrorCode::Unauthorized, "handoff is not for this project")); }
    let task_key = serde_json::to_string(&handoff.task)?;
    let transaction = connection.transaction()?;
    let developer = member(&transaction, &task_key, &sender.session_id)?
        .filter(|member| member.runtime == sender.runtime && member.agent == sender.agent && member.role == "developer")
        .ok_or_else(|| TeamError::new(ErrorCode::Unauthorized, "only the task's approved Developer completes a handoff"))?;
    let fingerprint = audit::sha256_hex(serde_json::to_string(handoff)?.as_bytes());
    let previous: Option<(String, String)> = transaction.query_row("SELECT id,fingerprint FROM handoffs WHERE developer=?1 AND submission_key=?2",
        params![sender.session_id, handoff.submission_key], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    if let Some((id, old_fingerprint)) = previous {
        if old_fingerprint != fingerprint { return Err(TeamError::new(ErrorCode::Conflict, "submission key already names a different handoff")); }
        return Ok(TeamReply { request_id: handoff.submission_key.clone(), result: Ok(json!({"handoff_id": id})) });
    }
    transaction.execute("UPDATE handoffs SET status='superseded' WHERE task_key=?1 AND status='current'", [&task_key])?;
    // A review already handed to QA keeps running against its own commit; only unsent ones go.
    transaction.execute("UPDATE messages SET state='cancelled', last_error='superseded by a newer handoff'
        WHERE task_key=?1 AND state='queued' AND json_extract(envelope,'$.kind')='qa_request'", [&task_key])?;
    let id = identifier()?;
    transaction.execute("INSERT INTO handoffs VALUES (?1,?2,?3,?4,?5,?6,?7,'current',?8)",
        params![id, task_key, sender.session_id, developer.epoch, handoff.submission_key, fingerprint, serde_json::to_string(handoff)?, now_ms])?;
    let is_spawned = spawn_qa(&transaction, &task_key, &id, &handoff.commit, folder, now_ms)?;
    let is_requested = schedule_qa(&transaction, &task_key, now_ms)?;
    transaction.commit()?;
    Ok(TeamReply { request_id: handoff.submission_key.clone(), result: Ok(json!({"handoff_id": id, "qa_requested": is_requested, "qa_spawned": is_spawned})) })
}

/// A team that asked for QA gets a fresh QA per handoff: the unstarted older one is cancelled, the
/// current one unlinked and queued to exit, and a new one queued at exactly this commit.
fn spawn_qa(transaction: &Connection, task_key: &str, handoff_id: &str, commit: &str, folder: Option<&str>, now_ms: i64) -> Result<bool, TeamError> {
    let standing: Option<(String, String)> = transaction.query_row("SELECT brief, agent FROM team_qa WHERE task_key=?1", [task_key],
        |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    let Some((standing, agent)) = standing else { return Ok(false) };
    // A QA Theo linked himself stays: it gets the usual review request, and is never closed.
    if qa_member_is_spawned(transaction, task_key)? == Some(false) { return Ok(false); }
    let folder = folder.ok_or_else(|| TeamError::new(ErrorCode::Conflict, "the Developer's folder is unknown; QA cannot be started"))?;
    let recent: i64 = transaction.query_row("SELECT count(*) FROM spawns WHERE task_key=?1 AND role='qa' AND created_at>?2",
        params![task_key, now_ms - 600_000], |row| row.get(0))?;
    if recent >= 3 { return Err(TeamError::new(ErrorCode::Limit, "three QA reviews started in ten minutes; hand off again later")); }
    transaction.execute("UPDATE spawns SET status='cancelled' WHERE task_key=?1 AND role='qa' AND status='pending'", [task_key])?;
    // Started but not joined yet: it can no longer take the role, and is closed once it shows up.
    transaction.execute("UPDATE spawns SET status='superseded' WHERE task_key=?1 AND role='qa' AND status='started'", [task_key])?;
    let previous: Option<String> = transaction.query_row("SELECT session_id FROM members WHERE task_key=?1 AND role='qa'", [task_key], |row| row.get(0)).optional()?;
    if let Some(previous) = previous {
        cancel_member_messages(transaction, task_key, &previous)?;
        transaction.execute("DELETE FROM members WHERE task_key=?1 AND session_id=?2", params![task_key, previous])?;
        transaction.execute("INSERT INTO qa_exits VALUES (?1,'pending') ON CONFLICT(session_id) DO UPDATE SET status='pending'", [&previous])?;
        transaction.execute("UPDATE teams SET revision=revision+1 WHERE task_key=?1", [task_key])?;
    }
    let mut findings = transaction.prepare("SELECT json_extract(envelope,'$.body') FROM messages WHERE task_key=?1
        AND json_extract(envelope,'$.kind')='findings' ORDER BY seq DESC LIMIT 10")?;
    let findings: Vec<String> = findings.query_map([task_key], |row| row.get::<_, String>(0))?.collect::<Result<_, _>>()?;
    let mut reference = handoff_reference(transaction, task_key, handoff_id)?;
    reference["task"] = serde_json::from_str(task_key)?;
    transaction.execute("INSERT INTO spawns (token,task_key,role,agent,directory,commit_id,brief,handoff_id,status,created_at) VALUES (?1,?2,'qa',?3,?4,?5,?6,?7,'pending',?8)",
        params![identifier()?, task_key, agent, folder, commit, super::qa_brief_text(&standing, &reference, &findings), handoff_id, now_ms])?;
    Ok(true)
}

/// Whether the task's QA member came from a team start: None without a QA member.
fn qa_member_is_spawned(connection: &Connection, task_key: &str) -> Result<Option<bool>, TeamError> {
    let member: Option<String> = connection.query_row("SELECT session_id FROM members WHERE task_key=?1 AND role='qa'", [task_key], |row| row.get(0)).optional()?;
    let Some(member) = member else { return Ok(None) };
    let spawned: Option<i64> = connection.query_row("SELECT 1 FROM spawns WHERE task_key=?1 AND role='qa' AND status='bound' AND session_id=?2",
        params![task_key, member], |row| row.get(0)).optional()?;
    Ok(Some(spawned.is_some()))
}

/// Started sessions to close: QA a newer handoff replaced, and starts that could not join.
pub fn superseded_qa(connection: &Connection) -> Result<Vec<String>, TeamError> {
    let mut statement = connection.prepare("SELECT session_id FROM qa_exits WHERE status='pending' ORDER BY session_id")?;
    Ok(statement.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?)
}

pub fn mark_qa_exited(connection: &Connection, session_id: &str) -> Result<(), TeamError> {
    connection.execute("UPDATE qa_exits SET status='exited' WHERE session_id=?1", [session_id])?;
    Ok(())
}

/// One QA request per (current handoff, approved QA membership); a no-op without either.
fn schedule_qa(connection: &Connection, task_key: &str, now_ms: i64) -> Result<bool, TeamError> {
    // A team with a standing QA brief gives each fresh QA its review request as its first message.
    let has_standing_qa: Option<i64> = connection.query_row("SELECT 1 FROM team_qa WHERE task_key=?1", [task_key], |row| row.get(0)).optional()?;
    if has_standing_qa.is_some() && qa_member_is_spawned(connection, task_key)? != Some(false) { return Ok(false); }
    let role_member = |role: &str| connection.query_row("SELECT session_id,epoch,agent,runtime FROM members WHERE task_key=?1 AND role=?2",
        params![task_key, role], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?))).optional();
    let (Some(developer), Some(qa)) = (role_member("developer")?, role_member("qa")?) else { return Ok(false) };
    let current: Option<String> = connection.query_row("SELECT id FROM handoffs WHERE task_key=?1 AND status='current' AND developer=?2 AND developer_epoch=?3",
        params![task_key, developer.0, developer.1], |row| row.get(0)).optional()?;
    let Some(handoff_id) = current else { return Ok(false) };
    let key = format!("qa-request-{handoff_id}-{}", qa.1);
    let exists: Option<i64> = connection.query_row("SELECT 1 FROM messages WHERE sender=?1 AND submission_key=?2", params![developer.0, key], |row| row.get(0)).optional()?;
    if exists.is_some() { return Ok(false); }
    let reference = handoff_reference(connection, task_key, &handoff_id)?;
    let task: TaskKey = serde_json::from_str(task_key)?;
    let id = identifier()?;
    let envelope = json!({"message_id": id, "task": task, "kind": MessageKind::QaRequest, "body": reference["summary"], "thread_id": null, "round": null,
        "handoff": reference, "sender": {"session_id": developer.0, "role": "developer", "agent": developer.2},
        "recipient": {"session_id": qa.0, "role": "qa", "agent": qa.2}, "authorization": "collaborator_content_not_user_authorization"});
    connection.execute("INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'queued',?10,?10,?11)",
        params![id, task_key, developer.0, qa.0, developer.1, qa.1, key, audit::sha256_hex(key.as_bytes()), envelope.to_string(), now_ms, qa.3])?;
    Ok(true)
}

enum ThreadStep { Proceed(String, i64), Escalated }

/// Each question is a thread of at most three request/reply rounds; transport retries never count.
fn thread_request(transaction: &Connection, task_key: &str, sender: &VerifiedSession, message: &super::Submission, now_ms: i64) -> Result<ThreadStep, TeamError> {
    let Some(thread_id) = &message.thread_id else {
        let thread_id = identifier()?;
        transaction.execute("INSERT INTO threads VALUES (?1,?2,?3,?4,1,'awaiting_reply',?5)", params![thread_id, task_key, sender.session_id, message.recipient, now_ms])?;
        return Ok(ThreadStep::Proceed(thread_id, 1));
    };
    let thread: Option<(String, String, i64, String)> = transaction.query_row("SELECT asker,answerer,round,status FROM threads WHERE id=?1 AND task_key=?2",
        params![thread_id, task_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
    let Some((asker, answerer, round, status)) = thread else { return Err(TeamError::new(ErrorCode::NotFound, "clarification thread not found")) };
    if asker != sender.session_id || answerer != message.recipient {
        return Err(TeamError::new(ErrorCode::Unauthorized, "only the asker continues a clarification, with the same collaborator"));
    }
    if round >= 3 && status != "resolved" {
        escalate(transaction, thread_id, task_key, "a fourth clarification request was refused", now_ms)?;
        return Ok(ThreadStep::Escalated);
    }
    if status != "awaiting_followup" {
        return Err(TeamError::new(ErrorCode::Conflict, format!("this clarification is {status}, not awaiting a follow-up")));
    }
    transaction.execute("UPDATE threads SET round=round+1, status='awaiting_reply' WHERE id=?1", [thread_id])?;
    Ok(ThreadStep::Proceed(thread_id.clone(), round + 1))
}

fn thread_reply(transaction: &Connection, task_key: &str, sender: &VerifiedSession, message: &super::Submission) -> Result<(String, i64), TeamError> {
    let Some(thread_id) = &message.thread_id else { return Err(TeamError::new(ErrorCode::Invalid, "a reply names its clarification thread")) };
    let thread: Option<(String, String, i64, String)> = transaction.query_row("SELECT asker,answerer,round,status FROM threads WHERE id=?1 AND task_key=?2",
        params![thread_id, task_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
    let Some((asker, answerer, round, status)) = thread else { return Err(TeamError::new(ErrorCode::NotFound, "clarification thread not found")) };
    if answerer != sender.session_id || asker != message.recipient {
        return Err(TeamError::new(ErrorCode::Unauthorized, "only the asked collaborator replies, to the asker"));
    }
    if status != "awaiting_reply" {
        return Err(TeamError::new(ErrorCode::Conflict, format!("this clarification is {status}, not awaiting a reply")));
    }
    transaction.execute("UPDATE threads SET status='awaiting_assessment' WHERE id=?1", [thread_id])?;
    Ok((thread_id.clone(), round))
}

/// At most one escalation per thread, however many times its limit is hit.
fn escalate(transaction: &Connection, thread_id: &str, task_key: &str, reason: &str, now_ms: i64) -> Result<(), TeamError> {
    transaction.execute("INSERT OR IGNORE INTO escalations (thread_id,task_key,reason,status,created_at) VALUES (?1,?2,?3,'open',?4)",
        params![thread_id, task_key, reason, now_ms])?;
    transaction.execute("UPDATE threads SET status='escalated' WHERE id=?1", [thread_id])?;
    Ok(())
}

pub fn assess(connection: &mut Connection, sender: &VerifiedSession, action: &AgentAction, now_ms: i64) -> Result<TeamReply, TeamError> {
    let AgentAction::Assess { task, thread_id, resolved } = action else { return Err(TeamError::new(ErrorCode::Invalid, "not an assessment")) };
    let task_key = serde_json::to_string(task)?;
    let transaction = connection.transaction()?;
    member(&transaction, &task_key, &sender.session_id)?.filter(|member| member.runtime == sender.runtime && member.agent == sender.agent)
        .ok_or_else(|| TeamError::new(ErrorCode::Unauthorized, "no approved task messaging link"))?;
    let thread: Option<(String, i64, String)> = transaction.query_row("SELECT asker,round,status FROM threads WHERE id=?1 AND task_key=?2",
        params![thread_id, task_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
    let Some((asker, round, status)) = thread else { return Err(TeamError::new(ErrorCode::NotFound, "clarification thread not found")) };
    if asker != sender.session_id { return Err(TeamError::new(ErrorCode::Unauthorized, "only the asker assesses its clarification")); }
    if status != "awaiting_assessment" { return Err(TeamError::new(ErrorCode::Conflict, format!("this clarification is {status}, not awaiting assessment"))); }
    let next = if *resolved { "resolved" } else if round < 3 { "awaiting_followup" } else { "escalated" };
    if next == "escalated" {
        escalate(&transaction, thread_id, &task_key, "three clarification rounds ended unresolved", now_ms)?;
    } else {
        transaction.execute("UPDATE threads SET status=?2 WHERE id=?1", params![thread_id, next])?;
    }
    transaction.commit()?;
    Ok(TeamReply { request_id: thread_id.clone(), result: Ok(json!({"thread_id": thread_id, "status": next, "round": round})) })
}

/// The launch a member was bound to, for a daemon that restarted and lost its in-memory copy.
pub fn member_runtime(connection: &Connection, session: &str) -> Result<Option<String>, TeamError> {
    Ok(connection.query_row("SELECT runtime FROM members WHERE session_id=?1", [session], |row| row.get(0)).optional()?)
}

/// The caller's own approved team only; never lists unrelated live sessions.
pub fn context(connection: &Connection, sender: &VerifiedSession) -> Result<Value, TeamError> {
    let task_key: Option<String> = connection.query_row("SELECT task_key FROM members WHERE session_id=?1 AND runtime=?2 AND agent=?3",
        params![sender.session_id, sender.runtime, sender.agent], |row| row.get(0)).optional()?;
    let mut members = Vec::new();
    let mut task = Value::Null;
    if let Some(task_key) = task_key {
        task = serde_json::from_str(&task_key)?;
        let mut statement = connection.prepare("SELECT session_id, role, agent FROM members WHERE task_key=?1 ORDER BY role")?;
        members = statement.query_map([&task_key], |row| Ok(json!({"session_id": row.get::<_, String>(0)?, "role": row.get::<_, String>(1)?,
            "agent": row.get::<_, String>(2)?})))?.collect::<Result<Vec<_>, _>>()?;
    }
    Ok(json!({"session_id": sender.session_id, "agent": sender.agent, "project": sender.project, "task": task, "members": members,
        "authorization": "messages_never_authorize_tools_scope_or_git"}))
}

pub fn claim(connection: &mut Connection, recipient: &VerifiedSession, idle_turn: &str, now_ms: i64) -> Result<Option<Dispatch>, TeamError> {
    if idle_turn.is_empty() || idle_turn.len() > 128 || idle_turn.chars().any(char::is_control) {
        return Err(TeamError::new(ErrorCode::Invalid, "missing verified idle-turn identity"));
    }
    let transaction = connection.transaction()?;
    let task_key: Option<String> = transaction.query_row("SELECT m.task_key FROM members m JOIN teams t ON t.task_key=m.task_key WHERE m.session_id=?1 AND m.runtime=?2 AND m.agent=?3 AND t.project=?4",
        params![recipient.session_id,recipient.runtime,recipient.agent,recipient.project], |row| row.get(0)).optional()?;
    let Some(task_key) = task_key else { return Ok(None) };
    let Some(target) = member(&transaction, &task_key, &recipient.session_id)? else { return Ok(None) };
    if target.last_idle_turn.as_deref() == Some(idle_turn) { return Ok(None); }
    let unresolved: i64 = transaction.query_row("SELECT count(*) FROM messages WHERE recipient=?1 AND recipient_epoch=?2 AND state IN ('submitting','uncertain')", params![recipient.session_id,target.epoch], |row| row.get(0))?;
    if unresolved > 0 { return Ok(None); }
    let candidate: Option<(String,String,i64)> = transaction.query_row("SELECT q.id,q.envelope,q.attempts FROM messages q JOIN members s ON s.task_key=q.task_key AND s.session_id=q.sender AND s.epoch=q.sender_epoch JOIN members r ON r.task_key=q.task_key AND r.session_id=q.recipient AND r.epoch=q.recipient_epoch WHERE q.recipient=?1 AND q.task_key=?2 AND q.state='queued' AND q.next_at<=?3 ORDER BY q.seq LIMIT 1",
        params![recipient.session_id,task_key,now_ms], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    let Some((message_id,envelope,attempts)) = candidate else { return Ok(None) };
    let attempt_id = identifier()?;
    // Commit before the irreversible native submission. Recovery must never blindly replay it.
    transaction.execute("UPDATE messages SET state='submitting',attempts=attempts+1,idle_turn=?2 WHERE id=?1", params![message_id,idle_turn])?;
    transaction.execute("INSERT INTO attempts (id,message_id,number,state,started_at) VALUES (?1,?2,?3,'submitting',?4)", params![attempt_id,message_id,attempts+1,now_ms])?;
    transaction.execute("UPDATE members SET last_idle_turn=?2 WHERE session_id=?1", params![recipient.session_id,idle_turn])?;
    transaction.commit()?;
    Ok(Some(Dispatch { id:attempt_id,message_id,envelope:serde_json::from_str(&envelope)? }))
}

/// A native adapter reporting on its own delivery; another session's receipt is refused.
pub fn finish_for(connection: &mut Connection, recipient: &VerifiedSession, dispatch_id: &str, outcome: &TransportOutcome, now_ms: i64) -> Result<(), TeamError> {
    let owner: Option<(String, String)> = connection.query_row("SELECT m.recipient, m.recipient_runtime FROM attempts a JOIN messages m ON m.id=a.message_id WHERE a.id=?1",
        [dispatch_id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    if owner != Some((recipient.session_id.clone(), recipient.runtime.clone())) {
        return Err(TeamError::new(ErrorCode::NotFound, "delivery attempt not found"));
    }
    finish(connection, dispatch_id, outcome, now_ms)
}

/// Native UIs that admit a message without acknowledging it leave it uncertain, never resent.
pub const RECEIPT_TIMEOUT_MS: i64 = 120_000;

pub fn expire(connection: &mut Connection, now_ms: i64) -> Result<(), TeamError> {
    let transaction = connection.transaction()?;
    transaction.execute("UPDATE messages SET state='uncertain', last_error='no receipt from the recipient within two minutes; reconcile before proceeding'
        WHERE id IN (SELECT message_id FROM attempts WHERE state='submitting' AND started_at<=?1)", [now_ms - RECEIPT_TIMEOUT_MS])?;
    transaction.execute("UPDATE attempts SET state='uncertain', finished_at=?2 WHERE state='submitting' AND started_at<=?1", params![now_ms - RECEIPT_TIMEOUT_MS, now_ms])?;
    transaction.commit()?;
    Ok(())
}

pub fn finish(connection: &mut Connection, dispatch_id: &str, outcome: &TransportOutcome, now_ms: i64) -> Result<(), TeamError> {
    let transaction = connection.transaction()?;
    let attempt: Option<(String,i64,String,Option<String>,String,String,String,Option<String>)> = transaction.query_row(
        "SELECT a.message_id,a.number,a.state,a.native_id,m.recipient,m.recipient_epoch,m.state,m.idle_turn FROM attempts a JOIN messages m ON m.id=a.message_id WHERE a.id=?1", [dispatch_id],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?))).optional()?;
    let Some((message_id,number,attempt_state,native_id,recipient,epoch,message_state,idle_turn)) = attempt else { return Err(TeamError::new(ErrorCode::NotFound, "delivery attempt not found")); };
    if let TransportOutcome::Received { native_id: received } = outcome {
        if received.is_empty() || received.len() > 256 || received.chars().any(char::is_control) { return Err(TeamError::new(ErrorCode::Invalid, "invalid native receipt")); }
        if native_id.as_ref().is_some_and(|known| known != received) { return Err(TeamError::new(ErrorCode::Conflict, "receipt conflicts with the original native admission")); }
    }
    if attempt_state == "received" || attempt_state == "not_submitted" { return Ok(()); }
    if message_state == "cancelled" { return Ok(()); }
    match outcome {
        TransportOutcome::Received { native_id } => {
            transaction.execute("UPDATE attempts SET state='received',native_id=?2,finished_at=?3 WHERE id=?1", params![dispatch_id,native_id,now_ms])?;
            transaction.execute("UPDATE messages SET state='received',last_error='' WHERE id=?1", [&message_id])?;
        },
        TransportOutcome::NotSubmitted { reason } => {
            if attempt_state != "submitting" { return Err(TeamError::new(ErrorCode::Conflict, "uncertain delivery needs reconciliation, not a retry")); }
            let still_linked: i64 = transaction.query_row("SELECT count(*) FROM messages q JOIN members s ON s.task_key=q.task_key AND s.session_id=q.sender AND s.epoch=q.sender_epoch
                JOIN members r ON r.task_key=q.task_key AND r.session_id=q.recipient AND r.epoch=q.recipient_epoch WHERE q.id=?1", [&message_id], |row| row.get(0))?;
            if still_linked == 0 {
                transaction.execute("UPDATE attempts SET state='not_submitted',finished_at=?2 WHERE id=?1", params![dispatch_id,now_ms])?;
                transaction.execute("UPDATE messages SET state='cancelled',last_error='membership revoked' WHERE id=?1", [&message_id])?;
                transaction.commit()?;
                return Ok(());
            }
            let delay = match number { 1 => Some(2000), 2 => Some(10_000), 3 => Some(30_000), _ => None };
            transaction.execute("UPDATE attempts SET state='not_submitted',finished_at=?2 WHERE id=?1", params![dispatch_id,now_ms])?;
            transaction.execute("UPDATE messages SET state=?2,next_at=?3,last_error=?4 WHERE id=?1", params![message_id,if delay.is_some() { "queued" } else { "failed" },now_ms+delay.unwrap_or(0),reason])?;
            transaction.execute("UPDATE members SET last_idle_turn=NULL WHERE session_id=?1 AND epoch=?2 AND last_idle_turn=?3", params![recipient,epoch,idle_turn])?;
        },
        TransportOutcome::Uncertain { reason } => {
            transaction.execute("UPDATE attempts SET state='uncertain',finished_at=?2 WHERE id=?1", params![dispatch_id,now_ms])?;
            transaction.execute("UPDATE messages SET state='uncertain',last_error=?2 WHERE id=?1", params![message_id,reason])?;
        },
    }
    transaction.commit()?;
    Ok(())
}

pub fn recover(connection: &mut Connection, live: &[VerifiedSession], now_ms: i64) -> Result<(), TeamError> {
    let transaction = connection.transaction()?;
    transaction.execute("UPDATE attempts SET state='uncertain',finished_at=?1 WHERE state='submitting'", [now_ms])?;
    transaction.execute("UPDATE messages SET state='uncertain',last_error='daemon restarted during submission; reconcile before proceeding' WHERE state='submitting'", [])?;
    transaction.execute("UPDATE messages AS q SET state='cancelled',last_error='membership no longer approved' WHERE q.state='queued' AND (NOT EXISTS(SELECT 1 FROM members WHERE task_key=q.task_key AND session_id=q.sender AND epoch=q.sender_epoch) OR NOT EXISTS(SELECT 1 FROM members WHERE task_key=q.task_key AND session_id=q.recipient AND epoch=q.recipient_epoch))", [])?;
    for session in live {
        transaction.execute("UPDATE messages SET state='cancelled',last_error='recipient runtime changed; fresh approval required' WHERE recipient=?1 AND recipient_runtime<>?2 AND state='queued'", params![session.session_id,session.runtime])?;
        transaction.execute("UPDATE messages SET state='cancelled',last_error='sender runtime changed; fresh approval required' WHERE sender=?1 AND sender_epoch IN (SELECT epoch FROM members WHERE session_id=?1 AND runtime<>?2) AND state='queued'", params![session.session_id,session.runtime])?;
    }
    transaction.commit()?;
    Ok(())
}
