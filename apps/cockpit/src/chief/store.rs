//! Protected chief grants, revision-bound host approvals and native-inbox ownership.

use super::{ChiefAgentAction, ChiefHostAction, WorkBrief, WorkRef, WorkerState};
use crate::teams::{ErrorCode, TeamError, TeamReply, VerifiedSession};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

fn conflict(message: &str) -> TeamError { TeamError::new(ErrorCode::Conflict, message) }
fn unauthorized() -> TeamError { TeamError::new(ErrorCode::Unauthorized, "no active chief grant for this verified runtime") }
fn unavailable() -> TeamError { TeamError::new(ErrorCode::Unavailable, "this chief action is unavailable") }

fn exact_live(session: &VerifiedSession, live: &[WorkerState]) -> bool {
    live.iter().any(|worker| worker.session == *session)
}

pub fn is_product_worker(connection: &Connection, session: &str) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM members WHERE session_id=?1) OR EXISTS(SELECT 1 FROM chief_worker_bindings WHERE session_id=?1 AND state='bound') OR EXISTS(SELECT 1 FROM chief_assignments WHERE worker=?1 AND state NOT IN ('completed','cancelled'))",
        [session], |row| row.get(0))?)
}

pub fn require_non_chief(connection: &Connection, session: &str) -> Result<(), TeamError> {
    if connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE session_id=?1 AND state IN ('active','paused'))", [session], |row| row.get::<_, bool>(0))? {
        return Err(TeamError::new(ErrorCode::Unauthorized, "disable the chief grant before acquiring a product-worker role"));
    }
    Ok(())
}

pub fn host(connection: &mut Connection, action: &ChiefHostAction, live: &[WorkerState], now_ms: i64) -> Result<TeamReply, TeamError> {
    let request_id = match action {
        ChiefHostAction::Enable { request_id, .. } | ChiefHostAction::Pause { request_id, .. }
        | ChiefHostAction::Resume { request_id, .. } | ChiefHostAction::Disable { request_id, .. }
        | ChiefHostAction::ApproveWork { request_id, .. } | ChiefHostAction::RevokeWork { request_id, .. }
        | ChiefHostAction::BindWorker { request_id, .. } | ChiefHostAction::Reconcile { request_id, .. }
        | ChiefHostAction::Resolve { request_id, .. } | ChiefHostAction::RequestSummary { request_id }
        | ChiefHostAction::AttestEvidence { request_id, .. } | ChiefHostAction::ImportEvidence { request_id, .. } | ChiefHostAction::ReconcileDelivery {request_id,..} | ChiefHostAction::ReconcileLaunch {request_id,..} => request_id.clone(),
    };
    let result = (|| {
        action.validate()?;
        let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(action)?);
        let transaction = connection.transaction()?;
        let previous: Option<(String, String)> = transaction.query_row("SELECT fingerprint,response FROM chief_actions WHERE request_id=?1", [&request_id],
            |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        if let Some((old, response)) = previous {
            if old != fingerprint { return Err(conflict("chief host request id reused with different content")); }
            return Ok(serde_json::from_str(&response)?);
        }
        let response = host_action(&transaction, action, live, now_ms)?;
        if !matches!(action,ChiefHostAction::Enable {..}|ChiefHostAction::Resolve {..}|ChiefHostAction::RequestSummary {..}) {
            coordination_event(&transaction,"host_control_changed",&json!({"source":"host","request_id":request_id}),now_ms)?;
        }
        transaction.execute("INSERT INTO chief_actions VALUES (?1,?2,?3,?4)", params![request_id, fingerprint, serde_json::to_string(&response)?, now_ms])?;
        transaction.commit()?;
        Ok(response)
    })();
    Ok(TeamReply { request_id, result })
}

fn host_action(connection: &Connection, action: &ChiefHostAction, live: &[WorkerState], now_ms: i64) -> Result<Value, TeamError> {
    match action {
        ChiefHostAction::Enable { session, .. } => {
            if !exact_live(session, live) || is_product_worker(connection, &session.session_id)? {
                return Err(unauthorized());
            }
            if connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE state IN ('active','paused'))", [], |row| row.get::<_, bool>(0))? {
                return Err(conflict("a global chief already owns scheduling"));
            }
            let epoch = crate::teams::store::identifier()?;
            let approval_hash = crate::audit::sha256_hex(&serde_json::to_vec(action)?);
            connection.execute("INSERT INTO chief_grants VALUES (?1,?2,?3,?4,?5,?1,1,'active',?6,?7)",
                params![epoch, session.session_id, session.agent, session.project, session.runtime, approval_hash, now_ms])?;
            connection.execute("INSERT INTO chief_coordination(epoch) VALUES (?1)", [&epoch])?;
            coordination_event(connection,"bootstrap",&json!({"epoch":epoch}),now_ms)?;
            schedule_wake(connection,now_ms)?;
            Ok(json!({"epoch": epoch, "state": "active"}))
        },
        ChiefHostAction::Pause { expected_epoch, .. } | ChiefHostAction::Resume { expected_epoch, .. } | ChiefHostAction::Disable { expected_epoch, .. } => {
            let state: Option<String> = connection.query_row("SELECT state FROM chief_grants WHERE epoch=?1 AND state!='disabled'", [expected_epoch], |row| row.get(0)).optional()?;
            let Some(state) = state else { return Err(conflict("chief grant epoch is stale")) };
            if matches!(action, ChiefHostAction::Resume { .. }) {
                let session = connection.query_row("SELECT session_id,agent,project,runtime FROM chief_grants WHERE epoch=?1", [expected_epoch], |row| Ok(VerifiedSession {
                    session_id: row.get(0)?, agent: row.get(1)?, project: row.get(2)?, runtime: row.get(3)?,
                }))?;
                if !exact_live(&session, live) || is_product_worker(connection, &session.session_id)? { return Err(unauthorized()); }
            }
            let next = match action {
                ChiefHostAction::Pause { .. } if state == "active" => "paused",
                ChiefHostAction::Resume { .. } if state == "paused" => "active",
                ChiefHostAction::Disable { .. } => "disabled",
                _ => return Err(conflict("chief grant is not in the expected lifecycle state")),
            };
            connection.execute("UPDATE chief_grants SET state=?2,revision=revision+1 WHERE epoch=?1", params![expected_epoch, next])?;
            if next == "disabled" {
                connection.execute("UPDATE chief_inbox SET state='cancelled' WHERE chief_epoch=?1 AND state='queued' AND json_extract(envelope,'$.kind')!='host_stop_notice'
                    AND NOT EXISTS(SELECT 1 FROM chief_delivery_attempts a WHERE json_extract(a.envelope,'$.message_id')=chief_inbox.id AND a.state<>'not_submitted')", [expected_epoch])?;
            }
            Ok(json!({"epoch": expected_epoch, "state": next}))
        },
        ChiefHostAction::BindWorker { session, role, expected_epoch, .. } => {
            if !exact_live(session, live) { return Err(unauthorized()); }
            if connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE session_id=?1 AND state!='disabled')", [&session.session_id], |row| row.get::<_, bool>(0))? {
                return Err(conflict("the chief cannot be a product worker"));
            }
            let old: Option<(String, i64)> = connection.query_row("SELECT epoch,revision FROM chief_worker_bindings WHERE session_id=?1 AND state='bound'", [&session.session_id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
            if old.as_ref().map(|(epoch, _)| epoch) != expected_epoch.as_ref() { return Err(conflict("worker binding epoch is stale")); }
            let revision = old.as_ref().map_or(1, |(_, revision)| revision + 1);
            if let Some((epoch, _)) = old {
                if connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_assignments WHERE worker=?1 AND state NOT IN ('completed','cancelled'))", [&session.session_id], |row| row.get::<_, bool>(0))? {
                    return Err(conflict("worker has an open assignment; reconcile before rebinding"));
                }
                connection.execute("UPDATE chief_worker_bindings SET state='revoked' WHERE epoch=?1", [epoch])?;
            }
            let epoch = crate::teams::store::identifier()?;
            let hash = crate::audit::sha256_hex(&serde_json::to_vec(action)?);
            connection.execute("INSERT INTO chief_worker_bindings VALUES (?1,?2,?3,?4,?5,?6,?7,'bound',?8,?9)",
                params![session.session_id, session.agent, session.project, session.runtime, epoch, serde_json::to_value(role)?.as_str(), revision, hash, now_ms])?;
            Ok(json!({"epoch": epoch, "revision": revision, "role": role}))
        },
        ChiefHostAction::ApproveWork { brief, approval_hash, expected_revision, .. } => {
            let latest = latest_revision(connection, &brief.work.task)?;
            let pending: Option<String> = connection.query_row("SELECT approval_hash FROM chief_work_proposals WHERE project=?1 AND task_id=?2 AND revision=?3",
                params![brief.work.task.project,brief.work.task.task_id,latest], |row|row.get(0)).optional()?;
            let already_approved: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_work_revisions WHERE project=?1 AND task_id=?2 AND revision=?3)",
                params![brief.work.task.project,brief.work.task.task_id,latest], |row|row.get(0))?;
            let exact_proposal = !already_approved && brief.work.revision == latest && pending.as_ref() == Some(approval_hash);
            if !exact_proposal && latest != *expected_revision { return Err(conflict("work revision or proposal is stale")); }
            let approved = current_briefs(connection)?;
            for prerequisite in &brief.prerequisites {
                if !approved.iter().any(|previous| previous.work == *prerequisite) { return Err(conflict("prerequisite revision is not currently approved")); }
                if reaches_task(connection, prerequisite, &brief.work, &mut Vec::new())? { return Err(conflict("chief dependency cycle")); }
            }
            invalidate(connection, &brief.work.task, &approved)?;
            connection.execute("INSERT INTO chief_work_revisions (project,task_id,revision,brief,approval_hash,state,created_at) VALUES (?1,?2,?3,?4,?5,'approved',?6)",
                params![brief.work.task.project, brief.work.task.task_id, brief.work.revision, serde_json::to_string(brief)?, approval_hash, now_ms])?;
            Ok(json!({"work": brief.work, "state": "approved"}))
        },
        ChiefHostAction::RevokeWork { work, .. } => {
            revoke_work(connection,work,now_ms)?;
            Ok(json!({"work": work, "state": "revoked"}))
        },
        ChiefHostAction::AttestEvidence { evidence, .. } => {
            record_evidence_in(connection, evidence, now_ms)?;
            let completed = complete_ready_in(connection, now_ms)?;
            Ok(json!({"recorded": evidence.id, "completed": completed}))
        },
        ChiefHostAction::ImportEvidence { work, gate, source_id, .. } => import_source(connection, work, *gate, source_id, now_ms),
        ChiefHostAction::Reconcile { assignment, expected_revision, evidence, .. } => {
            let revision: Option<i64> = connection.query_row("SELECT revision FROM chief_assignments WHERE id=?1 AND state IN ('uncertain','blocked')", [assignment], |row|row.get(0)).optional()?;
            if revision != Some(*expected_revision) { return Err(conflict("only the exact unresolved assignment can be reconciled")); }
            let state = match evidence {
                super::ReconciliationEvidence::Handled { .. } => "received",
                super::ReconciliationEvidence::SafeStop { confirmed_stopped:true, .. } => "cancelled",
                _ => return Err(conflict("host must explicitly confirm old worker stopped and unable to execute pending work")),
            };
            connection.execute("UPDATE chief_assignments SET state=?2,revision=revision+1 WHERE id=?1", params![assignment,state])?;
            connection.execute("UPDATE chief_inbox SET state=?2 WHERE assignment=?1 AND state IN ('queued','submitting','uncertain','failed')", params![assignment,state])?;
            connection.execute("UPDATE chief_delivery_attempts SET state=?2,finished_at=?3 WHERE assignment=?1 AND state IN ('submitting','uncertain')", params![assignment,state,now_ms])?;
            if state == "cancelled" {
                connection.execute("DELETE FROM scheduling_reservations WHERE token=?1 AND owner='chief'", [assignment])?;
                cancel_stopped_inbox(connection, assignment, now_ms)?;
            }
            material_event(connection, assignment, "reconciled", &serde_json::to_value(evidence)?, now_ms)?;
            Ok(json!({"assignment":assignment,"state":state}))
        },
        ChiefHostAction::ReconcileLaunch {token,expected_revision,evidence,..} => {
            let state=match evidence {super::ReconciliationEvidence::Handled {..}=>"submitted",super::ReconciliationEvidence::SafeStop {confirmed_stopped:true,..}=>"released",_=>return Err(conflict("explicit exact-launch safe stop is required"))};
            if connection.execute("UPDATE legacy_launches SET state=?3,revision=revision+1 WHERE token=?1 AND revision=?2 AND state IN ('reserved','uncertain','submitted')",params![token,expected_revision,state])?!=1 {return Err(conflict("exact unresolved launch revision is required"));}
            if state=="released" {connection.execute("DELETE FROM scheduling_reservations WHERE token=?1 AND owner='legacy'",[token])?;}
            connection.execute("UPDATE chief_decisions SET state='resolved',revision=revision+1,resolution=?2,resolved_at=?3 WHERE scope_key=?1 AND state='open'",params![format!("host-launch-{token}"),serde_json::to_string(evidence)?,now_ms])?;
            Ok(json!({"token":token,"state":state,"verified_delivery":false,"completion":false}))
        },
        ChiefHostAction::ReconcileDelivery {message,expected_revision,evidence,..} => {
            let record: Option<(String,String,i64)> = connection.query_row("SELECT envelope,recipient_identity,attempts FROM chief_inbox WHERE id=?1 AND assignment IS NULL AND state IN ('uncertain','failed')",[message],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
            let (encoded,identity,revision)=record.ok_or_else(||conflict("exact unresolved delivery is required"))?;
            if revision!=*expected_revision {return Err(conflict("delivery revision changed"));}
            let envelope:Value=serde_json::from_str(&encoded)?;
            let stopped=match evidence {super::ReconciliationEvidence::Handled {..}=>false,super::ReconciliationEvidence::SafeStop {confirmed_stopped:true,..}=>true,_=>return Err(conflict("explicit exact-target safe stop is required"))};
            if stopped && envelope["kind"]!="chief_wake" {
                let assignment=delivery_assignment(connection,message)?.ok_or_else(||conflict("delivery has no exact recipient assignment to stop"))?;
                connection.execute("UPDATE chief_assignments SET state='cancelled',revision=revision+1 WHERE id=?1",[&assignment])?;
                connection.execute("DELETE FROM scheduling_reservations WHERE token=?1 AND owner='chief'",[&assignment])?;
                cancel_stopped_inbox(connection,&assignment,now_ms)?;
            }
            // A host handling assertion is a tombstone, never a native receipt or chief-turn allowance.
            connection.execute("UPDATE chief_coordination SET failed=1 WHERE epoch IN (SELECT chief_epoch FROM chief_inbox WHERE id=?1 AND json_extract(envelope,'$.kind')='chief_wake')",[message])?;
            connection.execute("UPDATE chief_inbox SET state='cancelled' WHERE id=?1",[message])?;
            connection.execute("UPDATE chief_delivery_attempts SET state='cancelled',finished_at=?2 WHERE json_extract(envelope,'$.message_id')=?1",params![message,now_ms])?;
            connection.execute("UPDATE chief_decisions SET state='resolved',revision=revision+1,resolution=?2,resolved_at=?3 WHERE scope_key=?1 AND state='open'",params![format!("host-delivery-{message}"),serde_json::to_string(evidence)?,now_ms])?;
            Ok(json!({"message":message,"recipient":serde_json::from_str::<Value>(&identity)?,"state":"cancelled","handled":!stopped,"verified_delivery":false,"completion":false}))
        },
        ChiefHostAction::Resolve { decision, expected_revision, resolution, .. } => {
            if connection.execute("UPDATE chief_decisions SET state='resolved',revision=revision+1,resolution=?3,resolved_at=?4 WHERE id=?1 AND revision=?2 AND state='open'",
                params![decision,expected_revision,resolution,now_ms])? != 1 { return Err(conflict("decision revision is stale or already answered")); }
            let (revision,work,assignment): (i64,Option<String>,Option<String>) = connection.query_row("SELECT revision,work,assignment FROM chief_decisions WHERE id=?1",[decision],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
            let work = work.map(|encoded|serde_json::from_str::<WorkRef>(&encoded)).transpose()?;
            coordination_event(connection,"decision_resolved",&json!({"decision":decision,"decision_revision":revision,"work":work,"assignment":assignment}),now_ms)?;
            Ok(json!({"decision":decision,"state":"resolved","grants_authority":false}))
        },
        ChiefHostAction::RequestSummary { request_id } => {
            let pending: i64 = connection.query_row("SELECT count(*) FROM chief_summary_requests WHERE state IN ('queued','pending')",[],|row|row.get(0))?;
            if pending >= 1 { return Err(TeamError::new(ErrorCode::Limit,"a bounded summary request is already pending")); }
            let epoch: Option<String> = connection.query_row("SELECT epoch FROM chief_grants WHERE state IN ('active','paused')",[],|row|row.get(0)).optional()?;
            connection.execute("INSERT INTO chief_summary_requests(id,epoch,state,created_at,deadline) VALUES (?1,?2,'queued',?3,?4)",params![request_id,epoch,now_ms,now_ms.saturating_add(900_000)])?;
            let state = if epoch.is_none() {
                connection.execute("UPDATE chief_summary_requests SET state='failed' WHERE id=?1",[request_id])?;
                decision_record(connection,&format!("host-summary-{request_id}"),None,None,"unavailable","Requested summary cannot be delivered: no selected chief is available.",
                    "Host must select a verified supported chief before requesting another summary. Previous summary results remain unchanged.",now_ms)?;
                "failed"
            } else {"queued"};
            coordination_event(connection,"summary_requested",&json!({"request_id":request_id}),now_ms)?;
            Ok(json!({"request_id":request_id,"state":state}))
        },
    }
}

