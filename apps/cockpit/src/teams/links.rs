//! Protected shared-work identity and schema; no link grants team membership.

use super::{
    AgentAction, ErrorCode, HostAction, LinkProposal, LinkSubmission, MessageKind, TaskKey,
    TeamError, TeamReply, VerifiedSession, store,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};

fn invalid_mapping() -> TeamError {
    TeamError::new(
        ErrorCode::Invalid,
        "invalid or unavailable collaboration link",
    )
}

fn live_participant<'a>(
    live: &'a [VerifiedSession],
    id: &str,
) -> Result<&'a VerifiedSession, TeamError> {
    let mut matches = live.iter().filter(|session| session.session_id == id);
    let session = matches.next().ok_or_else(invalid_mapping)?;
    if matches.next().is_some()
        || session.runtime.is_empty()
        || crate::agents::of_session_id(id).map(|agent| agent.name) != Some(session.agent.as_str())
        || !session.project.starts_with("path:/")
        || session.project.chars().any(char::is_control)
    {
        return Err(invalid_mapping());
    }
    Ok(session)
}

fn bind(
    sender: &VerifiedSession,
    proposal: &LinkProposal,
    live: &[VerifiedSession],
) -> Result<[VerifiedSession; 2], TeamError> {
    if live_participant(live, &sender.session_id)? != sender {
        return Err(invalid_mapping());
    }
    let peer = live_participant(live, &proposal.peer_session_id)?;
    if sender.session_id == peer.session_id
        || sender.runtime == peer.runtime
        || (proposal.task.project != sender.project && proposal.task.project != peer.project)
        || proposal.submission_key.is_empty()
        || proposal.submission_key.len() > 128
        || !proposal
            .submission_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || proposal.purpose.trim().is_empty()
        || proposal.purpose.chars().count() > 1000
        || proposal.purpose.chars().any(char::is_control)
    {
        return Err(invalid_mapping());
    }
    Ok([sender.clone(), peer.clone()])
}

fn mapping_hash(
    task: &TaskKey,
    purpose: &str,
    revision: i64,
    generation: i64,
    participants: &[VerifiedSession; 2],
) -> Result<String, TeamError> {
    Ok(crate::audit::sha256_hex(serde_json::to_vec(&json!({"task":task,"purpose":purpose,"revision":revision,
        "work_generation":generation,"participants":participants,"permission":"coordination_messages_only"}))?.as_slice()))
}

fn receipt(id: &str, revision: i64, status: &str) -> Value {
    json!({"proposal_id":id,"revision":revision,"status":status})
}