fn cancel_stopped_inbox(connection: &Connection, assignment: &str, now_ms: i64) -> Result<(), TeamError> {
    // Coordination's sender work can differ from the stopped recipient's work; identity and epoch cannot.
    let messages = connection.prepare("SELECT i.id FROM chief_inbox i JOIN chief_inbox stopped ON stopped.assignment=?1
        WHERE i.recipient_identity=stopped.recipient_identity AND i.recipient_epoch=stopped.recipient_epoch
        AND (i.assignment=?1 OR (json_extract(i.envelope,'$.kind')='host_stop_notice' AND json_extract(i.envelope,'$.assignment')=?1)
            OR (json_extract(i.envelope,'$.kind')='chief_coordinate'
            AND coalesce(json_extract(i.envelope,'$.recipient_work.task.project'),json_extract(i.work,'$.task.project'))=json_extract(stopped.work,'$.task.project')
            AND coalesce(json_extract(i.envelope,'$.recipient_work.task.task_id'),json_extract(i.work,'$.task.task_id'))=json_extract(stopped.work,'$.task.task_id')
            AND coalesce(json_extract(i.envelope,'$.recipient_work.revision'),json_extract(i.work,'$.revision'))=json_extract(stopped.work,'$.revision')))")?
        .query_map([assignment], |row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for message in messages {
        connection.execute("UPDATE chief_inbox SET state='cancelled' WHERE id=?1", [&message])?;
        connection.execute("UPDATE chief_delivery_attempts SET state='cancelled',finished_at=?2 WHERE json_extract(envelope,'$.message_id')=?1", params![message,now_ms])?;
    }
    Ok(())
}

fn delivery_assignment(connection: &Connection, message: &str) -> Result<Option<String>, TeamError> {
    Ok(connection.query_row("SELECT a.id FROM chief_inbox i JOIN chief_assignments a ON a.worker=i.recipient
        AND a.worker_runtime=json_extract(i.recipient_identity,'$.runtime') AND a.worker_epoch=i.recipient_epoch
        WHERE i.id=?1 AND a.project=coalesce(json_extract(i.envelope,'$.recipient_work.task.project'),json_extract(i.work,'$.task.project'))
        AND a.task_id=coalesce(json_extract(i.envelope,'$.recipient_work.task.task_id'),json_extract(i.work,'$.task.task_id'))
        AND a.work_revision=coalesce(json_extract(i.envelope,'$.recipient_work.revision'),json_extract(i.work,'$.revision'))
        AND a.id=coalesce(json_extract(i.envelope,'$.recipient_assignment'),json_extract(i.envelope,'$.assignment'))",[message],|row|row.get(0)).optional()?)
}

fn current_briefs(connection: &Connection) -> Result<Vec<WorkBrief>, TeamError> {
    let mut statement = connection.prepare("SELECT brief FROM chief_work_revisions WHERE state IN ('approved','completed')")?;
    let encoded = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    encoded.iter().map(|brief| Ok(serde_json::from_str(brief)?)).collect()
}

fn revoke_work(connection: &Connection, work: &WorkRef, now_ms: i64) -> Result<(), TeamError> {
    let briefs = current_briefs(connection)?;
    if !briefs.iter().any(|brief|brief.work == *work) { return Err(conflict("revocation requires the exact current approved revision")); }
    let mut affected = vec![work.clone()];
    for _ in 0..briefs.len() {
        for brief in &briefs {
            if !affected.contains(&brief.work) && brief.prerequisites.iter().any(|dependency|affected.contains(dependency)) { affected.push(brief.work.clone()); }
        }
    }
    for target in affected {
        connection.execute("UPDATE chief_work_revisions SET state=?4 WHERE project=?1 AND task_id=?2 AND revision=?3",
            params![target.task.project,target.task.task_id,target.revision,if target==*work {"revoked"} else {"superseded"}])?;
        let claims = connection.prepare("SELECT id,state,chief_epoch FROM chief_assignments WHERE project=?1 AND task_id=?2 AND work_revision=?3 AND state NOT IN ('completed','cancelled')")?
            .query_map(params![target.task.project,target.task.task_id,target.revision],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
        for (assignment,state,chief_epoch) in claims {
            let submitted: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_delivery_attempts WHERE assignment=?1 AND state!='not_submitted')",[&assignment],|row|row.get(0))?;
            let unsent = matches!(state.as_str(),"queued"|"blocked") && !submitted;
            connection.execute("UPDATE chief_assignments SET state=?2,revision=revision+1 WHERE id=?1",params![assignment,if unsent {"cancelled"} else {"uncertain"}])?;
            if unsent {
                connection.execute("DELETE FROM scheduling_reservations WHERE token=?1 AND owner='chief'",[&assignment])?;
            } else {
                let (identity,epoch): (String,String) = connection.query_row("SELECT recipient_identity,recipient_epoch FROM chief_inbox WHERE assignment=?1",[&assignment],|row|Ok((row.get(0)?,row.get(1)?)))?;
                let recipient: VerifiedSession = serde_json::from_str(&identity)?;
                let id = crate::teams::store::identifier()?;
                let envelope = json!({"message_id":id,"kind":"host_stop_notice","assignment":assignment,"work":target,
                    "recipient":recipient,"recipient_epoch":epoch,"sender":{"role":"host"},
                    "body":"Host revoked this exact work. Stop at a safe boundary; completed effects are not undone. This notice grants no work, tool, rollback or execution authority."});
                connection.execute("INSERT INTO chief_events (id,source,submission_key,fingerprint,work,assignment,kind,body,created_at) VALUES (?1,'host',?1,?2,?3,?4,'host_stop_notice',?5,?6)",
                    params![id,crate::audit::sha256_hex(&serde_json::to_vec(&envelope)?),serde_json::to_string(&target)?,assignment,envelope.to_string(),now_ms])?;
                connection.execute("INSERT INTO chief_inbox (id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'queued',?8)",
                    params![id,chief_epoch,recipient.session_id,identity,epoch,serde_json::to_string(&target)?,envelope.to_string(),now_ms])?;
            }
            material_event(connection,&assignment,"work_revoked",&json!({"work":target,"previous_state":state,"ownership_retained":!unsent}),now_ms)?;
        }
        // Only conclusively unsent product messages are cancelled; uncertain native admission stays owned.
        connection.execute("UPDATE chief_inbox SET state='cancelled' WHERE state='queued' AND json_extract(envelope,'$.kind')!='host_stop_notice'
            AND (work=?1 OR json_extract(envelope,'$.recipient_work')=json(?1))
            AND NOT EXISTS(SELECT 1 FROM chief_delivery_attempts a WHERE json_extract(a.envelope,'$.message_id')=chief_inbox.id AND a.state!='not_submitted')",
            [serde_json::to_string(&target)?])?;
    }
    Ok(())
}

pub(crate) fn upgrade_evidence(connection: &Connection) -> Result<(), TeamError> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS chief_sources (
        id TEXT PRIMARY KEY, evidence TEXT NOT NULL, approval_hash TEXT NOT NULL, imported INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL);
        CREATE TRIGGER IF NOT EXISTS chief_source_immutable BEFORE UPDATE OF id,evidence,approval_hash,created_at ON chief_sources
        BEGIN SELECT RAISE(ABORT,'host source record is immutable'); END;")?;
    connection.execute_batch("CREATE TABLE IF NOT EXISTS chief_handoffs (handoff_id TEXT PRIMARY KEY, work TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS chief_apply_reviews (id TEXT PRIMARY KEY, review TEXT NOT NULL, created_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS chief_qa_results (sender TEXT NOT NULL, submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
            evidence_id TEXT NOT NULL, PRIMARY KEY(sender,submission_key));")?;
    Ok(())
}

/// Host producer API: callers must have observed/approved the source, not received an agent report.
pub fn publish_source(connection: &mut Connection, evidence: &super::Evidence, now_ms: i64) -> Result<(), TeamError> {
    evidence.validate()?;
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let hash: String = transaction.query_row("SELECT approval_hash FROM chief_work_revisions WHERE project=?1 AND task_id=?2 AND revision=?3 AND state IN ('approved','completed')",
        params![evidence.work.task.project,evidence.work.task.task_id,evidence.work.revision], |row|row.get(0))?;
    let encoded = serde_json::to_string(evidence)?;
    let previous: Option<String> = transaction.query_row("SELECT evidence FROM chief_sources WHERE id=?1", [&evidence.id], |row|row.get(0)).optional()?;
    if let Some(previous) = previous {
        if previous != encoded { return Err(conflict("host source id reused with conflicting content")); }
    } else {
        transaction.execute("INSERT INTO chief_sources VALUES (?1,?2,?3,0,?4)", params![evidence.id,encoded,hash,now_ms])?;
    }
    transaction.commit()?;
    Ok(())
}

fn import_source(connection: &Connection, work: &WorkRef, gate: super::GateKind, source_id: &str, now_ms: i64) -> Result<Value, TeamError> {
    let source: Option<(String,String)> = connection.query_row("SELECT evidence,approval_hash FROM chief_sources WHERE id=?1", [source_id], |row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    let (encoded,hash) = source.ok_or_else(||conflict("protected host source not found"))?;
    let evidence: super::Evidence = serde_json::from_str(&encoded)?;
    let current_hash: Option<String> = connection.query_row("SELECT approval_hash FROM chief_work_revisions WHERE project=?1 AND task_id=?2 AND revision=?3 AND state IN ('approved','completed')",
        params![work.task.project,work.task.task_id,work.revision], |row|row.get(0)).optional()?;
    if evidence.work != *work || evidence.kind != gate || current_hash.as_ref() != Some(&hash) { return Err(conflict("host source is for a different or historical approval")); }
    record_evidence_in(connection, &evidence, now_ms)?;
    connection.execute("UPDATE chief_sources SET imported=1 WHERE id=?1", [source_id])?;
    let completed = complete_ready_in(connection, now_ms)?;
    Ok(json!({"recorded":source_id,"completed":completed}))
}

pub fn replay_sources(connection: &mut Connection, now_ms: i64) -> Result<(), TeamError> {
    let pending = connection.prepare("SELECT evidence FROM chief_sources WHERE imported=0 ORDER BY rowid")?.query_map([], |row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for encoded in pending {
        let evidence: super::Evidence = serde_json::from_str(&encoded)?;
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        match import_source(&transaction,&evidence.work,evidence.kind,&evidence.id,now_ms) {
            Ok(_) => {},
            Err(error) if error.code == ErrorCode::Conflict => {
                transaction.execute("UPDATE chief_sources SET imported=-1 WHERE id=?1", [&evidence.id])?;
            },
            Err(error) => return Err(error),
        }
        transaction.commit()?;
    }
    Ok(())
}

fn material_event(connection: &Connection, assignment: &str, kind: &str, body: &Value, now_ms: i64) -> Result<(), TeamError> {
    let id = crate::teams::store::identifier()?;
    connection.execute("INSERT INTO chief_events (id,source,submission_key,fingerprint,assignment,kind,body,created_at) VALUES (?1,'host',?1,?2,?3,?4,?5,?6)",
        params![id,crate::audit::sha256_hex(body.to_string().as_bytes()),assignment,kind,body.to_string(),now_ms])?;
    Ok(())
}

pub fn record_evidence(connection: &mut Connection, evidence: &super::Evidence, now_ms: i64) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    record_evidence_in(&transaction, evidence, now_ms)?;
    complete_ready_in(&transaction, now_ms)?;
    transaction.commit()?;
    Ok(())
}

pub(crate) fn record_evidence_in(connection: &Connection, evidence: &super::Evidence, now_ms: i64) -> Result<(), TeamError> {
    evidence.validate()?;
    let encoded: Option<String> = connection.query_row("SELECT brief FROM chief_work_revisions WHERE project=?1 AND task_id=?2 AND revision=?3 AND state IN ('approved','completed')",
        params![evidence.work.task.project,evidence.work.task.task_id,evidence.work.revision], |row|row.get(0)).optional()?;
    let brief: WorkBrief = serde_json::from_str(&encoded.ok_or_else(|| conflict("evidence revision is not current"))?)?;
    let requirement = brief.gates.iter().find(|gate|gate.kind == evidence.kind).ok_or_else(||conflict("gate was not approved"))?;
    if requirement.source != evidence.source || requirement.expected["commit"] != evidence.commit { return Err(conflict("evidence identity differs from approved gate; a different source requires revised exact work approval")); }
    let kind = serde_json::to_value(evidence.kind)?;
    let old: Option<(String,String,i64,String,String,String,String)> = connection.query_row("SELECT project,task_id,work_revision,kind,commit_id,source,result FROM chief_evidence WHERE id=?1", [&evidence.id],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?))).optional()?;
    if let Some((project,task,revision,old_kind,commit,source,result)) = old {
        if project != evidence.work.task.project || task != evidence.work.task.task_id || revision != evidence.work.revision
            || kind != old_kind || commit != evidence.commit || source != evidence.source || serde_json::from_str::<Value>(&result)? != evidence.result {
            return Err(conflict("evidence id reused with conflicting content"));
        }
        return Ok(());
    }
    connection.execute("INSERT INTO chief_evidence VALUES (?1,?2,?3,?4,?5,?6,?7,?1,?8,?9)", params![evidence.id,evidence.work.task.project,evidence.work.task.task_id,
        evidence.work.revision,kind.as_str(),evidence.commit,evidence.source,evidence.result.to_string(),now_ms])?;
    coordination_event(connection,"evidence_recorded",&json!({"work":evidence.work,"evidence_id":evidence.id,"source":evidence.source}),now_ms)?;
    Ok(())
}

pub fn complete_ready(connection: &mut Connection, now_ms: i64) -> Result<Vec<WorkRef>, TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let completed = complete_ready_in(&transaction, now_ms)?;
    transaction.commit()?;
    Ok(completed)
}

pub(crate) fn complete_ready_in(connection: &Connection, now_ms: i64) -> Result<Vec<WorkRef>, TeamError> {
    let mut completed = Vec::new();
    for brief in current_briefs(connection)? {
        // No gates is an explicit non-applicability approval, not evidence that unstarted work ran.
        if brief.gates.is_empty() { continue; }
        let mut ready = match dependencies_complete(connection, &brief) {
            Ok(()) => true,
            Err(error) if error.code == ErrorCode::Conflict => false,
            Err(error) => return Err(error),
        };
        for requirement in &brief.gates {
            let Some(evidence) = latest_gate_evidence(connection,&brief.work,requirement.kind)? else { ready = false; break; };
            if super::evidence::validate(requirement,&evidence).is_err() || !current_evidence(connection,&evidence)? { ready = false; break; }
        }
        if ready {
            let changed = connection.execute("UPDATE chief_work_revisions SET state='completed' WHERE project=?1 AND task_id=?2 AND revision=?3 AND state='approved'", params![brief.work.task.project,brief.work.task.task_id,brief.work.revision])?;
            if changed > 0 {
                coordination_event(connection,"work_completed",&json!({"work":brief.work}),now_ms)?;
                let scope = serde_json::to_string(&brief.work)?;
                if connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_decisions WHERE scope_key=?1 AND decision_key='missing_evidence')",[&scope],|row|row.get::<_,bool>(0))? {
                    decision_record(connection,&scope,Some(&brief.work),None,"missing_evidence","Exact required evidence has now been recorded; host review remains unanswered.",
                        "Review the current evidence. Only host Resolve answers this decision; evidence does not grant permissions.",now_ms)?;
                }
                connection.execute("UPDATE chief_assignments SET finished_at=?4 WHERE project=?1 AND task_id=?2 AND work_revision=?3 AND state NOT IN ('completed','cancelled')",
                    params![brief.work.task.project,brief.work.task.task_id,brief.work.revision,now_ms])?;
                completed.push(brief.work);
            }
        } else {
            let changed = connection.execute("UPDATE chief_work_revisions SET state='approved' WHERE project=?1 AND task_id=?2 AND revision=?3 AND state='completed'", params![brief.work.task.project,brief.work.task.task_id,brief.work.revision])?;
            if changed > 0 {
                coordination_event(connection,"work_evidence_invalidated",&json!({"work":brief.work}),now_ms)?;
                invalidate_dependents(connection, &brief.work, now_ms)?;
            }
            let owns_work: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_assignments a JOIN chief_observations o ON o.session_id=a.worker
                WHERE a.project=?1 AND a.task_id=?2 AND a.work_revision=?3 AND a.state IN ('accepted','working','blocked')
                AND json_extract(o.observation,'$.ready')=1 AND json_extract(o.observation,'$.session.runtime')=a.worker_runtime
                AND json_extract(o.observation,'$.observed_at_ms')>a.created_at)",
                params![brief.work.task.project,brief.work.task.task_id,brief.work.revision], |row|row.get(0))?;
            if owns_work {
                let scope = serde_json::to_string(&brief.work)?;
                decision_record(connection,&scope,Some(&brief.work),None,"missing_evidence","Completion evidence is missing, failed, stale or unsupported.",
                    "Review the exact gate and provide its approved source; a different source requires revised exact work approval.",now_ms)?;
            }
        }
    }
    Ok(completed)
}

fn latest_gate_evidence(connection: &Connection, work: &WorkRef, kind: super::GateKind) -> Result<Option<super::Evidence>, TeamError> {
    let stored: Option<(String,String,String,String)> = connection.query_row("SELECT id,commit_id,source,result FROM chief_evidence WHERE project=?1 AND task_id=?2 AND work_revision=?3 AND kind=?4 ORDER BY rowid DESC LIMIT 1",
        params![work.task.project,work.task.task_id,work.revision,serde_json::to_value(kind)?.as_str()], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
    stored.map(|(id,commit,source,result)|Ok(super::Evidence {id,work:work.clone(),kind,commit,source,result:serde_json::from_str(&result)?})).transpose()
}

fn current_evidence(connection: &Connection, evidence: &super::Evidence) -> Result<bool, TeamError> {
    if evidence.source != "qa" { return Ok(true); }
    let task = serde_json::to_string(&evidence.work.task)?;
    let result = &evidence.result;
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM handoffs h JOIN chief_handoffs c ON c.handoff_id=h.id
        JOIN members qa ON qa.task_key=h.task_key AND qa.role='qa'
        JOIN members dev ON dev.task_key=h.task_key AND dev.role='developer' AND dev.session_id=h.developer AND dev.epoch=h.developer_epoch
        WHERE h.id=?1 AND h.task_key=?2 AND h.status='current' AND qa.session_id=?3 AND qa.runtime=?4 AND qa.epoch=?5 AND c.work=?6)",
        params![result["handoff_id"].as_str(),task,result["reviewer"].as_str(),result["reviewer_runtime"].as_str(),result["reviewer_epoch"].as_str(),serde_json::to_string(&evidence.work)?], |row|row.get(0))?)
}

pub(crate) fn bind_handoff(connection: &Connection, handoff_id: &str, task: &crate::teams::TaskKey, commit: &str) -> Result<(), TeamError> {
    for brief in current_briefs(connection)? {
        if brief.work.task == *task && brief.gates.iter().any(|gate|gate.kind == super::GateKind::Qa && gate.source == "qa" && gate.expected["commit"] == commit) {
            connection.execute("INSERT INTO chief_handoffs VALUES (?1,?2)", params![handoff_id,serde_json::to_string(&brief.work)?])?;
        }
    }
    Ok(())
}

fn invalidate_dependents(connection: &Connection, prerequisite: &WorkRef, now_ms: i64) -> Result<(), TeamError> {
    let briefs = current_briefs(connection)?;
    let mut affected = vec![prerequisite.clone()];
    for _ in 0..briefs.len() {
        for brief in &briefs {
            if !affected.contains(&brief.work) && brief.prerequisites.iter().any(|dependency|affected.contains(dependency)) { affected.push(brief.work.clone()); }
        }
    }
    for work in affected.iter().skip(1) {
        connection.execute("UPDATE chief_work_revisions SET state='superseded' WHERE project=?1 AND task_id=?2 AND revision=?3", params![work.task.project,work.task.task_id,work.revision])?;
        let claims = connection.prepare("SELECT id,state FROM chief_assignments WHERE project=?1 AND task_id=?2 AND work_revision=?3 AND state NOT IN ('completed','cancelled')")?
            .query_map(params![work.task.project,work.task.task_id,work.revision], |row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
        for (assignment,state) in claims {
            let submitted: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_delivery_attempts WHERE assignment=?1 AND state!='not_submitted')", [&assignment], |row|row.get(0))?;
            let next = if state == "queued" && !submitted { "cancelled" } else { "uncertain" };
            connection.execute("UPDATE chief_assignments SET state=?2,revision=revision+1 WHERE id=?1", params![assignment,next])?;
            connection.execute("UPDATE chief_inbox SET state='cancelled' WHERE work=?1 AND state='queued'", [serde_json::to_string(work)?])?;
            if next == "cancelled" { connection.execute("DELETE FROM scheduling_reservations WHERE token=?1 AND owner='chief'", [&assignment])?; }
            material_event(connection,&assignment,"prerequisite_invalidated",&json!({"prerequisite":prerequisite,"work":work,"previous_state":state}),now_ms)?;
        }
    }
    Ok(())
}

fn reaches_task(connection: &Connection, from: &WorkRef, target: &WorkRef, visited: &mut Vec<WorkRef>) -> Result<bool, TeamError> {
    if from.task == target.task { return Ok(true); }
    if visited.contains(from) { return Ok(false); }
    visited.push(from.clone());
    let encoded: String = connection.query_row("SELECT brief FROM chief_work_revisions WHERE project=?1 AND task_id=?2 AND revision=?3", params![from.task.project, from.task.task_id, from.revision], |row| row.get(0))?;
    let brief: WorkBrief = serde_json::from_str(&encoded)?;
    for prerequisite in brief.prerequisites {
        if reaches_task(connection, &prerequisite, target, visited)? { return Ok(true); }
    }
    Ok(false)
}

fn require_unclaimed(connection: &Connection, work: &WorkRef) -> Result<(), TeamError> {
    let approved: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_work_revisions WHERE project=?1 AND task_id=?2 AND revision=?3 AND state IN ('approved','completed'))", params![work.task.project, work.task.task_id, work.revision], |row| row.get(0))?;
    let claimed: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_assignments WHERE project=?1 AND task_id=?2 AND state NOT IN ('completed','cancelled'))", params![work.task.project, work.task.task_id], |row| row.get(0))?;
    if !approved || claimed { return Err(conflict("work must be currently approved and unclaimed")); }
    Ok(())
}

fn invalidate(connection: &Connection, task: &crate::teams::TaskKey, approved: &[WorkBrief]) -> Result<(), TeamError> {
    let mut affected: Vec<WorkRef> = approved.iter().filter(|brief| brief.work.task == *task).map(|brief| brief.work.clone()).collect();
    loop {
        let previous = affected.len();
        for brief in approved {
            if !affected.contains(&brief.work) && brief.prerequisites.iter().any(|prerequisite| affected.contains(prerequisite)) { affected.push(brief.work.clone()); }
        }
        if previous == affected.len() { break; }
    }
    for work in &affected { require_unclaimed(connection, work)?; }
    for work in affected {
        connection.execute("UPDATE chief_work_revisions SET state='superseded' WHERE project=?1 AND task_id=?2 AND revision=?3", params![work.task.project, work.task.task_id, work.revision])?;
    }
    Ok(())
}

/// Paused/absent chiefs and unresolved claims keep legacy scheduling closed, even after disable.
pub fn scheduling_owned(connection: &Connection) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE state IN ('active','paused')) OR EXISTS(SELECT 1 FROM chief_assignments WHERE state NOT IN ('completed','cancelled'))", [], |row| row.get(0))?)
}

/// Recover only protected identities, never a runtime supplied by the agent or writable snapshot.
pub fn runtime(connection: &Connection, session: &str) -> Result<Option<String>, TeamError> {
    let mut statement = connection.prepare("SELECT runtime FROM chief_grants WHERE session_id=?1 AND state!='disabled' UNION SELECT runtime FROM chief_worker_bindings WHERE session_id=?1 AND state='bound'")?;
    let runtimes = statement.query_map([session], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(if runtimes.len() == 1 { runtimes.into_iter().next() } else { None })
}

pub fn worker_role(connection: &Connection, session: &VerifiedSession) -> Result<Option<crate::teams::Role>, TeamError> {
    let member: Option<String> = connection.query_row("SELECT m.role FROM members m JOIN teams t ON t.task_key=m.task_key WHERE m.session_id=?1 AND m.runtime=?2 AND m.agent=?3 AND t.project=?4",
        params![session.session_id,session.runtime,session.agent,session.project], |row| row.get(0)).optional()?;
    if let Some(role) = member { return Ok(Some(serde_json::from_value(json!(role))?)); }
    let role: Option<String> = connection.query_row("SELECT role FROM chief_worker_bindings WHERE session_id=?1 AND runtime=?2 AND agent=?3 AND project=?4 AND state='bound'",
        params![session.session_id, session.runtime, session.agent, session.project], |row| row.get(0)).optional()?;
    role.map(|role| Ok(serde_json::from_value(json!(role))?)).transpose()
}

fn approved_work(connection: &Connection, work: &WorkRef) -> Result<(WorkBrief, String), TeamError> {
    let record: Option<(String, String)> = connection.query_row("SELECT brief,approval_hash FROM chief_work_revisions WHERE project=?1 AND task_id=?2 AND revision=?3 AND state='approved'",
        params![work.task.project, work.task.task_id, work.revision], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    let (encoded, hash) = record.ok_or_else(|| conflict("work revision is not approved"))?;
    Ok((serde_json::from_str(&encoded)?, hash))
}

fn ready_worker<'a>(live: &'a [WorkerState], id: &str, now_ms: i64) -> Result<&'a WorkerState, TeamError> {
    let workers = live.iter().filter(|worker| worker.session.session_id == id).collect::<Vec<_>>();
    let [worker] = workers.as_slice() else { return Err(conflict("worker readiness is unavailable")) };
    if !worker.ready || !worker.blocked_by.is_empty() || worker.observed_at_ms > now_ms
        || now_ms.saturating_sub(worker.observed_at_ms) > 30_000 {
        return Err(conflict("worker is not verified ready"));
    }
    Ok(worker)
}

// A team membership takes precedence over a standalone binding: neither can widen the other.
fn worker_epoch(connection: &Connection, session: &VerifiedSession, brief: &WorkBrief) -> Result<String, TeamError> {
    require_non_chief(connection, &session.session_id)?;
    if session.project != brief.work.task.project { return Err(unauthorized()); }
    let team: Option<(String, String, String, String, String)> = connection.query_row(
        "SELECT m.task_key,m.role,m.agent,m.runtime,m.epoch FROM members m WHERE m.session_id=?1", [&session.session_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))).optional()?;
    let role = serde_json::to_value(brief.role)?;
    if let Some((task, member_role, agent, runtime, epoch)) = team {
        if serde_json::from_str::<crate::teams::TaskKey>(&task)? != brief.work.task || role != member_role
            || agent != session.agent || runtime != session.runtime { return Err(unauthorized()); }
        return Ok(epoch);
    }
    let epoch: Option<String> = connection.query_row("SELECT epoch FROM chief_worker_bindings WHERE session_id=?1 AND agent=?2 AND project=?3 AND runtime=?4 AND role=?5 AND state='bound'",
        params![session.session_id, session.agent, session.project, session.runtime, role.as_str()], |row| row.get(0)).optional()?;
    epoch.ok_or_else(unauthorized)
}

fn dependencies_complete(connection: &Connection, brief: &WorkBrief) -> Result<(), TeamError> {
    for prerequisite in &brief.prerequisites {
        let completed: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_work_revisions WHERE project=?1 AND task_id=?2 AND revision=?3 AND state='completed')",
            params![prerequisite.task.project, prerequisite.task.task_id, prerequisite.revision], |row| row.get(0))?;
        if !completed { return Err(conflict("approved prerequisite revision is not completed")); }
    }
    Ok(())
}

fn dispatch(connection: &Connection, epoch: &str, key: &str, work: &WorkRef, worker: &str, action: &ChiefAgentAction, live: &[WorkerState], now_ms: i64) -> Result<Value, TeamError> {
    crate::teams::store::sleep::guard_sender(connection, worker)?;
    let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(action)?);
    let old: Option<(String, String)> = connection.query_row("SELECT id,fingerprint FROM chief_assignments WHERE chief_epoch=?1 AND submission_key=?2",
        params![epoch, key], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    if let Some((assignment, original)) = old {
        if original != fingerprint { return Err(conflict("chief submission key reused with different content")); }
        return Ok(json!({"assignment": assignment, "status": "assigned"}));
    }
    if connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_events WHERE source=?1 AND submission_key=?2)", params![epoch, key], |row| row.get::<_, bool>(0))? {
        return Err(conflict("chief submission key already used"));
    }
    let (brief, approval_hash) = approved_work(connection, work)?;
    dependencies_complete(connection, &brief)?;
    require_unclaimed(connection, work)?;
    let target = ready_worker(live, worker, now_ms)?;
    if target.role != Some(brief.role) { return Err(unauthorized()); }
    let worker_epoch = worker_epoch(connection, &target.session, &brief)?;
    require_unreserved(connection, &work.task, Some(worker))?;
    if connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_assignments WHERE worker=?1 AND state NOT IN ('completed','cancelled'))", [worker], |row| row.get::<_, bool>(0))?
        || !crate::teams::store::inbox_available(connection, &target.session, None)? { return Err(conflict("worker already owns work or an inbox delivery")); }
    let assignment = crate::teams::store::identifier()?;
    let grant: String = connection.query_row("SELECT id FROM chief_grants WHERE epoch=?1 AND state='active'", [epoch], |row| row.get(0))?;
    connection.execute("INSERT INTO chief_assignments (id,project,task_id,work_revision,worker,worker_runtime,worker_epoch,chief_grant,chief_epoch,state,revision,submission_key,fingerprint,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'queued',1,?10,?11,?12)",
        params![assignment, work.task.project, work.task.task_id, work.revision, worker, target.session.runtime, worker_epoch, grant, epoch, key, fingerprint, now_ms])?;
    connection.execute("INSERT INTO scheduling_reservations VALUES (?1,?2,?3,?4,'chief')",
        params![work.task.project, work.task.task_id, worker, assignment])?;
    let envelope = json!({"message_id": assignment, "task": work.task, "kind": "chief_assignment", "assignment": assignment,
        "approval": {"work": work, "hash": approval_hash, "role": brief.role, "brief": brief.brief, "prerequisites": brief.prerequisites},
        "body": "", "sender": {"role": "chief"}});
    queue_inbox(connection, &assignment, Some(&assignment), epoch, &target.session, &worker_epoch, work, &envelope, now_ms)?;
    Ok(json!({"assignment": assignment, "status": "assigned"}))
}

fn queue_inbox(connection: &Connection, id: &str, assignment: Option<&str>, epoch: &str, recipient: &VerifiedSession, recipient_epoch: &str, work: &WorkRef, envelope: &Value, now_ms: i64) -> Result<(), TeamError> {
    let pending: i64 = connection.query_row("SELECT count(*) FROM chief_inbox WHERE state IN ('queued','submitting','uncertain') AND json_extract(envelope,'$.kind')!='chief_wake' AND json_extract(envelope,'$.kind')!='host_stop_notice'",[],|row|row.get(0))?;
    if pending>=super::MAX_ACTIVE_WORK as i64 {return Err(TeamError::new(ErrorCode::Limit,"chief native inbox backlog is full; existing messages retained"))}
    let mut envelope = envelope.clone();
    let (session, agent): (String, String) = connection.query_row("SELECT session_id,agent FROM chief_grants WHERE epoch=?1 AND state='active'", [epoch], |row| Ok((row.get(0)?, row.get(1)?)))?;
    envelope["sender"] = json!({"session_id": session, "agent": agent, "role": "chief"});
    envelope["chief_epoch"] = json!(epoch);
    connection.execute("INSERT INTO chief_inbox (id,assignment,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'queued',?9)",
        params![id, assignment, epoch, recipient.session_id, serde_json::to_string(recipient)?, recipient_epoch, serde_json::to_string(work)?, serde_json::to_string(&envelope)?, now_ms])?;
    Ok(())
}

pub(crate) fn upgrade_inbox(connection: &Connection) -> Result<(), TeamError> {
    connection.execute_batch("CREATE TABLE chief_inbox (
        id TEXT PRIMARY KEY, assignment TEXT REFERENCES chief_assignments(id), chief_epoch TEXT NOT NULL,
        recipient TEXT NOT NULL, recipient_identity TEXT NOT NULL, recipient_epoch TEXT NOT NULL,
        work TEXT NOT NULL, envelope TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('queued','submitting','received','uncertain','cancelled','failed')),
        next_at INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE chief_observations (session_id TEXT PRIMARY KEY, observation TEXT NOT NULL);
        CREATE TABLE chief_submissions (sender TEXT NOT NULL, submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
            response TEXT NOT NULL, PRIMARY KEY(sender,submission_key));
        CREATE TABLE inbox_turns (recipient TEXT PRIMARY KEY, runtime TEXT NOT NULL, idle_turn TEXT NOT NULL);
        CREATE UNIQUE INDEX chief_inbox_inflight ON chief_inbox(recipient) WHERE state IN ('submitting','uncertain');")?;
    Ok(())
}

fn record_status(connection: &Connection, source: &str, assignment: Option<&str>, summary: &str, now_ms: i64) -> Result<(), TeamError> {
    let body = json!({"summary": summary, "provenance": "agent_report_not_evidence"});
    let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(&(assignment, &body))?);
    let latest: Option<String> = connection.query_row("SELECT fingerprint FROM chief_events WHERE source=?1 AND assignment IS ?2 AND kind IN ('status','worker_status') ORDER BY seq DESC LIMIT 1",params![source,assignment],|row|row.get(0)).optional()?;
    if latest.as_deref()==Some(&fingerprint) {return Ok(());}
    connection.execute("INSERT INTO chief_events (id,source,submission_key,fingerprint,assignment,kind,body,created_at) VALUES (?1,?2,?1,?3,?4,?7,?5,?6)",
        params![crate::teams::store::identifier()?, source, fingerprint, assignment, body.to_string(), now_ms,if assignment.is_some() {"worker_status"} else {"status"}])?;
    Ok(())
}

fn worker_action(connection: &Connection, sender: &VerifiedSession, action: &ChiefAgentAction, now_ms: i64) -> Result<Value, TeamError> {
    let assignment = match action {
        ChiefAgentAction::Accept { assignment } | ChiefAgentAction::Status { assignment: Some(assignment), .. } => assignment,
        _ => return Err(unauthorized()),
    };
    let record: Option<(String, String, i64, String, String, String)> = connection.query_row(
        "SELECT project,task_id,work_revision,worker_epoch,worker_runtime,state FROM chief_assignments WHERE id=?1 AND worker=?2",
        params![assignment, sender.session_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))).optional()?;
    let (project, task_id, revision, epoch, runtime, state) = record.ok_or_else(unauthorized)?;
    let work = WorkRef { task: crate::teams::TaskKey { project, task_id }, revision };
    let (brief, _) = approved_work(connection, &work)?;
    if runtime != sender.runtime || worker_epoch(connection, sender, &brief)? != epoch { return Err(unauthorized()); }
    if !matches!(state.as_str(), "received" | "accepted" | "working") {
        return Err(conflict("assignment has not been received or requires host reconciliation"));
    }
    match action {
        ChiefAgentAction::Accept { .. } => {
            if state == "received" {
                connection.execute("UPDATE chief_assignments SET state='accepted',revision=revision+1 WHERE id=?1", [assignment])?;
                material_event(connection,assignment,"assignment_accepted",&json!({"source":"assigned_worker_report_not_evidence"}),now_ms)?;
            }
            Ok(json!({"assignment": assignment, "status": "accepted"}))
        },
        ChiefAgentAction::Status { summary, .. } => {
            if state == "received" { return Err(conflict("accept the assignment before reporting work")); }
            record_status(connection, &epoch, Some(assignment), summary, now_ms)?;
            if state!="working" {
                connection.execute("UPDATE chief_assignments SET state='working',revision=revision+1 WHERE id=?1", [assignment])?;
                material_event(connection,assignment,"assignment_working",&json!({"source":"assigned_worker_report_not_evidence"}),now_ms)?;
            }
            Ok(json!({"assignment": assignment, "status": "recorded"}))
        },
        _ => Err(unauthorized()),
    }
}

fn coordinate(connection: &Connection, chief_epoch: &str, key: &str, work: &WorkRef, recipient: &str, body: &str, action: &ChiefAgentAction, live: &[WorkerState], now_ms: i64) -> Result<Value, TeamError> {
    crate::teams::store::sleep::guard_sender(connection, recipient)?;
    let (brief, hash) = approved_work(connection, work)?;
    let target: Option<(String, String, i64, String, String, String)> = connection.query_row(
        "SELECT project,task_id,work_revision,worker_epoch,worker_runtime,id FROM chief_assignments WHERE worker=?1 AND state IN ('received','accepted','working')",
        [recipient], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))).optional()?;
    let (project, task_id, revision, epoch, runtime, recipient_assignment) = target.ok_or_else(unauthorized)?;
    let target_work = WorkRef { task: crate::teams::TaskKey { project, task_id }, revision };
    if target_work != *work && !brief.prerequisites.contains(&target_work) { return Err(unauthorized()); }
    let (target_brief, _) = approved_work(connection, &target_work)?;
    let targets = live.iter().filter(|worker| worker.session.session_id == recipient).collect::<Vec<_>>();
    let [target] = targets.as_slice() else { return Err(unauthorized()) };
    if target.session.runtime != runtime || worker_epoch(connection, &target.session, &target_brief)? != epoch { return Err(unauthorized()); }
    let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(action)?);
    let previous: Option<(String, String)> = connection.query_row("SELECT id,fingerprint FROM chief_events WHERE source=?1 AND submission_key=?2", params![chief_epoch, key], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    if let Some((id, original)) = previous {
        if original != fingerprint { return Err(conflict("chief submission key reused with different content")); }
        return Ok(json!({"message_id": id, "status": "queued"}));
    }
    let id = crate::teams::store::identifier()?;
    let envelope = json!({"message_id": id, "task": work.task, "kind": "chief_coordinate", "body": body,
        "approval": {"work": work, "hash": hash}, "recipient_work": target_work, "recipient_assignment":recipient_assignment,"sender": {"role": "chief"}});
    connection.execute("INSERT INTO chief_events (id,source,submission_key,fingerprint,work,kind,body,created_at) VALUES (?1,?2,?3,?4,?5,'coordinate',?6,?7)",
        params![id, chief_epoch, key, fingerprint, serde_json::to_string(work)?, body, now_ms])?;
    queue_inbox(connection, &id, None, chief_epoch, &target.session, &epoch, work, &envelope, now_ms)?;
    Ok(json!({"message_id": id, "status": "queued"}))
}

/// Only daemon-owned observations belong here. A sandbox request cannot submit WorkerState.
pub fn observe(connection: &mut Connection, live: &[WorkerState], now_ms: i64) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    complete_ready_in(&transaction,now_ms)?;
    transaction.execute("DELETE FROM chief_observations", [])?;
    for worker in live {
        if worker.observed_at_ms > now_ms { continue; }
        transaction.execute("INSERT INTO chief_observations VALUES (?1,?2)", params![worker.session.session_id, serde_json::to_string(worker)?])?;
        if ready_worker(live,&worker.session.session_id,now_ms).is_ok() {
            let candidates = transaction.prepare("SELECT a.id,w.brief,a.worker_epoch FROM chief_assignments a JOIN chief_work_revisions w
                ON w.project=a.project AND w.task_id=a.task_id AND w.revision=a.work_revision
                WHERE a.worker=?1 AND a.worker_runtime=?2 AND a.state IN ('received','accepted','working') AND w.state='completed' AND a.finished_at<?3")?
                .query_map(params![worker.session.session_id,worker.session.runtime,worker.observed_at_ms], |row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
            for (assignment,encoded,epoch) in candidates {
                let brief: WorkBrief = serde_json::from_str(&encoded)?;
                if worker.role != Some(brief.role) || worker_epoch(&transaction,&worker.session,&brief).as_ref().ok() != Some(&epoch) { continue; }
                transaction.execute("UPDATE chief_assignments SET state='completed',revision=revision+1 WHERE id=?1", [&assignment])?;
                transaction.execute("DELETE FROM scheduling_reservations WHERE token=?1 AND owner='chief'", [&assignment])?;
                material_event(&transaction,&assignment,"safe_idle_release",&json!({"work":brief.work}),now_ms)?;
            }
        }
    }
    transaction.execute("UPDATE chief_assignments SET state='uncertain',revision=revision+1 WHERE id IN (SELECT assignment FROM chief_delivery_attempts WHERE state='submitting' AND started_at<=?1)", [now_ms.saturating_sub(crate::teams::store::RECEIPT_TIMEOUT_MS)])?;
    transaction.execute("UPDATE chief_inbox SET state='uncertain' WHERE id IN (SELECT json_extract(envelope,'$.message_id') FROM chief_delivery_attempts WHERE state='submitting' AND started_at<=?1)", [now_ms.saturating_sub(crate::teams::store::RECEIPT_TIMEOUT_MS)])?;
    transaction.execute("UPDATE chief_delivery_attempts SET state='uncertain',finished_at=?2 WHERE state='submitting' AND started_at<=?1", params![now_ms.saturating_sub(crate::teams::store::RECEIPT_TIMEOUT_MS),now_ms])?;
    observe_coordination(&transaction,live,now_ms)?;
    transaction.commit()?;
    Ok(())
}

pub fn recover(connection: &mut Connection, now_ms: i64) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute("UPDATE legacy_launches SET state='uncertain',revision=revision+1 WHERE state IN ('reserved','submitting')",[])?;
    transaction.execute("DELETE FROM chief_observations", [])?;
    transaction.execute("UPDATE chief_assignments SET state='uncertain',revision=revision+1 WHERE state='submitting'", [])?;
    transaction.execute("UPDATE chief_inbox SET state='uncertain' WHERE state='submitting'", [])?;
    transaction.execute("UPDATE chief_coordination SET failed=1 WHERE epoch IN (SELECT chief_epoch FROM chief_inbox WHERE state IN ('uncertain','failed') AND json_extract(envelope,'$.kind')='chief_wake')", [])?;
    transaction.execute("UPDATE chief_delivery_attempts SET state='uncertain',finished_at=?1 WHERE state='submitting'", [now_ms])?;
    transaction.commit()?;
    Ok(())
}