/// Host history only; participant context deliberately never calls this snapshot.
pub fn snapshot(
    connection: &Connection,
    task: Option<&TaskKey>,
    limit: usize,
) -> Result<Value, TeamError> {
    let task_key = task.map(serde_json::to_string).transpose()?;
    let mut statement = connection.prepare("SELECT p.id,p.sender,p.task_key,p.purpose,p.revision,p.mapping_hash,p.work_generation,
        p.status,p.closure_reason,p.parent_id,p.created_at,w.generation,w.state
        FROM link_proposals p LEFT JOIN link_work w ON w.task_key=p.task_key
        WHERE (?1 IS NULL OR p.task_key=?1) AND (p.status='pending' OR p.id IN (SELECT proposal_id FROM links WHERE state='active'
        OR id IN (SELECT id FROM links WHERE (?1 IS NULL OR task_key=?1) ORDER BY created_at DESC,id DESC LIMIT ?2))
        OR p.id IN (SELECT id FROM link_proposals WHERE (?1 IS NULL OR task_key=?1) ORDER BY created_at DESC,id DESC LIMIT ?2))
        ORDER BY p.created_at,p.id")?;
    let records = statement.query_map(params![task_key,limit.min(100) as i64], |row| Ok((
        json!({"proposal_id":row.get::<_,String>(0)?,"sender":row.get::<_,String>(1)?,"purpose":row.get::<_,String>(3)?,
            "revision":row.get::<_,i64>(4)?,"mapping_hash":row.get::<_,String>(5)?,"work_generation":row.get::<_,i64>(6)?,
            "status":row.get::<_,String>(7)?,"closure_reason":row.get::<_,Option<String>>(8)?,"parent_id":row.get::<_,Option<String>>(9)?,
            "created_at":row.get::<_,i64>(10)?,"permission":"coordination_messages_only"}),
        row.get::<_,String>(2)?,row.get::<_,Option<i64>>(11)?,row.get::<_,Option<String>>(12)?
    )))?.collect::<Result<Vec<_>,_>>()?;
    let mut proposals = Vec::new();
    for (mut record, task_key, generation, state) in records {
        let task: TaskKey = serde_json::from_str(&task_key)?;
        record["task"] = json!(task);
        let participants = connection.prepare("SELECT session_id,agent,project,runtime FROM link_participants WHERE proposal_id=?1 ORDER BY slot")?
            .query_map([record["proposal_id"].as_str().expect("stored id")], |row| Ok(VerifiedSession {
                session_id:row.get(0)?,agent:row.get(1)?,project:row.get(2)?,runtime:row.get(3)?
            }))?.collect::<Result<Vec<_>,_>>()?;
        record["participants"] = json!(participants);
        if record["status"] == "pending" || record["status"] == "approved" {
            let blocked = if generation != record["work_generation"].as_i64()
                || state.as_deref() != Some("open")
            {
                Some("shared work lifecycle is closed, missing or stale".to_string())
            } else {
                crate::todo::read_shared_work(&task)
                    .err()
                    .map(|error| error.message)
            };
            record["blocked_reason"] = json!(blocked);
        }
        proposals.push(record);
    }
    let mut statement = connection.prepare("SELECT id,proposal_id,state,closure_reason,created_at,closed_at,epoch FROM links
        WHERE (?1 IS NULL OR task_key=?1) AND (state='active' OR id IN
        (SELECT id FROM links WHERE (?1 IS NULL OR task_key=?1) ORDER BY created_at DESC,id DESC LIMIT ?2)) ORDER BY created_at,id")?;
    let records = statement.query_map(params![task_key,limit.min(100) as i64], |row| Ok(json!({"link_id":row.get::<_,String>(0)?,
        "proposal_id":row.get::<_,String>(1)?,"state":row.get::<_,String>(2)?,"closure_reason":row.get::<_,Option<String>>(3)?,
        "created_at":row.get::<_,i64>(4)?,"closed_at":row.get::<_,Option<i64>>(5)?,"epoch":row.get::<_,String>(6)?})))?
        .collect::<Result<Vec<_>,_>>()?;
    let mut links = Vec::new();
    for record in records {
        let proposal_id = record["proposal_id"].as_str().expect("stored proposal id");
        let mut detail = proposals
            .iter()
            .find(|proposal| proposal["proposal_id"] == proposal_id)
            .ok_or_else(|| {
                TeamError::new(ErrorCode::Storage, "link history is missing its proposal")
            })?
            .clone();
        detail
            .as_object_mut()
            .expect("record")
            .extend(record.as_object().expect("record").clone());
        links.push(detail);
    }
    Ok(json!({"link_proposals":proposals,"links":links}))
}

/// Approved routing facts and the requester's own status receipts, bound to its exact identity.
pub fn context(connection: &Connection, sender: &VerifiedSession) -> Result<Value, TeamError> {
    let mut approved = Vec::new();
    let mut approved_count = 0;
    let mut statement = connection.prepare("SELECT l.id,l.task_key,p.purpose,l.state,l.closure_reason,peer.session_id,peer.agent,peer.project,count(*) OVER ()
        FROM links l JOIN link_proposals p ON p.id=l.proposal_id
        JOIN link_participants own ON own.proposal_id=l.proposal_id
        JOIN link_participants peer ON peer.proposal_id=l.proposal_id AND peer.slot!=own.slot
        WHERE own.session_id=?1 AND own.agent=?2 AND own.project=?3 AND own.runtime=?4
        ORDER BY (l.state='active') DESC,l.created_at DESC,l.id DESC LIMIT 30")?;
    let mut rows = statement.query(params![
        sender.session_id,
        sender.agent,
        sender.project,
        sender.runtime
    ])?;
    while let Some(row) = rows.next()? {
        approved_count = row.get::<_, i64>(8)?;
        let task: TaskKey = serde_json::from_str(&row.get::<_, String>(1)?)?;
        approved.push(json!({"link_id":row.get::<_, String>(0)?,"task":task,"purpose":row.get::<_, String>(2)?,
            "status":row.get::<_, String>(3)?,"closure_reason":row.get::<_, Option<String>>(4)?,
            "peer":{"session_id":row.get::<_, String>(5)?,"agent":row.get::<_, String>(6)?,"project":row.get::<_, String>(7)?}}));
    }
    let mut proposals = Vec::new();
    let mut proposal_count = 0;
    let mut statement = connection.prepare(
        "SELECT p.id,p.revision,p.status,count(*) OVER () FROM link_proposals p
        JOIN link_participants own ON own.proposal_id=p.id AND own.session_id=p.sender
        WHERE p.sender=?1 AND own.agent=?2 AND own.project=?3 AND own.runtime=?4
        ORDER BY (p.status='pending') DESC,p.created_at DESC,p.id DESC LIMIT 30",
    )?;
    let mut rows = statement.query(params![
        sender.session_id,
        sender.agent,
        sender.project,
        sender.runtime
    ])?;
    while let Some(row) = rows.next()? {
        proposal_count = row.get::<_, i64>(3)?;
        proposals.push(receipt(
            &row.get::<_, String>(0)?,
            row.get(1)?,
            &row.get::<_, String>(2)?,
        ));
    }
    // Approval/intake cap active links and pending requests at ten each; history cannot displace them.
    let omitted_approved = approved_count - approved.len() as i64;
    let omitted_proposals = proposal_count - proposals.len() as i64;
    let mut context = json!({"approved":approved,"proposals":proposals});
    if omitted_approved + omitted_proposals > 0 {
        context["omitted_history"] =
            json!({"approved":omitted_approved,"proposals":omitted_proposals});
    }
    bound_context(&mut context, super::client::AGENT_REPLY_MAX)?;
    Ok(context)
}

/// Leave room for the enclosing Context reply without discarding active routing or pending receipts.
pub(super) fn bound_context(context: &mut Value, budget: usize) -> Result<(), TeamError> {
    while serde_json::to_vec(context)?.len() > budget {
        let removable = [("approved", "active"), ("proposals", "pending")]
            .into_iter()
            .find_map(|(field, current)| {
                context[field]
                    .as_array()
                    .expect("context collection")
                    .iter()
                    .rposition(|entry| entry["status"] != current)
                    .map(|index| (field, index))
            });
        let Some((field, index)) = removable else {
            return Err(TeamError::new(
                ErrorCode::Limit,
                "current collaboration context exceeds the reply limit",
            ));
        };
        context[field]
            .as_array_mut()
            .expect("context collection")
            .remove(index);
        if context.get("omitted_history").is_none() {
            context["omitted_history"] = json!({"approved":0,"proposals":0});
        }
        let omitted = context["omitted_history"][field]
            .as_u64()
            .expect("history count");
        context["omitted_history"][field] = json!(omitted + 1);
    }
    Ok(())
}

pub fn propose(
    connection: &mut Connection,
    sender: &VerifiedSession,
    proposal: &LinkProposal,
    live: &[VerifiedSession],
    now_ms: i64,
) -> Result<TeamReply, TeamError> {
    if live_participant(live, &sender.session_id)? != sender {
        return Err(invalid_mapping());
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    store::sleep::guard_sender(&transaction, &sender.session_id)?;
    let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(proposal)?);
    let previous: Option<(String, String, i64, String)> = transaction.query_row(
        "SELECT id,fingerprint,revision,status FROM link_proposals WHERE sender=?1 AND submission_key=?2",
        params![sender.session_id, proposal.submission_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
    if let Some((id, old_fingerprint, revision, status)) = previous {
        if fingerprint != old_fingerprint {
            return Err(TeamError::new(
                ErrorCode::Conflict,
                "submission key already names different contents",
            ));
        }
        return Ok(TeamReply {
            request_id: proposal.submission_key.clone(),
            result: Ok(receipt(&id, revision, &status)),
        });
    }
    let created = insert_proposal(&transaction, sender, proposal, live, 1, None, now_ms)?;
    transaction.commit()?;
    // A pending receipt is not a session-discovery API, even for a known peer ID.
    Ok(TeamReply {
        request_id: proposal.submission_key.clone(),
        result: Ok(receipt(
            created["proposal_id"].as_str().expect("created id"),
            1,
            "pending",
        )),
    })
}

fn insert_proposal(
    transaction: &Transaction<'_>,
    sender: &VerifiedSession,
    proposal: &LinkProposal,
    live: &[VerifiedSession],
    revision: i64,
    parent: Option<&str>,
    now_ms: i64,
) -> Result<Value, TeamError> {
    let participants = bind(sender, proposal, live)?;
    for participant in &participants {
        crate::chief::store::require_non_chief(transaction, &participant.session_id)?;
    }
    let (_, generation) = enroll_work(transaction, &proposal.task)?;
    let pending: i64 = transaction.query_row(
        "SELECT count(*) FROM link_proposals WHERE sender=?1 AND status='pending'",
        [&sender.session_id],
        |row| row.get(0),
    )?;
    if pending >= 10 {
        return Err(TeamError::new(
            ErrorCode::Limit,
            "pending link request limit reached",
        ));
    }
    store::guard_sender_budget(transaction, &sender.session_id, now_ms)?;
    let id = store::identifier()?;
    let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(proposal)?);
    let mapping_hash = mapping_hash(
        &proposal.task,
        &proposal.purpose,
        revision,
        generation,
        &participants,
    )?;
    transaction.execute("INSERT INTO link_proposals (id,sender,submission_key,fingerprint,task_key,purpose,revision,mapping_hash,work_generation,status,created_at,parent_id)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'pending',?10,?11)",
        params![id,sender.session_id,proposal.submission_key,fingerprint,serde_json::to_string(&proposal.task)?,proposal.purpose,revision,mapping_hash,generation,now_ms,parent])?;
    for (slot, participant) in participants.iter().enumerate() {
        transaction.execute(
            "INSERT INTO link_participants VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                id,
                slot as i64,
                participant.session_id,
                participant.agent,
                participant.project,
                participant.runtime
            ],
        )?;
    }
    Ok(
        json!({"proposal_id":id,"revision":revision,"mapping_hash":mapping_hash,"status":"pending","task":proposal.task,
        "purpose":proposal.purpose,"participants":participants,"work_generation":generation,"permission":"coordination_messages_only"}),
    )
}

struct StoredProposal {
    sender: String,
    task: TaskKey,
    purpose: String,
    revision: i64,
    mapping_hash: String,
    generation: i64,
}

fn pending_proposal(
    connection: &Connection,
    id: &str,
    revision: i64,
) -> Result<StoredProposal, TeamError> {
    let stored = connection.query_row("SELECT sender,task_key,purpose,revision,mapping_hash,work_generation FROM link_proposals
        WHERE id=?1 AND revision=?2 AND status='pending'", params![id,revision], |row| {
        Ok((row.get::<_, String>(0)?,row.get::<_, String>(1)?,row.get::<_, String>(2)?,row.get::<_, i64>(3)?,row.get::<_, String>(4)?,row.get::<_, i64>(5)?))
    }).optional()?.ok_or_else(|| TeamError::new(ErrorCode::Conflict, "link request changed; review it again"))?;
    Ok(StoredProposal {
        sender: stored.0,
        task: serde_json::from_str(&stored.1)?,
        purpose: stored.2,
        revision: stored.3,
        mapping_hash: stored.4,
        generation: stored.5,
    })
}

fn approve(
    transaction: &Transaction<'_>,
    id: &str,
    revision: i64,
    hash: &str,
    live: &[VerifiedSession],
    now_ms: i64,
) -> Result<Value, TeamError> {
    let proposal = pending_proposal(transaction, id, revision)?;
    if proposal.mapping_hash != hash {
        return Err(TeamError::new(
            ErrorCode::Conflict,
            "link mapping changed; review it again",
        ));
    }
    let participants = transaction.prepare("SELECT session_id,agent,project,runtime FROM link_participants WHERE proposal_id=?1 ORDER BY slot")?
        .query_map([id], |row| Ok(VerifiedSession { session_id: row.get(0)?, agent: row.get(1)?, project: row.get(2)?, runtime: row.get(3)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    let participants: [VerifiedSession; 2] =
        participants.try_into().map_err(|_| invalid_mapping())?;
    let original = LinkProposal {
        submission_key: "approval".into(),
        task: proposal.task.clone(),
        peer_session_id: participants[1].session_id.clone(),
        purpose: proposal.purpose.clone(),
    };
    if proposal.sender != participants[0].session_id
        || bind(&participants[0], &original, live)? != participants
        || mapping_hash(
            &proposal.task,
            &proposal.purpose,
            proposal.revision,
            proposal.generation,
            &participants,
        )? != hash
    {
        return Err(TeamError::new(
            ErrorCode::Conflict,
            "link identities changed; propose a new link",
        ));
    }
    require_open_work(transaction, &proposal.task, proposal.generation)?;
    close_replaced_identity_links(transaction, &proposal.task, &participants, now_ms)?;
    for participant in &participants {
        crate::chief::store::require_non_chief(transaction, &participant.session_id)?;
        let active: i64 = transaction.query_row("SELECT count(*) FROM links WHERE state='active' AND (session_low=?1 OR session_high=?1)",
            [&participant.session_id], |row| row.get(0))?;
        if active >= 10 {
            return Err(TeamError::new(
                ErrorCode::Limit,
                "active link limit reached",
            ));
        }
    }
    let (low, high) = if participants[0].session_id < participants[1].session_id {
        (&participants[0].session_id, &participants[1].session_id)
    } else {
        (&participants[1].session_id, &participants[0].session_id)
    };
    let task_key = serde_json::to_string(&proposal.task)?;
    let exists: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM links WHERE session_low=?1 AND session_high=?2 AND task_key=?3 AND state='active')",
        params![low,high,task_key], |row| row.get(0))?;
    if exists {
        return Err(TeamError::new(
            ErrorCode::Conflict,
            "this pair already has an active link for this work",
        ));
    }
    let link_id = store::identifier()?;
    transaction.execute(
        "INSERT INTO links VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'active',NULL,?10,NULL)",
        params![
            link_id,
            id,
            revision,
            hash,
            task_key,
            proposal.generation,
            low,
            high,
            store::identifier()?,
            now_ms
        ],
    )?;
    transaction.execute(
        "UPDATE link_proposals SET status='approved' WHERE id=?1",
        [id],
    )?;
    Ok(json!({"link_id":link_id,"proposal_id":id,"status":"active"}))
}

/// A restarted session gets a new runtime, and its old link can never deliver again (require_live).
/// Theo approving a fresh link for the same pair and work closes that dead one instead of
/// refusing the approval as a duplicate; a link whose identities still match stays a conflict.
fn close_replaced_identity_links(
    transaction: &Transaction<'_>,
    task: &TaskKey,
    participants: &[VerifiedSession; 2],
    now_ms: i64,
) -> Result<(), TeamError> {
    let mut statement = transaction.prepare("SELECT l.id,l.proposal_id FROM links l JOIN link_participants own ON own.proposal_id=l.proposal_id
        JOIN link_participants peer ON peer.proposal_id=l.proposal_id AND peer.slot!=own.slot
        WHERE l.state='active' AND l.task_key=?1 AND own.session_id=?2 AND peer.session_id=?3")?;
    let candidates = statement
        .query_map(
            params![
                serde_json::to_string(task)?,
                participants[0].session_id,
                participants[1].session_id
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    for (link_id, proposal_id) in candidates {
        let bound = transaction.prepare("SELECT session_id,agent,project,runtime FROM link_participants WHERE proposal_id=?1")?
            .query_map([&proposal_id], |row| Ok(VerifiedSession { session_id: row.get(0)?, agent: row.get(1)?, project: row.get(2)?, runtime: row.get(3)? }))?
            .collect::<Result<Vec<_>, _>>()?;
        if bound.iter().all(|identity| participants.contains(identity)) {
            continue;
        }
        transaction.execute("UPDATE links SET state='closed',closure_reason='identity_replaced',closed_at=?2 WHERE id=?1 AND state='active'", params![link_id,now_ms])?;
        transaction.execute("UPDATE messages SET state='cancelled',last_error='link identity replaced' WHERE scope_kind='link' AND link_id=?1 AND state IN ('queued','ready')", [&link_id])?;
    }
    Ok(())
}

/// Host socket only. Link controls share the existing host-action replay namespace atomically.
pub fn control(
    connection: &mut Connection,
    action: &HostAction,
    live: &[VerifiedSession],
    now_ms: i64,
) -> Result<TeamReply, TeamError> {
    let request_id = match action {
        HostAction::LinkPropose { request_id, .. }
        | HostAction::LinkApprove { request_id, .. }
        | HostAction::LinkEdit { request_id, .. }
        | HostAction::LinkReject { request_id, .. }
        | HostAction::LinkRevoke { request_id, .. } => request_id,
        _ => return Err(TeamError::new(ErrorCode::Invalid, "not a link control")),
    };
    let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(action)?);
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let previous: Option<(String, String)> = transaction
        .query_row(
            "SELECT fingerprint,response FROM host_actions WHERE request_id=?1",
            [request_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((old_fingerprint, response)) = previous {
        if fingerprint != old_fingerprint {
            return Err(TeamError::new(
                ErrorCode::Conflict,
                "host request id already used",
            ));
        }
        return Ok(TeamReply {
            request_id: request_id.clone(),
            result: Ok(serde_json::from_str(&response)?),
        });
    }
    let response = match action {
        HostAction::LinkPropose {
            requester_session_id,
            proposal,
            ..
        } => {
            let sender = live_participant(live, requester_session_id)?;
            bind(sender, proposal, live)?;
            let proposal = LinkProposal {
                submission_key: format!("host-{}", store::identifier()?),
                ..proposal.clone()
            };
            insert_proposal(&transaction, sender, &proposal, live, 1, None, now_ms)?
        }
        HostAction::LinkApprove {
            proposal_id,
            expected_revision,
            mapping_hash,
            ..
        } => approve(
            &transaction,
            proposal_id,
            *expected_revision,
            mapping_hash,
            live,
            now_ms,
        )?,
        HostAction::LinkEdit {
            proposal_id,
            expected_revision,
            replacement,
            ..
        } => {
            let original = pending_proposal(&transaction, proposal_id, *expected_revision)?;
            require_open_work(&transaction, &original.task, original.generation)?;
            let sender = live_participant(live, &original.sender)?;
            bind(sender, replacement, live)?;
            transaction.execute(
                "UPDATE link_proposals SET status='closed',closure_reason='superseded' WHERE id=?1",
                [proposal_id],
            )?;
            let proposal = LinkProposal {
                submission_key: format!("host-edit-{}", store::identifier()?),
                ..replacement.clone()
            };
            insert_proposal(
                &transaction,
                sender,
                &proposal,
                live,
                original.revision + 1,
                Some(proposal_id),
                now_ms,
            )?
        }
        HostAction::LinkReject {
            proposal_id,
            expected_revision,
            ..
        } => {
            pending_proposal(&transaction, proposal_id, *expected_revision)?;
            transaction.execute(
                "UPDATE link_proposals SET status='rejected',closure_reason='rejected' WHERE id=?1",
                [proposal_id],
            )?;
            receipt(proposal_id, *expected_revision, "rejected")
        }
        HostAction::LinkRevoke { link_id, .. } => {
            if transaction.execute("UPDATE links SET state='closed',closure_reason='revoked',closed_at=?2 WHERE id=?1 AND state='active'", params![link_id,now_ms])? != 1 {
                return Err(TeamError::new(ErrorCode::Conflict, "no active link to revoke"));
            }
            transaction.execute("UPDATE messages SET state='cancelled',last_error='link revoked' WHERE scope_kind='link' AND link_id=?1 AND state IN ('queued','ready')", [link_id])?;
            json!({"link_id":link_id,"status":"closed","closure_reason":"revoked"})
        }
        _ => unreachable!("non-link controls rejected above"),
    };
    transaction.execute(
        "INSERT INTO host_actions VALUES (?1,?2,?3)",
        params![request_id, fingerprint, response.to_string()],
    )?;
    transaction.commit()?;
    Ok(TeamReply {
        request_id: request_id.clone(),
        result: Ok(response),
    })
}

pub(super) struct ActiveLink {
    task: TaskKey,
    epoch: String,
    generation: i64,
    participants: [VerifiedSession; 2],
}

impl ActiveLink {
    pub fn require_live(&self, live: &[VerifiedSession]) -> Result<(), TeamError> {
        for participant in &self.participants {
            if !live
                .iter()
                .any(|session| session.session_id == participant.session_id)
            {
                return Err(TeamError::new(
                    ErrorCode::Unavailable,
                    "approved link participant is absent; delivery paused",
                ));
            }
            if live_participant(live, &participant.session_id)? != participant {
                return Err(TeamError::new(
                    ErrorCode::Conflict,
                    "link participant identity changed; fresh approval required",
                ));
            }
        }
        Ok(())
    }

    fn peer(&self, sender: &VerifiedSession) -> Result<&VerifiedSession, TeamError> {
        match self
            .participants
            .iter()
            .position(|participant| participant == sender)
        {
            Some(slot) => Ok(&self.participants[1 - slot]),
            None => Err(invalid_mapping()),
        }
    }
}

fn active(transaction: &Transaction<'_>, link_id: &str) -> Result<ActiveLink, TeamError> {
    let stored: Option<(String, String, String, i64)> = transaction.query_row(
        "SELECT l.task_key,l.epoch,l.proposal_id,l.work_generation FROM links l JOIN link_proposals p
         ON p.id=l.proposal_id AND p.revision=l.revision AND p.mapping_hash=l.mapping_hash
         AND p.task_key=l.task_key AND p.work_generation=l.work_generation
         WHERE l.id=?1 AND l.state='active' AND p.status='approved'", [link_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
    let (task, epoch, proposal, generation) = stored.ok_or_else(invalid_mapping)?;
    let task = serde_json::from_str(&task)?;
    let participants: [VerifiedSession; 2] = transaction.prepare("SELECT session_id,agent,project,runtime FROM link_participants WHERE proposal_id=?1 ORDER BY slot")?
        .query_map([proposal], |row| Ok(VerifiedSession { session_id: row.get(0)?, agent: row.get(1)?, project: row.get(2)?, runtime: row.get(3)? }))?
        .collect::<Result<Vec<_>, _>>()?.try_into().map_err(|_| invalid_mapping())?;
    for participant in &participants {
        crate::chief::store::require_non_chief(transaction, &participant.session_id)?;
    }
    Ok(ActiveLink {
        task,
        epoch,
        generation,
        participants,
    })
}

pub(super) fn delivery(
    transaction: &Transaction<'_>,
    message_id: &str,
) -> Result<ActiveLink, TeamError> {
    let (link_id, task, sender, recipient, source_epoch, target_epoch, runtime): (String, String, String, String, String, String, String) = transaction.query_row(
        "SELECT link_id,task_key,sender,recipient,sender_epoch,recipient_epoch,recipient_runtime FROM messages WHERE id=?1 AND scope_kind='link'",
        [message_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)))?;
    let link = active(transaction, &link_id)?;
    if task != serde_json::to_string(&link.task)?
        || source_epoch != link.epoch
        || target_epoch != link.epoch
        || sender == recipient
        || !link
            .participants
            .iter()
            .any(|participant| participant.session_id == sender)
        || !link.participants.iter().any(|participant| {
            participant.session_id == recipient && participant.runtime == runtime
        })
    {
        return Err(invalid_mapping());
    }
    require_open_work(transaction, &link.task, link.generation)?;
    Ok(link)
}

pub fn accept(
    connection: &mut Connection,
    sender: &VerifiedSession,
    message: &LinkSubmission,
    live: &[VerifiedSession],
    now_ms: i64,
) -> Result<TeamReply, TeamError> {
    if !matches!(
        message.kind,
        MessageKind::Notice | MessageKind::Clarification | MessageKind::Reply
    ) || (message.kind == MessageKind::Notice && message.thread_id.is_some())
        || message.body.trim().is_empty()
        || message.body.chars().count() > 4000
        || message
            .body
            .chars()
            .any(|character| character.is_control() && character != '\n')
        || message.submission_key.is_empty()
        || message.submission_key.len() > 128
        || !message
            .submission_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(TeamError::new(
            ErrorCode::Invalid,
            "invalid link coordination message",
        ));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let link = active(&transaction, &message.link_id)?;
    let recipient = link.peer(sender)?;
    link.require_live(live)?;
    require_open_work(&transaction, &link.task, link.generation)?;
    store::sleep::guard_sender(&transaction, &sender.session_id)?;
    let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(&AgentAction::LinkSend {
        message: message.clone(),
    })?);
    let previous: Option<(String, String, String)> = transaction
        .query_row(
            "SELECT id,fingerprint,envelope FROM messages WHERE sender=?1 AND submission_key=?2",
            params![sender.session_id, message.submission_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((id, original, envelope)) = previous {
        if original != fingerprint {
            return Err(TeamError::new(
                ErrorCode::Conflict,
                "submission key already names different contents",
            ));
        }
        let envelope: Value = serde_json::from_str(&envelope)?;
        return Ok(TeamReply {
            request_id: message.submission_key.clone(),
            result: Ok(
                json!({"message_id":id,"thread_id":envelope["thread_id"],"round":envelope["round"]}),
            ),
        });
    }
    let task_key = serde_json::to_string(&link.task)?;
    let pending: i64 = transaction.query_row("SELECT count(*) FROM messages WHERE task_key=?1 AND state IN ('queued','ready','submitting','uncertain')",
        [&task_key], |row| row.get(0))?;
    if pending >= 100 {
        return Err(TeamError::new(
            ErrorCode::Limit,
            "collaboration queue limit reached",
        ));
    }
    store::guard_sender_budget(&transaction, &sender.session_id, now_ms)?;
    let submission = super::Submission {
        task: link.task.clone(),
        submission_key: message.submission_key.clone(),
        recipient: recipient.session_id.clone(),
        kind: message.kind.clone(),
        body: message.body.clone(),
        thread_id: message.thread_id.clone(),
        handoff_id: None,
    };
    let thread = match message.kind {
        MessageKind::Clarification => match store::thread_request(
            &transaction,
            &task_key,
            Some(&message.link_id),
            sender,
            &submission,
            now_ms,
        )? {
            store::ThreadStep::Proceed(id, round) => Some((id, round)),
            store::ThreadStep::Escalated => {
                transaction.commit()?;
                return Err(TeamError::new(
                    ErrorCode::Limit,
                    "three clarification rounds are used up; this question is escalated to Theo",
                ));
            }
        },
        MessageKind::Reply => Some(store::thread_reply(
            &transaction,
            &task_key,
            Some(&message.link_id),
            sender,
            &submission,
        )?),
        _ => None,
    };
    let (thread_id, round) = thread.map_or((None, None), |(id, round)| (Some(id), Some(round)));
    let id = store::identifier()?;
    let envelope = json!({"message_id":id,"scope_kind":"link","link_id":message.link_id,"task":link.task,"kind":message.kind,
        "body":message.body,"thread_id":thread_id,"round":round,
        "sender":{"session_id":sender.session_id,"agent":sender.agent,"project":sender.project},
        "recipient":{"session_id":recipient.session_id,"agent":recipient.agent,"project":recipient.project},
        "authorization":"collaborator_content_not_user_authorization"});
    transaction.execute("INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime,scope_kind,link_id)
        VALUES (?1,?2,?3,?4,?5,?5,?6,?7,?8,'queued',?9,?9,?10,'link',?11)",
        params![id,task_key,sender.session_id,recipient.session_id,link.epoch,message.submission_key,fingerprint,envelope.to_string(),now_ms,recipient.runtime,message.link_id])?;
    transaction.commit()?;
    Ok(TeamReply {
        request_id: message.submission_key.clone(),
        result: Ok(json!({"message_id":id,"thread_id":thread_id,"round":round})),
    })
}

pub fn assess(
    connection: &mut Connection,
    sender: &VerifiedSession,
    link_id: &str,
    thread_id: &str,
    resolved: bool,
) -> Result<TeamReply, TeamError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let link = active(&transaction, link_id)?;
    link.peer(sender)?;
    require_open_work(&transaction, &link.task, link.generation)?;
    store::sleep::guard_sender(&transaction, &sender.session_id)?;
    let reply = store::assess_thread(
        &transaction,
        &serde_json::to_string(&link.task)?,
        Some(link_id),
        sender,
        thread_id,
        resolved,
        crate::util::now_secs() * 1000,
    )?;
    transaction.commit()?;
    Ok(reply)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedWork {
    pub task: TaskKey,
    pub title: String,
    pub status: String,
}

/// Enroll new work inside the proposal's transaction. Existing closure evidence is never reset.
pub fn enroll_work(
    transaction: &Transaction<'_>,
    task: &TaskKey,
) -> Result<(SharedWork, i64), TeamError> {
    lock_work(transaction)?;
    let work = crate::todo::read_shared_work(task)?;
    let task_key = serde_json::to_string(task)?;
    let lifecycle = work_generation(transaction, task)?;
    if let Some((generation, state)) = lifecycle {
        if state != "open" {
            return Err(TeamError::new(
                ErrorCode::Conflict,
                "shared work is closed; explicitly reopen the TODO first",
            ));
        }
        return Ok((work, generation));
    }
    refuse_lost_history(transaction, &task_key)?;
    transaction.execute(
        "INSERT INTO link_work VALUES (?1,?2,?3,0,'open',?4)",
        params![
            task_key,
            task.project,
            task.task_id,
            crate::util::now_secs() * 1000
        ],
    )?;
    Ok((work, 0))
}

/// Approval, intake and final dispatch must call this in their committing transaction.
/// It never enrolls missing lifecycle rows or trusts an earlier TODO snapshot.
pub fn require_open_work(
    transaction: &Transaction<'_>,
    task: &TaskKey,
    generation: i64,
) -> Result<SharedWork, TeamError> {
    lock_work(transaction)?;
    match work_generation(transaction, task)? {
        Some((current, state)) if current == generation && state == "open" => {
            crate::todo::read_shared_work(task)
        }
        _ => Err(TeamError::new(
            ErrorCode::Conflict,
            "shared work lifecycle is closed, missing or stale",
        )),
    }
}

fn lock_work(transaction: &Transaction<'_>) -> Result<(), TeamError> {
    // Also acquire the writer boundary if a future caller accidentally begins a deferred transaction.
    transaction.execute("UPDATE link_work SET updated_at=updated_at WHERE 0", [])?;
    Ok(())
}

fn work_generation(
    connection: &Connection,
    task: &TaskKey,
) -> Result<Option<(i64, String)>, TeamError> {
    Ok(connection.query_row("SELECT generation,state FROM link_work WHERE task_key=?1 AND project=?2 AND task_id=?3",
        params![serde_json::to_string(task)?, task.project, task.task_id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?)
}

fn refuse_lost_history(connection: &Connection, task_key: &str) -> Result<(), TeamError> {
    let has_history: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM link_proposals WHERE task_key=?1)
        OR EXISTS(SELECT 1 FROM links WHERE task_key=?1)",
        [task_key],
        |row| row.get(0),
    )?;
    if has_history {
        return Err(TeamError::new(
            ErrorCode::Storage,
            "shared work lifecycle history is missing; approvals cannot be restored",
        ));
    }
    Ok(())
}

/// Standalone host lifecycle closure. Use TodoList for JSON mutations so its guard spans both writes.
pub fn invalidate_work(
    connection: &mut Connection,
    task: &TaskKey,
    reason: &str,
) -> Result<(), TeamError> {
    let guard = crate::todo::lock_mutations()?;
    invalidate_work_guarded(connection, task, reason, &guard)
}

pub(crate) fn invalidate_work_guarded(
    connection: &mut Connection,
    task: &TaskKey,
    reason: &str,
    _guard: &crate::todo::MutationGuard,
) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let task_key = serde_json::to_string(task)?;
    if work_generation(&transaction, task)?.is_none() {
        refuse_lost_history(&transaction, &task_key)?;
    }
    let state = match reason {
        "done" => "done",
        "deleted" => "deleted",
        _ => "blocked",
    };
    let now = crate::util::now_secs() * 1000;
    transaction.execute("INSERT INTO link_work (task_key,project,task_id,generation,state,updated_at) VALUES (?1,?2,?3,1,?4,?5)
        ON CONFLICT(task_key) DO UPDATE SET generation=generation+1,state=excluded.state,updated_at=excluded.updated_at",
        params![task_key, task.project, task.task_id, state, now])?;
    transaction.execute("UPDATE links SET state='closed',closure_reason=?2,closed_at=?3 WHERE task_key=?1 AND state='active'",
        params![task_key, reason, now])?;
    transaction.execute("UPDATE link_proposals SET status='closed',closure_reason=?2 WHERE task_key=?1 AND status='pending'", params![task_key, reason])?;
    transaction.execute(
        "UPDATE messages SET state='cancelled',last_error=?2 WHERE scope_kind='link'
        AND link_id IN (SELECT id FROM links WHERE task_key=?1) AND state IN ('queued','ready')",
        params![task_key, reason],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Recheck already durable open JSON. TodoList holds the same guard across its explicit reopen.
pub fn reopen_work(connection: &mut Connection, task: &TaskKey) -> Result<(), TeamError> {
    let guard = crate::todo::lock_mutations()?;
    reopen_work_guarded(connection, task, &guard)
}

pub(crate) fn reopen_work_guarded(
    connection: &mut Connection,
    task: &TaskKey,
    _guard: &crate::todo::MutationGuard,
) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    crate::todo::read_shared_work(task)?;
    let changed = transaction.execute("UPDATE link_work SET state='open',updated_at=?2 WHERE task_key=?1 AND project=?3 AND task_id=?4",
        params![serde_json::to_string(task)?, crate::util::now_secs() * 1000, task.project, task.task_id])?;
    if changed != 1 {
        return Err(TeamError::new(
            ErrorCode::Storage,
            "shared work lifecycle is missing",
        ));
    }
    transaction.commit()?;
    Ok(())
}

/// Link schema (11) and shared delivery gates (12) join the store's writer transaction.
pub fn migrate(connection: &Connection) -> Result<(), TeamError> {
    let transaction = if connection.is_autocommit() {
        Some(connection.unchecked_transaction()?)
    } else {
        None
    };
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if !(10..=17).contains(&version) {
        return Err(TeamError::new(
            ErrorCode::Storage,
            "unsupported link database version",
        ));
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
    if version < 12 {
        connection.execute_batch("CREATE TABLE delivery_recipients (
            session_id TEXT NOT NULL, runtime TEXT NOT NULL, last_idle_turn TEXT, PRIMARY KEY(session_id,runtime));
            INSERT INTO delivery_recipients SELECT session_id,runtime,last_idle_turn FROM members;
            DROP INDEX IF EXISTS one_inflight_delivery;
            UPDATE attempts SET state='uncertain' WHERE state='submitting' AND message_id IN (
                SELECT id FROM messages WHERE state='submitting' AND (recipient,recipient_runtime) IN (
                    SELECT recipient,recipient_runtime FROM messages WHERE state='submitting' GROUP BY recipient,recipient_runtime HAVING count(*)>1));
            UPDATE messages SET state='uncertain',last_error='legacy concurrent deliveries require reconciliation' WHERE state='submitting'
                AND (recipient,recipient_runtime) IN (SELECT recipient,recipient_runtime FROM messages WHERE state='submitting'
                    GROUP BY recipient,recipient_runtime HAVING count(*)>1);
            CREATE UNIQUE INDEX one_inflight_delivery ON messages(recipient,recipient_runtime) WHERE state='submitting';
            ALTER TABLE threads ADD COLUMN scope_kind TEXT NOT NULL DEFAULT 'team' CHECK(scope_kind IN ('team','link'));
            ALTER TABLE threads ADD COLUMN link_id TEXT REFERENCES links(id);
            PRAGMA user_version=12;")?;
    }
    if let Some(transaction) = transaction {
        transaction.commit()?;
    }
    Ok(())
}