fn observed(connection: &Connection) -> Result<Vec<WorkerState>, TeamError> {
    let encoded = connection.prepare("SELECT observation FROM chief_observations")?.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    encoded.iter().map(|value| Ok(serde_json::from_str(value)?)).collect()
}

pub(crate) fn upgrade_events(connection: &Connection) -> Result<(), TeamError> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS chief_coordination (
        epoch TEXT PRIMARY KEY, delivered_seq INTEGER NOT NULL DEFAULT 0, last_admitted_at INTEGER,
        failed INTEGER NOT NULL DEFAULT 0, observation TEXT NOT NULL DEFAULT '', preview_records INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS chief_turns (dispatch_id TEXT PRIMARY KEY, epoch TEXT NOT NULL,
            idle_turn TEXT NOT NULL, native_id TEXT NOT NULL, through_seq INTEGER NOT NULL,
            mutations INTEGER NOT NULL DEFAULT 0, records INTEGER NOT NULL DEFAULT 0, admitted_at INTEGER NOT NULL,
            UNIQUE(epoch,idle_turn), UNIQUE(epoch,native_id));
        CREATE TABLE IF NOT EXISTS chief_attention (decision TEXT PRIMARY KEY, created_at INTEGER NOT NULL, notified_at INTEGER);
        INSERT OR IGNORE INTO chief_attention(decision,created_at) SELECT id,created_at FROM chief_decisions WHERE state='open';
        CREATE TABLE IF NOT EXISTS chief_summary_requests (id TEXT PRIMARY KEY, epoch TEXT, state TEXT NOT NULL,
            created_at INTEGER NOT NULL, deadline INTEGER NOT NULL, completed_at INTEGER, response TEXT);
        CREATE TABLE IF NOT EXISTS chief_review_generation (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL);
        INSERT OR IGNORE INTO chief_review_generation VALUES (1,0);
        INSERT OR IGNORE INTO chief_coordination(epoch) SELECT epoch FROM chief_grants WHERE state!='disabled';
        CREATE UNIQUE INDEX IF NOT EXISTS chief_one_queued_wake ON chief_inbox(chief_epoch)
            WHERE json_extract(envelope,'$.kind')='chief_wake' AND state IN ('queued','submitting','uncertain');")?;
    for table in ["chief_grants","chief_work_revisions","chief_worker_bindings","chief_work_proposals","chief_assignments",
        "chief_decisions","chief_sources","chief_evidence","chief_inbox","chief_delivery_attempts","chief_events","chief_summary_requests","members"] {
        for operation in ["INSERT","UPDATE","DELETE"] {
            connection.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS review_{table}_{operation} AFTER {operation} ON {table}
                BEGIN UPDATE chief_review_generation SET revision=revision+1 WHERE id=1; END;"))?;
        }
    }
    let historical_grants = connection.prepare("SELECT epoch FROM chief_grants WHERE state IN ('active','paused') AND NOT EXISTS(SELECT 1 FROM chief_events WHERE kind='bootstrap' AND json_extract(body,'$.epoch')=chief_grants.epoch)")?
        .query_map([],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for epoch in historical_grants {coordination_event(connection,"bootstrap",&json!({"epoch":epoch}),0)?;}
    schedule_wake(connection,0)?;
    upgrade_launches(connection)?;
    Ok(())
}

pub(crate) fn upgrade_launches(connection: &Connection) -> Result<(),TeamError> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS legacy_launches(token TEXT PRIMARY KEY,project TEXT NOT NULL,task_id TEXT NOT NULL,worker TEXT,
        state TEXT NOT NULL CHECK(state IN ('manual','reserved','submitting','uncertain','submitted','released')),revision INTEGER NOT NULL,created_at INTEGER NOT NULL);
        INSERT OR IGNORE INTO legacy_launches SELECT token,project,task_id,nullif(worker,''),'uncertain',1,0 FROM scheduling_reservations WHERE owner='legacy';")?;
    connection.execute_batch("UPDATE chief_summary_requests SET state='queued',deadline=created_at+900000 WHERE state='pending' AND NOT EXISTS(
        SELECT 1 FROM chief_turns t JOIN chief_delivery_attempts a ON a.id=t.dispatch_id,json_each(a.envelope,'$.summary_requests') r WHERE t.epoch=chief_summary_requests.epoch AND r.value=chief_summary_requests.id);")?;
    for operation in ["INSERT","UPDATE","DELETE"] {
        connection.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS review_legacy_launches_{operation} AFTER {operation} ON legacy_launches BEGIN UPDATE chief_review_generation SET revision=revision+1 WHERE id=1; END;"))?;
    }
    Ok(())
}

fn coordination_event(connection: &Connection, kind: &str, body: &Value, now_ms: i64) -> Result<(), TeamError> {
    let id = crate::teams::store::identifier()?;
    let work = body.get("work").filter(|work|!work.is_null()).map(|work|serde_json::from_value::<WorkRef>(work.clone())).transpose()?;
    if let Some(work) = &work {work.validate()?;}
    let assignment = body.get("assignment").and_then(Value::as_str);
    if let Some(assignment) = assignment {super::key(assignment)?;}
    connection.execute("INSERT INTO chief_events(id,source,submission_key,fingerprint,work,assignment,kind,body,created_at) VALUES (?1,'host',?1,?2,?3,?4,?5,?6,?7)",
        params![id,crate::audit::sha256_hex(&serde_json::to_vec(body)?),work.map(|work|serde_json::to_string(&work)).transpose()?,assignment,kind,body.to_string(),now_ms])?;
    Ok(())
}

fn decision_record(connection: &Connection, scope: &str, work: Option<&WorkRef>, assignment: Option<&str>, key: &str,
    question: &str, recommendation: &str, now_ms: i64) -> Result<Value, TeamError> {
    let previous: Option<(String,i64,String,String,String)> = connection.query_row("SELECT id,revision,question,recommendation,state FROM chief_decisions WHERE scope_key=?1 AND decision_key=?2 ORDER BY created_at DESC,rowid DESC LIMIT 1",
        params![scope,key],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
    if let Some((id,revision,old_question,old_recommendation,_)) = &previous {
        // A resolved blocker stays answered until genuinely new evidence changes its record.
        if old_question==question && old_recommendation==recommendation { return Ok(json!({"decision":id,"revision":revision})); }
    }
    if previous.as_ref().is_none_or(|(_,_,_,_,state)|state!="open") {
        let count: i64 = connection.query_row("SELECT count(*) FROM chief_decisions WHERE state='open'",[],|row|row.get(0))?;
        if count>=super::MAX_OPEN_DECISIONS as i64 { return Err(TeamError::new(ErrorCode::Limit,"chief decision capacity exceeded; unresolved decisions retained")); }
    }
    if let Some((id,revision,_,_,_)) = previous {
        connection.execute("UPDATE chief_decisions SET question=?2,recommendation=?3,revision=revision+1,state='open',resolution=NULL,resolved_at=NULL WHERE id=?1",params![id,question,recommendation])?;
        return Ok(json!({"decision":id,"revision":revision+1}));
    }
    let id = crate::teams::store::identifier()?;
    connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,work,assignment,question,recommendation,state,revision,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'open',1,?8)",
        params![id,scope,key,work.map(serde_json::to_string).transpose()?,assignment,question,recommendation,now_ms])?;
    connection.execute("INSERT INTO chief_attention(decision,created_at) VALUES (?1,?2)",params![id,now_ms])?;
    Ok(json!({"decision":id,"revision":1}))
}

fn material_workers(live: &[WorkerState], now_ms: i64) -> Value {
    let mut workers = live.iter().map(|worker| {
        let mut blocked = worker.blocked_by.clone();
        if worker.observed_at_ms>now_ms || now_ms.saturating_sub(worker.observed_at_ms)>30_000 { blocked.push("observation_stale".into()); }
        blocked.sort(); blocked.dedup();
        json!({"session":worker.session,"role":worker.role,"ready":worker.ready && blocked.is_empty(),"blocked_by":blocked})
    }).collect::<Vec<_>>();
    workers.sort_by_key(|worker|worker["session"]["session_id"].as_str().unwrap_or("").to_owned());
    json!(workers)
}

fn wake_ready(connection: &Connection, chief: &VerifiedSession, live: &[WorkerState], now_ms: i64) -> Result<bool, TeamError> {
    if chief.agent!="opencode" || !admission_verified(connection,chief)? || is_product_worker(connection,&chief.session_id)? { return Ok(false); }
    Ok(live.iter().any(|worker|worker.session==*chief && worker.observed_at_ms<=now_ms && now_ms.saturating_sub(worker.observed_at_ms)<=30_000
        && worker.blocked_by.iter().all(|reason|reason=="host_role_unbound") && (worker.ready || worker.blocked_by==["host_role_unbound"])))
}

fn schedule_wake(connection: &Connection, now_ms: i64) -> Result<(), TeamError> {
    let grant: Option<(String,VerifiedSession,i64,Option<i64>,bool)> = connection.query_row("SELECT g.epoch,g.session_id,g.agent,g.project,g.runtime,c.delivered_seq,c.last_admitted_at,c.failed
        FROM chief_grants g JOIN chief_coordination c ON c.epoch=g.epoch WHERE g.state='active'",[],|row|Ok((row.get(0)?,VerifiedSession {
            session_id:row.get(1)?,agent:row.get(2)?,project:row.get(3)?,runtime:row.get(4)?},row.get(5)?,row.get(6)?,row.get(7)?))).optional()?;
    let Some((epoch,chief,delivered,last_admitted,failed)) = grant else {return Ok(())};
    if failed || last_admitted.is_some_and(|last|now_ms<last.saturating_add(30_000)) {return Ok(())}
    let pending: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_events WHERE seq>?1 AND kind NOT IN ('status','coordinate','reprioritized'))",[delivered],|row|row.get(0))?;
    if !pending {return Ok(())}
    let exists: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_inbox WHERE chief_epoch=?1 AND json_extract(envelope,'$.kind')='chief_wake' AND state IN ('queued','submitting','uncertain'))",[&epoch],|row|row.get(0))?;
    if exists {return Ok(())}
    let id = crate::teams::store::identifier()?;
    let envelope = json!({"message_id":id,"kind":"chief_wake","epoch":epoch,"body":"Material coordination work is pending. Read bounded chief Context. No scope, tools, evidence or permissions are granted."});
    connection.execute("INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at) VALUES (?1,?2,?3,?4,?2,'null',?5,'queued',?6)",
        params![id,epoch,chief.session_id,serde_json::to_string(&chief)?,envelope.to_string(),now_ms])?;
    Ok(())
}

fn observe_coordination(connection: &Connection, live: &[WorkerState], now_ms: i64) -> Result<(), TeamError> {
    let grants = connection.prepare("SELECT epoch,session_id,agent,project,runtime,observation FROM chief_grants JOIN chief_coordination USING(epoch) WHERE state IN ('active','paused')")?
        .query_map([],|row|Ok((row.get::<_,String>(0)?,VerifiedSession {session_id:row.get(1)?,agent:row.get(2)?,project:row.get(3)?,runtime:row.get(4)?},row.get::<_,String>(5)?)))?.collect::<Result<Vec<_>,_>>()?;
    for (epoch,chief,old) in grants {
        // The chief's own native execution/idle transitions must not manufacture its next LLM turn.
        let product_workers = live.iter().filter(|worker|worker.session.session_id!=chief.session_id).cloned().collect::<Vec<_>>();
        let material = material_workers(&product_workers,now_ms).to_string();
        if old != material {
            connection.execute("UPDATE chief_coordination SET observation=?2 WHERE epoch=?1",params![epoch,material])?;
            coordination_event(connection,"availability_changed",&json!({"source":"daemon_derived_availability","workers":material_workers(&product_workers,now_ms)}),now_ms)?;
        }
        let chief_observation = live.iter().find(|worker|worker.session==chief);
        let proof_unavailable = chief_observation.is_some_and(|worker|worker.observed_at_ms>now_ms || now_ms.saturating_sub(worker.observed_at_ms)>30_000
            || worker.blocked_by.iter().any(|reason|matches!(reason.as_str(),"fresh_native_availability_required"|"live_queued_admission_verification_required")));
        if !exact_live(&chief,live) || chief.agent!="opencode" || proof_unavailable {
            decision_record(connection,&format!("host-chief-{epoch}"),None,None,"unavailable","Selected chief is missing, stale or has no supported native inbox.","Host must restore or replace the chief; no fallback dispatch is authorized.",now_ms)?;
        }
    }
    let critical = connection.prepare("SELECT id,project,task_id,work_revision,state FROM chief_assignments WHERE state IN ('uncertain','blocked')")?
        .query_map([],|row|Ok((row.get::<_,String>(0)?,WorkRef {task:crate::teams::TaskKey {project:row.get(1)?,task_id:row.get(2)?},revision:row.get(3)?},row.get::<_,String>(4)?)))?.collect::<Result<Vec<_>,_>>()?;
    for (assignment,work,state) in critical {
        decision_record(connection,&format!("host-assignment-{assignment}"),Some(&work),Some(&assignment),"reconcile",
            &format!("Assignment requires host reconciliation ({state}); native delivery or approved scope is uncertain."),"Review delivery and current approval. Do not resend or release ownership without host reconciliation.",now_ms)?;
    }
    let failed_wakes = connection.prepare("SELECT DISTINCT chief_epoch FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_wake' AND state='uncertain'")?
        .query_map([],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for epoch in failed_wakes {
        connection.execute("UPDATE chief_coordination SET failed=1 WHERE epoch=?1",[&epoch])?;
    }
    let unresolved=connection.prepare("SELECT id,work FROM chief_inbox WHERE assignment IS NULL AND state IN ('uncertain','failed')")?
        .query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    for (message,encoded_work) in unresolved {
        let work:Option<WorkRef>=serde_json::from_str(&encoded_work)?;
        decision_record(connection,&format!("host-delivery-{message}"),work.as_ref(),None,"delivery","Exact native delivery needs host recovery; handling is not verified delivery or completion.","Inspect the exact delivery and recipient. Confirm handled without releasing work, or explicitly confirm safe stop before cancellation.",now_ms)?;
    }
    connection.execute("UPDATE chief_summary_requests SET state='failed' WHERE state IN ('queued','pending') AND deadline<=?1",[now_ms])?;
    connection.execute("UPDATE legacy_launches SET state='uncertain',revision=revision+1 WHERE state IN ('reserved','submitting') AND created_at<=?1",[now_ms.saturating_sub(30_000)])?;
    let launches=connection.prepare("SELECT token FROM legacy_launches WHERE state='uncertain'")?.query_map([],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for token in launches {decision_record(connection,&format!("host-launch-{token}"),None,None,"launch","Exact legacy launch is not conclusively settled; ownership remains reserved.","Inspect this exact launch token. Handling retains ownership; only an explicit safe-stop assertion can release unknown execution.",now_ms)?;}
    schedule_wake(connection,now_ms)
}

fn current_turn(connection: &Connection, epoch: &str) -> Result<Option<(String,i64,i64,i64)>, TeamError> {
    Ok(connection.query_row("SELECT dispatch_id,through_seq,mutations,records FROM chief_turns WHERE epoch=?1 ORDER BY rowid DESC LIMIT 1",[epoch],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?)
}

fn charge_mutation(connection: &Connection, epoch: &str) -> Result<String, TeamError> {
    if connection.query_row("SELECT failed FROM chief_coordination WHERE epoch=?1",[epoch],|row|row.get::<_,bool>(0))? {
        return Err(TeamError::new(ErrorCode::Unavailable,"chief native coordination delivery requires host review; previous allowance is closed"));
    }
    let Some((dispatch,_,mutations,_)) = current_turn(connection,epoch)? else {return Err(TeamError::new(ErrorCode::Unavailable,"chief mutations require the first admitted verified native coordination wake"))};
    if mutations>=20 {return Err(TeamError::new(ErrorCode::Limit,"chief turn mutation budget exhausted; wait for eligible material work at a new safe idle boundary"))}
    connection.execute("UPDATE chief_turns SET mutations=mutations+1 WHERE dispatch_id=?1",[&dispatch])?;
    Ok(dispatch)
}

enum InboxValidation { Ready, Deferred, Obsolete }

fn validate_inbox(connection: &Connection, recipient: &VerifiedSession, epoch: &str, chief_epoch: &str, work: &WorkRef, envelope: &Value, live: &[WorkerState], now_ms: i64) -> Result<InboxValidation, TeamError> {
    if envelope["kind"] == "host_stop_notice" {
        let retained: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_assignments a JOIN chief_events e ON e.assignment=a.id
            WHERE a.id=?1 AND a.worker=?2 AND a.worker_runtime=?3 AND a.worker_epoch=?4 AND a.project=?5 AND a.task_id=?6 AND a.work_revision=?7
            AND a.state NOT IN ('completed','cancelled') AND e.id=?8 AND e.source='host' AND e.kind='host_stop_notice' AND e.body=?9)",
            params![envelope["assignment"].as_str(),recipient.session_id,recipient.runtime,epoch,work.task.project,work.task.task_id,work.revision,
                envelope["message_id"].as_str(),envelope.to_string()],|row|row.get(0))?;
        if !retained { return Ok(InboxValidation::Obsolete); }
        // The exact old recipient gets a safety notice, not a new assignment or permission grant.
        return Ok(if ready_worker(live,&recipient.session_id,now_ms).is_ok_and(|worker|worker.session==*recipient) {
            InboxValidation::Ready
        } else { InboxValidation::Deferred });
    }
    let grant: Option<(VerifiedSession, String)> = connection.query_row("SELECT session_id,agent,project,runtime,state FROM chief_grants WHERE epoch=?1", [chief_epoch], |row| Ok((VerifiedSession {
        session_id: row.get(0)?, agent: row.get(1)?, project: row.get(2)?, runtime: row.get(3)?,
    }, row.get(4)?))).optional()?;
    let Some((chief, state)) = grant else { return Ok(InboxValidation::Obsolete) };
    if state == "disabled" { return Ok(InboxValidation::Obsolete); }
    let (brief, hash) = match approved_work(connection, work) {
        Ok(approved) => approved,
        Err(error) if error.code == ErrorCode::Conflict => return Ok(InboxValidation::Obsolete),
        Err(error) => return Err(error),
    };
    if envelope["approval"]["hash"] != hash { return Ok(InboxValidation::Obsolete); }
    let target_brief = if envelope["kind"] == "chief_coordinate" {
        let target_work: WorkRef = serde_json::from_value(envelope["recipient_work"].clone())?;
        if target_work != *work && !brief.prerequisites.contains(&target_work) { return Ok(InboxValidation::Obsolete); }
        let active: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_assignments WHERE worker=?1 AND project=?2 AND task_id=?3 AND work_revision=?4 AND worker_epoch=?5 AND state IN ('received','accepted','working'))",
            params![recipient.session_id, target_work.task.project, target_work.task.task_id, target_work.revision, epoch], |row| row.get(0))?;
        if !active { return Ok(InboxValidation::Obsolete); }
        match approved_work(connection, &target_work) {
            Ok((approved, _)) => approved,
            Err(error) if error.code == ErrorCode::Conflict => return Ok(InboxValidation::Obsolete),
            Err(error) => return Err(error),
        }
    } else {
        let active: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_assignments WHERE id=?1 AND state='queued' AND worker=?2 AND worker_runtime=?3 AND worker_epoch=?4 AND chief_epoch=?5)",
            params![envelope["assignment"].as_str(), recipient.session_id, recipient.runtime, epoch, chief_epoch], |row| row.get(0))?;
        if !active { return Ok(InboxValidation::Obsolete); }
        brief
    };
    match worker_epoch(connection, recipient, &target_brief) {
        Ok(current) if current == epoch => {},
        Ok(_) => return Ok(InboxValidation::Obsolete),
        Err(error) if error.code == ErrorCode::Unauthorized => return Ok(InboxValidation::Obsolete),
        Err(error) => return Err(error),
    }
    if state != "active" || !exact_live(&chief, live) || is_product_worker(connection, &chief.session_id)? { return Ok(InboxValidation::Deferred); }
    let chief_observation = live.iter().find(|worker| worker.session == chief).expect("exact_live checked above");
    if now_ms.saturating_sub(chief_observation.observed_at_ms) > 30_000 || chief_observation.observed_at_ms > now_ms { return Ok(InboxValidation::Deferred); }
    let worker = match ready_worker(live, &recipient.session_id, now_ms) {
        Ok(worker) => worker,
        Err(error) if error.code == ErrorCode::Conflict => return Ok(InboxValidation::Deferred),
        Err(error) => return Err(error),
    };
    if worker.session != *recipient || worker.role != Some(target_brief.role) { return Ok(InboxValidation::Deferred); }
    if envelope["kind"] != "chief_coordinate" {
        match dependencies_complete(connection, &target_brief) {
            Ok(()) => {},
            Err(error) if error.code == ErrorCode::Conflict => return Ok(InboxValidation::Deferred),
            Err(error) => return Err(error),
        }
    }
    Ok(InboxValidation::Ready)
}

pub fn claim(connection: &mut Connection, recipient: &VerifiedSession, idle_turn: &str, now_ms: i64) -> Result<Option<crate::teams::Dispatch>, TeamError> {
    if idle_turn.is_empty() || idle_turn.len() > 128 || idle_turn.chars().any(char::is_control) { return Err(TeamError::new(ErrorCode::Invalid, "missing verified idle turn")); }
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if !crate::teams::store::inbox_available(&transaction, recipient, Some(idle_turn))? { return Ok(None); }
    let live = observed(&transaction)?;
    loop {
        let candidate: Option<(String, String, String, String, String, String, Option<String>)> = transaction.query_row(
            "SELECT id,recipient_identity,recipient_epoch,chief_epoch,work,envelope,assignment FROM chief_inbox WHERE recipient=?1 AND state='queued' AND next_at<=?2 ORDER BY rowid LIMIT 1",
            params![recipient.session_id,now_ms], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?))).optional()?;
        let Some((message_id, identity, epoch, chief_epoch, work, envelope, assignment)) = candidate else { transaction.commit()?; return Ok(None) };
        let identity = serde_json::from_str::<VerifiedSession>(&identity)?;
        let mut envelope = serde_json::from_str::<Value>(&envelope)?;
        let validation = if envelope["kind"]=="chief_wake" {
            let active: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE epoch=?1 AND state='active' AND session_id=?2 AND runtime=?3)",params![chief_epoch,identity.session_id,identity.runtime],|row|row.get(0))?;
            let used_boundary: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_turns WHERE epoch=?1 AND idle_turn=?2)",params![chief_epoch,idle_turn],|row|row.get(0))?;
            if !active || used_boundary { InboxValidation::Deferred }
            else if wake_ready(&transaction,&identity,&live,now_ms)? {
                let through: i64 = transaction.query_row("SELECT coalesce(max(seq),0) FROM chief_events",[],|row|row.get(0))?;
                envelope["through_seq"] = json!(through);
                let continuation: Option<String> = transaction.query_row("SELECT body FROM chief_events WHERE source='host' AND kind='context_backlog' AND json_extract(body,'$.epoch')=?1 AND seq>(SELECT delivered_seq FROM chief_coordination WHERE epoch=?1) ORDER BY seq DESC LIMIT 1",[&chief_epoch],|row|row.get(0)).optional()?;
                envelope["context_continuation"] = continuation.map(|body|serde_json::from_str::<Value>(&body)).transpose()?.unwrap_or(Value::Null);
                envelope["summary_requests"] = json!(transaction.prepare("SELECT id FROM chief_summary_requests WHERE state='queued' AND epoch=?1 ORDER BY created_at LIMIT 1")?
                    .query_map([&chief_epoch],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?);
                transaction.execute("UPDATE chief_inbox SET envelope=?2 WHERE id=?1",params![message_id,envelope.to_string()])?;
                InboxValidation::Ready
            } else { InboxValidation::Deferred }
        } else { validate_inbox(&transaction, &identity, &epoch, &chief_epoch, &serde_json::from_str::<WorkRef>(&work)?, &envelope, &live, now_ms)? };
        match validation {
            InboxValidation::Obsolete => {
                let cancelled = transaction.execute("UPDATE chief_inbox SET state='cancelled' WHERE id=?1 AND state='queued'
                    AND NOT EXISTS(SELECT 1 FROM chief_delivery_attempts a WHERE json_extract(a.envelope,'$.message_id')=chief_inbox.id AND a.state<>'not_submitted')", [&message_id])?;
                if cancelled > 0 { continue; }
                transaction.commit()?;
                return Ok(None);
            },
            InboxValidation::Deferred => { transaction.commit()?; return Ok(None); },
            InboxValidation::Ready if identity != *recipient => { transaction.commit()?; return Ok(None); },
            InboxValidation::Ready => {},
        }
        let id = format!("chief_{}", crate::teams::store::identifier()?);
        transaction.execute("UPDATE chief_inbox SET state='submitting',attempts=attempts+1 WHERE id=?1", [&message_id])?;
        transaction.execute("INSERT INTO chief_delivery_attempts (id,assignment,event_id,recipient,recipient_runtime,recipient_epoch,kind,envelope,idle_turn,state,started_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'submitting',?10)",
            params![id,assignment,if assignment.is_none() && envelope["kind"]!="chief_wake" { Some(&message_id) } else { None },recipient.session_id,recipient.runtime,epoch,envelope["kind"].as_str(),envelope.to_string(),idle_turn,now_ms])?;
        if let Some(assignment) = assignment {
            transaction.execute("UPDATE chief_assignments SET state='submitting',revision=revision+1 WHERE id=?1", [assignment])?;
        }
        crate::teams::store::record_inbox_turn(&transaction, recipient, idle_turn)?;
        transaction.commit()?;
        return Ok(Some(crate::teams::Dispatch { id, message_id, envelope }));
    }
}

pub fn owns_dispatch(connection: &Connection, dispatch_id: &str) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_delivery_attempts WHERE id=?1)", [dispatch_id], |row| row.get(0))?)
}

pub fn finish_for(connection: &mut Connection, recipient: &VerifiedSession, dispatch_id: &str, outcome: &crate::teams::TransportOutcome, now_ms: i64) -> Result<(), TeamError> {
    use crate::teams::TransportOutcome;
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let attempt: Option<(String, String, Option<String>, String, Option<String>, String)> = transaction.query_row(
        "SELECT a.state,a.envelope,a.native_id,a.idle_turn,a.assignment,i.recipient_identity FROM chief_delivery_attempts a JOIN chief_inbox i ON i.id=json_extract(a.envelope,'$.message_id') WHERE a.id=?1",
        [dispatch_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional()?;
    let (state, envelope, native_id, turn, assignment, identity) = attempt.ok_or_else(|| TeamError::new(ErrorCode::NotFound, "delivery attempt not found"))?;
    if serde_json::from_str::<VerifiedSession>(&identity)? != *recipient { return Err(unauthorized()); }
    if let TransportOutcome::Received { native_id: received } = outcome {
        if received.is_empty() || received.len() > 256 || received.chars().any(char::is_control) { return Err(TeamError::new(ErrorCode::Invalid, "invalid native receipt")); }
        if native_id.as_ref().is_some_and(|known| known != received) { return Err(conflict("receipt conflicts with original admission")); }
    }
    let lifecycle: Option<(String,String)> = assignment.as_ref().map(|assignment|transaction.query_row(
        "SELECT a.state,w.state FROM chief_assignments a JOIN chief_work_revisions w ON w.project=a.project AND w.task_id=a.task_id AND w.revision=a.work_revision WHERE a.id=?1",
        [assignment], |row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))).transpose()?;
    if lifecycle.as_ref().is_some_and(|(assignment_state,_)|assignment_state=="cancelled") {
        transaction.execute("UPDATE chief_delivery_attempts SET state='cancelled',receipt=?2,finished_at=?3 WHERE id=?1", params![dispatch_id,serde_json::to_string(outcome)?,now_ms])?;
        transaction.execute("UPDATE chief_inbox SET state='cancelled' WHERE id=json_extract(?1,'$.message_id')", [&envelope])?;
        transaction.commit()?;
        return Ok(());
    }
    if state == "received" || state == "not_submitted" || state == "cancelled" { return Ok(()); }
    let envelope: Value = serde_json::from_str(&envelope)?;
    let message_id = envelope["message_id"].as_str().ok_or_else(|| conflict("missing inbox identity"))?;
    if envelope["kind"]=="chief_wake" {
        let epoch = envelope["epoch"].as_str().ok_or_else(||conflict("missing chief epoch"))?;
        match outcome {
            TransportOutcome::Received {native_id} if state=="submitting" && !transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_turns WHERE epoch=?1 AND native_id=?2)",params![epoch,native_id],|row|row.get::<_,bool>(0))? => {
                let active: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE epoch=?1 AND state='active' AND session_id=?2 AND runtime=?3)",params![epoch,recipient.session_id,recipient.runtime],|row|row.get(0))?;
                if active && recipient.agent=="opencode" && admission_verified(&transaction,recipient)? {
                    transaction.execute("INSERT OR IGNORE INTO chief_turns(dispatch_id,epoch,idle_turn,native_id,through_seq,admitted_at) VALUES (?1,?2,?3,?4,?5,?6)",
                        params![dispatch_id,epoch,turn,native_id,envelope["through_seq"].as_i64(),now_ms])?;
                    transaction.execute("UPDATE chief_coordination SET delivered_seq=max(delivered_seq,?2),last_admitted_at=?3 WHERE epoch=?1",params![epoch,envelope["through_seq"].as_i64(),now_ms])?;
                    transaction.execute("UPDATE chief_summary_requests SET state='pending',deadline=?3 WHERE epoch=?1 AND state='queued' AND deadline>?4
                        AND id IN (SELECT value FROM json_each(?2,'$.summary_requests'))",params![epoch,envelope.to_string(),now_ms.saturating_add(120_000),now_ms])?;
                }
                transaction.execute("UPDATE chief_delivery_attempts SET state='received',native_id=?2,finished_at=?3 WHERE id=?1",params![dispatch_id,native_id,now_ms])?;
                transaction.execute("UPDATE chief_inbox SET state='received' WHERE id=?1",[message_id])?;
            },
            _ => {
                transaction.execute("UPDATE chief_delivery_attempts SET state='uncertain',receipt=?2,finished_at=?3 WHERE id=?1",params![dispatch_id,serde_json::to_string(outcome)?,now_ms])?;
                transaction.execute("UPDATE chief_inbox SET state='uncertain' WHERE id=?1",[message_id])?;
                transaction.execute("UPDATE chief_coordination SET failed=1 WHERE epoch=?1",[epoch])?;
                decision_record(&transaction,&format!("host-delivery-{message_id}"),None,None,"delivery","Exact native delivery needs host recovery; handling is not verified delivery or completion.","Inspect the exact delivery and recipient. Confirm handled without releasing work, or explicitly confirm safe stop before cancellation.",now_ms)?;
            },
        }
        transaction.commit()?;
        return Ok(());
    }
    let (attempt_state, mut inbox_state, mut assignment_state, next_at, native_id) = match outcome {
        TransportOutcome::Received { native_id } => ("received", "received", "received", now_ms, Some(native_id.as_str())),
        TransportOutcome::Uncertain { .. } => ("uncertain", "uncertain", "uncertain", now_ms, None),
        TransportOutcome::NotSubmitted { .. } => {
            if state != "submitting" { return Err(conflict("uncertain delivery needs host reconciliation")); }
            let count: i64 = transaction.query_row("SELECT attempts FROM chief_inbox WHERE id=?1", [message_id], |row| row.get(0))?;
            let delay = match count { 1 => Some(2_000), 2 => Some(10_000), 3 => Some(30_000), _ => None };
            transaction.execute("DELETE FROM inbox_turns WHERE recipient=?1 AND runtime=?2 AND idle_turn=?3", params![recipient.session_id,recipient.runtime,turn])?;
            ("not_submitted", if delay.is_some() { "queued" } else { "failed" }, if delay.is_some() { "queued" } else { "blocked" }, now_ms.saturating_add(delay.unwrap_or(0)), None)
        },
    };
    if lifecycle.as_ref().is_some_and(|(_,work_state)|!matches!(work_state.as_str(),"approved"|"completed")) {
        assignment_state = "uncertain";
        if inbox_state=="queued" || inbox_state=="failed" { inbox_state="cancelled"; }
        if matches!(outcome,TransportOutcome::NotSubmitted { .. }) {
            let others_submitted: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_delivery_attempts WHERE assignment=?1 AND id!=?2 AND state!='not_submitted')",
                params![assignment,dispatch_id],|row|row.get(0))?;
            if !others_submitted { assignment_state = "cancelled"; }
        }
    }
    transaction.execute("UPDATE chief_delivery_attempts SET state=?2,native_id=?3,receipt=?4,finished_at=?5 WHERE id=?1", params![dispatch_id,attempt_state,native_id,serde_json::to_string(outcome)?,now_ms])?;
    transaction.execute("UPDATE chief_inbox SET state=?2,next_at=?3 WHERE id=?1", params![message_id,inbox_state,next_at])?;
    if let Some(assignment) = assignment {
        transaction.execute("UPDATE chief_assignments SET state=?2,revision=revision+1 WHERE id=?1", params![assignment,assignment_state])?;
        material_event(&transaction,&assignment,"native_delivery",&json!({"state":assignment_state,"source":"native_adapter_receipt_not_completion_evidence"}),now_ms)?;
        if assignment_state == "cancelled" {
            transaction.execute("DELETE FROM scheduling_reservations WHERE token=?1 AND owner='chief'",[&assignment])?;
            cancel_stopped_inbox(&transaction,&assignment,now_ms)?;
        }
    }
    transaction.commit()?;
    Ok(())
}

pub fn agent(connection: &mut Connection, sender: &VerifiedSession, action: &ChiefAgentAction, live: &[WorkerState], now_ms: i64) -> Result<TeamReply, TeamError> {
    let request_id = match action { ChiefAgentAction::Reprioritize { submission_key, .. } | ChiefAgentAction::Dispatch { submission_key, .. }
        | ChiefAgentAction::Coordinate { submission_key, .. } | ChiefAgentAction::ProposeWork { submission_key, .. } => submission_key.clone(), _ => "chief".into() };
    let result = (|| {
        action.validate()?;
        if !exact_live(sender, live) { return Err(unauthorized()); }
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        crate::teams::store::sleep::guard_sender(&transaction, &sender.session_id)?;
        let is_chief: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE session_id=?1 AND state IN ('active','paused'))",[&sender.session_id],|row|row.get(0))?;
        if let ChiefAgentAction::ProposeWork { submission_key, proposal } = action && !is_chief {
            let response = propose_work(&transaction,sender,submission_key,proposal,now_ms)?;
            transaction.commit()?;
            return Ok(response);
        }
        if matches!(action, ChiefAgentAction::Accept { .. } | ChiefAgentAction::Status { assignment: Some(_), .. }) {
            let response = worker_action(&transaction, sender, action, now_ms)?;
            transaction.commit()?;
            return Ok(response);
        }
        if is_product_worker(&transaction, &sender.session_id)? { return Err(unauthorized()); }
        let epoch: Option<String> = transaction.query_row("SELECT epoch FROM chief_grants WHERE session_id=?1 AND agent=?2 AND project=?3 AND runtime=?4 AND state='active'",
            params![sender.session_id, sender.agent, sender.project, sender.runtime], |row| row.get(0)).optional()?;
        let epoch = epoch.ok_or_else(unauthorized)?;
        let submission = match action {
            ChiefAgentAction::Dispatch { submission_key, .. } | ChiefAgentAction::Reprioritize { submission_key, .. }
                | ChiefAgentAction::Coordinate { submission_key, .. } => Some(submission_key.clone()),
            _ => None,
        };
        let sender_identity = serde_json::to_string(sender)?;
        let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(action)?);
        if let Some(key) = &submission {
            let old: Option<(String, String)> = transaction.query_row("SELECT fingerprint,response FROM chief_submissions WHERE sender=?1 AND submission_key=?2",
                params![sender_identity,key], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
            if let Some((original, response)) = old {
                if original != fingerprint { return Err(conflict("chief sender submission key reused with different content")); }
                return Ok(serde_json::from_str(&response)?);
            }
        }
        let budget_turn = if !matches!(action,ChiefAgentAction::Context {..}|ChiefAgentAction::ContextSnapshot {..}) {Some(charge_mutation(&transaction,&epoch)?)} else {None};
        transaction.commit()?;
        // Failed actions still consume their allowance, but never leave partially applied mutations.
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if !transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE epoch=?1 AND state='active')",[&epoch],|row|row.get::<_,bool>(0))? {return Err(unauthorized())}
        if let Some(expected_turn) = budget_turn {
            let failed: bool = transaction.query_row("SELECT failed FROM chief_coordination WHERE epoch=?1",[&epoch],|row|row.get(0))?;
            if failed || current_turn(&transaction,&epoch)?.as_ref().map(|(dispatch,_,_,_)|dispatch)!=Some(&expected_turn) {
                return Err(conflict("chief native boundary changed after budget reservation; no mutation was applied"));
            }
        }
        let response = match action {
            ChiefAgentAction::Context { after, limit } => event_context(&transaction, live, &epoch, *after, *limit, now_ms)?,
            ChiefAgentAction::ContextSnapshot {after,limit,generation} => bounded_chief_snapshot(&transaction,live,&epoch,*after,*limit,generation.as_deref(),now_ms)?,
            ChiefAgentAction::SummaryResponse {request_id,sections,evidence_ids} => {
                let requested: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_summary_requests WHERE id=?1 AND epoch=?2 AND state='pending' AND deadline>?3)",params![request_id,epoch,now_ms],|row|row.get(0))?;
                let linked: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_turns t JOIN chief_delivery_attempts a ON a.id=t.dispatch_id,json_each(a.envelope,'$.summary_requests') r WHERE t.epoch=?1 AND r.value=?2)",params![epoch,request_id],|row|row.get(0))?;
                if !requested || !linked {return Err(conflict("summary response requires an exact pending request admitted to this chief epoch"))}
                let mut references = Vec::new();
                for id in evidence_ids {
                    let verified = summary_reference_is_current(&transaction,id)?;
                    references.push(json!({"id":id,"verified_current":verified,"provenance":if verified {"protected_current_evidence"} else {"narrative_unverified"}}));
                }
                let response = json!({"sections":sections,"references":references,"provenance":"chief_narrative_unverified","request_id":request_id});
                transaction.execute("UPDATE chief_summary_requests SET state='completed',completed_at=?2,response=?3 WHERE id=?1",params![request_id,now_ms,response.to_string()])?;
                json!({"request_id":request_id,"state":"completed"})
            },
            ChiefAgentAction::ProposeWork {submission_key,proposal} => propose_work(&transaction,sender,submission_key,proposal,now_ms)?,
            ChiefAgentAction::Decision {work,key,question,recommendation} => {
                if matches!(key.as_str(),"capacity"|"idle"|"no_approved_work"|"no-approved-work"|"progress"|"handoff") {
                    json!({"status":"silent","reason":key})
                } else {
                    if let Some(work) = work {approved_work(&transaction,work)?;}
                    let scope = work.as_ref().map_or_else(||Ok(format!("chief-{epoch}")),serde_json::to_string)?;
                    decision_record(&transaction,&scope,work.as_ref(),None,key,question,recommendation,now_ms)?
                }
            },
            ChiefAgentAction::Dispatch { submission_key, work, worker } => dispatch(&transaction, &epoch, submission_key, work, worker, action, live, now_ms)?,
            ChiefAgentAction::Coordinate { submission_key, work, recipient, body } => coordinate(&transaction, &epoch, submission_key, work, recipient, body, action, live, now_ms)?,
            ChiefAgentAction::Status { assignment: None, summary } => {
                record_status(&transaction, &epoch, None, summary, now_ms)?;
                json!({"status": "recorded"})
            },
            ChiefAgentAction::Reprioritize { submission_key, ordered } => {
                let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(action)?);
                if transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_assignments WHERE chief_epoch=?1 AND submission_key=?2)", params![epoch, submission_key], |row| row.get::<_, bool>(0))? {
                    return Err(conflict("chief submission key already used"));
                }
                let old: Option<String> = transaction.query_row("SELECT fingerprint FROM chief_events WHERE source=?1 AND submission_key=?2", params![epoch, submission_key], |row| row.get(0)).optional()?;
                if let Some(old) = old {
                    if old != fingerprint { return Err(conflict("chief submission key reused with different content")); }
                    return Ok(json!({"status": "reprioritized"}));
                }
                for work in ordered { require_unclaimed(&transaction, work)?; }
                let minimum: i64 = transaction.query_row("SELECT coalesce(min(priority),0) FROM chief_work_revisions WHERE state='approved'", [], |row| row.get(0))?;
                let first = minimum.checked_sub(ordered.len() as i64).ok_or_else(|| conflict("chief priority range exhausted"))?;
                for (priority, work) in ordered.iter().enumerate() {
                    transaction.execute("UPDATE chief_work_revisions SET priority=?4 WHERE project=?1 AND task_id=?2 AND revision=?3", params![work.task.project, work.task.task_id, work.revision, first + priority as i64])?;
                }
                transaction.execute("INSERT INTO chief_events (id,source,submission_key,fingerprint,kind,body,created_at) VALUES (?1,?2,?3,?4,'reprioritized','{}',?5)", params![crate::teams::store::identifier()?, epoch, submission_key, fingerprint, now_ms])?;
                json!({"status": "reprioritized"})
            },
            _ => return Err(unavailable()),
        };
        if let Some(key) = submission {
            transaction.execute("INSERT INTO chief_submissions VALUES (?1,?2,?3,?4)", params![sender_identity,key,fingerprint,serde_json::to_string(&response)?])?;
        }
        transaction.commit()?;
        Ok(response)
    })();
    Ok(TeamReply { request_id, result })
}

/// Explicit safe projection: never serialize registry entries, action payloads, or event bodies.
fn event_context(connection: &Connection, live: &[WorkerState], epoch: &str, after: i64, limit: usize, now_ms: i64) -> Result<Value, TeamError> {
    super::validate_cursor(after,limit)?;
    let head: i64 = connection.query_row("SELECT coalesce(max(seq),0) FROM chief_events",[],|row|row.get(0))?;
    let turn = current_turn(connection,epoch)?;
    let through = turn.as_ref().map_or(head,|(_,through,_,_)|*through);
    let remaining = context_remaining(connection,epoch,turn.as_ref())?;
    if remaining==0 {return Err(TeamError::new(ErrorCode::Limit,"chief turn event-record budget exhausted"))}
    let mut result = if after==0 || after<head.saturating_sub(1000) || after>through {
        context_recovery_snapshot(connection,live,after,limit.min(remaining))?
    } else {
        let events = connection.prepare("SELECT seq,source,kind,work,assignment,body FROM chief_events WHERE seq>?1 AND seq<=?2 ORDER BY seq LIMIT ?3")?
            .query_map(params![after,through,limit.min(remaining) as i64],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,String>(5)?)))?
            .collect::<Result<Vec<_>,_>>()?;
        let mut records = Vec::new();
        let mut needs_snapshot = false;
        for (sequence,origin,kind,encoded_work,mut assignment,body) in events {
            let mut work = encoded_work.map(|encoded|serde_json::from_str::<WorkRef>(&encoded)).transpose()?;
            if let Some(work) = &work {work.validate()?;}
            let mut decision_id: Option<String> = None;
            let mut decision_revision: Option<i64> = None;
            let mut evidence_id: Option<String> = None;
            if origin=="host" && matches!(kind.as_str(),"decision_resolved"|"evidence_recorded"|"work_completed"|"work_evidence_invalidated"|"work_revoked"|"prerequisite_invalidated"|"safe_idle_release") {
                let identifiers: Value = serde_json::from_str(&body)?;
                if let Some(value) = identifiers.get("work").filter(|value|!value.is_null()) {
                    let identified_work: WorkRef = serde_json::from_value(value.clone())?;
                    identified_work.validate()?;
                    work=Some(identified_work);
                }
                if assignment.is_none() {assignment=identifiers["assignment"].as_str().map(str::to_owned);}
                if kind=="decision_resolved" {
                    decision_id=identifiers["decision"].as_str().filter(|id|super::key(id).is_ok()).map(str::to_owned);
                    decision_revision=identifiers["decision_revision"].as_i64().filter(|revision|*revision>0);
                    needs_snapshot=decision_id.is_none() || decision_revision.is_none();
                } else {
                    needs_snapshot=work.is_none();
                    if kind=="evidence_recorded" {
                        evidence_id=identifiers["evidence_id"].as_str().filter(|id|super::key(id).is_ok()).map(str::to_owned);
                        needs_snapshot |= evidence_id.is_none();
                    }
                }
            } else {
                needs_snapshot=match kind.as_str() {
                    "assignment_accepted"|"assignment_working"|"worker_status"|"native_delivery"|"reconciled"|"host_stop_notice" => assignment.is_none(),
                    "coordinate" => work.is_none(),
                    "status"|"context_backlog" => false,
                    _ => true,
                };
            }
            if let Some(assignment) = &assignment {super::key(assignment)?;}
            if needs_snapshot {break;}
            let source = match kind.as_str() {
                "assignment_accepted"|"assignment_working"|"worker_status" => "assigned_worker_report_not_evidence",
                "native_delivery" => "native_adapter_report_not_completion_evidence",
                _ if origin=="host" => "host",
                _ => "agent_report_not_evidence",
            };
            records.push(json!({"sequence":sequence,"source":source,"kind":kind,"work":work,"assignment":assignment,"decision_id":decision_id,"decision_revision":decision_revision,"evidence_id":evidence_id}));
            if kind=="worker_status" {
                let report: Value=serde_json::from_str(&body)?;
                records.last_mut().expect("event was added")["summary"]=report["summary"].clone();
            }
        }
        if needs_snapshot {context_recovery_snapshot(connection,live,after,limit.min(remaining))?}
        else {
            let page = super::context_page(&records,limit.min(remaining))?;
            let next = page.as_array().and_then(|records|records.last()).and_then(|record|record["sequence"].as_i64()).unwrap_or(after);
            json!({"records":page,"next":next,"has_more":next<through,"snapshot_required":false})
        }
    };
    result["bootstrap_pending"] = json!(turn.is_none());
    if let Some((dispatch,_,_,_)) = turn {
        let count = result["records"].as_array().map_or(0,Vec::len);
        connection.execute("UPDATE chief_turns SET records=records+?2 WHERE dispatch_id=?1",params![dispatch,count as i64])?;
        if count==remaining && result["has_more"]==true {
            let continuation = if result["snapshot_required"]==true {json!({"snapshot_after":result["snapshot_next"],"generation":result["snapshot_generation"]})}
                else {json!({"resume_after":result["next"]})};
            context_continuation(connection,&dispatch,&continuation,now_ms)?;
        }
    } else {connection.execute("UPDATE chief_coordination SET preview_records=preview_records+?2 WHERE epoch=?1",params![epoch,result["records"].as_array().map_or(0,Vec::len) as i64])?;}
    result["cursor_kind"] = json!("event_sequence");
    result["event_cursor"] = json!(through);
    Ok(result)
}

fn context_recovery_snapshot(connection: &Connection, live: &[WorkerState], after: i64, limit: usize) -> Result<Value, TeamError> {
    let mut result = context_snapshot(connection,live,0,limit)?;
    result["snapshot_required"] = json!(true);
    result["snapshot_next"] = result["next"].clone();
    result["snapshot_generation"] = json!(context_generation(connection,live)?);
    result["next"] = json!(after);
    Ok(result)
}

fn review_generation(connection: &Connection, live: &[WorkerState], now_ms: i64) -> Result<String, TeamError> {
    let revision: i64 = connection.query_row("SELECT revision FROM chief_review_generation WHERE id=1",[],|row|row.get(0))?;
    Ok(crate::audit::sha256_hex(&serde_json::to_vec(&(revision,material_workers(live,now_ms)))?))
}

fn summary_reference_is_current(connection: &Connection, id: &str) -> Result<bool, TeamError> {
    let encoded: Option<String> = connection.query_row("SELECT json_object('id',id,'work',json_object('task',json_object('project',project,'task_id',task_id),'revision',work_revision),
        'kind',kind,'commit',commit_id,'source',source,'result',json(result)) FROM chief_evidence WHERE id=?1",[id],|row|row.get(0)).optional()?;
    let Some(encoded) = encoded else {return Ok(false)};
    let evidence: super::Evidence = serde_json::from_str(&encoded)?;
    let current: Option<String> = connection.query_row("SELECT brief FROM chief_work_revisions WHERE project=?1 AND task_id=?2 AND revision=?3 AND state IN ('approved','completed')",
        params![evidence.work.task.project,evidence.work.task.task_id,evidence.work.revision],|row|row.get(0)).optional()?;
    let Some(encoded) = current else {return Ok(false)};
    let brief: WorkBrief = serde_json::from_str(&encoded)?;
    Ok(latest_gate_evidence(connection,&evidence.work,evidence.kind)?.is_some_and(|latest|latest.id==evidence.id)
        && current_evidence(connection,&evidence)? && brief.gates.iter().any(|gate|gate.kind==evidence.kind && super::evidence::validate(gate,&evidence).is_ok()))
}

fn bounded_chief_snapshot(connection: &Connection, live: &[WorkerState], epoch: &str, after: i64, limit: usize, generation: Option<&str>, now_ms: i64) -> Result<Value, TeamError> {
    let current_generation = context_generation(connection,live)?;
    if (after>0 && generation.is_none()) || generation.is_some_and(|expected|expected!=current_generation) {
        return Ok(json!({"records":[],"snapshot_required":true,"invalidated":true,"next":0,"generation":current_generation}));
    }
    let turn = current_turn(connection,epoch)?;
    let remaining = context_remaining(connection,epoch,turn.as_ref())?;
    if remaining==0 {return Err(TeamError::new(ErrorCode::Limit,"chief turn event-record budget exhausted"))}
    let mut page = context_snapshot(connection,live,after,limit.min(remaining))?;
    let head: i64 = connection.query_row("SELECT coalesce(max(seq),0) FROM chief_events",[],|row|row.get(0))?;
    page["event_cursor"] = json!(turn.as_ref().map_or(head,|(_,through,_,_)|*through));
    page["generation"] = json!(current_generation);
    page["cursor_kind"] = json!("snapshot_offset");
    page["bootstrap_pending"] = json!(turn.is_none());
    if let Some((dispatch,_,_,_)) = turn {
        let count = page["records"].as_array().map_or(0,Vec::len);
        connection.execute("UPDATE chief_turns SET records=records+?2 WHERE dispatch_id=?1",params![dispatch,count as i64])?;
        if count==remaining && page["has_more"]==true {context_continuation(connection,&dispatch,&json!({"snapshot_after":page["next"],"generation":page["generation"]}),now_ms)?;}
    } else {connection.execute("UPDATE chief_coordination SET preview_records=preview_records+?2 WHERE epoch=?1",params![epoch,page["records"].as_array().map_or(0,Vec::len) as i64])?;}
    Ok(page)
}

fn context_remaining(connection: &Connection, epoch: &str, turn: Option<&(String,i64,i64,i64)>) -> Result<usize, TeamError> {
    let records = if let Some((_,_,_,records)) = turn {*records}
        else {connection.query_row("SELECT preview_records FROM chief_coordination WHERE epoch=?1",[epoch],|row|row.get::<_,i64>(0))?};
    Ok(50i64.saturating_sub(records).max(0) as usize)
}

fn context_continuation(connection: &Connection, dispatch: &str, continuation: &Value, now_ms: i64) -> Result<(), TeamError> {
    let epoch: String = connection.query_row("SELECT epoch FROM chief_turns WHERE dispatch_id=?1",[dispatch],|row|row.get(0))?;
    let mut continuation = continuation.clone();
    continuation["epoch"] = json!(epoch);
    connection.execute("INSERT OR IGNORE INTO chief_events(id,source,submission_key,fingerprint,kind,body,created_at) VALUES (?1,'host',?2,?3,'context_backlog',?4,?5)",
        params![crate::teams::store::identifier()?,format!("backlog-{dispatch}"),crate::audit::sha256_hex(&serde_json::to_vec(&continuation)?),continuation.to_string(),now_ms])?;
    Ok(())
}

fn context_generation(connection: &Connection, live: &[WorkerState]) -> Result<String, TeamError> {
    Ok(crate::audit::sha256_hex(&serde_json::to_vec(&context_records(connection,live)?)?))
}

fn context_snapshot(connection: &Connection, live: &[WorkerState], after: i64, limit: usize) -> Result<Value, TeamError> {
    super::validate_cursor(after, limit)?;
    let records = context_records(connection,live)?;
    let start = usize::try_from(after).unwrap_or(usize::MAX).min(records.len());
    let page = super::context_page(&records[start..], limit)?;
    let next = start + page.as_array().expect("context page is an array").len();
    Ok(json!({"records": page, "next": next, "has_more": next < records.len(), "scheduling_owned": scheduling_owned(connection)?}))
}

fn context_records(connection: &Connection, live: &[WorkerState]) -> Result<Vec<Value>, TeamError> {
    let mut records = Vec::new();
    records.extend(connection.prepare("SELECT e.assignment,json_extract(e.body,'$.summary'),e.created_at FROM chief_events e
        JOIN chief_assignments a ON a.id=e.assignment WHERE e.kind IN ('status','worker_status') AND a.state NOT IN ('completed','cancelled')
        AND e.seq=(SELECT max(latest.seq) FROM chief_events latest WHERE latest.assignment=e.assignment AND latest.kind IN ('status','worker_status')) ORDER BY e.assignment")?
        .query_map([],|row|Ok(json!({"kind":"worker_status","source":"assigned_worker_report_not_evidence","assignment":row.get::<_,String>(0)?,"summary":row.get::<_,String>(1)?,"reported_at_ms":row.get::<_,i64>(2)?})))?.collect::<Result<Vec<_>,_>>()?);
    let mut statement = connection.prepare("SELECT session_id,epoch,state FROM chief_grants WHERE state!='disabled' ORDER BY id")?;
    records.extend(statement.query_map([], |row| Ok(json!({"kind": "chief", "session_id": row.get::<_, String>(0)?, "epoch": row.get::<_, String>(1)?, "state": row.get::<_, String>(2)?})))?.collect::<Result<Vec<_>, _>>()?);
    for worker in live {
        records.push(json!({"kind": "worker", "session_id": worker.session.session_id, "agent": worker.session.agent,
            "project": worker.session.project, "role": worker.role, "ready": worker.ready, "blocked_by": worker.blocked_by}));
    }
    let mut statement = connection.prepare("SELECT brief,priority,state FROM chief_work_revisions WHERE state IN ('approved','completed') ORDER BY priority,project,task_id")?;
    let work = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?,row.get::<_,String>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    for (encoded, priority, state) in work {
        let brief: WorkBrief = serde_json::from_str(&encoded)?;
        records.push(json!({"kind": "work", "project": brief.work.task.project, "task_id": brief.work.task.task_id,
            "revision": brief.work.revision, "state": state, "priority": priority, "brief": brief.brief,
            "role": brief.role, "prerequisites": brief.prerequisites}));
    }
    records.extend(connection.prepare("SELECT id,work,state,revision,resolution FROM chief_decisions ORDER BY created_at,id")?
        .query_map([],|row|Ok(json!({"kind":"decision","source":"protected_host_control","id":row.get::<_,String>(0)?,
            "work":row.get::<_,Option<String>>(1)?.map(|encoded|serde_json::from_str::<Value>(&encoded)).transpose().map_err(|error|rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
            "state":row.get::<_,String>(2)?,"revision":row.get::<_,i64>(3)?,"resolution":row.get::<_,Option<String>>(4)?,"resolution_grants_authority":false})))?
        .collect::<Result<Vec<_>,_>>()?);
    records.extend(connection.prepare("SELECT id,project,task_id,work_revision,worker,state,revision FROM chief_assignments ORDER BY created_at,id")?
        .query_map([],|row|Ok(json!({"kind":"assignment","source":"protected_dispatch_ledger","id":row.get::<_,String>(0)?,
            "work":{"task":{"project":row.get::<_,String>(1)?,"task_id":row.get::<_,String>(2)?},"revision":row.get::<_,i64>(3)?},
            "worker":row.get::<_,String>(4)?,"state":row.get::<_,String>(5)?,"revision":row.get::<_,i64>(6)?})))?.collect::<Result<Vec<_>,_>>()?);
    records.extend(connection.prepare("SELECT e.id,e.project,e.task_id,e.work_revision,e.kind,e.source FROM chief_evidence e JOIN chief_work_revisions w
        ON w.project=e.project AND w.task_id=e.task_id AND w.revision=e.work_revision WHERE w.state IN ('approved','completed') ORDER BY e.id")?
        .query_map([],|row|Ok(json!({"kind":"evidence","id":row.get::<_,String>(0)?,"work":{"task":{"project":row.get::<_,String>(1)?,"task_id":row.get::<_,String>(2)?},"revision":row.get::<_,i64>(3)?},
            "gate":row.get::<_,String>(4)?,"source":row.get::<_,String>(5)?,"requires_current_gate_validation":true})))?.collect::<Result<Vec<_>,_>>()?);
    for record in &mut records {
        if record.get("source").is_none() {record["source"]=json!(if record["kind"]=="worker" {"daemon_derived_availability"} else {"protected_host_approval"});}
    }
    Ok(records)
}

/// Host-only structured review records. Never use this projection for agent Context.
pub fn snapshot_page(connection: &Connection, live: &[WorkerState], after: i64, limit: usize, generation: Option<&str>, now_ms: i64) -> Result<Value, TeamError> {
    super::validate_cursor(after,limit)?;
    let current = review_generation(connection,live,now_ms)?;
    if (after>0 && generation.is_none()) || generation.is_some_and(|expected|expected!=current) {
        return Ok(json!({"records":[],"next":0,"has_more":false,"generation":current,"invalidated":true,"snapshot_required":true,"projection":"host_review","cursor_kind":"snapshot_offset"}));
    }
    let mut result = snapshot(connection,live,after,limit)?;
    result["generation"] = json!(current);
    Ok(result)
}

pub fn snapshot(connection: &Connection, live: &[WorkerState], after: i64, limit: usize) -> Result<Value, TeamError> {
    review_records(connection,live,after,limit,false)
}

pub fn audit_page(connection: &Connection, live: &[WorkerState], after: i64, limit: usize, generation: Option<&str>, now_ms: i64) -> Result<Value, TeamError> {
    super::validate_cursor(after,limit)?;
    let current=review_generation(connection,live,now_ms)?;
    if (after>0 && generation.is_none()) || generation.is_some_and(|expected|expected!=current) {
        return Ok(json!({"records":[],"next":0,"has_more":false,"generation":current,"invalidated":true,"projection":"host_audit"}));
    }
    let mut page=review_records(connection,live,after,limit,true)?;
    page["generation"]=json!(current); page["projection"]=json!("host_audit");
    Ok(page)
}

fn review_records(connection: &Connection, live: &[WorkerState], after: i64, limit: usize, audit: bool) -> Result<Value, TeamError> {
    super::validate_cursor(after,limit)?;
    // Stable category/key ordering within a read. Only whole records cross the bounded reply boundary.
    let mut statement = connection.prepare("WITH selected_proposals AS (
        SELECT * FROM chief_work_proposals p WHERE NOT EXISTS(SELECT 1 FROM chief_work_proposals newer WHERE newer.project=p.project AND newer.task_id=p.task_id AND newer.revision>p.revision)
        AND NOT EXISTS(SELECT 1 FROM chief_work_revisions w WHERE w.project=p.project AND w.task_id=p.task_id AND w.revision>=p.revision)
        ), selected_assignments AS (
        SELECT * FROM chief_assignments WHERE state NOT IN ('completed','cancelled')
        OR id IN (SELECT id FROM chief_assignments ORDER BY created_at DESC,rowid DESC LIMIT 16)
        OR id IN (SELECT assignment FROM chief_decisions WHERE state='open')
        ), selected_work AS (
        SELECT * FROM chief_work_revisions w WHERE state='approved'
        OR rowid IN (SELECT rowid FROM chief_work_revisions WHERE state='completed' ORDER BY created_at DESC,rowid DESC LIMIT 16)
        OR EXISTS(SELECT 1 FROM selected_assignments a WHERE a.project=w.project AND a.task_id=w.task_id AND a.work_revision=w.revision)
        OR EXISTS(SELECT 1 FROM chief_decisions d WHERE d.state='open' AND json_extract(d.work,'$.task.project')=w.project AND json_extract(d.work,'$.task.task_id')=w.task_id AND json_extract(d.work,'$.revision')=w.revision)
        OR EXISTS(SELECT 1 FROM chief_work_revisions active,json_each(active.brief,'$.prerequisites') p WHERE active.state='approved'
            AND json_extract(p.value,'$.task.project')=w.project AND json_extract(p.value,'$.task.task_id')=w.task_id AND json_extract(p.value,'$.revision')=w.revision)
        OR EXISTS(SELECT 1 FROM selected_proposals pending,json_each(pending.brief,'$.prerequisites') p
            WHERE json_extract(p.value,'$.task.project')=w.project AND json_extract(p.value,'$.task.task_id')=w.task_id AND json_extract(p.value,'$.revision')=w.revision)
        ), records(category,identity,record) AS (
        SELECT 0,id,json_object('kind','chief','session',json_object('session_id',session_id,'agent',agent,'project',project,'runtime',runtime),
            'session_id',session_id,'epoch',epoch,'revision',revision,'state',state,'approval_hash',approval_hash,'created_at_ms',created_at,
            'live_verified',EXISTS(SELECT 1 FROM json_each(?1) WHERE json_extract(value,'$.session.session_id')=chief_grants.session_id
                AND json_extract(value,'$.session.runtime')=chief_grants.runtime AND json_extract(value,'$.session.project')=chief_grants.project
                AND json_extract(value,'$.session.agent')=chief_grants.agent)) FROM chief_grants WHERE ?4 OR state!='disabled' OR epoch IN (SELECT chief_epoch FROM selected_assignments)
        UNION ALL SELECT 1,json_extract(value,'$.session.session_id'),json_object('kind','worker','session',json_extract(value,'$.session'),
            'session_id',json_extract(value,'$.session.session_id'),'role',json_extract(value,'$.role'),'ready',json_extract(value,'$.ready'),
            'blocked_by',json_extract(value,'$.blocked_by'),'observed_at_ms',json_extract(value,'$.observed_at_ms'),
            'observation_provenance','daemon_derived_availability') FROM json_each(?1)
        UNION ALL SELECT 2,epoch,json_object('kind','binding','session',json_object('session_id',session_id,'agent',agent,'project',project,'runtime',runtime),
            'epoch',epoch,'revision',revision,'role',role,'state',state,'approval_hash',approval_hash,'created_at_ms',created_at) FROM chief_worker_bindings WHERE ?4 OR state='bound' OR epoch IN (SELECT worker_epoch FROM selected_assignments)
        UNION ALL SELECT 3,m.session_id,json_object('kind','team_member','task',json_object('project',t.project,'task_id',t.task_id),'session',json_object('session_id',m.session_id,
            'agent',m.agent,'runtime',m.runtime,'project',t.project),'role',m.role,'epoch',m.epoch,'team_revision',t.revision) FROM members m JOIN teams t ON t.task_key=m.task_key
        UNION ALL SELECT 4,p.id,json_object('kind','proposal','id',p.id,'sender',json(p.sender),'submission_key',p.submission_key,'brief',json(p.brief),
            'approval_hash',p.approval_hash,'created_at_ms',p.created_at,'provenance','agent_proposal_not_authority',
            'freshness',CASE WHEN EXISTS(SELECT 1 FROM chief_work_revisions w WHERE w.project=p.project AND w.task_id=p.task_id AND w.revision=p.revision)
                THEN (SELECT state FROM chief_work_revisions w WHERE w.project=p.project AND w.task_id=p.task_id AND w.revision=p.revision)
                WHEN p.revision < (SELECT max(revision) FROM (SELECT revision FROM chief_work_proposals WHERE project=p.project AND task_id=p.task_id UNION ALL
                    SELECT revision FROM chief_work_revisions WHERE project=p.project AND task_id=p.task_id)) THEN 'superseded' ELSE 'pending' END) FROM chief_work_proposals p
            WHERE ?4 OR p.id IN (SELECT id FROM selected_proposals)
        UNION ALL SELECT 5,project||'/'||task_id||'/'||revision,json_object('kind','work','brief',json(brief),'approval_hash',approval_hash,
            'state',state,'priority',priority,'created_at_ms',created_at) FROM chief_work_revisions WHERE ?4 OR (project,task_id,revision) IN (SELECT project,task_id,revision FROM selected_work)
        UNION ALL SELECT 6,id,json_object('kind','assignment','id',id,'work',json_object('task',json_object('project',project,'task_id',task_id),'revision',work_revision),
            'worker',worker,'worker_runtime',worker_runtime,'worker_epoch',worker_epoch,
            'worker_identity',json((SELECT recipient_identity FROM chief_inbox WHERE assignment=chief_assignments.id)),
            'chief_grant',chief_grant,'chief_epoch',chief_epoch,
            'state',state,'revision',revision,'created_at_ms',created_at,'finished_at_ms',finished_at) FROM chief_assignments WHERE ?4 OR id IN (SELECT id FROM selected_assignments)
        UNION ALL SELECT 7,id,json_object('kind','decision','id',id,'scope_key',scope_key,'decision_key',decision_key,'work',json(work),'assignment',assignment,
            'question',question,'recommendation',recommendation,'state',state,'revision',revision,'resolution',resolution,'created_at_ms',created_at,'resolved_at_ms',resolved_at) FROM chief_decisions WHERE ?4 OR state='open'
        UNION ALL SELECT 8,id,json_object('kind','source','id',id,'evidence',json(evidence),'approval_hash',approval_hash,'imported',imported,'created_at_ms',created_at) FROM chief_sources s WHERE ?4 OR
            ((json_extract(evidence,'$.work.task.project'),json_extract(evidence,'$.work.task.task_id'),json_extract(evidence,'$.work.revision')) IN (SELECT project,task_id,revision FROM selected_work)
            AND (rowid IN (SELECT rowid FROM chief_sources newer WHERE json_extract(newer.evidence,'$.work')=json_extract(s.evidence,'$.work') AND json_extract(newer.evidence,'$.kind')=json_extract(s.evidence,'$.kind') ORDER BY rowid DESC LIMIT 3)
                OR id IN (SELECT source_id FROM chief_evidence e WHERE e.rowid=(SELECT max(newer.rowid) FROM chief_evidence newer WHERE newer.project=e.project AND newer.task_id=e.task_id AND newer.work_revision=e.work_revision AND newer.kind=e.kind))))
        UNION ALL SELECT 9,id,json_object('kind','evidence','id',id,'work',json_object('task',json_object('project',project,'task_id',task_id),'revision',work_revision),
            'gate',kind,'commit',commit_id,'source',source,'source_id',source_id,'result',json(result),'created_at_ms',created_at) FROM chief_evidence e WHERE ?4 OR
            ((project,task_id,work_revision) IN (SELECT project,task_id,revision FROM selected_work) AND rowid=(SELECT max(rowid) FROM chief_evidence newer WHERE newer.project=e.project AND newer.task_id=e.task_id AND newer.work_revision=e.work_revision AND newer.kind=e.kind))
            OR id IN (SELECT json_extract(value,'$.id') FROM json_each((SELECT response FROM chief_summary_requests WHERE state='completed' ORDER BY completed_at DESC LIMIT 1),'$.references'))
        UNION ALL SELECT 10,id,json_object('kind','delivery','id',id,'assignment',assignment,'event_id',event_id,'recipient',recipient,'recipient_runtime',recipient_runtime,
            'recipient_epoch',recipient_epoch,'delivery_kind',kind,'envelope',json(envelope),'state',state,'native_id',native_id,'receipt',json(receipt),
            'started_at_ms',started_at,'finished_at_ms',finished_at) FROM chief_delivery_attempts WHERE ?4 OR state IN ('submitting','uncertain')
        UNION ALL SELECT 11,id,json_object('kind','event','id',id,'sequence',seq,'source',source,'work',json(work),'assignment',assignment,
            'event_kind',kind,'body',body,'created_at_ms',created_at) FROM chief_events WHERE ?4 OR id IN (SELECT event_id FROM chief_delivery_attempts WHERE state IN ('submitting','uncertain'))
        UNION ALL SELECT 12,identity,json_object('kind','admission','session',json(identity),'adapter',adapter,'evidence',evidence,'verified_at_ms',verified_at) FROM chief_admission_verifications WHERE ?4 OR identity IN (SELECT json_extract(value,'$.session') FROM json_each(?1))
        UNION ALL SELECT 13,id,json_object('kind','inbox','id',id,'assignment',assignment,'chief_epoch',chief_epoch,'recipient',json(recipient_identity),
            'recipient_epoch',recipient_epoch,'work',json(work),'envelope',json(envelope),'state',state,'next_at_ms',next_at,'attempts',attempts,'revision',attempts) FROM chief_inbox WHERE ?4 OR state IN ('queued','submitting','uncertain','failed')
        UNION ALL SELECT 14,token,json_object('kind','legacy_launch','id',token,'project',project,'task_id',task_id,'worker',worker,'state',state,'revision',revision,'created_at_ms',created_at) FROM legacy_launches WHERE ?4 OR state!='released'
        UNION ALL SELECT 15,'summary',json_object('kind','summary','state',coalesce((SELECT state FROM chief_summary_requests ORDER BY created_at DESC,rowid DESC LIMIT 1),'unavailable'),
            'request',json((SELECT json_object('id',id,'state',state,'created_at_ms',created_at,'deadline_ms',deadline) FROM chief_summary_requests ORDER BY created_at DESC,rowid DESC LIMIT 1)),
            'last_completed_at_ms',(SELECT completed_at FROM chief_summary_requests WHERE state='completed' ORDER BY completed_at DESC LIMIT 1),
            'last_result',json((SELECT response FROM chief_summary_requests WHERE state='completed' ORDER BY completed_at DESC LIMIT 1)))
        ) SELECT record FROM records ORDER BY category,identity LIMIT ?2 OFFSET ?3")?;
    let encoded = statement.query_map(params![serde_json::to_string(live)?,limit as i64 + 1,after,audit],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    let mut records = encoded.iter().map(|record|Ok(serde_json::from_str::<Value>(record)?)).collect::<Result<Vec<_>,TeamError>>()?;
    for record in &mut records {
        if record["kind"] == "summary" && let Some(references) = record["last_result"]["references"].as_array_mut() {
            for reference in references {
                let id = reference["id"].as_str().ok_or_else(||conflict("summary reference lacks its persisted identity"))?;
                let verified = summary_reference_is_current(connection,id)?;
                reference["verified_current"] = json!(verified);
                reference["provenance"] = json!(if verified {"protected_current_evidence"} else {"narrative_unverified"});
            }
        }
        if record["kind"] == "source" {
            let evidence: super::Evidence = serde_json::from_value(record["evidence"].clone())?;
            let current: Option<(String,String)> = connection.query_row("SELECT brief,approval_hash FROM chief_work_revisions WHERE project=?1 AND task_id=?2 AND revision=?3 AND state IN ('approved','completed')",
                params![evidence.work.task.project,evidence.work.task.task_id,evidence.work.revision],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
            let applicability = (|| {
                let (encoded,hash) = current.ok_or_else(||conflict("source approval is no longer current"))?;
                if record["approval_hash"] != hash { return Err(conflict("source approval hash differs")); }
                let brief: WorkBrief = serde_json::from_str(&encoded)?;
                let gate = brief.gates.iter().find(|gate|gate.kind==evidence.kind).ok_or_else(||conflict("source gate is not approved"))?;
                super::evidence::validate(gate,&evidence)?;
                if !current_evidence(connection,&evidence)? { return Err(conflict("source role/handoff is stale")); }
                Ok::<_,TeamError>(())
            })();
            record["can_import"] = json!(applicability.is_ok());
            record["unavailable_reason"] = json!(applicability.err().map(|error|error.message));
        }
    }
    let page = super::context_page(&records,limit)?;
    let count = page.as_array().expect("context page is array").len();
    Ok(json!({"records":page,"next":after.saturating_add(count as i64),"has_more":count<records.len(),
        "scheduling_owned":scheduling_owned(connection)?,"projection":"host_review","cursor_kind":"snapshot_offset"}))
}

/// Upgrade the shared protected team/chief database atomically, refusing future versions.
pub fn migrate(connection: &mut Connection) -> Result<(), TeamError> {
    crate::teams::store::migrate(connection)
}

pub(crate) fn upgrade_review(connection: &Connection) -> Result<(), TeamError> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS chief_work_proposals (
        id TEXT PRIMARY KEY, sender TEXT NOT NULL, submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
        project TEXT NOT NULL, task_id TEXT NOT NULL, revision INTEGER NOT NULL, brief TEXT NOT NULL,
        approval_hash TEXT NOT NULL, created_at INTEGER NOT NULL, UNIQUE(sender,submission_key), UNIQUE(project,task_id,revision));
        CREATE TRIGGER IF NOT EXISTS chief_work_proposal_immutable BEFORE UPDATE ON chief_work_proposals
        BEGIN SELECT RAISE(ABORT,'work proposal is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS chief_work_proposal_no_delete BEFORE DELETE ON chief_work_proposals
        BEGIN SELECT RAISE(ABORT,'work proposal is immutable'); END;")?;
    Ok(())
}

fn latest_revision(connection: &Connection, task: &crate::teams::TaskKey) -> Result<i64, TeamError> {
    Ok(connection.query_row("SELECT coalesce(max(revision),0) FROM (
        SELECT revision FROM chief_work_revisions WHERE project=?1 AND task_id=?2 UNION ALL
        SELECT revision FROM chief_work_proposals WHERE project=?1 AND task_id=?2)", params![task.project,task.task_id], |row|row.get(0))?)
}

fn propose_work(connection: &Connection, sender: &VerifiedSession, key: &str, proposal: &super::WorkProposal, now_ms: i64) -> Result<Value, TeamError> {
    let chief: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE session_id=?1 AND agent=?2 AND project=?3 AND runtime=?4 AND state='active')",
        params![sender.session_id,sender.agent,sender.project,sender.runtime], |row|row.get(0))?;
    if !chief && (proposal.task.project != sender.project || proposal.prerequisites.iter().any(|work|work.task.project != sender.project)) { return Err(unauthorized()); }
    let sender_session_id = &sender.session_id;
    let sender = serde_json::to_string(sender)?;
    let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(proposal)?);
    let previous: Option<(String,String,String,String,i64)> = connection.query_row(
        "SELECT id,fingerprint,brief,approval_hash,created_at FROM chief_work_proposals WHERE sender=?1 AND submission_key=?2",
        params![sender,key], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
    let (id,encoded,hash,created_at) = if let Some((id,original,encoded,hash,created_at)) = previous {
        if original != fingerprint { return Err(conflict("work proposal key reused with different content")); }
        (id,encoded,hash,created_at)
    } else {
        let recent: i64 = connection.query_row("SELECT count(*) FROM chief_work_proposals WHERE json_extract(sender,'$.session_id')=?1 AND created_at>?2",
            params![sender_session_id,now_ms.saturating_sub(60_000)],|row|row.get(0))?;
        if recent >= 10 { return Err(TeamError::new(ErrorCode::Limit,"work proposal rate limit reached")); }
        let pending: i64 = connection.query_row("SELECT count(*) FROM chief_work_proposals p
            WHERE NOT (p.project=?1 AND p.task_id=?2)
            AND NOT EXISTS(SELECT 1 FROM chief_work_proposals newer WHERE newer.project=p.project AND newer.task_id=p.task_id AND newer.revision>p.revision)
            AND NOT EXISTS(SELECT 1 FROM chief_work_revisions w WHERE w.project=p.project AND w.task_id=p.task_id AND w.revision>=p.revision)",
            params![proposal.task.project,proposal.task.task_id],|row|row.get(0))?;
        if pending >= super::MAX_ACTIVE_WORK as i64 { return Err(TeamError::new(ErrorCode::Limit,"pending work proposal intake is full")); }
        let revision = latest_revision(connection,&proposal.task)?.checked_add(1).ok_or_else(||conflict("work revision exhausted"))?;
        let brief = proposal.at_revision(revision);
        let hash = brief.approval_hash()?;
        let id = crate::teams::store::identifier()?;
        let encoded = serde_json::to_string(&brief)?;
        connection.execute("INSERT INTO chief_work_proposals VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![id,sender,key,fingerprint,proposal.task.project,proposal.task.task_id,revision,encoded,hash,now_ms])?;
        (id,encoded,hash,now_ms)
    };
    Ok(json!({"id":id,"sender":serde_json::from_str::<Value>(&sender)?,"brief":serde_json::from_str::<Value>(&encoded)?,
        "approval_hash":hash,"created_at_ms":created_at,"provenance":"agent_proposal_not_authority"}))
}

pub(crate) fn upgrade_scheduling(connection: &Connection) -> Result<(), TeamError> {
    connection.execute_batch("CREATE TABLE scheduling_reservations (
        project TEXT NOT NULL, task_id TEXT NOT NULL, worker TEXT NOT NULL, token TEXT NOT NULL UNIQUE,
        owner TEXT NOT NULL CHECK(owner IN ('chief','legacy','observed')),
        PRIMARY KEY(project,task_id));
        INSERT INTO scheduling_reservations SELECT project,task_id,worker,id,'chief' FROM chief_assignments
            WHERE state NOT IN ('completed','cancelled');
        CREATE TABLE chief_admission_verifications (
            identity TEXT PRIMARY KEY, adapter TEXT NOT NULL, evidence TEXT NOT NULL, verified_at INTEGER NOT NULL);")?;
    Ok(())
}

pub fn verify_admission(connection: &mut Connection, session: &VerifiedSession, adapter: &str, evidence: &str, now_ms: i64) -> Result<(), TeamError> {
    connection.execute("INSERT OR REPLACE INTO chief_admission_verifications VALUES (?1,?2,?3,?4)",
        params![serde_json::to_string(session)?,adapter,evidence,now_ms])?;
    Ok(())
}

pub fn admission_verified(connection: &Connection, session: &VerifiedSession) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_admission_verifications WHERE identity=?1 AND adapter='opencode-queued-v1')",
        [serde_json::to_string(session)?], |row| row.get(0))?)
}

pub fn legacy_worker_busy(connection: &Connection, worker: &str) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM scheduling_reservations WHERE worker=?1 AND owner<>'chief')", [worker], |row| row.get(0))?)
}

pub fn observe_resumed(connection: &Connection, live: &[VerifiedSession]) -> Result<(), TeamError> {
    for session in live {
        connection.execute("UPDATE legacy_launches SET state='released',revision=revision+1 WHERE token IN (SELECT token FROM scheduling_reservations WHERE owner='legacy' AND project=?1 AND task_id=?2 AND worker=?3)",
            params![session.project,format!("resume:{}",session.session_id),session.session_id])?;
        connection.execute("DELETE FROM scheduling_reservations WHERE owner='legacy' AND project=?1 AND task_id=?2 AND worker=?3",
            params![session.project,format!("resume:{}",session.session_id),session.session_id])?;
    }
    Ok(())
}

pub fn require_task_mapping(connection: &Connection, session: &VerifiedSession, task: &crate::teams::TaskKey) -> Result<(), TeamError> {
    let member: Option<(String,String,String)> = connection.query_row("SELECT task_key,runtime,agent FROM members WHERE session_id=?1", [&session.session_id],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    if let Some((key,runtime,agent)) = member {
        if serde_json::from_str::<crate::teams::TaskKey>(&key)? != *task || runtime != session.runtime || agent != session.agent { return Err(unauthorized()); }
    }
    Ok(())
}

fn require_unreserved(connection: &Connection, task: &crate::teams::TaskKey, worker: Option<&str>) -> Result<(), TeamError> {
    connection.execute("DELETE FROM scheduling_reservations WHERE owner='chief' AND token IN (SELECT id FROM chief_assignments WHERE state IN ('completed','cancelled'))", [])?;
    // Only a pending legacy launch may still pick any session of its project; observed "doing" work
    // without a session reserves its own task, and per-worker busy checks cover whoever runs it.
    let owner: Option<(String, String)> = connection.query_row(
        "SELECT owner,worker FROM scheduling_reservations WHERE ((project=?1 OR project='') AND task_id=?2)
            OR (worker<>'' AND worker=?3) OR ((project=?1 OR project='') AND worker='' AND owner='legacy') LIMIT 1",
        params![task.project, task.task_id, worker.unwrap_or("")], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    if let Some((owner, worker)) = owner { return Err(conflict(&format!("work already reserved by {owner}: {worker}"))); }
    Ok(())
}

/// Host-only reservation before any legacy send/spawn. A lost reply is not permission to retry.
pub fn reserve_legacy(connection: &mut Connection, task: &crate::teams::TaskKey, worker: Option<&str>, _now_ms: i64) -> Result<String, TeamError> {
    reserve_legacy_kind(connection,task,worker,false,_now_ms)
}

pub fn reserve_legacy_kind(connection: &mut Connection, task: &crate::teams::TaskKey, worker: Option<&str>, manual: bool, now_ms: i64) -> Result<String, TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if scheduling_owned(&transaction)? { return Err(conflict("chief owns scheduling (including paused, absent or unresolved work)")); }
    let protected: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_work_revisions WHERE project=?1 AND task_id=?2)",
        params![task.project,task.task_id], |row| row.get(0))?;
    if protected { return Err(conflict("chief-owned work requires host reconciliation, not legacy handoff")); }
    require_unreserved(&transaction, task, worker)?;
    let token = crate::teams::store::identifier()?;
    transaction.execute("INSERT INTO scheduling_reservations VALUES (?1,?2,?3,?4,'legacy')", params![task.project,task.task_id,worker.unwrap_or(""),token])?;
    transaction.execute("INSERT INTO legacy_launches VALUES (?1,?2,?3,?4,?5,1,?6)",params![token,task.project,task.task_id,worker,if manual {"manual"} else {"reserved"},now_ms])?;
    transaction.commit()?;
    Ok(token)
}

pub fn settle_launch(connection: &mut Connection, token: &str, expected_revision: i64, outcome: &str) -> Result<Value,TeamError> {
    let transaction=connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let state:Option<String>=transaction.query_row("SELECT state FROM legacy_launches WHERE token=?1 AND revision=?2",params![token,expected_revision],|row|row.get(0)).optional()?;
    let next=match (state.as_deref(),outcome) {
        (Some("reserved"),"begin")=>"submitting",
        (Some("submitting"),"submitted")=>"submitted",
        (Some("submitting"),"uncertain")=>"uncertain",
        (Some("reserved"|"submitting"),"not_submitted")=>"released",
        (Some("manual"),"undo_manual")=>"released",
        _=>return Err(conflict("invalid or stale exact launch transition")),
    };
    transaction.execute("UPDATE legacy_launches SET state=?2,revision=revision+1 WHERE token=?1",params![token,next])?;
    if next=="released" {transaction.execute("DELETE FROM scheduling_reservations WHERE token=?1 AND owner='legacy'",[token])?;}
    transaction.commit()?;
    Ok(json!({"token":token,"state":next,"revision":expected_revision+1}))
}

pub fn release_manual(connection: &mut Connection, project: &str, task: &str, expected_token: &str) -> Result<(),TeamError> {
    let manual:Option<(String,i64)>=connection.query_row("SELECT token,revision FROM legacy_launches WHERE project=?1 AND task_id=?2 AND token=?3 AND state='manual'",params![project,task,expected_token],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    if let Some((token,revision))=manual {settle_launch(connection,&token,revision,"undo_manual")?;}
    Ok(())
}

/// Observe legacy work without conferring chief approval or completing a protected assignment.
pub fn reconcile_todos(connection: &mut Connection, todos: &crate::todo::TodoList, _now_ms: i64) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for item in &todos.items {
        let project = format!("path:{}", item.cwd());
        let task = item.id();
        if item.status() == "doing" {
            transaction.execute("INSERT OR IGNORE INTO scheduling_reservations VALUES (?1,?2,?3,?4,'observed')",
                params![project,task,item.session(),crate::teams::store::identifier()?])?;
            transaction.execute("UPDATE scheduling_reservations SET worker=?3 WHERE project=?1 AND task_id=?2 AND owner<>'chief' AND worker=''", params![project,task,item.session()])?;
        } else if item.status() == "done" {
            transaction.execute("DELETE FROM scheduling_reservations WHERE project=?1 AND task_id=?2 AND owner<>'chief' AND token NOT IN (SELECT token FROM legacy_launches WHERE state IN ('reserved','submitting','uncertain'))", params![project,task])?;
        }
    }
    for (task, issue) in &todos.issues {
        if issue["status"] == "doing" {
            transaction.execute("INSERT INTO scheduling_reservations SELECT '',?1,?2,?3,'observed'
                WHERE NOT EXISTS(SELECT 1 FROM scheduling_reservations WHERE task_id=?1)",
                params![task,issue["session"].as_str().unwrap_or(""),crate::teams::store::identifier()?])?;
            transaction.execute("UPDATE scheduling_reservations SET worker=?2 WHERE task_id=?1 AND owner<>'chief' AND worker=''",
                params![task,issue["session"].as_str().unwrap_or("")])?;
        } else if issue["status"] == "done" {
            transaction.execute("DELETE FROM scheduling_reservations WHERE task_id=?1 AND owner<>'chief' AND token NOT IN (SELECT token FROM legacy_launches WHERE state IN ('reserved','submitting','uncertain'))", [task])?;
        }
    }
    // Only the protected chief lifecycle can release a chief reservation.
    transaction.execute("UPDATE legacy_launches SET state='released',revision=revision+1 WHERE state IN ('manual','submitted') AND NOT EXISTS(SELECT 1 FROM scheduling_reservations r WHERE r.token=legacy_launches.token)",[])?;
    transaction.execute("DELETE FROM scheduling_reservations WHERE owner='chief' AND token IN (SELECT id FROM chief_assignments WHERE state IN ('completed','cancelled'))", [])?;
    transaction.commit()?;
    Ok(())
}

/// Projection is deliberately separate from the authoritative ledger; a failed JSON save is replayable.
pub fn owned_work(connection: &Connection) -> Result<Vec<Value>, TeamError> {
    let mut statement = connection.prepare("SELECT project,task_id,worker,id,state FROM chief_assignments WHERE state NOT IN ('completed','cancelled') ORDER BY project,task_id")?;
    let mut work = statement.query_map([], |row| Ok(json!({"project":row.get::<_,String>(0)?, "task_id":row.get::<_,String>(1)?,
        "worker":row.get::<_,String>(2)?, "assignment":row.get::<_,String>(3)?, "state":row.get::<_,String>(4)?, "status":"doing"})))?
        .collect::<Result<Vec<_>,_>>()?;
    let completed = connection.prepare("SELECT w.project,w.task_id,a.worker,a.id FROM chief_work_revisions w JOIN chief_assignments a
        ON a.project=w.project AND a.task_id=w.task_id AND a.work_revision=w.revision WHERE w.state='completed'")?
        .query_map([], |row|Ok(json!({"project":row.get::<_,String>(0)?,"task_id":row.get::<_,String>(1)?,"worker":row.get::<_,String>(2)?,"assignment":row.get::<_,String>(3)?,"state":"completed","status":"done"})))?.collect::<Result<Vec<_>,_>>()?;
    for claim in completed {
        work.retain(|existing|existing["task_id"] != claim["task_id"] || existing["project"] != claim["project"]);
        work.push(claim);
    }
    let mut statement = connection.prepare("SELECT DISTINCT project,task_id FROM chief_work_revisions WHERE NOT EXISTS
        (SELECT 1 FROM chief_assignments a WHERE a.project=chief_work_revisions.project AND a.task_id=chief_work_revisions.task_id AND a.state NOT IN ('completed','cancelled')) ORDER BY project,task_id")?;
    work.extend(statement.query_map([], |row| Ok(json!({"project":row.get::<_,String>(0)?, "task_id":row.get::<_,String>(1)?, "state":"chief_managed"})))?.collect::<Result<Vec<_>,_>>()?);
    Ok(work)
}

pub(crate) fn upgrade_worker_history(connection: &Connection) -> Result<(), TeamError> {
    connection.execute_batch("ALTER TABLE chief_worker_bindings RENAME TO chief_worker_bindings_v3;
        CREATE TABLE chief_worker_bindings (
            session_id TEXT NOT NULL, agent TEXT NOT NULL, project TEXT NOT NULL, runtime TEXT NOT NULL,
            epoch TEXT NOT NULL UNIQUE, role TEXT NOT NULL CHECK(role IN ('designer','developer','qa')),
            revision INTEGER NOT NULL CHECK(revision>0), state TEXT NOT NULL CHECK(state IN ('bound','revoked')),
            approval_hash TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY(session_id,epoch));
        INSERT INTO chief_worker_bindings SELECT * FROM chief_worker_bindings_v3;
        DROP TABLE chief_worker_bindings_v3;
        CREATE UNIQUE INDEX chief_one_bound_worker ON chief_worker_bindings(session_id) WHERE state='bound';
        CREATE TRIGGER chief_worker_approval_immutable BEFORE UPDATE OF session_id,agent,project,runtime,epoch,role,revision,approval_hash ON chief_worker_bindings
            BEGIN SELECT RAISE(ABORT,'chief worker identity and approval are immutable'); END;")?;
    Ok(())
}

pub(crate) fn create_schema(connection: &Connection) -> Result<(), TeamError> {
    connection.execute_batch("CREATE TABLE chief_grants (
            id TEXT PRIMARY KEY, session_id TEXT NOT NULL, agent TEXT NOT NULL, project TEXT NOT NULL,
            runtime TEXT NOT NULL, epoch TEXT NOT NULL UNIQUE, revision INTEGER NOT NULL CHECK(revision>0),
            state TEXT NOT NULL CHECK(state IN ('active','paused','disabled')), approval_hash TEXT NOT NULL, created_at INTEGER NOT NULL);
        CREATE UNIQUE INDEX chief_one_active_grant ON chief_grants((1)) WHERE state IN ('active','paused');
        CREATE TABLE chief_worker_bindings (
            session_id TEXT NOT NULL, agent TEXT NOT NULL, project TEXT NOT NULL, runtime TEXT NOT NULL,
            epoch TEXT NOT NULL UNIQUE, role TEXT NOT NULL CHECK(role IN ('designer','developer','qa')),
            revision INTEGER NOT NULL CHECK(revision>0), state TEXT NOT NULL CHECK(state IN ('bound','revoked')),
            approval_hash TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY(session_id,epoch));
        CREATE UNIQUE INDEX chief_one_bound_worker ON chief_worker_bindings(session_id) WHERE state='bound';
        CREATE TABLE chief_work_revisions (
            project TEXT NOT NULL, task_id TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>0),
            brief TEXT NOT NULL, approval_hash TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('approved','revoked','superseded','completed')),
            priority INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL,
            PRIMARY KEY(project,task_id,revision));
        CREATE TABLE chief_assignments (
            id TEXT PRIMARY KEY, project TEXT NOT NULL, task_id TEXT NOT NULL, work_revision INTEGER NOT NULL,
            worker TEXT NOT NULL, worker_runtime TEXT NOT NULL, worker_epoch TEXT NOT NULL,
            chief_grant TEXT NOT NULL, chief_epoch TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('queued','submitting','received','accepted','working','blocked','uncertain','completed','cancelled')),
            revision INTEGER NOT NULL CHECK(revision>0), submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
            created_at INTEGER NOT NULL, finished_at INTEGER,
            FOREIGN KEY(project,task_id,work_revision) REFERENCES chief_work_revisions(project,task_id,revision),
            UNIQUE(chief_epoch,submission_key));
        CREATE UNIQUE INDEX chief_one_open_work_claim ON chief_assignments(project,task_id) WHERE state NOT IN ('completed','cancelled');
        CREATE UNIQUE INDEX chief_one_open_worker_assignment ON chief_assignments(worker) WHERE state NOT IN ('completed','cancelled');
        CREATE TABLE chief_events (
            seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, source TEXT NOT NULL,
            submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL, work TEXT, assignment TEXT,
            kind TEXT NOT NULL, body TEXT NOT NULL, created_at INTEGER NOT NULL, UNIQUE(source,submission_key));
        CREATE TABLE chief_decisions (
            id TEXT PRIMARY KEY, scope_key TEXT NOT NULL, decision_key TEXT NOT NULL, work TEXT,
            assignment TEXT, question TEXT NOT NULL CHECK(length(question) BETWEEN 1 AND 1000),
            recommendation TEXT NOT NULL CHECK(length(recommendation) BETWEEN 1 AND 1000),
            state TEXT NOT NULL CHECK(state IN ('open','resolved','superseded')),
            revision INTEGER NOT NULL CHECK(revision>0), resolution TEXT, created_at INTEGER NOT NULL, resolved_at INTEGER);
        CREATE UNIQUE INDEX chief_one_open_decision ON chief_decisions(scope_key,decision_key) WHERE state='open';
        CREATE TABLE chief_evidence (
            id TEXT PRIMARY KEY, project TEXT NOT NULL, task_id TEXT NOT NULL, work_revision INTEGER NOT NULL,
            kind TEXT NOT NULL CHECK(kind IN ('tests','qa','apply')), commit_id TEXT NOT NULL,
            source TEXT NOT NULL CHECK(source IN ('host_run','qa','reviewed_apply','host_attestation')),
            source_id TEXT NOT NULL, result TEXT NOT NULL, created_at INTEGER NOT NULL,
            FOREIGN KEY(project,task_id,work_revision) REFERENCES chief_work_revisions(project,task_id,revision),
            UNIQUE(source,source_id,project,task_id,work_revision,kind));
        CREATE TABLE chief_delivery_attempts (
            id TEXT PRIMARY KEY, assignment TEXT REFERENCES chief_assignments(id), event_id TEXT REFERENCES chief_events(id),
            recipient TEXT NOT NULL, recipient_runtime TEXT NOT NULL, recipient_epoch TEXT NOT NULL,
            kind TEXT NOT NULL, envelope TEXT NOT NULL, idle_turn TEXT,
            state TEXT NOT NULL CHECK(state IN ('submitting','received','not_submitted','uncertain','cancelled')),
            native_id TEXT, receipt TEXT, started_at INTEGER NOT NULL, finished_at INTEGER);
        CREATE UNIQUE INDEX chief_one_inflight_recipient ON chief_delivery_attempts(recipient) WHERE state IN ('submitting','uncertain');
        CREATE TABLE chief_actions (request_id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, response TEXT NOT NULL, created_at INTEGER NOT NULL);
        CREATE TRIGGER chief_work_immutable BEFORE UPDATE OF project,task_id,revision,brief,approval_hash ON chief_work_revisions
            BEGIN SELECT RAISE(ABORT,'approved chief brief and hash are immutable'); END;
        CREATE TRIGGER chief_evidence_immutable BEFORE UPDATE ON chief_evidence
            BEGIN SELECT RAISE(ABORT,'chief evidence is immutable'); END;
        CREATE TRIGGER chief_grant_immutable BEFORE UPDATE OF session_id,agent,project,runtime,epoch,approval_hash ON chief_grants
            BEGIN SELECT RAISE(ABORT,'chief grant identity and approval are immutable'); END;
        CREATE TRIGGER chief_worker_approval_immutable BEFORE UPDATE OF session_id,agent,project,runtime,epoch,role,revision,approval_hash ON chief_worker_bindings
            BEGIN SELECT RAISE(ABORT,'chief worker approval hash is immutable'); END;")?;
    // Both inserts and reactivation count: closing an item must not allow reopening beyond the cap.
    for (table, state, maximum) in [
        ("chief_work_revisions", "approved", super::MAX_ACTIVE_WORK),
        ("chief_decisions", "open", super::MAX_OPEN_DECISIONS),
    ] {
        connection.execute_batch(&format!("CREATE TRIGGER {table}_capacity_insert BEFORE INSERT ON {table}
                WHEN NEW.state='{state}' AND (SELECT count(*) FROM {table} WHERE state='{state}') >= {maximum}
                BEGIN SELECT RAISE(ABORT,'chief active record limit reached'); END;
            CREATE TRIGGER {table}_capacity_update BEFORE UPDATE OF state ON {table}
                WHEN NEW.state='{state}' AND OLD.state<>'{state}' AND (SELECT count(*) FROM {table} WHERE state='{state}') >= {maximum}
                BEGIN SELECT RAISE(ABORT,'chief active record limit reached'); END;"))?;
    }
    Ok(())
}
