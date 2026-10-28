//! Protected chief grants, host decisions and native-inbox wake delivery.

use super::{ChiefAgentAction, ChiefHostAction, EscalationCategory, WorkerState};
use crate::teams::{ErrorCode, TeamError, TeamReply, VerifiedSession};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

fn conflict(message: &str) -> TeamError { TeamError::new(ErrorCode::Conflict, message) }

pub(crate) fn upgrade_to_16(connection: &Connection) -> Result<(), TeamError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= 16 { return Ok(()); }
    connection.execute_batch("CREATE TABLE IF NOT EXISTS chief_startup (
        singleton INTEGER PRIMARY KEY CHECK(singleton=1), id TEXT NOT NULL UNIQUE, session_id TEXT NOT NULL UNIQUE, directory TEXT NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('starting','checking','active','failed')),
        revision INTEGER NOT NULL, created_at INTEGER NOT NULL, error TEXT, identity TEXT);
        PRAGMA user_version=16;")?;
    Ok(())
}

/// Version 17 lets Theo put an escalated question off: `later_until` (epoch ms), NULL when not deferred.
pub(crate) fn upgrade_to_17(connection: &Connection) -> Result<(), TeamError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= 17 { return Ok(()); }
    if !has_column(connection, "chief_decisions", "later_until")? {
        connection.execute_batch("ALTER TABLE chief_decisions ADD COLUMN later_until INTEGER;")?;
    }
    connection.execute_batch("PRAGMA user_version=17;")?;
    Ok(())
}

/// Created on every open, like the sleep tables: the Chief's requests to start a fresh session for a
/// queued todo, and which reservation acted on each.
pub(crate) fn ensure_session_starts(connection: &Connection) -> Result<(), TeamError> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS chief_session_starts (
        id TEXT PRIMARY KEY, epoch TEXT NOT NULL, project TEXT NOT NULL, task_id TEXT NOT NULL, reason TEXT NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('requested','started','expired')), token TEXT,
        created_at INTEGER NOT NULL, started_at INTEGER);")?;
    if !has_column(connection, "chief_session_starts", "theo_queued")? {
        connection.execute_batch("ALTER TABLE chief_session_starts ADD COLUMN theo_queued INTEGER NOT NULL DEFAULT 0;")?;
    }
    // A request with a worker gives the todo to that live session instead of starting one.
    if !has_column(connection, "chief_session_starts", "worker")? {
        connection.execute_batch("ALTER TABLE chief_session_starts ADD COLUMN worker TEXT;")?;
    }
    if !has_column(connection, "chief_session_starts", "worker_runtime")? {
        connection.execute_batch("ALTER TABLE chief_session_starts ADD COLUMN worker_runtime TEXT;")?;
    }
    Ok(())
}

/// Protected host startup state; absence never means an in-flight launch failed.
pub fn startup(connection: &Connection) -> Result<Option<Value>, TeamError> {
    // An activated attempt whose grant was later disabled is over: Start may begin a new one.
    let encoded: Option<String> = connection.query_row("SELECT json_object('id',id,'session_id',session_id,'directory',directory,
        'state',CASE WHEN state='active' AND NOT EXISTS(SELECT 1 FROM chief_grants g WHERE g.session_id=chief_startup.session_id AND g.state IN ('active','paused')) THEN 'ended' ELSE state END,
        'revision',revision,'created_at_ms',created_at,'error',error,'session',json(identity),'can_retry',json(CASE state WHEN 'failed' THEN 'true' ELSE 'false' END),
        'check',json((SELECT json_object('id',i.id,'state',i.state,'session',json(i.recipient_identity)) FROM chief_inbox i
            WHERE json_extract(i.envelope,'$.kind')='chief_check' AND i.chief_epoch=chief_startup.id ORDER BY i.rowid DESC LIMIT 1)))
        FROM chief_startup WHERE singleton=1", [], |row| row.get(0)).optional()?;
    encoded.map(|value|serde_json::from_str(&value).map_err(TeamError::from)).transpose()
}
fn unauthorized() -> TeamError { TeamError::new(ErrorCode::Unauthorized, "no active chief grant for this verified runtime") }

/// The unresolved startup attempt and its exact live reserved session, while that session still lacks
/// its own admission verification: the only target of Theo's delivery check.
fn check_target(connection: &Connection, live: &[WorkerState]) -> Result<Option<(String, VerifiedSession)>, TeamError> {
    let Some(startup) = startup(connection)? else { return Ok(None) };
    if startup["state"] != "checking" || chief_session(connection)?.is_some() { return Ok(None); }
    let (Some(attempt), Some(session_id), Some(directory)) = (startup["id"].as_str(), startup["session_id"].as_str(), startup["directory"].as_str()) else { return Ok(None) };
    let Some(worker) = live.iter().find(|worker| worker.session.session_id == session_id && worker.session.agent == "opencode"
        && worker.session.project == format!("path:{directory}")) else { return Ok(None) };
    if is_product_worker(connection, session_id)? || admission_verified(connection, &worker.session)? { return Ok(None); }
    Ok(Some((attempt.to_string(), worker.session.clone())))
}

fn exact_live(session: &VerifiedSession, live: &[WorkerState]) -> bool {
    live.iter().any(|worker| worker.session == *session)
}

pub fn is_product_worker(connection: &Connection, session: &str) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM members WHERE session_id=?1) OR EXISTS(SELECT 1 FROM links WHERE state='active' AND (session_low=?1 OR session_high=?1))",
        [session], |row| row.get(0))?)
}

pub fn is_chief(connection: &Connection, session_id: &str) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE session_id=?1 AND state IN ('active','paused'))", [session_id], |row| row.get(0))?)
}

/// The session holding the one active or paused grant, if any.
pub fn chief_session(connection: &Connection) -> Result<Option<String>, TeamError> {
    Ok(connection.query_row("SELECT session_id FROM chief_grants WHERE state IN ('active','paused')", [], |row| row.get(0)).optional()?)
}

/// What the column's 5 Chief tab badge needs on every tab, without the full Chief snapshot: the
/// grant's state (active, paused, or null without one) and the open decisions not put off.
pub fn badge(connection: &Connection, now_ms: i64) -> Result<Value, TeamError> {
    let state: Option<String> = connection.query_row("SELECT state FROM chief_grants WHERE state IN ('active','paused')", [], |row| row.get(0)).optional()?;
    let needs_you: i64 = connection.query_row("SELECT count(*) FROM chief_decisions WHERE state='open' AND (later_until IS NULL OR later_until<=?1)", [now_ms], |row| row.get(0))?;
    Ok(json!({"state": state, "needs_you": needs_you}))
}

pub fn require_non_chief(connection: &Connection, session: &str) -> Result<(), TeamError> {
    if is_chief(connection, session)? {
        return Err(TeamError::new(ErrorCode::Unauthorized, "disable the chief grant before acquiring a product-worker role"));
    }
    Ok(())
}

pub fn host(connection: &mut Connection, action: &ChiefHostAction, live: &[WorkerState], now_ms: i64) -> Result<TeamReply, TeamError> {
    let request_id = match action {
        ChiefHostAction::Start { request_id, .. } | ChiefHostAction::SettleStartup { request_id, .. } | ChiefHostAction::SendCheck { request_id, .. }
        | ChiefHostAction::Enable { request_id, .. } | ChiefHostAction::Recover { request_id, .. } | ChiefHostAction::Pause { request_id, .. }
        | ChiefHostAction::Resume { request_id, .. } | ChiefHostAction::Disable { request_id, .. }
        | ChiefHostAction::Resolve { request_id, .. } | ChiefHostAction::AnswerDecision { request_id, .. } | ChiefHostAction::LaterDecision { request_id, .. } | ChiefHostAction::RequestSummary { request_id }
        | ChiefHostAction::ReconcileDelivery {request_id,..} | ChiefHostAction::ReconcileLaunch {request_id,..} | ChiefHostAction::Overrule { request_id, .. }
        | ChiefHostAction::DeleteHabit { request_id, .. } | ChiefHostAction::RestoreHabit { request_id, .. } | ChiefHostAction::RescopeHabit { request_id, .. } => request_id.clone(),
    };
    let result = (|| {
        action.validate()?;
        let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(action)?);
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let previous: Option<(String, String)> = transaction.query_row("SELECT fingerprint,response FROM chief_actions WHERE request_id=?1", [&request_id],
            |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        if let Some((old, response)) = previous {
            if old != fingerprint { return Err(conflict("chief host request id reused with different content")); }
            return Ok(serde_json::from_str(&response)?);
        }
        let response = host_action(&transaction, action, live, now_ms)?;
        if !matches!(action,ChiefHostAction::Start {..}|ChiefHostAction::SendCheck {..}|ChiefHostAction::Enable {..}|ChiefHostAction::Resolve {..}|ChiefHostAction::AnswerDecision {..}|ChiefHostAction::LaterDecision {..}|ChiefHostAction::Overrule {..}|ChiefHostAction::RequestSummary {..}
            |ChiefHostAction::DeleteHabit {..}|ChiefHostAction::RestoreHabit {..}|ChiefHostAction::RescopeHabit {..}) {
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
        ChiefHostAction::Start { previous_attempt, .. } => {
            if chief_session(connection)?.is_some() { return Err(conflict("a global chief is already enabled")); }
            let previous = startup(connection)?;
            if let Some(previous) = previous.as_ref() {
                if matches!(previous["state"].as_str(), Some("starting" | "checking")) {
                    return Ok(previous.clone());
                }
                let is_ended = previous["state"] == "ended" && previous_attempt.as_deref().is_none_or(|attempt|previous["id"].as_str() == Some(attempt));
                if !is_ended && (previous["can_retry"] != true || previous_attempt.as_deref() != previous["id"].as_str()) {
                    return Err(conflict("retry must name the current conclusively failed Chief startup"));
                }
            } else if previous_attempt.is_some() {
                return Err(conflict("Chief startup attempt does not exist"));
            }
            let id = crate::teams::store::identifier()?;
            let countdown = !(now_ms as u64).wrapping_mul(4096).wrapping_add(1) & 0xffffffffffff;
            let session_id = format!("ses_{countdown:012x}{}", &id[..14]);
            let directory = crate::paths::home().join("IdeaProjects/cockpit-chief");
            connection.execute("INSERT INTO chief_startup(singleton,id,session_id,directory,state,revision,created_at) VALUES (1,?1,?2,?4,'starting',1,?3)
                ON CONFLICT(singleton) DO UPDATE SET id=excluded.id,session_id=excluded.session_id,state='starting',revision=chief_startup.revision+1,
                directory=excluded.directory,created_at=excluded.created_at,error=NULL,identity=NULL", params![id,session_id,now_ms,directory.to_string_lossy()])?;
            Ok(startup(connection)?.expect("inserted Chief startup"))
        },
        ChiefHostAction::SettleStartup { attempt, expected_revision, evidence, .. } => {
            let super::ReconciliationEvidence::SafeStop { note, confirmed_stopped: true } = evidence else {
                return Err(conflict("settling a Chief startup needs a confirmed safe stop"));
            };
            if connection.execute("UPDATE chief_startup SET state='failed',revision=revision+1,error=?3 WHERE id=?1 AND revision=?2 AND state IN ('starting','checking')",
                params![attempt,expected_revision,format!("Theo confirmed no Chief session remains: {note}")])? != 1 {
                return Err(conflict("exact unresolved Chief startup revision is required"));
            }
            Ok(startup(connection)?.expect("settled Chief startup"))
        },
        ChiefHostAction::SendCheck { attempt, .. } => {
            let Some((attempt, session)) = check_target(connection, live)?.filter(|(current, _)| current == attempt) else {
                return Err(TeamError::new(ErrorCode::Unavailable, "this Chief startup has no live session waiting for a delivery check"));
            };
            let pending: Option<String> = connection.query_row("SELECT id FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_check'
                AND chief_epoch=?1 AND state IN ('queued','submitting')", [&attempt], |row| row.get(0)).optional()?;
            if let Some(pending) = pending { return Ok(json!({"check": pending})); }
            let id = crate::teams::store::identifier()?;
            let envelope = json!({"message_id": id, "kind": "chief_check", "attempt": attempt});
            connection.execute("INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at) VALUES (?1,?2,?3,?4,?2,'null',?5,'queued',?6)",
                params![id, attempt, session.session_id, serde_json::to_string(&session)?, envelope.to_string(), now_ms])?;
            Ok(json!({"check": id}))
        },
        ChiefHostAction::Enable { session, .. } => {
            if !exact_live(session, live) || is_product_worker(connection, &session.session_id)? {
                return Err(unauthorized());
            }
            if connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE state IN ('active','paused'))", [], |row| row.get::<_, bool>(0))? {
                return Err(conflict("a global chief is already enabled"));
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
        ChiefHostAction::Recover { request_id, expected_epoch, expected_revision, session } => {
            let previous: Option<(VerifiedSession, String)> = connection.query_row(
                "SELECT session_id,agent,project,runtime,state FROM chief_grants WHERE epoch=?1 AND revision=?2 AND state IN ('active','paused')",
                params![expected_epoch,expected_revision], |row| Ok((VerifiedSession {
                    session_id:row.get(0)?,agent:row.get(1)?,project:row.get(2)?,runtime:row.get(3)?,
                },row.get(4)?))).optional()?;
            let Some((previous, state)) = previous else { return Err(conflict("exact retained Chief grant revision is required")); };
            if previous.session_id!=session.session_id || previous.agent!=session.agent || previous.project!=session.project
                || previous.runtime==session.runtime || session.agent!="opencode" || is_product_worker(connection,&session.session_id)? {
                return Err(unauthorized());
            }
            let matching = live.iter().filter(|worker|worker.session.session_id==session.session_id).collect::<Vec<_>>();
            if !matches!(matching.as_slice(), [worker] if worker.session==*session && worker.observed_at_ms<=now_ms && now_ms.saturating_sub(worker.observed_at_ms)<=30_000) {
                return Err(unauthorized());
            }
            let verified: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_admission_verifications
                WHERE identity=?1 AND adapter='opencode-queued-v1' AND verified_at<=?2 AND verified_at>=?3)",
                params![serde_json::to_string(session)?,now_ms,now_ms.saturating_sub(300_000)], |row|row.get(0))?;
            if !verified { return Err(TeamError::new(ErrorCode::Unavailable,"Chief recovery requires this exact runtime's host native admission verification from the last five minutes")); }
            // Reuse revocation cleanup inside this same transaction; in-flight/uncertain deliveries remain untouched.
            host_action(connection,&ChiefHostAction::Disable {request_id:request_id.clone(),expected_epoch:expected_epoch.clone()},live,now_ms)?;
            // Retire future request authority, not its already-reserved launch or reconciliation token.
            let retired_requests=connection.execute("UPDATE chief_session_starts SET state='expired' WHERE epoch=?1 AND state='requested'",[expected_epoch])?;
            let epoch=crate::teams::store::identifier()?;
            let approval_hash=crate::audit::sha256_hex(&serde_json::to_vec(action)?);
            connection.execute("INSERT INTO chief_grants VALUES (?1,?2,?3,?4,?5,?1,1,?6,?7,?8)",
                params![epoch,session.session_id,session.agent,session.project,session.runtime,state,approval_hash,now_ms])?;
            connection.execute("INSERT INTO chief_coordination(epoch) VALUES (?1)",[&epoch])?;
            coordination_event(connection,"recovered",&json!({"request_id":request_id,"previous_epoch":expected_epoch,"previous_session":previous,"epoch":epoch,"session":session,"state":state,"retired_requests":retired_requests}),now_ms)?;
            coordination_event(connection,"bootstrap",&json!({"epoch":epoch}),now_ms)?;
            schedule_wake(connection,now_ms)?;
            Ok(json!({"previous_epoch":expected_epoch,"epoch":epoch,"session":session,"state":state}))
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
                let queued_answers = connection.prepare("SELECT id FROM chief_inbox WHERE chief_epoch=?1 AND state='queued' AND json_extract(envelope,'$.kind') IN ('chief_answer','chief_form_reply')
                    AND NOT EXISTS(SELECT 1 FROM chief_delivery_attempts a WHERE json_extract(a.envelope,'$.message_id')=chief_inbox.id AND a.state<>'not_submitted')")?
                    .query_map([expected_epoch], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                for message in queued_answers { cancel_answer(connection, &message)?; }
                connection.execute("UPDATE chief_inbox SET state='cancelled' WHERE chief_epoch=?1 AND state='queued'
                    AND NOT EXISTS(SELECT 1 FROM chief_delivery_attempts a WHERE json_extract(a.envelope,'$.message_id')=chief_inbox.id AND a.state<>'not_submitted')", [expected_epoch])?;
            }
            Ok(json!({"epoch": expected_epoch, "state": next}))
        },
        ChiefHostAction::ReconcileLaunch {token,expected_revision,evidence,..} => {
            let state=match evidence {super::ReconciliationEvidence::Handled {..}=>"submitted",super::ReconciliationEvidence::SafeStop {confirmed_stopped:true,..}=>"released",_=>return Err(conflict("explicit exact-launch safe stop is required"))};
            if connection.execute("UPDATE legacy_launches SET state=?3,revision=revision+1 WHERE token=?1 AND revision=?2 AND state IN ('reserved','uncertain','submitted')",params![token,expected_revision,state])?!=1 {return Err(conflict("exact unresolved launch revision is required"));}
            if state=="released" {connection.execute("DELETE FROM scheduling_reservations WHERE token=?1 AND owner='legacy'",[token])?;}
            connection.execute("UPDATE chief_decisions SET state='resolved',revision=revision+1,resolution=?2,resolved_at=?3 WHERE scope_key=?1 AND state='open'",params![format!("host-launch-{token}"),serde_json::to_string(evidence)?,now_ms])?;
            Ok(json!({"token":token,"state":state,"verified_delivery":false,"completion":false}))
        },
        ChiefHostAction::ReconcileDelivery {message,expected_revision,evidence,..} => {
            let record: Option<(String,i64)> = connection.query_row("SELECT recipient_identity,attempts FROM chief_inbox WHERE id=?1 AND state IN ('uncertain','failed')",[message],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
            let (identity,revision)=record.ok_or_else(||conflict("exact unresolved delivery is required"))?;
            if revision!=*expected_revision {return Err(conflict("delivery revision changed"));}
            let stopped=match evidence {super::ReconciliationEvidence::Handled {..}=>false,super::ReconciliationEvidence::SafeStop {confirmed_stopped:true,..}=>true,_=>return Err(conflict("explicit exact-target safe stop is required"))};
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
            let revision: i64 = connection.query_row("SELECT revision FROM chief_decisions WHERE id=?1",[decision],|row|row.get(0))?;
            coordination_event(connection,"decision_resolved",&json!({"decision":decision,"decision_revision":revision}),now_ms)?;
            Ok(json!({"decision":decision,"state":"resolved","grants_authority":false}))
        },
        ChiefHostAction::AnswerDecision { decision, expected_revision, answer, .. } => {
            let escalation: Option<Option<String>> = connection.query_row("SELECT stop_id FROM chief_decisions WHERE id=?1 AND revision=?2 AND state='open'",
                params![decision,expected_revision],|row|row.get(0)).optional()?;
            let stop = escalation.ok_or_else(|| conflict("decision revision is stale or already answered"))?
                .ok_or_else(|| conflict("this decision is not an escalated stop; resolve it with a note"))?;
            let queued = queue_answer(connection, live, AnswerAuthor::Theo, &stop, Reply::Text(answer), &format!("Theo answered escalation {decision}"), now_ms)?;
            connection.execute("UPDATE chief_decisions SET state='resolved',revision=revision+1,resolution=?3,resolved_at=?4 WHERE id=?1 AND revision=?2",
                params![decision,expected_revision,format!("Answered by Theo: {answer}"),now_ms])?;
            coordination_event(connection,"decision_resolved",&json!({"decision":decision,"decision_revision":expected_revision+1,"answer":queued["answer"]}),now_ms)?;
            Ok(json!({"decision":decision,"state":"resolved","answer":queued["answer"],"delivery":"queued","grants_authority":false}))
        },
        ChiefHostAction::LaterDecision { decision, expected_revision, .. } => {
            let until = now_ms.saturating_add(super::CHIEF_LATER_MS);
            if connection.execute("UPDATE chief_decisions SET later_until=?3 WHERE id=?1 AND revision=?2 AND state='open' AND stop_id IS NOT NULL",
                params![decision,expected_revision,until])? != 1 {
                return Err(conflict("decision revision is stale, already answered or not an escalated question"));
            }
            Ok(json!({"decision":decision,"state":"open","later_until_ms":until}))
        },
        ChiefHostAction::Overrule { answer, correction, .. } => queue_correction(connection, live, answer, correction, now_ms),
        ChiefHostAction::DeleteHabit { habit, .. } => change_habit(connection, habit, HabitChange::State("deleted"), now_ms),
        ChiefHostAction::RestoreHabit { habit, .. } => change_habit(connection, habit, HabitChange::State("active"), now_ms),
        ChiefHostAction::RescopeHabit { habit, everywhere, .. } => change_habit(connection, habit, HabitChange::Scope { everywhere: *everywhere }, now_ms),
        ChiefHostAction::RequestSummary { request_id } => {
            let pending: i64 = connection.query_row("SELECT count(*) FROM chief_summary_requests WHERE state IN ('queued','pending')",[],|row|row.get(0))?;
            if pending >= 1 { return Err(TeamError::new(ErrorCode::Limit,"a bounded summary request is already pending")); }
            let epoch: Option<String> = connection.query_row("SELECT epoch FROM chief_grants WHERE state IN ('active','paused')",[],|row|row.get(0)).optional()?;
            connection.execute("INSERT INTO chief_summary_requests(id,epoch,state,created_at,deadline) VALUES (?1,?2,'queued',?3,?4)",params![request_id,epoch,now_ms,now_ms.saturating_add(900_000)])?;
            let state = if epoch.is_none() {
                connection.execute("UPDATE chief_summary_requests SET state='failed' WHERE id=?1",[request_id])?;
                decision_record(connection,&format!("host-summary-{request_id}"),"unavailable","Requested summary cannot be delivered: no selected chief is available.",
                    "Host must select a verified supported chief before requesting another summary. Previous summary results remain unchanged.",now_ms)?;
                "failed"
            } else {"queued"};
            coordination_event(connection,"summary_requested",&json!({"request_id":request_id}),now_ms)?;
            Ok(json!({"request_id":request_id,"state":state}))
        },
    }
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

/// Recover only protected identities, never a runtime supplied by the agent or writable snapshot.
pub fn runtime(connection: &Connection, session: &str) -> Result<Option<String>, TeamError> {
    let mut statement = connection.prepare("SELECT runtime FROM chief_grants WHERE session_id=?1 AND state!='disabled'")?;
    let runtimes = statement.query_map([session], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(if runtimes.len() == 1 { runtimes.into_iter().next() } else { None })
}

/// The epoch of the active grant held by exactly this verified session; paused or disabled grants have none.
pub fn active_epoch(connection: &Connection, session: &VerifiedSession) -> Result<Option<String>, TeamError> {
    Ok(connection.query_row("SELECT epoch FROM chief_grants WHERE session_id=?1 AND agent=?2 AND project=?3 AND runtime=?4 AND state='active'",
        params![session.session_id, session.agent, session.project, session.runtime], |row| row.get(0)).optional()?)
}

pub fn worker_role(connection: &Connection, session: &VerifiedSession) -> Result<Option<crate::teams::Role>, TeamError> {
    let member: Option<String> = connection.query_row("SELECT m.role FROM members m JOIN teams t ON t.task_key=m.task_key WHERE m.session_id=?1 AND m.runtime=?2 AND m.agent=?3 AND t.project=?4",
        params![session.session_id,session.runtime,session.agent,session.project], |row| row.get(0)).optional()?;
    member.map(|role| Ok(serde_json::from_value(json!(role))?)).transpose()
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

fn record_status(connection: &Connection, source: &str, summary: &str, now_ms: i64) -> Result<(), TeamError> {
    let body = json!({"summary": summary, "provenance": "agent_report_not_evidence"});
    let fingerprint = crate::audit::sha256_hex(&serde_json::to_vec(&body)?);
    let latest: Option<String> = connection.query_row("SELECT fingerprint FROM chief_events WHERE source=?1 AND kind='status' ORDER BY seq DESC LIMIT 1",[source],|row|row.get(0)).optional()?;
    if latest.as_deref()==Some(&fingerprint) {return Ok(());}
    connection.execute("INSERT INTO chief_events (id,source,submission_key,fingerprint,kind,body,created_at) VALUES (?1,?2,?1,?3,'status',?4,?5)",
        params![crate::teams::store::identifier()?, source, fingerprint, body.to_string(), now_ms])?;
    Ok(())
}

/// Only daemon-owned observations belong here. A sandbox request cannot submit WorkerState.
pub fn observe(connection: &mut Connection, live: &[WorkerState], now_ms: i64) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute("DELETE FROM chief_observations", [])?;
    for worker in live {
        if worker.observed_at_ms > now_ms { continue; }
        transaction.execute("INSERT INTO chief_observations VALUES (?1,?2)", params![worker.session.session_id, serde_json::to_string(worker)?])?;
    }
    expire_todo_gifts(&transaction,&live.iter().map(|worker|worker.session.clone()).collect::<Vec<_>>(),now_ms)?;
    transaction.execute("UPDATE chief_inbox SET state='uncertain' WHERE id IN (SELECT json_extract(envelope,'$.message_id') FROM chief_delivery_attempts WHERE state='submitting' AND started_at<=?1)", [now_ms.saturating_sub(crate::teams::store::RECEIPT_TIMEOUT_MS)])?;
    transaction.execute("UPDATE chief_delivery_attempts SET state='uncertain',finished_at=?2 WHERE state='submitting' AND started_at<=?1", params![now_ms.saturating_sub(crate::teams::store::RECEIPT_TIMEOUT_MS),now_ms])?;
    log_uncertain_answers(&transaction)?;
    observe_coordination(&transaction,live,now_ms)?;
    transaction.commit()?;
    Ok(())
}
pub fn recover(connection: &mut Connection, now_ms: i64) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute("UPDATE legacy_launches SET state='uncertain',revision=revision+1 WHERE state IN ('reserved','submitting')",[])?;
    transaction.execute("DELETE FROM chief_observations", [])?;
    transaction.execute("UPDATE chief_inbox SET state='uncertain' WHERE state='submitting'", [])?;
    transaction.execute("UPDATE chief_coordination SET failed=1 WHERE epoch IN (SELECT chief_epoch FROM chief_inbox WHERE state IN ('uncertain','failed') AND json_extract(envelope,'$.kind')='chief_wake')", [])?;
    transaction.execute("UPDATE chief_delivery_attempts SET state='uncertain',finished_at=?1 WHERE state='submitting'", [now_ms])?;
    log_uncertain_answers(&transaction)?;
    transaction.commit()?;
    Ok(())
}

/// A Chief answer or Theo's correction whose delivery the inbox no longer knows the outcome of is logged as uncertain.
fn log_uncertain_answers(connection: &Connection) -> Result<(), TeamError> {
    connection.execute("UPDATE chief_answers SET delivery='uncertain' WHERE delivery='queued' AND id IN (SELECT json_extract(envelope,'$.answer')
        FROM chief_inbox WHERE state='uncertain' AND json_extract(envelope,'$.kind') IN ('chief_answer','chief_form_reply'))", [])?;
    connection.execute("UPDATE chief_overrules SET delivery='uncertain' WHERE delivery='queued' AND id IN (SELECT json_extract(envelope,'$.overrule')
        FROM chief_inbox WHERE state='uncertain' AND json_extract(envelope,'$.kind')='chief_correction')", [])?;
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
    create_review_triggers(connection)?;
    let historical_grants = connection.prepare("SELECT epoch FROM chief_grants WHERE state IN ('active','paused') AND NOT EXISTS(SELECT 1 FROM chief_events WHERE kind='bootstrap' AND json_extract(body,'$.epoch')=chief_grants.epoch)")?
        .query_map([],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for epoch in historical_grants {coordination_event(connection,"bootstrap",&json!({"epoch":epoch}),0)?;}
    schedule_wake(connection,0)?;
    upgrade_launches(connection)?;
    Ok(())
}

/// Tables whose changes bump the host review generation. Tables dropped in version 13 must stay out:
/// a fresh database never has them and a trigger on a missing table fails the whole migration.
fn create_review_triggers(connection: &Connection) -> Result<(), TeamError> {
    for table in ["chief_grants","chief_decisions","chief_inbox","chief_delivery_attempts","chief_events","chief_summary_requests","members"] {
        for operation in ["INSERT","UPDATE","DELETE"] {
            connection.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS review_{table}_{operation} AFTER {operation} ON {table}
                BEGIN UPDATE chief_review_generation SET revision=revision+1 WHERE id=1; END;"))?;
        }
    }
    Ok(())
}

fn has_column(connection: &Connection, table: &str, column: &str) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name=?2)", [table, column], |row| row.get(0))?)
}

/// Version 13 retires the dispatch protocol's tables and adds the stop/answer records.
///
/// Runs inside the migration transaction with foreign keys on, so no foreign key may still point at a
/// table when it is dropped: the two child tables are rebuilt without their `assignment` column first.
/// Every step is idempotent because a database can carry the version 13 shape under an older number.
/// `chief_decisions.work` and `assignment` stay as unused columns: dropping them needs a rebuild that
/// buys nothing, and `clear_dispatch_leftovers` still reads them to retire old open decisions.
pub(crate) fn upgrade_to_13(connection: &Connection) -> Result<(), TeamError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= 13 { return Ok(()); }
    if has_column(connection, "chief_inbox", "assignment")? {
        connection.execute_batch("CREATE TABLE chief_inbox_v13 (
                id TEXT PRIMARY KEY, chief_epoch TEXT NOT NULL,
                recipient TEXT NOT NULL, recipient_identity TEXT NOT NULL, recipient_epoch TEXT NOT NULL,
                work TEXT NOT NULL, envelope TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('queued','submitting','received','uncertain','cancelled','failed')),
                next_at INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0);
            INSERT INTO chief_inbox_v13 SELECT id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at,attempts FROM chief_inbox;
            DROP TABLE chief_inbox;
            ALTER TABLE chief_inbox_v13 RENAME TO chief_inbox;")?;
    }
    if has_column(connection, "chief_delivery_attempts", "assignment")? {
        connection.execute_batch("CREATE TABLE chief_delivery_attempts_v13 (
                id TEXT PRIMARY KEY, event_id TEXT REFERENCES chief_events(id),
                recipient TEXT NOT NULL, recipient_runtime TEXT NOT NULL, recipient_epoch TEXT NOT NULL,
                kind TEXT NOT NULL, envelope TEXT NOT NULL, idle_turn TEXT,
                state TEXT NOT NULL CHECK(state IN ('submitting','received','not_submitted','uncertain','cancelled')),
                native_id TEXT, receipt TEXT, started_at INTEGER NOT NULL, finished_at INTEGER);
            INSERT INTO chief_delivery_attempts_v13 SELECT id,event_id,recipient,recipient_runtime,recipient_epoch,kind,envelope,idle_turn,state,native_id,receipt,started_at,finished_at FROM chief_delivery_attempts;
            DROP TABLE chief_delivery_attempts;
            ALTER TABLE chief_delivery_attempts_v13 RENAME TO chief_delivery_attempts;")?;
    }
    connection.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS chief_inbox_inflight ON chief_inbox(recipient) WHERE state IN ('submitting','uncertain');
        CREATE UNIQUE INDEX IF NOT EXISTS chief_one_queued_wake ON chief_inbox(chief_epoch)
            WHERE json_extract(envelope,'$.kind')='chief_wake' AND state IN ('queued','submitting','uncertain');
        CREATE UNIQUE INDEX IF NOT EXISTS chief_one_inflight_recipient ON chief_delivery_attempts(recipient) WHERE state IN ('submitting','uncertain');")?;
    create_review_triggers(connection)?;
    for table in ["chief_evidence","chief_assignments","chief_work_revisions","chief_work_proposals","chief_sources",
        "chief_handoffs","chief_apply_reviews","chief_qa_results","chief_submissions","chief_worker_bindings"] {
        connection.execute_batch(&format!("DROP TABLE IF EXISTS {table};"))?;
    }
    connection.execute_batch("CREATE TABLE IF NOT EXISTS chief_stops (
            id TEXT PRIMARY KEY, session_id TEXT NOT NULL, runtime TEXT NOT NULL, boundary TEXT NOT NULL,
            project TEXT NOT NULL, task_id TEXT NOT NULL, bundle TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('open','answered','escalated','superseded')),
            created_at INTEGER NOT NULL, UNIQUE(session_id, boundary));
        CREATE TABLE IF NOT EXISTS chief_answers (
            id TEXT PRIMARY KEY, stop_id TEXT NOT NULL REFERENCES chief_stops(id), session_id TEXT NOT NULL,
            question TEXT NOT NULL, answer TEXT NOT NULL, reason TEXT NOT NULL,
            source TEXT NOT NULL CHECK(source IN ('chief','theo')),
            delivery TEXT NOT NULL CHECK(delivery IN ('queued','received','uncertain','cancelled')),
            created_at INTEGER NOT NULL);")?;
    for (column, definition) in [("session_id", "TEXT"), ("stop_id", "TEXT"),
        ("category", "TEXT CHECK(category IN ('security','scope','not_sure'))"), ("suggested_answer", "TEXT")] {
        if !has_column(connection, "chief_decisions", column)? {
            connection.execute_batch(&format!("ALTER TABLE chief_decisions ADD COLUMN {column} {definition};"))?;
        }
    }
    connection.execute_batch("PRAGMA user_version=13;")?;
    Ok(())
}

/// Version 14 adds the log of Theo's corrections to Chief answers and the indexes behind the per-session counts and the newest-answers review.
pub(crate) fn upgrade_to_14(connection: &Connection) -> Result<(), TeamError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= 14 { return Ok(()); }
    connection.execute_batch("CREATE TABLE IF NOT EXISTS chief_overrules (
            id TEXT PRIMARY KEY, answer_id TEXT NOT NULL UNIQUE REFERENCES chief_answers(id),
            session_id TEXT NOT NULL, runtime TEXT NOT NULL, correction TEXT NOT NULL,
            delivery TEXT NOT NULL CHECK(delivery IN ('queued','received','uncertain','cancelled')),
            created_at INTEGER NOT NULL, finished_at INTEGER);
        CREATE INDEX IF NOT EXISTS chief_answers_by_session ON chief_answers(session_id, created_at);
        CREATE INDEX IF NOT EXISTS chief_answers_by_created_at ON chief_answers(created_at);
        PRAGMA user_version=14;")?;
    Ok(())
}

/// Version 15 adds Theo's habits and the list of habits each answer used.
/// A habit's `project` is a canonical project key, or NULL for everywhere; `origin_project` keeps the first one so a rescope can go back.
/// `source` is what Theo did (an overrule or his own answer); `source_kind` says where the host saw it: an overrule, his answer to an
/// escalation, or his message in a stopped session's bundle. `reason` is the Chief's, NULL for overrule habits.
pub(crate) fn upgrade_to_15(connection: &Connection) -> Result<(), TeamError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= 15 { return Ok(()); }
    connection.execute_batch("CREATE TABLE IF NOT EXISTS chief_habits (
            id TEXT PRIMARY KEY, text TEXT NOT NULL, project TEXT, origin_project TEXT,
            source TEXT NOT NULL CHECK(source IN ('overrule','theo_answer')),
            source_kind TEXT NOT NULL CHECK(source_kind IN ('overrule','theo_answer','theo_message')), source_id TEXT NOT NULL, reason TEXT,
            state TEXT NOT NULL CHECK(state IN ('active','deleted')),
            created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS chief_habits_by_state ON chief_habits(state, created_at);")?;
    if !has_column(connection, "chief_answers", "habits")? {
        connection.execute_batch("ALTER TABLE chief_answers ADD COLUMN habits TEXT NOT NULL DEFAULT '[]';")?;
    }
    connection.execute_batch("PRAGMA user_version=15;")?;
    Ok(())
}

/// How many answers the Chief gave a session in its current task, with the newest few for the session card.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AnswerCount {
    pub session_id: String,
    pub count: u32,
    pub recent: Vec<Value>,
}

/// Counts `source='chief'` answers that were not cancelled and whose stop belongs to the task of the
/// session's latest stop. Theo's own answers and answers of earlier tasks never count.
pub fn answer_counts(connection: &Connection) -> Result<Vec<AnswerCount>, TeamError> {
    let mut statement = connection.prepare("WITH latest_task AS (
            SELECT session_id, task_id FROM (
                SELECT session_id, task_id, row_number() OVER (PARTITION BY session_id ORDER BY created_at DESC, rowid DESC) AS recency FROM chief_stops)
            WHERE recency=1),
        counted AS (
            SELECT a.session_id AS session_id, a.created_at AS created_at, a.rowid AS answer_rowid, a.question AS question, a.answer AS answer, a.delivery AS delivery
            FROM chief_answers a JOIN chief_stops s ON s.id=a.stop_id JOIN latest_task l ON l.session_id=a.session_id AND l.task_id=s.task_id
            WHERE a.source='chief' AND a.delivery!='cancelled'),
        ranked AS (
            SELECT *, count(*) OVER (PARTITION BY session_id) AS total,
                row_number() OVER (PARTITION BY session_id ORDER BY created_at DESC, answer_rowid DESC) AS recency
            FROM counted)
        SELECT session_id,total,created_at,question,answer,delivery FROM ranked WHERE recency<=3 ORDER BY session_id,recency")?;
    let mut counts: Vec<AnswerCount> = Vec::new();
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?, json!({
        "created_at_ms": row.get::<_, i64>(2)?, "question": row.get::<_, String>(3)?,
        "answer": row.get::<_, String>(4)?, "delivery": row.get::<_, String>(5)?}))))?;
    for row in rows {
        let (session_id, count, recent) = row?;
        match counts.last_mut() {
            Some(last) if last.session_id == session_id => last.recent.push(recent),
            _ => counts.push(AnswerCount { session_id, count, recent: vec![recent] }),
        }
    }
    Ok(counts)
}

/// What the Chief did since a time, for the daily digest: counts, and the newest few answers.
#[derive(Debug, Clone, PartialEq)]
pub struct ChiefDigest {
    pub answered: u32,
    pub escalated: u32,
    pub overruled: u32,
    pub lines: Vec<DigestAnswer>,
    /// Answers counted in `answered` that `lines` leaves out.
    pub more: u32,
    /// The newest few habits Theo has had since, of `new_habits` in all.
    pub habits: Vec<DigestHabit>,
    pub new_habits: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DigestHabit {
    pub text: String,
    /// The canonical project, or `None` for everywhere.
    pub project: Option<String>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DigestAnswer {
    pub session_id: String,
    pub question: String,
    pub answer: String,
}

pub const DIGEST_ANSWER_LINES: usize = 5;
pub const DIGEST_HABIT_LINES: usize = 3;

/// Answers the Chief gave after `since_ms` and did not cancel, the escalations it raised and the
/// corrections Theo made in the same time, less those cancelled before they reached the agent. Theo's own answers are not the Chief's.
/// Habits count when created since and still active.
pub fn answers_since(connection: &Connection, since_ms: i64) -> Result<ChiefDigest, TeamError> {
    let count = |sql: &str| -> Result<u32, TeamError> { Ok(connection.query_row(sql, [since_ms], |row| row.get(0))?) };
    let answered = count("SELECT count(*) FROM chief_answers WHERE source='chief' AND delivery!='cancelled' AND created_at>?1")?;
    let escalated = count("SELECT count(*) FROM chief_decisions WHERE category IS NOT NULL AND created_at>?1")?;
    let overruled = count("SELECT count(*) FROM chief_overrules WHERE delivery!='cancelled' AND created_at>?1")?;
    let lines = connection.prepare("SELECT session_id,question,answer FROM chief_answers WHERE source='chief' AND delivery!='cancelled' AND created_at>?1
            ORDER BY created_at DESC,rowid DESC LIMIT ?2")?
        .query_map(params![since_ms, DIGEST_ANSWER_LINES as i64], |row| Ok(DigestAnswer { session_id: row.get(0)?, question: row.get(1)?, answer: row.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    let more = answered.saturating_sub(lines.len() as u32);
    let new_habits = count("SELECT count(*) FROM chief_habits WHERE state='active' AND created_at>?1")?;
    let habits = connection.prepare("SELECT text,project,created_at FROM chief_habits WHERE state='active' AND created_at>?1 ORDER BY created_at DESC,rowid DESC LIMIT ?2")?
        .query_map(params![since_ms, DIGEST_HABIT_LINES as i64], |row| Ok(DigestHabit { text: row.get(0)?, project: row.get(1)?, created_at_ms: row.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ChiefDigest { answered, escalated, overruled, lines, more, habits, new_habits })
}

/// The team database for reading only, with a short wait on a lock: it is never created or migrated here.
pub fn open_read_only(path: &std::path::Path) -> Result<Connection, TeamError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {},
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(TeamError::new(ErrorCode::NotFound, "no team database yet")),
        Err(error) => return Err(error.into()),
    }
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    connection.busy_timeout(std::time::Duration::from_millis(250))?;
    Ok(connection)
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
    connection.execute("INSERT INTO chief_events(id,source,submission_key,fingerprint,kind,body,created_at) VALUES (?1,'host',?1,?2,?3,?4,?5)",
        params![id,crate::audit::sha256_hex(&serde_json::to_vec(body)?),kind,body.to_string(),now_ms])?;
    Ok(())
}

/// Whether this exact stop boundary of the session was ever recorded, in any state.
pub fn stop_recorded(connection: &Connection, session_id: &str, boundary: &str) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_stops WHERE session_id=?1 AND boundary=?2)", [session_id, boundary], |row| row.get(0))?)
}

/// Sessions that still have an open or escalated stop, or an answered one whose answer has not gone
/// out yet, whether or not they are live any more.
pub fn sessions_with_open_stops(connection: &Connection) -> Result<Vec<String>, TeamError> {
    let mut statement = connection.prepare("SELECT DISTINCT session_id FROM chief_stops WHERE state IN ('open','escalated')
        OR (state='answered' AND id IN (SELECT json_extract(envelope,'$.stop') FROM chief_inbox WHERE state='queued' AND json_extract(envelope,'$.kind') IN ('chief_answer','chief_form_reply')))
        ORDER BY session_id")?;
    Ok(statement.query_map([], |row| row.get(0))?.collect::<Result<Vec<_>, _>>()?)
}

/// Closes every open or escalated stop of the session except the one at `current_boundary`; `None`
/// closes them all (the agent resumed, or is no longer stopped on an open task). An escalated stop
/// takes its open decision with it: nothing is left to answer.
/// An answered stop whose chat answer still waits in the inbox stays answered: a session that only
/// resumed briefly gets that answer at its next stop on the same task (`record_stop`). Only a session
/// that is gone loses it (`close_stops_of_gone_session`). A queued form reply is cancelled: its form is gone.
pub fn supersede_stops(connection: &Connection, session_id: &str, current_boundary: Option<&str>) -> Result<(), TeamError> {
    for message in queued_replies_of(connection, session_id, current_boundary)? {
        let kind: Option<String> = connection.query_row("SELECT json_extract(envelope,'$.kind') FROM chief_inbox WHERE id=?1", [&message], |row| row.get(0))?;
        if kind.as_deref() == Some("chief_form_reply") { cancel_answer(connection, &message)?; }
    }
    connection.execute("UPDATE chief_decisions SET state='superseded',revision=revision+1 WHERE state='open' AND stop_id IN
        (SELECT id FROM chief_stops WHERE session_id=?1 AND state='escalated' AND (?2 IS NULL OR boundary<>?2))", params![session_id, current_boundary])?;
    connection.execute("UPDATE chief_stops SET state='superseded' WHERE session_id=?1 AND state IN ('open','escalated') AND (?2 IS NULL OR boundary<>?2)",
        params![session_id, current_boundary])?;
    Ok(())
}

/// The session is no longer live: its stops close and every answer still queued for it is cancelled.
pub fn close_stops_of_gone_session(connection: &Connection, session_id: &str) -> Result<(), TeamError> {
    for message in queued_replies_of(connection, session_id, None)? { cancel_answer(connection, &message)?; }
    supersede_stops(connection, session_id, None)
}

/// The inbox rows of answers and form replies still queued for the session's answered stops, oldest
/// first; `except_boundary` leaves out the stop at that boundary.
fn queued_replies_of(connection: &Connection, session_id: &str, except_boundary: Option<&str>) -> Result<Vec<String>, TeamError> {
    Ok(connection.prepare("SELECT i.id FROM chief_inbox i JOIN chief_stops s ON s.id=json_extract(i.envelope,'$.stop')
        WHERE i.state='queued' AND json_extract(i.envelope,'$.kind') IN ('chief_answer','chief_form_reply') AND s.session_id=?1 AND s.state='answered'
        AND (?2 IS NULL OR s.boundary<>?2) ORDER BY i.rowid")?
        .query_map(params![session_id, except_boundary], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?)
}

/// Moves a chat answer still queued for an earlier stop of this runtime and task to its new stop, which
/// it then answers; the envelope names the question it was written for. Anything else (a form reply,
/// whose form is gone, or an answer for another runtime or task) is cancelled. Returns whether an
/// answer was carried.
fn carry_queued_answers(connection: &Connection, stop: &str, session: &VerifiedSession, boundary: &str, task_id: &str) -> Result<bool, TeamError> {
    let mut is_carried = false;
    for message in queued_replies_of(connection, &session.session_id, Some(boundary))? {
        let envelope: String = connection.query_row("SELECT envelope FROM chief_inbox WHERE id=?1", [&message], |row| row.get(0))?;
        let mut envelope: Value = serde_json::from_str(&envelope)?;
        let earlier: (String, String, String) = connection.query_row("SELECT s.runtime,s.task_id,a.question FROM chief_answers a JOIN chief_stops s ON s.id=a.stop_id WHERE a.id=?1",
            [envelope["answer"].as_str()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        let (runtime, earlier_task, question) = earlier;
        let is_carriable = envelope["kind"] == "chief_answer" && runtime == session.runtime && !task_id.is_empty() && earlier_task == task_id;
        if !is_carriable { cancel_answer(connection, &message)?; continue; }
        let earlier_stop = envelope["stop"].as_str().unwrap_or_default().to_string();
        envelope["stop"] = json!(stop);
        if envelope.get("earlier_question").is_none() { envelope["earlier_question"] = json!(question); }
        connection.execute("UPDATE chief_inbox SET envelope=?2 WHERE id=?1", params![message, envelope.to_string()])?;
        connection.execute("UPDATE chief_stops SET state='superseded' WHERE id=?1 AND state='answered'", [earlier_stop])?;
        is_carried = true;
    }
    if is_carried { connection.execute("UPDATE chief_stops SET state='answered' WHERE id=?1", [stop])?; }
    Ok(is_carried)
}

/// Withdraws a queued answer that can no longer reach the stop it answers, and that stop with it.
fn cancel_answer(connection: &Connection, message_id: &str) -> Result<(), TeamError> {
    let envelope: String = connection.query_row("SELECT envelope FROM chief_inbox WHERE id=?1", [message_id], |row| row.get(0))?;
    let envelope: Value = serde_json::from_str(&envelope)?;
    connection.execute("UPDATE chief_inbox SET state='cancelled' WHERE id=?1 AND state='queued'", [message_id])?;
    connection.execute("UPDATE chief_answers SET delivery='cancelled' WHERE id=?1 AND delivery='queued'", [envelope["answer"].as_str()])?;
    connection.execute("UPDATE chief_stops SET state='superseded' WHERE id=?1 AND state='answered'", [envelope["stop"].as_str()])?;
    Ok(())
}

/// Withdraws queued corrections whose session is no longer live, or is live under another runtime:
/// neither will ever get them. Each overrule is cancelled together with its inbox row.
pub fn cancel_stranded_corrections(connection: &mut Connection, live: &[WorkerState], now_ms: i64) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let queued = transaction.prepare("SELECT i.id,i.envelope,o.session_id,o.runtime FROM chief_inbox i JOIN chief_overrules o ON o.id=json_extract(i.envelope,'$.overrule')
        WHERE i.state='queued' AND json_extract(i.envelope,'$.kind')='chief_correction' AND o.delivery='queued'")?
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (message_id, envelope, session_id, runtime) in queued {
        let is_reachable = live.iter().any(|worker| worker.session.session_id == session_id && worker.session.runtime == runtime);
        if !is_reachable { cancel_correction(&transaction, &message_id, &serde_json::from_str(&envelope)?, now_ms)?; }
    }
    transaction.commit()?;
    Ok(())
}

/// Withdraws a queued correction whose session restarted or is gone: the new runtime never got the answer it corrects.
fn cancel_correction(connection: &Connection, message_id: &str, envelope: &Value, now_ms: i64) -> Result<(), TeamError> {
    connection.execute("UPDATE chief_inbox SET state='cancelled' WHERE id=?1 AND state='queued'", [message_id])?;
    connection.execute("UPDATE chief_overrules SET delivery='cancelled',finished_at=?2 WHERE id=?1 AND delivery='queued'", params![envelope["overrule"].as_str(), now_ms])?;
    Ok(())
}

/// Who wrote an answer. The Chief's answers need the Chief's grant and the 30 s grace, and name the
/// habits they relied on; Theo's, given on purpose from the Chief view, need neither (every readiness
/// check still applies).
enum AnswerAuthor<'a> { Chief { epoch: &'a str, habits: &'a [String] }, Theo }

/// What a Theo answer carries where a Chief answer carries its grant epoch. No grant has this epoch,
/// so disabling the Chief never cancels Theo's answer.
const THEO_ANSWER_EPOCH: &str = "theo";

/// What an answer gives the stopped agent: a message, or the option to pick in its open form.
enum Reply<'a> { Text(&'a str), FormChoice(&'a str) }

/// Queues a reply to a stop for the stopped session's current runtime and logs it. The Chief answers
/// an open stop; Theo answers an escalated one. A restarted session is a different runtime: the
/// answer would reach an agent that never asked.
fn queue_answer(connection: &Connection, live: &[WorkerState], author: AnswerAuthor, stop: &str, reply: Reply, reason: &str, now_ms: i64) -> Result<Value, TeamError> {
    let (epoch, source, required_state, habits) = match author {
        AnswerAuthor::Chief { epoch, habits } => (epoch, "chief", "open", habits),
        AnswerAuthor::Theo => (THEO_ANSWER_EPOCH, "theo", "escalated", &[][..]),
    };
    require_citable_habits(connection, stop, habits)?;
    let record: Option<(String, String, String, String)> = connection.query_row("SELECT session_id,runtime,state,bundle FROM chief_stops WHERE id=?1", [stop],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
    let (session_id, runtime, state, bundle) = record.ok_or_else(|| TeamError::new(ErrorCode::NotFound, "stop not found"))?;
    if state != required_state { return Err(conflict("the stop is no longer waiting for this answer; the agent moved on")); }
    let worker = live.iter().find(|worker| worker.session.session_id == session_id).ok_or_else(|| conflict("the stopped session is not live"))?;
    if worker.session.runtime != runtime { return Err(conflict("the stopped session was restarted; this stop belongs to its previous runtime")); }
    let bundle: super::bundle::StopBundle = serde_json::from_str(&bundle)?;
    match (&reply, &bundle.question) {
        (Reply::Text(_), Some(_)) => return Err(conflict(match author {
            AnswerAuthor::Chief { .. } => "the stop is an open question form; a chat answer cannot settle it, use answer_form or escalate",
            AnswerAuthor::Theo => "the stop is an open question form; a chat answer cannot settle it, answer it in the session itself",
        })),
        (Reply::FormChoice(_), None) => return Err(conflict("the stop has no open question form; answer it with a message")),
        // The bundle shows only the first question: a choice for it would leave the others unanswered.
        (Reply::FormChoice(_), Some(form)) if form.question_count != 1 => return Err(conflict("the form asks more than one question; escalate it")),
        _ => {},
    }
    let reply = match (reply, &bundle.question) {
        (Reply::FormChoice(choice), Some(form)) => Reply::FormChoice(form_option(&form.options, choice)?),
        (reply, _) => reply,
    };
    let question = stop_question(&bundle);
    let message_id = crate::teams::store::identifier()?;
    let answer_id = crate::teams::store::identifier()?;
    let mut envelope = json!({"message_id":message_id,"epoch":epoch,"source":source,"stop":stop,"answer":answer_id});
    let text = match reply {
        Reply::Text(text) => { envelope["kind"] = json!("chief_answer"); envelope["text"] = json!(text); text.to_string() },
        Reply::FormChoice(choice) => { envelope["kind"] = json!("chief_form_reply"); envelope["choice"] = json!(choice); format!("Form choice: {choice}") },
    };
    connection.execute("INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at) VALUES (?1,?2,?3,?4,?2,'null',?5,'queued',?6)",
        params![message_id, epoch, session_id, serde_json::to_string(&worker.session)?, envelope.to_string(), now_ms])?;
    connection.execute("INSERT INTO chief_answers(id,stop_id,session_id,question,answer,reason,source,delivery,created_at,habits) VALUES (?1,?2,?3,?4,?5,?6,?7,'queued',?8,?9)",
        params![answer_id, stop, session_id, question, text, reason, source, now_ms, serde_json::to_string(habits)?])?;
    connection.execute("UPDATE chief_stops SET state='answered' WHERE id=?1", [stop])?;
    Ok(json!({"answer":answer_id,"stop":stop,"delivery":"queued"}))
}

/// What a correction quotes of the answer it corrects: whitespace collapsed, at most 200 characters.
/// The Chief view reuses it so its review shows the text the agent will receive.
pub fn answer_excerpt(answer: &str) -> String { excerpt(&answer.split_whitespace().collect::<Vec<_>>().join(" "), 200) }

pub const HABIT_TEXT_LIMIT: usize = 500;
/// A habit's text: Theo's words on one line, at most 500 characters (cut with `…`).
pub fn habit_text(correction: &str) -> String { excerpt(&correction.split_whitespace().collect::<Vec<_>>().join(" "), HABIT_TEXT_LIMIT) }

/// Every habit an answer names must be active and apply to the stop's project (its own or everywhere).
fn require_citable_habits(connection: &Connection, stop: &str, habits: &[String]) -> Result<(), TeamError> {
    if habits.is_empty() { return Ok(()); }
    let project: Option<String> = connection.query_row("SELECT project FROM chief_stops WHERE id=?1", [stop], |row| row.get(0)).optional()?;
    let project = project.ok_or_else(|| TeamError::new(ErrorCode::NotFound, "stop not found"))?;
    for habit in habits {
        let record: Option<(Option<String>, String)> = connection.query_row("SELECT project,state FROM chief_habits WHERE id=?1", [habit],
            |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        let Some((habit_project, state)) = record else { return Err(TeamError::new(ErrorCode::Invalid, "the answer names a habit that does not exist")) };
        if state != "active" { return Err(TeamError::new(ErrorCode::Invalid, "the answer names a habit Theo deleted")); }
        if habit_project.is_some_and(|habit_project| habit_project != project) {
            return Err(TeamError::new(ErrorCode::Invalid, "the answer names a habit of another project"));
        }
    }
    Ok(())
}

/// What makes two habit texts the same habit: trimmed, whitespace collapsed, case ignored.
fn habit_key(text: &str) -> String { habit_text(text).to_lowercase() }

/// Records a preference Theo showed himself. `source_id` is his answer to an escalation, or a stop whose
/// bundle holds a message of his (`BundleMessage::from_theo`: no launch prompt, brief or cockpit prompt); the habit's
/// project is that stop's project or, for his answer only, everywhere. The same habit already active in that scope is kept as
/// it is; one Theo deleted in a scope it would reach again is refused.
fn add_habit(connection: &Connection, text: &str, project: Option<&str>, source_id: &str, reason: &str, now_ms: i64) -> Result<Value, TeamError> {
    let answer_project: Option<String> = connection.query_row("SELECT s.project FROM chief_answers a JOIN chief_stops s ON s.id=a.stop_id WHERE a.id=?1 AND a.source='theo'",
        [source_id], |row| row.get(0)).optional()?;
    let (source_project, source_kind) = match answer_project {
        Some(project) => (project, "theo_answer"),
        None => {
            let stop: Option<(String, String)> = connection.query_row("SELECT project,bundle FROM chief_stops WHERE id=?1", [source_id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
            let (project, bundle) = stop.ok_or_else(|| TeamError::new(ErrorCode::NotFound, "source_id names neither an answer of Theo's nor a stop"))?;
            let bundle: super::bundle::StopBundle = serde_json::from_str(&bundle)?;
            if !bundle.recent.iter().any(|message| message.role == "user" && message.from_theo && !message.text.trim().is_empty()) {
                return Err(TeamError::new(ErrorCode::Invalid, "this stop shows no message from Theo; only his own words make a habit"));
            }
            (project, "theo_message")
        },
    };
    if project.is_some_and(|project| project != source_project) {
        return Err(TeamError::new(ErrorCode::Invalid, "a habit applies to its source's project or everywhere"));
    }
    if project.is_none() && source_kind == "theo_message" {
        return Err(TeamError::new(ErrorCode::Invalid, "only Theo widens a habit from a message (p)"));
    }
    let text = habit_text(text);
    let same_text = habits_like(connection, &text)?;
    if let Some(id) = active_habit_in(&same_text, project) {
        return Ok(json!({"habit":id,"state":"active","project":project,"added":false}));
    }
    // A deleted project habit would come back through an everywhere one, and a deleted everywhere one through any project.
    let reaches_deleted = same_text.iter().any(|(_, habit_project, state)| state == "deleted"
        && (habit_project.is_none() || project.is_none() || habit_project.as_deref() == project));
    if reaches_deleted { return Err(conflict("Theo deleted this habit; the Chief does not add it back")); }
    let id = crate::teams::store::identifier()?;
    connection.execute("INSERT INTO chief_habits(id,text,project,origin_project,source,source_kind,source_id,reason,state,created_at,changed_at) VALUES (?1,?2,?3,?4,'theo_answer',?5,?6,?7,'active',?8,?8)",
        params![id, text, project, source_project, source_kind, source_id, reason, now_ms])?;
    coordination_event(connection, "habit_changed", &json!({"habit":id,"state":"active","project":project}), now_ms)?;
    Ok(json!({"habit":id,"state":"active","project":project,"added":true}))
}

/// Every habit, in any scope or state, that is the same habit as `text`: (id, project, state).
fn habits_like(connection: &Connection, text: &str) -> Result<Vec<(String, Option<String>, String)>, TeamError> {
    let wanted = habit_key(text);
    Ok(connection.prepare("SELECT id,text,project,state FROM chief_habits ORDER BY created_at,rowid")?
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, String>(3)?)))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter().filter(|(_, habit, _, _)| habit_key(habit) == wanted)
        .map(|(id, _, habit_project, state)| (id, habit_project, state)).collect())
}

fn active_habit_in<'a>(same_text: &'a [(String, Option<String>, String)], project: Option<&str>) -> Option<&'a str> {
    same_text.iter().find(|(_, habit_project, state)| state == "active" && habit_project.as_deref() == project).map(|(id, _, _)| id.as_str())
}

enum HabitChange { State(&'static str), Scope { everywhere: bool } }

/// Changes one habit and records it as an event: `chief_habits` has no review triggers, so the event is what moves the review generation.
/// A change to what the habit already is writes nothing. The event wakes no Chief: habits reach it with its next context.
fn change_habit(connection: &Connection, habit: &str, change: HabitChange, now_ms: i64) -> Result<Value, TeamError> {
    let record: Option<(Option<String>, Option<String>, String)> = connection.query_row("SELECT project,origin_project,state FROM chief_habits WHERE id=?1", [habit],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
    let (project, origin_project, state) = record.ok_or_else(|| TeamError::new(ErrorCode::NotFound, "habit not found"))?;
    let (new_project, new_state) = match change {
        HabitChange::State(new_state) => (project.clone(), new_state.to_string()),
        HabitChange::Scope { everywhere: true } => (None, state.clone()),
        HabitChange::Scope { everywhere: false } => (Some(origin_project.ok_or_else(|| conflict("no project to scope to"))?), state.clone()),
    };
    if (&new_project, &new_state) != (&project, &state) {
        connection.execute("UPDATE chief_habits SET project=?2,state=?3,changed_at=?4 WHERE id=?1", params![habit, new_project, new_state, now_ms])?;
        coordination_event(connection, "habit_changed", &json!({"habit":habit,"state":new_state,"project":new_project}), now_ms)?;
    }
    Ok(json!({"habit":habit,"state":new_state,"project":new_project}))
}

/// Queues Theo's correction of an answer the session already got (or may have got), for the runtime
/// that got it. The inbox row is written in the same transaction: its triggers bump the review generation.
fn queue_correction(connection: &Connection, live: &[WorkerState], answer: &str, correction: &str, now_ms: i64) -> Result<Value, TeamError> {
    let record: Option<(String, String, String, String, String, bool, String)> = connection.query_row("SELECT a.session_id,s.runtime,a.answer,a.delivery,a.source,
            EXISTS(SELECT 1 FROM chief_overrules o WHERE o.answer_id=a.id),s.project FROM chief_answers a JOIN chief_stops s ON s.id=a.stop_id WHERE a.id=?1", [answer],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?))).optional()?;
    let (session_id, runtime, answer_text, delivery, source, is_overruled, project) = record.ok_or_else(|| TeamError::new(ErrorCode::NotFound, "answer not found"))?;
    if source != "chief" { return Err(conflict("this answer is yours, not the Chief's; send the agent a new message instead")); }
    if is_overruled { return Err(conflict("the answer is already overruled")); }
    match delivery.as_str() {
        "received" | "uncertain" => {},
        "queued" => return Err(conflict("the answer is not sent yet; it cannot be overruled")),
        _ => return Err(conflict("the answer was cancelled; it never reached the agent")),
    }
    let worker = live.iter().find(|worker| worker.session.session_id == session_id).ok_or_else(|| conflict("the session is not live"))?;
    if worker.session.runtime != runtime { return Err(conflict("the session was restarted since this answer; the correction would reach an agent that never got it")); }
    let overrule_id = crate::teams::store::identifier()?;
    let message_id = crate::teams::store::identifier()?;
    let answer_excerpt = answer_excerpt(&answer_text);
    let envelope = json!({"message_id":message_id,"kind":"chief_correction","epoch":THEO_ANSWER_EPOCH,"source":"theo",
        "overrule":overrule_id,"answer_excerpt":answer_excerpt,"text":correction});
    connection.execute("INSERT INTO chief_overrules(id,answer_id,session_id,runtime,correction,delivery,created_at) VALUES (?1,?2,?3,?4,?5,'queued',?6)",
        params![overrule_id, answer, session_id, runtime, correction, now_ms])?;
    connection.execute("INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at) VALUES (?1,?2,?3,?4,?2,'null',?5,'queued',?6)",
        params![message_id, THEO_ANSWER_EPOCH, session_id, serde_json::to_string(&worker.session)?, envelope.to_string(), now_ms])?;
    let habit = habit_text(correction);
    if active_habit_in(&habits_like(connection, &habit)?, Some(&project)).is_none() {
        connection.execute("INSERT INTO chief_habits(id,text,project,origin_project,source,source_kind,source_id,state,created_at,changed_at) VALUES (?1,?2,?3,?3,'overrule','overrule',?4,'active',?5,?5)",
            params![crate::teams::store::identifier()?, habit, project, overrule_id, now_ms])?;
    }
    coordination_event(connection, "answer_overruled", &json!({"answer":answer,"overrule":overrule_id,"session":session_id}), now_ms)?;
    Ok(json!({"overrule":overrule_id,"answer":answer,"delivery":"queued"}))
}

/// The bundle's option the Chief named, matched as the plugin matches: exactly, then trimmed and
/// case-insensitive. An option the bundle shows shortened or masked cannot be named: the host would
/// not know what it picks.
fn form_option<'a>(options: &'a [String], choice: &str) -> Result<&'a str, TeamError> {
    let invalid = |message: &str| TeamError::new(ErrorCode::Invalid, message);
    if options.is_empty() { return Err(invalid("the form asks for free text, not an option; escalate it")); }
    let mut matches: Vec<&String> = options.iter().filter(|option| *option == choice).collect();
    if matches.is_empty() {
        let wanted = choice.trim().to_lowercase();
        matches = options.iter().filter(|option| option.trim().to_lowercase() == wanted).collect();
    }
    let [option] = matches.as_slice() else { return Err(invalid("the choice names no single option of the form")) };
    if option.ends_with('…') || option.contains("[redacted]") { return Err(invalid("the option is shortened or masked in the bundle; escalate it")); }
    Ok(option.as_str())
}

/// What the stopped agent asked: the form's question when it waits on one, else the end of its last message.
fn stop_question(bundle: &super::bundle::StopBundle) -> String {
    let asked = bundle.question.as_ref().map_or(bundle.last_assistant.as_str(), |form| form.question.as_str());
    if asked.trim().is_empty() { "(the agent's last message has no text)".into() } else { excerpt(asked, 300) }
}

/// Hands an open stop to Theo as a decision that carries its stop, category and the Chief's suggestion.
fn escalate_stop(connection: &Connection, epoch: &str, stop: &str, category: EscalationCategory, reason: &str, suggested_answer: Option<&str>, now_ms: i64) -> Result<Value, TeamError> {
    let record: Option<(String, String, String)> = connection.query_row("SELECT session_id,state,bundle FROM chief_stops WHERE id=?1", [stop],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
    let (session_id, state, bundle) = record.ok_or_else(|| TeamError::new(ErrorCode::NotFound, "stop not found"))?;
    if state != "open" { return Err(conflict("the stop is no longer open; the agent moved on")); }
    let open: i64 = connection.query_row("SELECT count(*) FROM chief_decisions WHERE state='open'", [], |row| row.get(0))?;
    if open >= super::MAX_OPEN_DECISIONS as i64 { return Err(TeamError::new(ErrorCode::Limit, "chief decision capacity exceeded; unresolved decisions retained")); }
    let question = stop_question(&serde_json::from_str(&bundle)?);
    let id = crate::teams::store::identifier()?;
    connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at,session_id,stop_id,category,suggested_answer)
        VALUES (?1,?2,?3,?4,?5,'open',1,?6,?7,?8,?9,?10)",
        params![id, format!("chief-{epoch}"), format!("stop-{stop}"), question, reason, now_ms, session_id, stop, category.as_str(), suggested_answer])?;
    connection.execute("INSERT INTO chief_attention(decision,created_at) VALUES (?1,?2)", params![id, now_ms])?;
    connection.execute("UPDATE chief_stops SET state='escalated' WHERE id=?1", [stop])?;
    Ok(json!({"decision":id,"revision":1,"stop":stop,"state":"escalated"}))
}

/// The host policy that let the Chief dispatch a todo its own project filed without Theo's Q.
const OWN_TODOS_POLICY: &str = "chief-dispatches-own-todos";

/// Why the Chief may dispatch a todo: what the host records with each request.
struct StartableTodo { project: String, is_theo_queued: bool, policy: Option<&'static str> }

/// The todo the Chief may start a session for or give to a session. Any todo still to do
/// qualifies, queued or not, except one waiting on others or for the designer; one a session of
/// the Chief's own project filed needs Theo's Q first, so the Chief cannot start work it gave
/// itself, unless Theo turned on the chief-dispatches-own-todos setting. The host reads it on each
/// request and again before delivering a gift; a fresh-session request already made lapses within
/// SESSION_START_OPEN_MS.
fn chief_startable_todo(todos: &crate::todo::TodoList, chief_project: &str, todo: &str) -> Result<StartableTodo, TeamError> {
    let item = todos.items.iter().find(|item| item.id() == todo).ok_or_else(|| TeamError::new(ErrorCode::NotFound, "no todo has this id"))?;
    if item.status() != "todo" || item.is_waiting() || item.is_design() {
        return Err(conflict("the Chief starts sessions only for todos still to do, waiting on nothing and not for the designer"));
    }
    let chief_folder = chief_project.rsplit('/').next().unwrap_or(chief_project);
    let is_theo_queued = todos.is_queued(todo) && item.is_queued_by_theo();
    let mut policy = None;
    if item.is_filed_from(chief_folder) && !is_theo_queued {
        if !crate::settings::chief_dispatches_own_todos().map_err(|error| TeamError::new(ErrorCode::Storage, &error))? {
            return Err(conflict("a todo the Chief filed waits until Theo queues it with Q, or turns on `cockpit setting chief-dispatches-own-todos on`"));
        }
        policy = Some(OWN_TODOS_POLICY);
    }
    Ok(StartableTodo { project: crate::teams::canonical_project(&item.cwd())?, is_theo_queued, policy })
}

/// Lapses the requests the queue did not act on in time, then refuses a second one for the todo.
fn require_no_waiting_request(connection: &Connection, project: &str, todo: &str, now_ms: i64) -> Result<(), TeamError> {
    connection.execute("UPDATE chief_session_starts SET state='expired' WHERE state='requested' AND created_at<=?1",
        [now_ms.saturating_sub(super::SESSION_START_OPEN_MS)])?;
    if connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_session_starts WHERE state='requested' AND project=?1 AND task_id=?2)", params![project, todo], |row| row.get::<_, bool>(0))? {
        return Err(conflict("a session start already waits for this todo"));
    }
    Ok(())
}

/// The live session the Chief names, never the Chief itself.
fn chief_target<'a>(live: &'a [WorkerState], chief: &VerifiedSession, session: &str) -> Result<&'a WorkerState, TeamError> {
    let worker = live.iter().find(|worker| worker.session.session_id == session).ok_or_else(|| TeamError::new(ErrorCode::NotFound, "no live session has this id"))?;
    if worker.session.session_id == chief.session_id { return Err(conflict("the Chief cannot send work or messages to itself")); }
    if worker.session.agent != "opencode" { return Err(conflict("Chief messages and todo gifts require a live OpenCode session")); }
    Ok(worker)
}

/// Records the Chief's request; the queue's next pass starts the session through the usual
/// reservation, which then skips only the rule that no session of the project may be live.
/// Todos Theo queued himself with Q do not count against `starts_per_hour`: the Q is host-recorded,
/// so it holds where the Chief's word that Theo asked would not.
fn request_session_start(connection: &Connection, todos: &crate::todo::TodoList, chief_project: &str, epoch: &str, todo: &str, project: &str, reason: &str, starts_per_hour: i64, now_ms: i64) -> Result<Value, TeamError> {
    let StartableTodo { project: canonical, is_theo_queued, policy } = chief_startable_todo(todos, chief_project, todo)?;
    if canonical.rsplit('/').next() != Some(project) { return Err(conflict("the todo belongs to another project")); }
    require_no_waiting_request(connection, &canonical, todo, now_ms)?;
    let recent: i64 = connection.query_row("SELECT count(*) FROM chief_session_starts WHERE created_at>?1 AND theo_queued=0 AND worker IS NULL", [now_ms.saturating_sub(3_600_000)], |row| row.get(0))?;
    if !is_theo_queued && recent >= starts_per_hour {
        return Err(TeamError::new(ErrorCode::Limit, &format!("the Chief may request at most {starts_per_hour} session starts an hour (Theo's chief-starts-per-hour); todos Theo queued with Q do not count")));
    }
    let id = crate::teams::store::identifier()?;
    connection.execute("INSERT INTO chief_session_starts(id,epoch,project,task_id,reason,state,created_at,theo_queued) VALUES (?1,?2,?3,?4,?5,'requested',?6,?7)",
        params![id, epoch, canonical, todo, reason, now_ms, is_theo_queued])?;
    coordination_event(connection, "session_start_requested", &json!({"source":"chief","request":id,"project":canonical,"task_id":todo,"reason":reason,"theo_queued":is_theo_queued,"dispatch_policy":policy}), now_ms)?;
    Ok(json!({"request": id, "state": "requested", "project": canonical, "todo": todo, "dispatch_policy": policy}))
}

/// Records the Chief's request to give a todo to a live session of its project. Its native inbox
/// takes it once the usual queue admission and reservation checks pass; a
/// gift starts no session, so it does not count against the hourly session starts.
fn request_todo_gift(connection: &Connection, todos: &crate::todo::TodoList, live: &[WorkerState], chief: &VerifiedSession, epoch: &str, todo: &str, session: &str, reason: &str, now_ms: i64) -> Result<Value, TeamError> {
    let StartableTodo { project: canonical, is_theo_queued, policy } = chief_startable_todo(todos, &chief.project, todo)?;
    let worker = chief_target(live, chief, session)?;
    if worker.session.project != canonical { return Err(conflict("the session works in another project than the todo")); }
    require_no_waiting_request(connection, &canonical, todo, now_ms)?;
    if connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_session_starts WHERE state='requested' AND worker=?1)", [session], |row| row.get::<_, bool>(0))? {
        return Err(conflict("a todo the Chief gave already waits for this session"));
    }
    let id = crate::teams::store::identifier()?;
    require_task_mapping(connection, &worker.session, &crate::teams::TaskKey {project: canonical.clone(), task_id: todo.into()})?;
    connection.execute("INSERT INTO chief_session_starts(id,epoch,project,task_id,reason,state,created_at,theo_queued,worker,worker_runtime) VALUES (?1,?2,?3,?4,?5,'requested',?6,?7,?8,?9)",
        params![id, epoch, canonical, todo, reason, now_ms, is_theo_queued, session, worker.session.runtime])?;
    coordination_event(connection, "todo_gift_requested", &json!({"source":"chief","request":id,"project":canonical,"task_id":todo,"session":session,"reason":reason,"dispatch_policy":policy}), now_ms)?;
    Ok(json!({"request": id, "state": "requested", "project": canonical, "todo": todo, "session": session, "dispatch_policy": policy}))
}

/// The open Chief request to start a fresh session for this task, as (request id, reason).
pub fn open_session_start(connection: &Connection, project: &str, task_id: &str, now_ms: i64) -> Result<Option<(String, String)>, TeamError> {
    Ok(connection.query_row("SELECT id,reason FROM chief_session_starts WHERE state='requested' AND project=?1 AND task_id=?2 AND worker IS NULL AND created_at>?3",
        params![project, task_id, now_ms.saturating_sub(super::SESSION_START_OPEN_MS)], |row| Ok((row.get(0)?, row.get(1)?))).optional()?)
}

/// The todo ids the Chief asked a fresh session for that the queue has not acted on yet: the queue
/// starts them even when Theo did not queue them.
pub fn open_session_start_tasks(connection: &Connection, now_ms: i64) -> Result<std::collections::HashSet<String>, TeamError> {
    Ok(connection.prepare("SELECT task_id FROM chief_session_starts WHERE state='requested' AND worker IS NULL AND created_at>?1")?
        .query_map([now_ms.saturating_sub(super::SESSION_START_OPEN_MS)], |row| row.get(0))?
        .collect::<Result<_, _>>()?)
}

/// Retire gifts that can no longer be admitted, before checking exclusivity or native delivery.
pub fn expire_todo_gifts(connection: &Connection, live: &[VerifiedSession], now_ms: i64) -> Result<(), TeamError> {
    connection.execute("UPDATE chief_session_starts SET state='expired' WHERE state='requested' AND worker IS NOT NULL
        AND (created_at<=?1 OR epoch IN (SELECT epoch FROM chief_grants WHERE state='disabled'))",[now_ms.saturating_sub(super::SESSION_START_OPEN_MS)])?;
    let gifts=connection.prepare("SELECT id,worker,worker_runtime FROM chief_session_starts WHERE state='requested' AND worker IS NOT NULL")?
        .query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
    for (id,worker,runtime) in gifts {
        if !live.iter().any(|session|session.session_id==worker && session.runtime==runtime) {
            connection.execute("UPDATE chief_session_starts SET state='expired' WHERE id=?1",[id])?;
        }
    }
    Ok(())
}

/// A gift is claimed and reserved by its exact native recipient, not by a column cache or a shell
/// command that can retarget after a restart. All mutations share the inbox transaction.
pub fn claim_todo_gift(connection: &mut Connection, recipient: &VerifiedSession, idle_turn: &str, now_ms: i64) -> Result<Option<crate::teams::Dispatch>, TeamError> {
    if idle_turn.is_empty() || idle_turn.len()>128 || idle_turn.chars().any(char::is_control) {return Err(super::invalid("missing verified idle turn"));}
    let pending: bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_session_starts WHERE state='requested' AND worker=?1 AND worker_runtime=?2)",
        params![recipient.session_id,recipient.runtime],|row|row.get(0))?;
    if !pending {return Ok(None);}
    // Same lock order as ordinary TODO mutations: TODO first, then SQLite. Keep it until the
    // conditional todo -> doing transition is durable, so a concurrent completion cannot be reopened.
    let guard=crate::todo::lock_mutations().map_err(|error|TeamError::new(ErrorCode::Storage,error.to_string()))?;
    let transaction=connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let live=observed(&transaction)?;
    expire_todo_gifts(&transaction,&live.iter().map(|worker|worker.session.clone()).collect::<Vec<_>>(),now_ms)?;
    if !crate::teams::store::inbox_available(&transaction,recipient,Some(idle_turn))? {transaction.commit()?;return Ok(None);}
    let ready=live.iter().any(|worker|worker.session==*recipient && worker.session.agent=="opencode" && worker.observed_at_ms<=now_ms && now_ms-worker.observed_at_ms<=30_000
        && worker.blocked_by.iter().all(|reason|reason=="live_queued_admission_verification_required"));
    if !ready {transaction.commit()?;return Ok(None);}
    let gift: Option<(String,String,String,String)> = transaction.query_row("SELECT s.id,s.task_id,s.epoch,g.project FROM chief_session_starts s JOIN chief_grants g ON g.epoch=s.epoch
        WHERE s.state='requested' AND s.worker=?1 AND s.worker_runtime=?2 AND g.state='active'",params![recipient.session_id,recipient.runtime],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
    let Some((request,todo,epoch,chief_project))=gift else {transaction.commit()?;return Ok(None)};
    let mut todos=crate::todo::TodoList::load_checked().map_err(|error|TeamError::new(ErrorCode::Storage,error))?;
    let role=worker_role(&transaction,recipient)?;
    if matches!(role,Some(crate::teams::Role::Designer|crate::teams::Role::Qa)) || crate::sessions::role_of(&recipient.session_id)=="designer" {
        transaction.execute("UPDATE chief_session_starts SET state='expired' WHERE id=?1",[request])?;
        transaction.commit()?;return Ok(None);
    }
    let project=match chief_startable_todo(&todos,&chief_project,&todo) {
        Ok(eligible)=>eligible.project,
        Err(error) if matches!(error.code,ErrorCode::NotFound|ErrorCode::Conflict) => {
            transaction.execute("UPDATE chief_session_starts SET state='expired' WHERE id=?1",[request])?;
            transaction.commit()?;return Ok(None);
        },
        Err(error)=>return Err(error),
    };
    if project!=recipient.project {
        transaction.execute("UPDATE chief_session_starts SET state='expired' WHERE id=?1",[request])?;
        transaction.commit()?;return Ok(None);
    }
    let task=crate::teams::TaskKey {project,task_id:todo.clone()};
    if let Err(error)=require_task_mapping(&transaction,recipient,&task).and_then(|()|require_unreserved(&transaction,&task,Some(&recipient.session_id))) {
        if matches!(error.code,ErrorCode::Unauthorized|ErrorCode::Conflict) {
            transaction.execute("UPDATE chief_session_starts SET state='expired' WHERE id=?1",[request])?;
            transaction.commit()?;return Ok(None);
        }
        return Err(error);
    }
    let item=todos.items.iter().find(|item|item.id()==todo).expect("startable todo exists");
    let prompt=crate::todo::with_todo_note(&crate::todo::prompt_of(&crate::todo::Entry::Typed(item.clone())),&todo);
    super::text(&prompt,crate::prompts::PROMPT_MAX)?;
    let token=insert_launch(&transaction,&task,Some(&recipient.session_id),"submitting",now_ms)?;
    let message_id=crate::teams::store::identifier()?;
    let id=format!("chief_{}",crate::teams::store::identifier()?);
    let envelope=json!({"message_id":message_id,"kind":"chief_todo","epoch":epoch,"text":prompt,"todo":todo,"reservation":token});
    transaction.execute("INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at,attempts)
        VALUES (?1,?2,?3,?4,?2,'null',?5,'submitting',?6,1)",params![message_id,epoch,recipient.session_id,serde_json::to_string(recipient)?,envelope.to_string(),now_ms])?;
    transaction.execute("INSERT INTO chief_delivery_attempts(id,recipient,recipient_runtime,recipient_epoch,kind,envelope,idle_turn,state,started_at)
        VALUES (?1,?2,?3,?4,'chief_todo',?5,?6,'submitting',?7)",params![id,recipient.session_id,recipient.runtime,epoch,envelope.to_string(),idle_turn,now_ms])?;
    crate::teams::store::record_inbox_turn(&transaction,recipient,idle_turn)?;
    mark_session_started(&transaction,&request,&token,now_ms)?;
    transaction.commit()?;
    let item=todos.items.iter_mut().find(|item|item.id()==todo).expect("startable todo still locked");
    item.0.insert("status".into(),json!("doing"));
    item.0.insert("session".into(),json!(recipient.session_id));
    item.0.insert("updated".into(),json!(now_ms/1000));
    todos.save_guarded(&guard).map_err(|error|TeamError::new(ErrorCode::Storage,format!("todo gift reserved but status could not be saved: {error}; delivery needs review")))?;
    Ok(Some(crate::teams::Dispatch {id,message_id,envelope}))
}

/// Records the Chief's message for a live session; its native inbox takes it only when idle.
/// Bound to the session's current runtime: a restart lapses it.
fn request_message(connection: &Connection, live: &[WorkerState], chief: &VerifiedSession, epoch: &str, session: &str, text: &str, reason: &str, now_ms: i64) -> Result<Value, TeamError> {
    let worker = chief_target(live, chief, session)?;
    connection.execute("UPDATE chief_inbox SET state='cancelled' WHERE state='queued' AND json_extract(envelope,'$.kind')='chief_message'
        AND json_extract(envelope,'$.expires_at')<=?1", [now_ms])?;
    let waiting: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_inbox WHERE recipient=?1 AND state IN ('queued','submitting','uncertain')
        AND json_extract(envelope,'$.kind')='chief_message')", [session], |row|row.get(0))?;
    if waiting {return Err(conflict("a Chief message already waits for this session"));}
    let recent: i64 = connection.query_row("SELECT count(*) FROM chief_events WHERE kind='chief_message_requested' AND created_at>?1",
        [now_ms.saturating_sub(3_600_000)], |row|row.get(0))?;
    if recent >= super::CHIEF_MESSAGES_PER_HOUR {return Err(TeamError::new(ErrorCode::Limit,"the Chief may send at most 20 messages an hour"));}
    let id = crate::teams::store::identifier()?;
    let envelope = json!({"message_id":id,"kind":"chief_message","epoch":epoch,"text":text,"reason":reason,
        "expires_at":now_ms.saturating_add(super::SESSION_START_OPEN_MS)});
    connection.execute("INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at)
        VALUES (?1,?2,?3,?4,?2,'null',?5,'queued',?6)",
        params![id,epoch,session,serde_json::to_string(&worker.session)?,envelope.to_string(),now_ms])?;
    coordination_event(connection,"chief_message_requested",&json!({"source":"chief","request":id,"session":session,"reason":reason}),now_ms)?;
    Ok(json!({"request":id,"state":"queued","session":session}))
}

/// What the session receives: the Chief's text, then the host's boundary, which the Chief cannot
/// leave out or word differently.
pub fn message_prompt(text: &str) -> String {
    format!("Message from the cockpit Chief:\n{}\n\n{}", crate::approval::escape_hiding(text, "", ""), super::CHIEF_MESSAGE_BOUNDARY)
}

pub fn mark_session_started(connection: &Connection, request: &str, token: &str, now_ms: i64) -> Result<(), TeamError> {
    let changed = connection.execute("UPDATE chief_session_starts SET state='started',token=?2,started_at=?3 WHERE id=?1 AND state='requested'", params![request, token, now_ms])?;
    if changed != 1 { return Err(conflict("the Chief's session start request is no longer open")); }
    coordination_event(connection, "session_started", &json!({"source":"host","request":request,"reservation":token}), now_ms)
}

fn excerpt(text: &str, maximum: usize) -> String {
    if text.chars().count() <= maximum { return text.to_string(); }
    let mut shortened: String = text.chars().take(maximum - 1).collect();
    shortened.push('…');
    shortened
}

/// Stores a stop and wakes the Chief for it. `None` when this boundary was already recorded.
/// The bundle is stored as a JSON object: the Chief receives it nested in a Context record, where an
/// escaped string would cost more bytes than the bundle's own size bound allows.
/// An empty `task.task_id` is a session asking for an answer with no todo in progress. A stop that
/// takes over an answer still queued for this task (`carry_queued_answers`) is answered already and
/// wakes no one.
pub fn record_stop(connection: &mut Connection, session: &VerifiedSession, boundary: &str, task: &crate::teams::TaskKey,
    bundle: &super::bundle::StopBundle, now_ms: i64) -> Result<Option<String>, TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if stop_recorded(&transaction, &session.session_id, boundary)? { return Ok(None); }
    supersede_stops(&transaction, &session.session_id, Some(boundary))?;
    let id = crate::teams::store::identifier()?;
    transaction.execute("INSERT INTO chief_stops(id,session_id,runtime,boundary,project,task_id,bundle,state,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'open',?8)",
        params![id, session.session_id, session.runtime, boundary, task.project, task.task_id, serde_json::to_string(bundle)?, now_ms])?;
    if !carry_queued_answers(&transaction, &id, session, boundary, &task.task_id)? {
        coordination_event(&transaction, "stop_detected", &json!({"stop":id,"session":session.session_id}), now_ms)?;
        schedule_wake(&transaction, now_ms)?;
    }
    transaction.commit()?;
    Ok(Some(id))
}

fn decision_record(connection: &Connection, scope: &str, key: &str, question: &str, recommendation: &str, now_ms: i64) -> Result<Value, TeamError> {
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
    connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at) VALUES (?1,?2,?3,?4,?5,'open',1,?6)",
        params![id,scope,key,question,recommendation,now_ms])?;
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
    Ok(live.iter().any(|worker|worker.session==*chief && worker.observed_at_ms<=now_ms && now_ms.saturating_sub(worker.observed_at_ms)<=30_000 && worker.ready && worker.blocked_by.is_empty()))
}

fn schedule_wake(connection: &Connection, now_ms: i64) -> Result<(), TeamError> {
    let grant: Option<(String,VerifiedSession,i64,Option<i64>,bool)> = connection.query_row("SELECT g.epoch,g.session_id,g.agent,g.project,g.runtime,c.delivered_seq,c.last_admitted_at,c.failed
        FROM chief_grants g JOIN chief_coordination c ON c.epoch=g.epoch WHERE g.state='active'",[],|row|Ok((row.get(0)?,VerifiedSession {
            session_id:row.get(1)?,agent:row.get(2)?,project:row.get(3)?,runtime:row.get(4)?},row.get(5)?,row.get(6)?,row.get(7)?))).optional()?;
    let Some((epoch,chief,delivered,last_admitted,failed)) = grant else {return Ok(())};
    if failed || last_admitted.is_some_and(|last|now_ms<last.saturating_add(30_000)) {return Ok(())}
    // A turn that ended before reading what it admitted gets a bounded re-wake: no new event would wake it.
    if !material_pending(connection,delivered)? && !needs_unread_retry(connection,&epoch)? {return Ok(())}
    let exists: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_inbox WHERE chief_epoch=?1 AND json_extract(envelope,'$.kind')='chief_wake' AND state IN ('queued','submitting','uncertain'))",[&epoch],|row|row.get(0))?;
    if exists {return Ok(())}
    let id = crate::teams::store::identifier()?;
    let envelope = json!({"message_id":id,"kind":"chief_wake","epoch":epoch,"body":"Material coordination work is pending. Read bounded chief Context. No tools or permissions are granted."});
    connection.execute("INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at) VALUES (?1,?2,?3,?4,?2,'null',?5,'queued',?6)",
        params![id,epoch,chief.session_id,serde_json::to_string(&chief)?,envelope.to_string(),now_ms])?;
    Ok(())
}

fn material_pending(connection: &Connection, delivered: i64) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_events WHERE seq>?1 AND kind NOT IN ('status','coordinate','reprioritized','habit_changed','session_start_requested','session_started'))",[delivered],|row|row.get(0))?)
}

fn needs_unread_retry(connection: &Connection, epoch: &str) -> Result<bool, TeamError> {
    Ok(latest_turn_behind(connection,epoch)? && unread_context(connection,epoch)?.1<UNREAD_CONTEXT_TURNS)
}

/// Once re-waking has not got the Chief reading, Theo sees why coordination stopped, once per read position.
fn notice_unread_context(connection: &Connection, epoch: &str, chief: &VerifiedSession, live: &[WorkerState], now_ms: i64) -> Result<(), TeamError> {
    let (position,stalled) = unread_context(connection,epoch)?;
    if stalled<UNREAD_CONTEXT_TURNS || !wake_ready(connection,chief,live,now_ms)? {return Ok(())}
    let position = position.unwrap_or(0);
    let notice = decision_record(connection,&format!("host-context-{epoch}"),"context_unread",
        &format!("The Chief ended {UNREAD_CONTEXT_TURNS} admitted turns in a row without reading its coordination Context to the end; it has read through event {position}. Stopped sessions, questions and stalled work after that event may be unhandled. The host no longer re-wakes it for them; only new events wake it."),
        &format!("Ask the Chief to read Context with after:{position} and report the exact error if it cannot. Do not relax budgets or resend uncertain deliveries; route a persistent failure to the existing Chief context repair todo."),now_ms);
    // A full decision list is already in front of Theo; failing here would also stop every material wake.
    match notice { Err(error) if error.code==ErrorCode::Limit => Ok(()), other => other.map(|_|()) }
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
            decision_record(connection,&format!("host-chief-{epoch}"),"unavailable","Selected chief is missing, stale or has no supported native inbox.","Host must restore or replace the chief; no fallback is authorized.",now_ms)?;
        }
    }
    let failed_wakes = connection.prepare("SELECT DISTINCT chief_epoch FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_wake' AND state='uncertain'")?
        .query_map([],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for epoch in failed_wakes {
        connection.execute("UPDATE chief_coordination SET failed=1 WHERE epoch=?1",[&epoch])?;
    }
    let unresolved=connection.prepare("SELECT id FROM chief_inbox WHERE state IN ('uncertain','failed')")?
        .query_map([],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for message in unresolved {
        decision_record(connection,&format!("host-delivery-{message}"),"delivery","Exact native delivery needs host recovery; handling is not verified delivery or completion.","Inspect the exact delivery and recipient. Confirm handled, or explicitly confirm safe stop before cancellation.",now_ms)?;
    }
    connection.execute("UPDATE chief_summary_requests SET state='failed' WHERE state IN ('queued','pending') AND deadline<=?1",[now_ms])?;
    connection.execute("UPDATE legacy_launches SET state='uncertain',revision=revision+1 WHERE state IN ('reserved','submitting') AND created_at<=?1",[now_ms.saturating_sub(30_000)])?;
    let launches=connection.prepare("SELECT token FROM legacy_launches WHERE state='uncertain'")?.query_map([],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for token in launches {decision_record(connection,&format!("host-launch-{token}"),"launch","Exact legacy launch is not conclusively settled; ownership remains reserved.","Inspect this exact launch token. Handling retains ownership; only an explicit safe-stop assertion can release unknown execution.",now_ms)?;}
    let active: Option<(String,VerifiedSession)> = connection.query_row("SELECT epoch,session_id,agent,project,runtime FROM chief_grants WHERE state='active'",[],
        |row|Ok((row.get(0)?,VerifiedSession {session_id:row.get(1)?,agent:row.get(2)?,project:row.get(3)?,runtime:row.get(4)?}))).optional()?;
    if let Some((epoch,chief)) = active {notice_unread_context(connection,&epoch,&chief,live,now_ms)?;}
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

/// Charged before the read, like a mutation, so a failing inspection still spends its allowance.
fn charge_inspection(connection: &Connection, epoch: &str) -> Result<String, TeamError> {
    if connection.query_row("SELECT failed FROM chief_coordination WHERE epoch=?1",[epoch],|row|row.get::<_,bool>(0))? {
        return Err(TeamError::new(ErrorCode::Unavailable,"chief native coordination delivery requires host review; previous allowance is closed"));
    }
    let Some((dispatch,_,_,_)) = current_turn(connection,epoch)? else {return Err(TeamError::new(ErrorCode::Unavailable,"chief inspections require the first admitted verified native coordination wake"))};
    let inspections: i64 = connection.query_row("SELECT inspections FROM chief_turns WHERE dispatch_id=?1",[&dispatch],|row|row.get(0))?;
    if inspections>=super::INSPECTIONS_PER_TURN {return Err(TeamError::new(ErrorCode::Limit,"chief turn inspection budget exhausted; wait for the next admitted wake"))}
    connection.execute("UPDATE chief_turns SET inspections=inspections+1 WHERE dispatch_id=?1",[&dispatch])?;
    Ok(dispatch)
}

pub(crate) fn ensure_turn_inspections(connection: &Connection) -> Result<(), TeamError> {
    if !has_column(connection,"chief_turns","inspections")? {
        connection.execute_batch("ALTER TABLE chief_turns ADD COLUMN inspections INTEGER NOT NULL DEFAULT 0;")?;
    }
    Ok(())
}

/// Host-recorded state of one session or todo. Todo titles and results are what sessions wrote:
/// reports, not evidence of completion. The only conversation shown is the open stop's masked bundle.
fn inspect(connection: &Connection, todos: &crate::todo::TodoList, live: &[WorkerState], session: Option<&str>, todo: Option<&str>, now_ms: i64) -> Result<Value, TeamError> {
    let mut result = json!({"kind":"inspection","source":"protected_host_inspection"});
    if let Some(todo) = todo {
        let item = todos.items.iter().find(|item|item.id()==todo).ok_or_else(||TeamError::new(ErrorCode::NotFound,"no todo has this id"))?;
        result["todo"] = inspected_todo(todos,item);
        result["todo"]["session_starts"] = json!(connection.prepare("SELECT id,worker,state FROM chief_session_starts WHERE task_id=?1 ORDER BY rowid DESC LIMIT 5")?
            .query_map([todo],|row|Ok(json!({"request":row.get::<_,String>(0)?,"session":row.get::<_,Option<String>>(1)?,"state":row.get::<_,String>(2)?})))?
            .collect::<Result<Vec<_>,_>>()?);
        let owner = item.session();
        result["owner"] = if owner.is_empty() {Value::Null} else {inspected_session(connection,todos,live,&owner,now_ms)?};
    }
    if let Some(session) = session {
        result["session"] = inspected_session(connection,todos,live,session,now_ms)?;
    }
    // As in Context: the Chief reads Theo's habits before it answers a stop.
    if ["owner","session"].iter().any(|key|result[*key]["open_stop"].is_object()) {
        result["habits"] = habits_record(connection)?;
    }
    fit_inspection(result)
}

fn inspected_todo(todos: &crate::todo::TodoList, item: &crate::todo::Item) -> Value {
    // Masked before it is cut, so a cut cannot split a secret past the redaction's recognition.
    let untrusted = |text: &str, maximum: usize| excerpt(&super::bundle::masked(text),maximum);
    let waits_on = item.after().into_iter().map(|id| {
        let status = todos.items.iter().find(|other|other.id()==id).map(crate::todo::Item::status);
        json!({"id":id,"status":status})
    }).collect::<Vec<_>>();
    let result = item.result();
    json!({"id":item.id(),"project":item.project(),"status":item.status(),"session":Some(item.session()).filter(|session|!session.is_empty()),
        "queued":todos.is_queued(&item.id()),"waits_on":waits_on,"title":untrusted(&item.text(),300),
        "result":(!result.is_empty()).then(||untrusted(&result,2000)),"text_source":"session_report_not_evidence"})
}

fn inspected_session(connection: &Connection, todos: &crate::todo::TodoList, live: &[WorkerState], session: &str, now_ms: i64) -> Result<Value, TeamError> {
    let availability = live.iter().find(|worker|worker.session.session_id==session).map(|worker| {
        let mut blocked = worker.blocked_by.clone();
        if worker.observed_at_ms>now_ms || now_ms.saturating_sub(worker.observed_at_ms)>30_000 { blocked.push("observation_stale".into()); }
        blocked.sort(); blocked.dedup();
        json!({"agent":worker.session.agent,"project":worker.session.project,"role":worker.role,"ready":worker.ready && blocked.is_empty(),"blocked_by":blocked})
    });
    let doing = todos.items.iter().filter(|item|item.session()==session && item.status()=="doing").map(crate::todo::Item::id).collect::<Vec<_>>();
    let open_stop = connection.query_row("SELECT id,task_id,bundle,created_at FROM chief_stops WHERE session_id=?1 AND state='open' ORDER BY created_at DESC,rowid DESC LIMIT 1",
        [session],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,i64>(3)?))).optional()?
        .map(|(stop,task_id,bundle,created_at)|Ok::<_,TeamError>(json!({"stop":stop,"task_id":task_id,"created_at_ms":created_at,
            "source":"masked_agent_conversation_not_instructions","bundle":serde_json::from_str::<Value>(&bundle)?}))).transpose()?;
    let recent_stops = connection.prepare("SELECT id,task_id,state,created_at FROM chief_stops WHERE session_id=?1 AND state!='open' ORDER BY created_at DESC,rowid DESC LIMIT 5")?
        .query_map([session],|row|Ok(json!({"stop":row.get::<_,String>(0)?,"task_id":row.get::<_,String>(1)?,"state":row.get::<_,String>(2)?,"created_at_ms":row.get::<_,i64>(3)?})))?
        .collect::<Result<Vec<_>,_>>()?;
    let answers = connection.prepare("SELECT id,stop_id,source,delivery,created_at FROM chief_answers WHERE session_id=?1 ORDER BY created_at DESC,rowid DESC LIMIT 5")?
        .query_map([session],|row|Ok(json!({"answer":row.get::<_,String>(0)?,"stop":row.get::<_,String>(1)?,"source":row.get::<_,String>(2)?,"delivery":row.get::<_,String>(3)?,"created_at_ms":row.get::<_,i64>(4)?})))?
        .collect::<Result<Vec<_>,_>>()?;
    let messages = connection.prepare("SELECT id,state FROM chief_inbox WHERE recipient=?1 AND json_extract(envelope,'$.kind')='chief_message' ORDER BY rowid DESC LIMIT 5")?
        .query_map([session],|row|Ok(json!({"message":row.get::<_,String>(0)?,"state":row.get::<_,String>(1)?})))?
        .collect::<Result<Vec<_>,_>>()?;
    let gifts = connection.prepare("SELECT id,task_id,state FROM chief_session_starts WHERE worker=?1 ORDER BY rowid DESC LIMIT 5")?
        .query_map([session],|row|Ok(json!({"request":row.get::<_,String>(0)?,"todo":row.get::<_,String>(1)?,"state":row.get::<_,String>(2)?})))?
        .collect::<Result<Vec<_>,_>>()?;
    Ok(json!({"session_id":session,"live":availability.is_some(),"availability":availability,"doing_todos":doing,"open_stop":open_stop,
        "recent_stops":recent_stops,"answers":answers,"messages":messages,"todo_gifts":gifts}))
}

/// The bundle and the habits record are each bounded, but with the project's instruction files beside
/// them an inspection could pass the reply bound. The instruction files go first: they are project
/// rules the stop's Context record also carries, while the conversation and habits decide the answer.
fn fit_inspection(mut result: Value) -> Result<Value, TeamError> {
    let stop_path = if result["owner"].is_object() {"/owner/open_stop"} else {"/session/open_stop"};
    if serde_json::to_vec(&result)?.len() >= super::CONTEXT_BYTES {
        if let Some(stop) = result.pointer_mut(stop_path).and_then(Value::as_object_mut) {
            if let Some(bundle) = stop.get_mut("bundle").and_then(Value::as_object_mut) { bundle.remove("instructions"); }
            stop.insert("instructions_omitted".into(),json!(true));
        }
    }
    if serde_json::to_vec(&result)?.len() >= super::CONTEXT_BYTES { return Err(TeamError::new(ErrorCode::Limit,"chief inspection exceeds reply bound")); }
    Ok(result)
}

enum InboxValidation { Ready, Deferred, Obsolete }

pub fn claim(connection: &mut Connection, recipient: &VerifiedSession, idle_turn: &str, now_ms: i64) -> Result<Option<crate::teams::Dispatch>, TeamError> {
    if idle_turn.is_empty() || idle_turn.len() > 128 || idle_turn.chars().any(char::is_control) { return Err(TeamError::new(ErrorCode::Invalid, "missing verified idle turn")); }
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if !crate::teams::store::inbox_available(&transaction, recipient, Some(idle_turn))? { return Ok(None); }
    let live = observed(&transaction)?;
    loop {
        let candidate: Option<(String, String, String, String, String)> = transaction.query_row(
            "SELECT id,recipient_identity,recipient_epoch,chief_epoch,envelope FROM chief_inbox WHERE recipient=?1 AND state='queued' AND next_at<=?2
                AND coalesce(json_extract(envelope,'$.kind'),'') NOT IN ('chief_answer','chief_form_reply','chief_correction','chief_check') ORDER BY rowid LIMIT 1",
            params![recipient.session_id,now_ms], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
        let Some((message_id, identity, epoch, chief_epoch, envelope)) = candidate else { transaction.commit()?; return Ok(None) };
        let identity = serde_json::from_str::<VerifiedSession>(&identity)?;
        let mut envelope = serde_json::from_str::<Value>(&envelope)?;
        let validation = if envelope["kind"]=="chief_message" {
            if identity != *recipient || envelope["expires_at"].as_i64().is_none_or(|deadline|deadline<=now_ms) {
                InboxValidation::Obsolete
            } else {
                let active: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE epoch=?1 AND state='active')",[&chief_epoch],|row|row.get(0))?;
                let launching: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM scheduling_reservations r JOIN legacy_launches l ON l.token=r.token
                    WHERE r.worker=?1 AND l.state IN ('reserved','submitting','uncertain'))",[&recipient.session_id],|row|row.get(0))?;
                // Coordination can resume the current task, but cannot overtake a launch or another native turn.
                let ready = live.iter().any(|worker|worker.session==identity && worker.observed_at_ms<=now_ms && now_ms-worker.observed_at_ms<=30_000
                    && worker.blocked_by.iter().all(|reason|matches!(reason.as_str(),"live_queued_admission_verification_required"|"legacy_work_in_progress")));
                if active && ready && !launching {InboxValidation::Ready} else {InboxValidation::Deferred}
            }
        } else if envelope["kind"]=="chief_wake" {
            let active: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE epoch=?1 AND state='active' AND session_id=?2 AND runtime=?3)",params![chief_epoch,identity.session_id,identity.runtime],|row|row.get(0))?;
            let used_boundary: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_turns WHERE epoch=?1 AND idle_turn=?2)",params![chief_epoch,idle_turn],|row|row.get(0))?;
            let delivered: Option<i64> = transaction.query_row("SELECT delivered_seq FROM chief_coordination WHERE epoch=?1",[&chief_epoch],|row|row.get(0)).optional()?;
            // A re-wake the Chief no longer needs, because it read on before the wake reached it.
            let needed = active && (material_pending(&transaction,delivered.unwrap_or(0))? || needs_unread_retry(&transaction,&chief_epoch)?);
            if active && !needed { InboxValidation::Obsolete }
            else if !active || used_boundary { InboxValidation::Deferred }
            else if wake_ready(&transaction,&identity,&live,now_ms)? {
                let through: i64 = transaction.query_row("SELECT coalesce(max(seq),0) FROM chief_events",[],|row|row.get(0))?;
                envelope["through_seq"] = json!(through);
                let continuation: Option<String> = transaction.query_row("SELECT body FROM chief_events WHERE source='host' AND kind='context_backlog' AND json_extract(body,'$.epoch')=?1 AND seq>(SELECT delivered_seq FROM chief_coordination WHERE epoch=?1) ORDER BY seq DESC LIMIT 1",[&chief_epoch],|row|row.get(0)).optional()?;
                let position = unread_context(&transaction,&chief_epoch)?.0.filter(|position|*position>0 && *position<through);
                envelope["context_continuation"] = match (continuation,position) {
                    (Some(body),_) => serde_json::from_str::<Value>(&body)?,
                    // Without it the wake's after:0 restarts a full snapshot instead of reading on from the Chief's position.
                    (None,Some(position)) => json!({"resume_after":position,"epoch":chief_epoch}),
                    (None,None) => Value::Null,
                };
                envelope["summary_requests"] = json!(transaction.prepare("SELECT id FROM chief_summary_requests WHERE state='queued' AND epoch=?1 ORDER BY created_at LIMIT 1")?
                    .query_map([&chief_epoch],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?);
                transaction.execute("UPDATE chief_inbox SET envelope=?2 WHERE id=?1",params![message_id,envelope.to_string()])?;
                InboxValidation::Ready
            } else { InboxValidation::Deferred }
        } else {
            // Only the coordination wake is deliverable here (answers and corrections go through `claim_reply`); any other
            // queued kind is a leftover and is cancelled.
            InboxValidation::Obsolete
        };
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
        transaction.execute("INSERT INTO chief_delivery_attempts (id,recipient,recipient_runtime,recipient_epoch,kind,envelope,idle_turn,state,started_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'submitting',?8)",
            params![id,recipient.session_id,recipient.runtime,epoch,envelope["kind"].as_str(),envelope.to_string(),idle_turn,now_ms])?;
        crate::teams::store::record_inbox_turn(&transaction, recipient, idle_turn)?;
        transaction.commit()?;
        return Ok(Some(crate::teams::Dispatch { id, message_id, envelope }));
    }
}

/// Hands the reserved Chief session Theo's delivery check, once per idle turn like every other inbox. A
/// check for another runtime or a settled attempt is cancelled, never delivered.
pub fn claim_check(connection: &mut Connection, recipient: &VerifiedSession, idle_turn: &str, now_ms: i64) -> Result<Option<crate::teams::Dispatch>, TeamError> {
    if idle_turn.is_empty() || idle_turn.len() > 128 || idle_turn.chars().any(char::is_control) { return Err(TeamError::new(ErrorCode::Invalid, "missing verified idle turn")); }
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if !crate::teams::store::inbox_available(&transaction, recipient, Some(idle_turn))? { return Ok(None); }
    let candidate: Option<(String, String, String)> = transaction.query_row(
        "SELECT id,recipient_identity,envelope FROM chief_inbox WHERE recipient=?1 AND state='queued' AND json_extract(envelope,'$.kind')='chief_check' ORDER BY rowid LIMIT 1",
        [&recipient.session_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
    let Some((message_id, identity, envelope)) = candidate else { transaction.commit()?; return Ok(None) };
    let envelope: Value = serde_json::from_str(&envelope)?;
    let is_current: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_startup WHERE id=?1 AND state='checking')", [envelope["attempt"].as_str()], |row| row.get(0))?;
    if serde_json::from_str::<VerifiedSession>(&identity)? != *recipient || !is_current {
        transaction.execute("UPDATE chief_inbox SET state='cancelled' WHERE id=?1", [&message_id])?;
        transaction.commit()?;
        return Ok(None);
    }
    let id = format!("chief_{}", crate::teams::store::identifier()?);
    transaction.execute("UPDATE chief_inbox SET state='submitting',attempts=attempts+1 WHERE id=?1", [&message_id])?;
    transaction.execute("INSERT INTO chief_delivery_attempts (id,recipient,recipient_runtime,recipient_epoch,kind,envelope,idle_turn,state,started_at) VALUES (?1,?2,?3,?4,'chief_check',?5,?6,'submitting',?7)",
        params![id, recipient.session_id, recipient.runtime, envelope["attempt"].as_str(), envelope.to_string(), idle_turn, now_ms])?;
    crate::teams::store::record_inbox_turn(&transaction, recipient, idle_turn)?;
    transaction.commit()?;
    Ok(Some(crate::teams::Dispatch { id, message_id, envelope }))
}

/// Readiness reasons a Chief answer may ignore: the answer is meant for a session that just stopped
/// (the idle guard), sits at its own question (`human_input`) and works on a todo (its observed
/// reservation). The osd turn gate already refused permission prompts.
///
/// Only the Chief's own session ever gets a queued-admission verification (Theo's delivery check), so
/// `live_queued_admission_verification_required` is on every worker and would hold every reply
/// forever. A reply needs no such check: only the plugin's receipt under the reply's own stable id
/// settles it as received, and anything else turns it uncertain with a host delivery decision.
const ANSWER_IGNORES: [&str; 4] = ["idle_guard", "human_input", "legacy_work_in_progress", "live_queued_admission_verification_required"];
/// A form reply goes to a session waiting on the very form it settles, whose runtime still runs the
/// turn that asked (`native_busy_at_form`; a plain `native_busy` still holds it).
const FORM_REPLY_IGNORES: [&str; 6] = ["idle_guard", "human_input", "legacy_work_in_progress", "live_queued_admission_verification_required",
    "native_form_pending", "native_busy_at_form"];

/// Hands the stopped session the answer to its stop. `boundary` is the session's current stop
/// boundary (`state:since`, whole seconds) as the daemon sees it, and `idle_turn` the same idle period
/// as every other inbox names it, so one idle period still takes one message.
pub fn claim_answer(connection: &mut Connection, recipient: &VerifiedSession, idle_turn: &str, boundary: &str, now_ms: i64) -> Result<Option<crate::teams::Dispatch>, TeamError> {
    claim_reply(connection, recipient, idle_turn, boundary, "chief_answer", &ANSWER_IGNORES, now_ms)
}

/// Hands a session waiting on its open form the Chief's choice for it, under the same rules as
/// `claim_answer`; the daemon calls it only while the session waits on a form, never a permission.
pub fn claim_form_reply(connection: &mut Connection, recipient: &VerifiedSession, idle_turn: &str, boundary: &str, now_ms: i64) -> Result<Option<crate::teams::Dispatch>, TeamError> {
    claim_reply(connection, recipient, idle_turn, boundary, "chief_form_reply", &FORM_REPLY_IGNORES, now_ms)
}

/// Hands the session Theo's correction of an earlier answer, ready as Theo's own answers are: at any
/// stop of the runtime that got the answer. A restarted session never gets it.
pub fn claim_correction(connection: &mut Connection, recipient: &VerifiedSession, idle_turn: &str, boundary: &str, now_ms: i64) -> Result<Option<crate::teams::Dispatch>, TeamError> {
    claim_reply(connection, recipient, idle_turn, boundary, "chief_correction", &ANSWER_IGNORES, now_ms)
}

fn claim_reply(connection: &mut Connection, recipient: &VerifiedSession, idle_turn: &str, boundary: &str, kind: &str, ignores: &[&str], now_ms: i64) -> Result<Option<crate::teams::Dispatch>, TeamError> {
    if idle_turn.is_empty() || idle_turn.len() > 128 || idle_turn.chars().any(char::is_control) { return Err(TeamError::new(ErrorCode::Invalid, "missing verified idle turn")); }
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if !crate::teams::store::inbox_available(&transaction, recipient, Some(idle_turn))? { return Ok(None); }
    let live = observed(&transaction)?;
    loop {
        let candidate: Option<(String, String, String)> = transaction.query_row(
            "SELECT id,recipient_identity,envelope FROM chief_inbox WHERE recipient=?1 AND state='queued' AND json_extract(envelope,'$.kind')=?2 ORDER BY rowid LIMIT 1",
            params![recipient.session_id, kind], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
        let Some((message_id, identity, envelope)) = candidate else { transaction.commit()?; return Ok(None) };
        let identity = serde_json::from_str::<VerifiedSession>(&identity)?;
        let envelope = serde_json::from_str::<Value>(&envelope)?;
        if kind == "chief_correction" {
            let runtime: Option<String> = transaction.query_row("SELECT runtime FROM chief_overrules WHERE id=?1 AND delivery='queued'", [envelope["overrule"].as_str()], |row| row.get(0)).optional()?;
            if identity != *recipient || runtime.as_ref() != Some(&recipient.runtime) { cancel_correction(&transaction, &message_id, &envelope, now_ms)?; continue; }
        } else {
            let stop: Option<(String, String, String)> = transaction.query_row("SELECT runtime,boundary,state FROM chief_stops WHERE id=?1", [envelope["stop"].as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
            let is_answer_of_this_runtime = identity == *recipient && stop.as_ref().is_some_and(|(runtime, _, state)| state == "answered" && *runtime == recipient.runtime);
            // The session stopped again before the daemon recorded the new stop: the answer waits for it (`carry_queued_answers`).
            if kind == "chief_answer" && is_answer_of_this_runtime && stop.as_ref().is_some_and(|(_, stop_boundary, _)| stop_boundary != boundary) {
                transaction.commit()?;
                return Ok(None);
            }
            let answers_this_stop = is_answer_of_this_runtime && stop.is_some_and(|(_, stop_boundary, _)| stop_boundary == boundary);
            // Theo corrected the session after the Chief wrote this chat answer: the Chief must not contradict him.
            // A form reply is never cancelled here: the correction is claimed only at a chat stop, so the
            // session would stay on its form.
            let is_overtaken: bool = kind == "chief_answer" && envelope["source"] != "theo" && transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_overrules o
                JOIN chief_answers a ON a.id=?1 JOIN chief_stops s ON s.id=a.stop_id
                WHERE o.session_id=s.session_id AND o.created_at>=s.created_at AND o.created_at>a.created_at)", [envelope["answer"].as_str()], |row| row.get(0))?;
            if !answers_this_stop || is_overtaken { cancel_answer(&transaction, &message_id)?; continue; }
        }
        if !answer_ready(&transaction, recipient, &envelope, boundary, ignores, &live, now_ms)? { transaction.commit()?; return Ok(None); }
        let id = format!("chief_{}", crate::teams::store::identifier()?);
        transaction.execute("UPDATE chief_inbox SET state='submitting',attempts=attempts+1 WHERE id=?1", [&message_id])?;
        transaction.execute("INSERT INTO chief_delivery_attempts (id,recipient,recipient_runtime,recipient_epoch,kind,envelope,idle_turn,state,started_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'submitting',?8)",
            params![id, recipient.session_id, recipient.runtime, envelope["epoch"].as_str(), kind, envelope.to_string(), idle_turn, now_ms])?;
        crate::teams::store::record_inbox_turn(&transaction, recipient, idle_turn)?;
        transaction.commit()?;
        return Ok(Some(crate::teams::Dispatch { id, message_id, envelope }));
    }
}

/// For a Chief answer: the answering Chief is still active and the grace since the stop is over. Theo
/// answered on purpose, so his answer needs neither. For both, the daemon's fresh observation of the
/// session finds nothing in the way but what `ignores` lists. A legacy launch on its way to
/// the session still waits. A paused Chief's answer waits for Resume.
fn answer_ready(connection: &Connection, recipient: &VerifiedSession, envelope: &Value, boundary: &str, ignores: &[&str], live: &[WorkerState], now_ms: i64) -> Result<bool, TeamError> {
    if envelope["source"] != "theo" {
        let is_chief_active: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE epoch=?1 AND state='active')", [envelope["epoch"].as_str()], |row| row.get(0))?;
        if !is_chief_active { return Ok(false); }
        let Some(since) = boundary.rsplit_once(':').and_then(|(_, since)| since.parse::<i64>().ok()) else { return Ok(false) };
        // The boundary keeps whole seconds: the stop may have come up to a second after `since`.
        let stopped_by_ms = since.saturating_add(1).saturating_mul(1000);
        if now_ms.saturating_sub(stopped_by_ms) < super::CHIEF_ANSWER_GRACE_S * 1000 { return Ok(false); }
    }
    // A submitted launch keeps its reservation until the todo is done; only one still in flight counts.
    let is_launching: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM scheduling_reservations r JOIN legacy_launches l ON l.token=r.token
        WHERE r.worker=?1 AND r.owner='legacy' AND l.state IN ('reserved','submitting','uncertain'))", [&recipient.session_id], |row| row.get(0))?;
    if is_launching { return Ok(false); }
    Ok(live.iter().any(|worker| worker.session == *recipient && worker.observed_at_ms <= now_ms && now_ms - worker.observed_at_ms <= 30_000
        && worker.blocked_by.iter().all(|reason| ignores.contains(&reason.as_str()))))
}

pub fn owns_dispatch(connection: &Connection, dispatch_id: &str) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_delivery_attempts WHERE id=?1)", [dispatch_id], |row| row.get(0))?)
}

pub fn finish_for(connection: &mut Connection, recipient: &VerifiedSession, dispatch_id: &str, outcome: &crate::teams::TransportOutcome, now_ms: i64) -> Result<(), TeamError> {
    use crate::teams::TransportOutcome;
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let attempt: Option<(String, String, Option<String>, String, String)> = transaction.query_row(
        "SELECT a.state,a.envelope,a.native_id,a.idle_turn,i.recipient_identity FROM chief_delivery_attempts a JOIN chief_inbox i ON i.id=json_extract(a.envelope,'$.message_id') WHERE a.id=?1",
        [dispatch_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
    let (state, envelope, native_id, turn, identity) = attempt.ok_or_else(|| TeamError::new(ErrorCode::NotFound, "delivery attempt not found"))?;
    if serde_json::from_str::<VerifiedSession>(&identity)? != *recipient { return Err(unauthorized()); }
    if let TransportOutcome::Received { native_id: received } = outcome {
        if received.is_empty() || received.len() > 256 || received.chars().any(char::is_control) { return Err(TeamError::new(ErrorCode::Invalid, "invalid native receipt")); }
        if native_id.as_ref().is_some_and(|known| known != received) { return Err(conflict("receipt conflicts with original admission")); }
    }
    if state == "received" || state == "not_submitted" || state == "cancelled" { return Ok(()); }
    let envelope: Value = serde_json::from_str(&envelope)?;
    let message_id = envelope["message_id"].as_str().ok_or_else(|| conflict("missing inbox identity"))?;
    if envelope["kind"]=="chief_message" || envelope["kind"]=="chief_todo" {
        match outcome {
            TransportOutcome::Received {native_id} if state=="submitting" && *native_id==format!("msg_cockpit{message_id}") => {
                transaction.execute("UPDATE chief_delivery_attempts SET state='received',native_id=?2,finished_at=?3 WHERE id=?1",params![dispatch_id,native_id,now_ms])?;
                transaction.execute("UPDATE chief_inbox SET state='received' WHERE id=?1",[message_id])?;
                if envelope["kind"]=="chief_todo" {
                    transaction.execute("UPDATE legacy_launches SET state='submitted',revision=revision+1 WHERE token=?1 AND state='submitting'",[envelope["reservation"].as_str()])?;
                }
                coordination_event(&transaction,"chief_message_received",&json!({"message":message_id,"session":recipient.session_id}),now_ms)?;
            },
            _ => {
                transaction.execute("UPDATE chief_delivery_attempts SET state='uncertain',receipt=?2,finished_at=?3 WHERE id=?1",params![dispatch_id,serde_json::to_string(outcome)?,now_ms])?;
                transaction.execute("UPDATE chief_inbox SET state='uncertain' WHERE id=?1",[message_id])?;
                if envelope["kind"]=="chief_todo" {
                    transaction.execute("UPDATE legacy_launches SET state='uncertain',revision=revision+1 WHERE token=?1 AND state='submitting'",[envelope["reservation"].as_str()])?;
                }
                decision_record(&transaction,&format!("host-delivery-{message_id}"),"delivery","Chief message delivery is uncertain; it will not be resent.","Inspect the exact delivery before settling it.",now_ms)?;
            },
        }
        transaction.commit()?;
        return Ok(());
    }
    if envelope["kind"]=="chief_answer" || envelope["kind"]=="chief_form_reply" || envelope["kind"]=="chief_correction" {
        // The plugin admits an answer or a correction under the stable id `msg_cockpit<message id>` and
        // settles a form as `form:<form id>`; any other receipt is no proof.
        let is_proof = |native_id: &str| if envelope["kind"]=="chief_form_reply" { native_id.starts_with("form:frm_") } else { native_id==format!("msg_cockpit{message_id}") };
        let is_correction = envelope["kind"]=="chief_correction";
        match outcome {
            TransportOutcome::Received {native_id} if state=="submitting" && is_proof(native_id) => {
                transaction.execute("UPDATE chief_delivery_attempts SET state='received',native_id=?2,finished_at=?3 WHERE id=?1",params![dispatch_id,native_id,now_ms])?;
                transaction.execute("UPDATE chief_inbox SET state='received' WHERE id=?1",[message_id])?;
                if is_correction {
                    transaction.execute("UPDATE chief_overrules SET delivery='received',finished_at=?2 WHERE id=?1",params![envelope["overrule"].as_str(),now_ms])?;
                } else {
                    transaction.execute("UPDATE chief_answers SET delivery='received' WHERE id=?1",[envelope["answer"].as_str()])?;
                }
            },
            _ => {
                transaction.execute("UPDATE chief_delivery_attempts SET state='uncertain',receipt=?2,finished_at=?3 WHERE id=?1",params![dispatch_id,serde_json::to_string(outcome)?,now_ms])?;
                transaction.execute("UPDATE chief_inbox SET state='uncertain' WHERE id=?1",[message_id])?;
                if is_correction {
                    transaction.execute("UPDATE chief_overrules SET delivery='uncertain',finished_at=?2 WHERE id=?1 AND delivery<>'received'",params![envelope["overrule"].as_str(),now_ms])?;
                } else {
                    transaction.execute("UPDATE chief_answers SET delivery='uncertain' WHERE id=?1 AND delivery<>'received'",[envelope["answer"].as_str()])?;
                }
                decision_record(&transaction,&format!("host-delivery-{message_id}"),"delivery","Exact native delivery needs host recovery; handling is not verified delivery or completion.","Inspect the exact delivery and recipient. Confirm handled, or explicitly confirm safe stop before cancellation.",now_ms)?;
            },
        }
        transaction.commit()?;
        return Ok(());
    }
    if envelope["kind"]=="chief_check" {
        // Only the stable inbox id proves native admission. Anything else ends this check without a
        // delivery decision: Theo sends another one.
        match outcome {
            TransportOutcome::Received {native_id} if state=="submitting" && *native_id==format!("msg_cockpit{message_id}") => {
                transaction.execute("UPDATE chief_delivery_attempts SET state='received',native_id=?2,finished_at=?3 WHERE id=?1",params![dispatch_id,native_id,now_ms])?;
                transaction.execute("UPDATE chief_inbox SET state='received' WHERE id=?1",[message_id])?;
            },
            _ => {
                transaction.execute("UPDATE chief_delivery_attempts SET state='cancelled',receipt=?2,finished_at=?3 WHERE id=?1",params![dispatch_id,serde_json::to_string(outcome)?,now_ms])?;
                transaction.execute("UPDATE chief_inbox SET state='cancelled' WHERE id=?1",[message_id])?;
            },
        }
        transaction.commit()?;
        return Ok(());
    }
    if envelope["kind"]!="chief_wake" {
        // A leftover from the removed dispatch protocol: its delivery can no longer be acted on.
        transaction.execute("UPDATE chief_delivery_attempts SET state='cancelled',receipt=?2,finished_at=?3 WHERE id=?1", params![dispatch_id,serde_json::to_string(outcome)?,now_ms])?;
        transaction.execute("UPDATE chief_inbox SET state='cancelled' WHERE id=?1", [message_id])?;
        transaction.commit()?;
        return Ok(());
    }
    let epoch = envelope["epoch"].as_str().ok_or_else(||conflict("missing chief epoch"))?;
    match outcome {
        TransportOutcome::Received {native_id} if state=="submitting" && !transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_turns WHERE epoch=?1 AND native_id=?2)",params![epoch,native_id],|row|row.get::<_,bool>(0))? => {
            let active: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_grants WHERE epoch=?1 AND state='active' AND session_id=?2 AND runtime=?3)",params![epoch,recipient.session_id,recipient.runtime],|row|row.get(0))?;
            if active && recipient.agent=="opencode" && admission_verified(&transaction,recipient)? {
                transaction.execute("INSERT OR IGNORE INTO chief_turns(dispatch_id,epoch,idle_turn,native_id,through_seq,admitted_at,read_through)
                    VALUES (?1,?2,?3,?4,?5,?6,(SELECT coalesce(max(read_through),0) FROM chief_turns WHERE epoch=?2))",
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
            decision_record(&transaction,&format!("host-delivery-{message_id}"),"delivery","Exact native delivery needs host recovery; handling is not verified delivery or completion.","Inspect the exact delivery and recipient. Confirm handled to keep ownership, or explicitly confirm safe stop before cancellation.",now_ms)?;
        },
    }
    transaction.commit()?;
    Ok(())
}

pub fn agent(connection: &mut Connection, sender: &VerifiedSession, action: &ChiefAgentAction, live: &[WorkerState], now_ms: i64) -> Result<TeamReply, TeamError> {
    let result = (|| {
        action.validate()?;
        if !exact_live(sender, live) { return Err(unauthorized()); }
        let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        crate::teams::store::sleep::guard_sender(&transaction, &sender.session_id)?;
        if is_product_worker(&transaction, &sender.session_id)? { return Err(unauthorized()); }
        let epoch: Option<String> = transaction.query_row("SELECT epoch FROM chief_grants WHERE session_id=?1 AND agent=?2 AND project=?3 AND runtime=?4 AND state='active'",
            params![sender.session_id, sender.agent, sender.project, sender.runtime], |row| row.get(0)).optional()?;
        let epoch = epoch.ok_or_else(unauthorized)?;
        // A citation the host refuses is a malformed answer, refused like one before the budget is charged.
        if let ChiefAgentAction::Answer { stop, habits, .. } | ChiefAgentAction::AnswerForm { stop, habits, .. } = action {
            require_citable_habits(&transaction, stop, habits)?;
        }
        let budget_turn = match action {
            ChiefAgentAction::Context {..}|ChiefAgentAction::ContextSnapshot {..} => None,
            ChiefAgentAction::Inspect {..} => Some(charge_inspection(&transaction,&epoch)?),
            _ => Some(charge_mutation(&transaction,&epoch)?),
        };
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
            ChiefAgentAction::SummaryResponse {request_id,sections} => {
                let requested: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_summary_requests WHERE id=?1 AND epoch=?2 AND state='pending' AND deadline>?3)",params![request_id,epoch,now_ms],|row|row.get(0))?;
                let linked: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM chief_turns t JOIN chief_delivery_attempts a ON a.id=t.dispatch_id,json_each(a.envelope,'$.summary_requests') r WHERE t.epoch=?1 AND r.value=?2)",params![epoch,request_id],|row|row.get(0))?;
                if !requested || !linked {return Err(conflict("summary response requires an exact pending request admitted to this chief epoch"))}
                let response = json!({"sections":sections,"provenance":"chief_narrative_unverified","request_id":request_id});
                transaction.execute("UPDATE chief_summary_requests SET state='completed',completed_at=?2,response=?3 WHERE id=?1",params![request_id,now_ms,response.to_string()])?;
                json!({"request_id":request_id,"state":"completed"})
            },
            ChiefAgentAction::Decision {key,question,recommendation} => {
                if matches!(key.as_str(),"capacity"|"idle"|"no_approved_work"|"no-approved-work"|"progress"|"handoff") {
                    json!({"status":"silent","reason":key})
                } else {
                    decision_record(&transaction,&format!("chief-{epoch}"),key,question,recommendation,now_ms)?
                }
            },
            ChiefAgentAction::Status { summary } => {
                record_status(&transaction, &epoch, summary, now_ms)?;
                json!({"status": "recorded"})
            },
            ChiefAgentAction::Answer { stop, text, reason, habits } => queue_answer(&transaction, live, AnswerAuthor::Chief { epoch: &epoch, habits }, stop, Reply::Text(text), reason, now_ms)?,
            ChiefAgentAction::AnswerForm { stop, choice, reason, habits } => queue_answer(&transaction, live, AnswerAuthor::Chief { epoch: &epoch, habits }, stop, Reply::FormChoice(choice), reason, now_ms)?,
            ChiefAgentAction::AddHabit { text, project, source_id, reason } => add_habit(&transaction, text, project.as_deref(), source_id, reason, now_ms)?,
            ChiefAgentAction::Escalate { stop, category, reason, suggested_answer } => escalate_stop(&transaction, &epoch, stop, *category, reason, suggested_answer.as_deref(), now_ms)?,
            ChiefAgentAction::StartSession { todo, project, reason } => {
                let todos = crate::todo::TodoList::load_checked().map_err(|error| TeamError::new(ErrorCode::Storage, error))?;
                let starts_per_hour = crate::settings::chief_starts_per_hour().map_err(|error| TeamError::new(ErrorCode::Storage, &error))?;
                request_session_start(&transaction, &todos, &sender.project, &epoch, todo, project, reason, starts_per_hour, now_ms)?
            },
            ChiefAgentAction::GiveTodo { todo, session, reason } => {
                let todos = crate::todo::TodoList::load_checked().map_err(|error| TeamError::new(ErrorCode::Storage, error))?;
                request_todo_gift(&transaction, &todos, live, sender, &epoch, todo, session, reason, now_ms)?
            },
            ChiefAgentAction::Message { session, text, reason } => request_message(&transaction, live, sender, &epoch, session, text, reason, now_ms)?,
            ChiefAgentAction::Inspect { session, todo } => {
                let todos = crate::todo::TodoList::load_checked().map_err(|error| TeamError::new(ErrorCode::Storage, error))?;
                inspect(&transaction, &todos, live, session.as_deref(), todo.as_deref(), now_ms)?
            },
        };
        transaction.commit()?;
        Ok(response)
    })();
    Ok(TeamReply { request_id: "chief".into(), result })
}

// Invalid availability data keeps the existing snapshot-recovery path instead of skipping an event.
fn context_availability_workers(body: &str) -> Option<Vec<Value>> {
    #[derive(serde::Deserialize)]
    struct Availability { workers: Vec<Worker> }
    #[derive(serde::Deserialize)]
    struct Worker {
        session: VerifiedSession,
        role: Option<crate::teams::Role>,
        ready: bool,
        blocked_by: Vec<String>,
    }
    let availability: Availability = serde_json::from_str(body).ok()?;
    Some(availability.workers.into_iter().map(|worker|json!({"session_id":worker.session.session_id,
        "agent":worker.session.agent,"project":worker.session.project,"role":worker.role,
        "ready":worker.ready,"blocked_by":worker.blocked_by})).collect())
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
        context_recovery_snapshot(connection,live,epoch,through,after,limit.min(remaining),now_ms)?
    } else {
        let events = connection.prepare("SELECT seq,source,kind,body FROM chief_events WHERE seq>?1 AND seq<=?2 ORDER BY seq LIMIT ?3")?
            .query_map(params![after,through,limit.min(remaining) as i64],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?)))?
            .collect::<Result<Vec<_>,_>>()?;
        let mut records = Vec::new();
        let mut needs_snapshot = false;
        for (sequence,origin,kind,body) in events {
            if origin=="host" && kind=="availability_changed" {
                let Some(workers) = context_availability_workers(&body) else {needs_snapshot=true; break;};
                let record = json!({"sequence":sequence,"source":"daemon_derived_availability","kind":kind,
                    "complete_product_roster":true,"workers":workers});
                if serde_json::to_vec(&record)?.len()+3>=super::CONTEXT_BYTES {needs_snapshot=true; break;}
                records.push(record);
                continue;
            }
            if origin=="host" && kind=="todo_completed" {
                let identifiers: Value = serde_json::from_str(&body)?;
                let Some(mut record) = identifiers["completion"].as_str().map(|completion|todo_completion_record(connection,completion)).transpose()?.flatten()
                    else {needs_snapshot=true; break;};
                // A snapshot frozen below this event may have returned it already: say so instead of repeating it.
                if record["delivered"]==true {
                    record = json!({"kind":"todo_completed","source":"host","completion":record["completion"],"todo":record["todo"],"already_delivered":true});
                }
                record.as_object_mut().expect("completion record is an object").remove("delivered");
                record["sequence"] = json!(sequence);
                records.push(record);
                continue;
            }
            let resolved_with_answer = kind=="decision_resolved" && serde_json::from_str::<Value>(&body).is_ok_and(|identifiers|identifiers.get("answer").is_some());
            if origin=="host" && (resolved_with_answer || matches!(kind.as_str(),"stop_detected"|"chief_message_requested"|"chief_message_received")) {
                let Some(snapshot_records) = event_snapshot_records(connection,&kind,&body)? else {needs_snapshot=true; break;};
                for mut record in snapshot_records {
                    // Once per page is enough for the stops after it.
                    if record["kind"]=="habits" && records.iter().any(|kept: &Value|kept["kind"]=="habits") {continue;}
                    record["sequence"] = json!(sequence);
                    if serde_json::to_vec(&record)?.len()+3>=super::CONTEXT_BYTES {needs_snapshot=true; break;}
                    records.push(record);
                }
                if needs_snapshot {break;}
                continue;
            }
            let mut decision_id: Option<String> = None;
            let mut decision_revision: Option<i64> = None;
            if origin=="host" && kind=="decision_resolved" {
                let identifiers: Value = serde_json::from_str(&body)?;
                decision_id=identifiers["decision"].as_str().filter(|id|super::key(id).is_ok()).map(str::to_owned);
                decision_revision=identifiers["decision_revision"].as_i64().filter(|revision|*revision>0);
                // Theo's answer and its id are only in the snapshot's decision record: the Chief needs both to learn a habit from it.
                needs_snapshot=decision_id.is_none() || decision_revision.is_none() || identifiers.get("answer").is_some();
            } else {
                needs_snapshot=!matches!(kind.as_str(),"status"|"context_backlog"|"habit_changed");
            }
            if needs_snapshot {break;}
            let source = if origin=="host" {"host"} else {"agent_report_not_evidence"};
            records.push(json!({"sequence":sequence,"source":source,"kind":kind,"decision_id":decision_id,"decision_revision":decision_revision}));
        }
        if needs_snapshot {context_recovery_snapshot(connection,live,epoch,through,after,limit.min(remaining),now_ms)?}
        else {
            let mut page = super::context_page(&records,limit.min(remaining))?;
            let cut = page.as_array_mut().expect("context page is an array");
            // Habits go with the stop after them: a page ending between the two ends before both instead.
            if cut.len()<records.len() && cut.last().is_some_and(|record|record["kind"]=="habits") {cut.pop();}
            if cut.is_empty() && !records.is_empty() {context_recovery_snapshot(connection,live,epoch,through,after,limit.min(remaining),now_ms)?}
            else {
                let next = cut.last().and_then(|record|record["sequence"].as_i64()).unwrap_or(after);
                json!({"records":page,"next":next,"has_more":next<through,"snapshot_required":false})
            }
        }
    };
    result["bootstrap_pending"] = json!(turn.is_none());
    acknowledge_completions(connection,epoch,&result["records"],now_ms)?;
    if let Some((dispatch,_,_,_)) = turn {
        let count = result["records"].as_array().map_or(0,Vec::len);
        connection.execute("UPDATE chief_turns SET records=records+?2 WHERE dispatch_id=?1",params![dispatch,count as i64])?;
        let read = if result["has_more"]==false {Some(through)} else if result["snapshot_required"]==false {result["next"].as_i64()} else {None};
        if let Some(read) = read {record_read(connection,&dispatch,read)?;}
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

fn context_recovery_snapshot(connection: &Connection, live: &[WorkerState], epoch: &str, event_cursor: i64, after: i64, limit: usize, now_ms: i64) -> Result<Value, TeamError> {
    let (generation,records) = freeze_context(connection,live,epoch,event_cursor,now_ms)?;
    let mut result = context_snapshot(&records,0,limit)?;
    result["snapshot_required"] = json!(true);
    result["snapshot_next"] = result["next"].clone();
    result["snapshot_generation"] = json!(generation);
    result["next"] = json!(after);
    Ok(result)
}

fn review_generation(connection: &Connection, live: &[WorkerState], now_ms: i64) -> Result<String, TeamError> {
    let revision: i64 = connection.query_row("SELECT revision FROM chief_review_generation WHERE id=1",[],|row|row.get(0))?;
    Ok(crate::audit::sha256_hex(&serde_json::to_vec(&(revision,startup(connection)?,material_workers(live,now_ms)))?))
}

fn bounded_chief_snapshot(connection: &Connection, live: &[WorkerState], epoch: &str, after: i64, limit: usize, generation: Option<&str>, now_ms: i64) -> Result<Value, TeamError> {
    super::validate_cursor(after,limit)?;
    let turn = current_turn(connection,epoch)?;
    let (generation,event_cursor,records) = match generation {
        None if after==0 => {
            let head: i64 = connection.query_row("SELECT coalesce(max(seq),0) FROM chief_events",[],|row|row.get(0))?;
            let event_cursor = turn.as_ref().map_or(head,|(_,through,_,_)|*through);
            let (generation,records) = freeze_context(connection,live,epoch,event_cursor,now_ms)?;
            (generation,event_cursor,records)
        },
        None => return Ok(invalidated_snapshot()),
        Some(expected) => {
            let frozen: Option<(i64,String)> = connection.query_row("SELECT event_cursor,records FROM chief_context_snapshots WHERE epoch=?1 AND generation=?2",
                params![epoch,expected],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
            let Some((event_cursor,encoded)) = frozen else {return Ok(invalidated_snapshot())};
            (expected.to_owned(),event_cursor,serde_json::from_str::<Vec<Value>>(&encoded)?)
        },
    };
    let remaining = context_remaining(connection,epoch,turn.as_ref())?;
    if remaining==0 {return Err(TeamError::new(ErrorCode::Limit,"chief turn event-record budget exhausted"))}
    let mut page = context_snapshot(&records,after,limit.min(remaining))?;
    page["event_cursor"] = json!(event_cursor);
    page["generation"] = json!(generation);
    page["cursor_kind"] = json!("snapshot_offset");
    page["bootstrap_pending"] = json!(turn.is_none());
    acknowledge_completions(connection,epoch,&page["records"],now_ms)?;
    if let Some((dispatch,_,_,_)) = turn {
        let count = page["records"].as_array().map_or(0,Vec::len);
        connection.execute("UPDATE chief_turns SET records=records+?2 WHERE dispatch_id=?1",params![dispatch,count as i64])?;
        if page["has_more"]==false {record_read(connection,&dispatch,event_cursor)?;}
        if count==remaining && page["has_more"]==true {context_continuation(connection,&dispatch,&json!({"snapshot_after":page["next"],"generation":page["generation"]}),now_ms)?;}
    } else {connection.execute("UPDATE chief_coordination SET preview_records=preview_records+?2 WHERE epoch=?1",params![epoch,page["records"].as_array().map_or(0,Vec::len) as i64])?;}
    Ok(page)
}

fn context_remaining(connection: &Connection, epoch: &str, turn: Option<&(String,i64,i64,i64)>) -> Result<usize, TeamError> {
    let records = if let Some((_,_,_,records)) = turn {*records}
        else {connection.query_row("SELECT preview_records FROM chief_coordination WHERE epoch=?1",[epoch],|row|row.get::<_,i64>(0))?};
    Ok(50i64.saturating_sub(records).max(0) as usize)
}

fn record_read(connection: &Connection, dispatch: &str, read_through: i64) -> Result<(), TeamError> {
    connection.execute("UPDATE chief_turns SET read_through=max(coalesce(read_through,0),?2) WHERE dispatch_id=?1",params![dispatch,read_through])?;
    Ok(())
}

/// Admitted Context turns re-woken without new material events before the host stops and asks Theo.
const UNREAD_CONTEXT_TURNS: usize = 3;

/// The Chief's read position, and how many of its latest turns in a row ended before it read the
/// Context they admitted while record budget was left. A turn that spent its whole budget made progress.
fn unread_context(connection: &Connection, epoch: &str) -> Result<(Option<i64>, usize), TeamError> {
    let turns = connection.prepare("SELECT through_seq,records,read_through FROM chief_turns WHERE epoch=?1 ORDER BY rowid DESC LIMIT ?2")?
        .query_map(params![epoch,UNREAD_CONTEXT_TURNS as i64],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,Option<i64>>(2)?)))?
        .collect::<Result<Vec<_>,_>>()?;
    let position = turns.first().and_then(|(_,_,read)|*read);
    let stalled = turns.iter().take_while(|(through,records,read)|read.is_some_and(|read|read<*through) && *records<50).count();
    Ok((position,stalled))
}

fn latest_turn_behind(connection: &Connection, epoch: &str) -> Result<bool, TeamError> {
    Ok(connection.query_row("SELECT read_through<through_seq FROM chief_turns WHERE epoch=?1 ORDER BY rowid DESC LIMIT 1",[epoch],
        |row|row.get::<_,Option<bool>>(0)).optional()?.flatten().unwrap_or(false))
}

fn context_continuation(connection: &Connection, dispatch: &str, continuation: &Value, now_ms: i64) -> Result<(), TeamError> {
    let epoch: String = connection.query_row("SELECT epoch FROM chief_turns WHERE dispatch_id=?1",[dispatch],|row|row.get(0))?;
    let mut continuation = continuation.clone();
    continuation["epoch"] = json!(epoch);
    connection.execute("INSERT OR IGNORE INTO chief_events(id,source,submission_key,fingerprint,kind,body,created_at) VALUES (?1,'host',?2,?3,'context_backlog',?4,?5)",
        params![crate::teams::store::identifier()?,format!("backlog-{dispatch}"),crate::audit::sha256_hex(&serde_json::to_vec(&continuation)?),continuation.to_string(),now_ms])?;
    Ok(())
}

fn invalidated_snapshot() -> Value {
    json!({"records":[],"snapshot_required":true,"invalidated":true,"next":0,"generation":null})
}

/// Pages after the first are served from this frozen copy, so live churn between turns cannot invalidate them;
/// the Chief catches up on later changes through the event feed from the frozen event_cursor.
/// Only the newest snapshot is kept: one active Chief grant at a time, and a new freeze retires the old copy.
fn freeze_context(connection: &Connection, live: &[WorkerState], epoch: &str, event_cursor: i64, now_ms: i64) -> Result<(String, Vec<Value>), TeamError> {
    let records = context_records(connection,live)?;
    let encoded = serde_json::to_string(&records)?;
    let generation = crate::teams::store::identifier()?;
    connection.execute("DELETE FROM chief_context_snapshots",[])?;
    connection.execute("INSERT INTO chief_context_snapshots(epoch,generation,event_cursor,records,created_at) VALUES (?1,?2,?3,?4,?5)",
        params![epoch,generation,event_cursor,encoded,now_ms])?;
    Ok((generation,records))
}

pub(crate) fn ensure_context_snapshots(connection: &Connection) -> Result<(), TeamError> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS chief_context_snapshots (
        epoch TEXT PRIMARY KEY, generation TEXT NOT NULL, event_cursor INTEGER NOT NULL, records TEXT NOT NULL, created_at INTEGER NOT NULL);")?;
    // `read_through`: the event sequence the Chief has read Context through, as of this turn. NULL only on
    // turns admitted before this column existed: their progress is unknown, so they never count as stalled.
    // Later turns start from the epoch's furthest read, or 0 when there is none.
    let columns = connection.prepare("PRAGMA table_info(chief_turns)")?.query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|name| name == "read_through") { connection.execute("ALTER TABLE chief_turns ADD COLUMN read_through INTEGER", [])?; }
    Ok(())
}

/// Created on every open: one row per result of a todo a Chief filed, keyed by the todo and the
/// hash of its result, so a summary added after the item was done arrives once more and nothing else
/// twice. `delivered_*` is the receipt: set once a Chief's Context returned the record.
pub(crate) fn ensure_todo_completions(connection: &Connection) -> Result<(), TeamError> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS chief_todo_completions (
        id TEXT PRIMARY KEY, todo_id TEXT NOT NULL, result_hash TEXT NOT NULL, project TEXT NOT NULL, title TEXT NOT NULL,
        result TEXT NOT NULL, blocked_report INTEGER NOT NULL, filed_epoch TEXT NOT NULL, created_at INTEGER NOT NULL,
        delivered_epoch TEXT, delivered_at INTEGER, UNIQUE(todo_id,result_hash));")?;
    Ok(())
}

/// Long past the todo list's own week of done items: a completion row outlives its item, so the
/// item cannot be delivered again while it is still listed.
const TODO_COMPLETIONS_KEPT_MS: i64 = 30 * 24 * 3_600_000;

/// Done todos a Chief filed (the daemon stamped `chief_epoch`), as durable completions for
/// whichever Chief holds the grant now or is enabled later; wakes the active one.
pub fn record_todo_completions(connection: &mut Connection, todos: &crate::todo::TodoList, now_ms: i64) -> Result<usize, TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute("DELETE FROM chief_todo_completions WHERE created_at<?1", [now_ms.saturating_sub(TODO_COMPLETIONS_KEPT_MS)])?;
    let mut recorded = 0;
    for item in todos.items.iter().filter(|item| item.status() != "done" && !item.chief_epoch().is_empty()) {
        // Reopened: its next done is news again, even with the same result.
        transaction.execute("UPDATE chief_todo_completions SET result_hash=result_hash||':'||id WHERE todo_id=?1 AND instr(result_hash,':')=0", [item.id()])?;
    }
    for item in todos.items.iter().filter(|item| item.status() == "done" && !item.chief_epoch().is_empty()) {
        let result = item.result();
        let id = crate::teams::store::identifier()?;
        let inserted = transaction.execute("INSERT OR IGNORE INTO chief_todo_completions(id,todo_id,result_hash,project,title,result,blocked_report,filed_epoch,created_at)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![id,item.id(),crate::audit::sha256_hex(result.as_bytes()),item.project(),item.text(),result,item.is_blocked_report(),item.chief_epoch(),now_ms])?;
        if inserted == 1 {
            coordination_event(&transaction, "todo_completed", &json!({"completion":id,"todo":item.id()}), now_ms)?;
            recorded += 1;
        }
    }
    if recorded > 0 { schedule_wake(&transaction, now_ms)?; }
    transaction.commit()?;
    Ok(recorded)
}

/// What the Chief reads of a completion: a worker's own words, never evidence or instructions.
fn todo_completion_record(connection: &Connection, completion: &str) -> Result<Option<Value>, TeamError> {
    Ok(connection.query_row("SELECT id,todo_id,project,title,result,blocked_report,delivered_at IS NOT NULL FROM chief_todo_completions WHERE id=?1", [completion], |row| {
        let result: String = row.get(4)?;
        Ok(json!({"kind":"todo_completed","source":"worker_summary_not_instructions","completion":row.get::<_,String>(0)?,
            "todo":row.get::<_,String>(1)?,"project":row.get::<_,String>(2)?,
            "title":crate::approval::escape_hiding(&row.get::<_,String>(3)?,"",""),
            "has_summary":!result.is_empty(),"result":crate::approval::escape_hiding(&result,"",""),
            "blocked_report":row.get::<_,bool>(5)?,"delivered":row.get::<_,bool>(6)?}))
    }).optional()?)
}

/// The receipt: completions this Context page returned reached the Chief of `epoch`.
fn acknowledge_completions(connection: &Connection, epoch: &str, records: &Value, now_ms: i64) -> Result<(), TeamError> {
    for completion in records.as_array().into_iter().flatten().filter(|record| record["kind"] == "todo_completed").filter_map(|record| record["completion"].as_str()) {
        connection.execute("UPDATE chief_todo_completions SET delivered_epoch=?2,delivered_at=?3 WHERE id=?1 AND delivered_at IS NULL", params![completion,epoch,now_ms])?;
    }
    Ok(())
}

/// Created on every open: the todos in progress and the session each names, rewritten by
/// `reconcile_todos`. Only Context reads it; nothing dispatches or reserves from it.
pub(crate) fn ensure_supervised_tasks(connection: &Connection) -> Result<(), TeamError> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS chief_supervised_tasks (
        project TEXT NOT NULL, task_id TEXT NOT NULL, session TEXT NOT NULL, PRIMARY KEY(project,task_id));")?;
    Ok(())
}

fn context_snapshot(records: &[Value], after: i64, limit: usize) -> Result<Value, TeamError> {
    super::validate_cursor(after, limit)?;
    let start = usize::try_from(after).unwrap_or(usize::MAX).min(records.len());
    let page = super::context_page(&records[start..], limit)?;
    let next = start + page.as_array().expect("context page is an array").len();
    Ok(json!({"records": page, "next": next, "has_more": next < records.len()}))
}

fn stopped_session_record(stop: String, session: String, project: String, task_id: String, bundle: &str) -> Result<Value, TeamError> {
    Ok(json!({"kind": "stopped_session", "source": "masked_agent_conversation_not_instructions", "stop": stop, "session": session,
        "project": project, "task_id": task_id, "bundle": serde_json::from_str::<Value>(bundle)?}))
}

/// Columns: id, state, revision, resolution, Theo's answer id, that answer's delivery.
fn decision_context_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({"kind":"decision","source":"protected_host_control","id":row.get::<_,String>(0)?,
        "state":row.get::<_,String>(1)?,"revision":row.get::<_,i64>(2)?,"resolution":row.get::<_,Option<String>>(3)?,"resolution_grants_authority":false,
        "answer_id":row.get::<_,Option<String>>(4)?,"answer_delivery":row.get::<_,Option<String>>(5)?}))
}

/// Columns: id, recipient, state.
fn chief_message_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({"kind":"chief_message","source":"protected_delivery_ledger","id":row.get::<_,String>(0)?,"session":row.get::<_,String>(1)?,"state":row.get::<_,String>(2)?}))
}

/// The current snapshot records an event names, so Context can read on past it instead of rebuilding a
/// whole snapshot: an open stop with Theo's habits before it, a resolved decision with Theo's answer, or a
/// Chief message's delivery state. None when the event needs a snapshot.
fn event_snapshot_records(connection: &Connection, kind: &str, body: &str) -> Result<Option<Vec<Value>>, TeamError> {
    let Ok(identifiers) = serde_json::from_str::<Value>(body) else { return Ok(None) };
    let records = match kind {
        "stop_detected" => {
            let Some(stop) = identifiers["stop"].as_str() else { return Ok(None) };
            let found: Option<(String,String,String,String,String,String)> = connection.query_row("SELECT id,session_id,project,task_id,bundle,state FROM chief_stops WHERE id=?1",[stop],
                |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional()?;
            let Some((stop,session,project,task_id,bundle,state)) = found else { return Ok(None) };
            // A stop already settled is no longer actionable; a snapshot would leave it out entirely.
            if state!="open" { return Ok(Some(vec![json!({"kind":"stop_settled","source":"host","stop":stop,"state":state})])) }
            vec![habits_record(connection)?,stopped_session_record(stop,session,project,task_id,&bundle)?]
        },
        "decision_resolved" => {
            let Some(decision) = identifiers["decision"].as_str() else { return Ok(None) };
            let Some(record) = connection.query_row("SELECT d.id,d.state,d.revision,d.resolution,a.id,a.delivery FROM chief_decisions d
                LEFT JOIN chief_answers a ON a.id=(SELECT id FROM chief_answers WHERE stop_id=d.stop_id AND source='theo' ORDER BY created_at DESC,rowid DESC LIMIT 1)
                WHERE d.id=?1",[decision],decision_context_record).optional()? else { return Ok(None) };
            vec![record]
        },
        "chief_message_requested" | "chief_message_received" => {
            let Some(message) = identifiers[if kind=="chief_message_requested" {"request"} else {"message"}].as_str() else { return Ok(None) };
            // A todo gift's delivery has no message record of its own: the snapshot shows it in the dispatch ledger.
            let Some(record) = connection.query_row("SELECT id,recipient,state FROM chief_inbox WHERE id=?1 AND json_extract(envelope,'$.kind')='chief_message'",
                [message],chief_message_record).optional()? else { return Ok(None) };
            vec![record]
        },
        _ => return Ok(None),
    };
    Ok(Some(records))
}

fn context_records(connection: &Connection, live: &[WorkerState]) -> Result<Vec<Value>, TeamError> {
    let mut records = Vec::new();
    let mut statement = connection.prepare("SELECT session_id,epoch,state FROM chief_grants WHERE state!='disabled' ORDER BY id")?;
    records.extend(statement.query_map([], |row| Ok(json!({"kind": "chief", "session_id": row.get::<_, String>(0)?, "epoch": row.get::<_, String>(1)?, "state": row.get::<_, String>(2)?})))?.collect::<Result<Vec<_>, _>>()?);
    // Right after the chief record: a turn reads at most 50 records and a stop is what it is woken for.
    let stops = connection.prepare("SELECT id,session_id,project,task_id,bundle FROM chief_stops WHERE state='open' ORDER BY created_at,id")?
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    if !stops.is_empty() { records.push(habits_record(connection)?); }
    for (stop, session, project, task_id, bundle) in stops {
        records.push(stopped_session_record(stop, session, project, task_id, &bundle)?);
    }
    if let Some(record) = supervised_tasks_record(connection, live)? { records.push(record); }
    for worker in live {
        records.push(json!({"kind": "worker", "session_id": worker.session.session_id, "agent": worker.session.agent,
            "project": worker.session.project, "role": worker.role, "ready": worker.ready, "blocked_by": worker.blocked_by}));
    }
    // `answer_id` names Theo's answer to an escalated stop, so the Chief can give it as a habit's source.
    // `answer_delivery` says whether that answer reached the session: a resolved escalation alone does not.
    records.extend(connection.prepare("WITH latest AS (SELECT d.id,d.state,d.revision,d.resolution,
            (SELECT a.id FROM chief_answers a WHERE a.stop_id=d.stop_id AND a.source='theo' ORDER BY a.created_at DESC,a.rowid DESC LIMIT 1) AS answer,d.created_at,d.resolved_at
            FROM chief_decisions d WHERE d.state='open' OR d.id IN (
                SELECT id FROM chief_decisions WHERE state='resolved'
                ORDER BY resolved_at DESC,created_at DESC,id DESC LIMIT 20))
            SELECT l.id,l.state,l.revision,l.resolution,l.answer,(SELECT delivery FROM chief_answers WHERE id=l.answer) FROM latest l
            ORDER BY (l.state='open') DESC,
                CASE WHEN l.state='open' THEN l.created_at ELSE l.resolved_at END DESC,l.created_at DESC,l.id DESC")?
        .query_map([],decision_context_record)?
        .collect::<Result<Vec<_>,_>>()?);
    for record in &mut records {
        if record.get("source").is_none() {record["source"]=json!(if record["kind"]=="worker" {"daemon_derived_availability"} else {"protected_host_approval"});}
    }
    // Every unread one, uncapped: the snapshot's event cursor passes their events, so one left out
    // here would wait for the next snapshot with nothing to wake the Chief for it.
    let pending = connection.prepare("SELECT id FROM chief_todo_completions WHERE delivered_at IS NULL ORDER BY created_at,rowid")?
        .query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    for completion in pending {
        if let Some(mut record) = todo_completion_record(connection, &completion)? {
            record.as_object_mut().expect("completion record is an object").remove("delivered");
            records.push(record);
        }
    }
    records.extend(connection.prepare("SELECT id,recipient,state FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_message'
        AND (state IN ('queued','submitting','uncertain','failed') OR rowid IN (SELECT rowid FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_message' ORDER BY rowid DESC LIMIT 20)) ORDER BY rowid")?
        .query_map([],chief_message_record)?
        .collect::<Result<Vec<_>,_>>()?);
    records.extend(connection.prepare("SELECT id,task_id,worker,state FROM chief_session_starts WHERE worker IS NOT NULL
        AND (state='requested' OR rowid IN (SELECT rowid FROM chief_session_starts WHERE worker IS NOT NULL ORDER BY rowid DESC LIMIT 20)) ORDER BY rowid")?
        .query_map([],|row|Ok(json!({"kind":"todo_gift","source":"protected_dispatch_ledger","id":row.get::<_,String>(0)?,"todo":row.get::<_,String>(1)?,"session":row.get::<_,String>(2)?,"state":row.get::<_,String>(3)?})))?
        .collect::<Result<Vec<_>,_>>()?);
    Ok(records)
}

const SUPERVISED_TASKS_LISTED_MAX: usize = 20;
/// The record's serialised size, so it always fits a Context page beside a stopped session.
const SUPERVISED_TASKS_RECORD_BYTES: usize = 12_000;
const SUPERVISED_TEXT_MAX_CHARS: usize = 200;
const OWNER_DELIVERY_KINDS: &str = "'chief_message','chief_todo','chief_answer','chief_form_reply','chief_correction'";

/// One read-only record of the todos in progress, so a Chief can follow work past the end of its
/// owner's turn or session. Owners that need attention come first: `missing` (the todo names no
/// usable session), `gone` (its session is not live), `stopped` (live and not working, a question
/// form included), then `busy`. `paused` marks an owner put to sleep on purpose. `stop` and
/// `delivery` are the owner's latest stop and the latest Chief or Theo delivery to it. Present only
/// when some todo is in progress; todos that do not fit are counted in `omitted`.
///
/// Project, todo and owner come from the todo list, which agents write: the owner must look like a
/// session id and the text is escaped and bounded.
fn supervised_tasks_record(connection: &Connection, live: &[WorkerState]) -> Result<Option<Value>, TeamError> {
    let rows = connection.prepare("SELECT project,task_id,session FROM chief_supervised_tasks")?
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.is_empty() { return Ok(None); }
    let mut ranked = Vec::new();
    for (project, task, owner) in rows {
        let owner = Some(owner).filter(|owner| (1..=64).contains(&owner.len())
            && owner.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')));
        let stop: Option<(String, String)> = match &owner {
            None => None,
            Some(owner) => connection.query_row("SELECT id,state FROM chief_stops WHERE session_id=?1 AND state<>'superseded' AND task_id IN (?2,'')
                ORDER BY created_at DESC,rowid DESC LIMIT 1", params![owner, task], |row| Ok((row.get(0)?, row.get(1)?))).optional()?,
        };
        let worker = owner.as_ref().and_then(|owner| live.iter().find(|worker| worker.session.session_id == *owner));
        let state = match worker {
            _ if owner.is_none() => "missing",
            None => "gone",
            Some(_) if stop.as_ref().is_some_and(|(_, state)| state == "open") => "stopped",
            Some(worker) if worker.blocked_by.iter().any(|reason| reason == "busy" || reason == "native_busy") => "busy",
            Some(_) => "stopped",
        };
        let rank = ["missing", "gone", "stopped", "busy"].iter().position(|known| *known == state).expect("listed owner state");
        ranked.push((rank, project, task, owner, state, stop));
    }
    ranked.sort_by(|a, b| (a.0, &a.1, &a.2).cmp(&(b.0, &b.1, &b.2)));
    let total = ranked.len();
    let mut record = json!({"kind": "supervised_tasks", "source": "todo_list_and_protected_ledgers_read_only_grants_no_authority", "tasks": []});
    // Room for the `omitted` count the record may need.
    let mut bytes = serde_json::to_vec(&record)?.len() + 32;
    let mut asleep: Option<Vec<String>> = None;
    let mut listed = Vec::new();
    for (_, project, task, owner, state, stop) in ranked.into_iter().take(SUPERVISED_TASKS_LISTED_MAX) {
        let mut paused = false;
        let mut delivery: Option<(String, String, String)> = None;
        if let Some(owner) = &owner {
            paused = crate::teams::store::sleep::paused(connection, owner)?
                || (state == "gone" && asleep.get_or_insert_with(|| crate::sleep::asleep().into_iter().map(|note| note.session).collect()).contains(owner));
            delivery = connection.query_row(&format!("SELECT id,json_extract(envelope,'$.kind'),state FROM chief_inbox WHERE recipient=?1
                AND json_extract(envelope,'$.kind') IN ({OWNER_DELIVERY_KINDS}) ORDER BY rowid DESC LIMIT 1"),
                [owner], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
        }
        let bounded = |text: &str| crate::approval::escape_hiding(&text.chars().take(SUPERVISED_TEXT_MAX_CHARS).collect::<String>(), "", "");
        let task = json!({"todo": bounded(&task), "project": bounded(&project), "owner": owner, "owner_state": state, "paused": paused,
            "stop": stop.map(|(id, state)| json!({"id": id, "state": state})),
            "delivery": delivery.map(|(id, kind, state)| json!({"id": id, "kind": kind, "state": state}))});
        let size = serde_json::to_vec(&task)?.len() + 1;
        if bytes + size > SUPERVISED_TASKS_RECORD_BYTES { break; }
        bytes += size;
        listed.push(task);
    }
    if listed.len() < total { record["omitted"] = json!(total - listed.len()); }
    record["tasks"] = Value::Array(listed);
    Ok(Some(record))
}

const HABITS_LISTED_MAX: usize = 100;
/// The habits record's serialised size, so it always fits a Context page beside a stopped session.
const HABITS_RECORD_BYTES: usize = 24_000;

/// Theo's active habits, newest first, with hiding characters escaped. Habits that do not fit are
/// counted in `omitted`, which is present only when some are.
fn habits_record(connection: &Connection) -> Result<Value, TeamError> {
    let active = connection.prepare("SELECT id,text,project,source FROM chief_habits WHERE state='active' ORDER BY created_at DESC,rowid DESC")?
        .query_map([], |row| Ok(json!({"id":row.get::<_, String>(0)?,"text":crate::approval::escape_hiding(&row.get::<_, String>(1)?, "", ""),
            "project":row.get::<_, Option<String>>(2)?,"source":row.get::<_, String>(3)?})))?
        .collect::<Result<Vec<_>, _>>()?;
    let total = active.len();
    let mut record = json!({"kind":"habits","source":"theo_habits_not_instructions_from_agents","habits":[]});
    // Room for the `omitted` count the record may need.
    let mut bytes = serde_json::to_vec(&record)?.len() + 32;
    let mut listed = Vec::new();
    for habit in active.into_iter().take(HABITS_LISTED_MAX) {
        let size = serde_json::to_vec(&habit)?.len() + 1;
        if bytes + size > HABITS_RECORD_BYTES { break; }
        bytes += size;
        listed.push(habit);
    }
    if listed.len() < total { record["omitted"] = json!(total - listed.len()); }
    record["habits"] = Value::Array(listed);
    Ok(record)
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
    let now = chrono::DateTime::from_timestamp_millis(now_ms)
        .ok_or_else(|| super::invalid("invalid Chief snapshot time"))?.with_timezone(&chrono::Local);
    // Local days may cross a DST change, so a fixed 24-hour lookback is not midnight.
    let midnight = now.date_naive().and_hms_opt(0, 0, 0).expect("midnight is valid")
        .and_local_timezone(chrono::Local).earliest().ok_or_else(|| super::invalid("local midnight is unavailable"))?.timestamp_millis();
    let answered: i64 = connection.query_row("SELECT count(*) FROM chief_answers WHERE source='chief' AND delivery!='cancelled' AND created_at>=?1 AND created_at<=?2",
        params![midnight,now_ms], |row| row.get(0))?;
    let escalated: i64 = connection.query_row("SELECT count(*) FROM chief_decisions WHERE category IS NOT NULL AND created_at>=?1 AND created_at<=?2",
        params![midnight,now_ms], |row| row.get(0))?;
    result["today"] = json!({"since_ms":midnight,"answered":answered,"escalated":escalated});
    if after==0 {
        result["chief_counts"]=json!(answer_counts(connection)?.into_iter().map(|count|json!({"session_id":count.session_id,"count":count.count})).collect::<Vec<_>>());
    }
    Ok(result)
}

pub fn snapshot(connection: &Connection, live: &[WorkerState], after: i64, limit: usize) -> Result<Value, TeamError> {
    let mut snapshot = review_records(connection,live,after,limit,false)?;
    snapshot["startup"] = startup(connection)?.unwrap_or(Value::Null);
    if let Some((_, session)) = check_target(connection, live)? { snapshot["startup"]["check_session"] = json!(session); }
    Ok(snapshot)
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
    let mut statement = connection.prepare("WITH records(category,identity,record) AS (
        SELECT 0,id,json_object('kind','chief','session',json_object('session_id',session_id,'agent',agent,'project',project,'runtime',runtime),
            'session_id',session_id,'epoch',epoch,'revision',revision,'state',state,'approval_hash',approval_hash,'created_at_ms',created_at,
            'live_verified',EXISTS(SELECT 1 FROM json_each(?1) WHERE json_extract(value,'$.session.session_id')=chief_grants.session_id
                AND json_extract(value,'$.session.runtime')=chief_grants.runtime AND json_extract(value,'$.session.project')=chief_grants.project
                AND json_extract(value,'$.session.agent')=chief_grants.agent)) FROM chief_grants WHERE ?4 OR state!='disabled'
        UNION ALL SELECT 1,json_extract(value,'$.session.session_id'),json_object('kind','worker','session',json_extract(value,'$.session'),
            'session_id',json_extract(value,'$.session.session_id'),'role',json_extract(value,'$.role'),'ready',json_extract(value,'$.ready'),
            'blocked_by',json_extract(value,'$.blocked_by'),'observed_at_ms',json_extract(value,'$.observed_at_ms'),
            'observation_provenance','daemon_derived_availability') FROM json_each(?1)
        UNION ALL SELECT 3,m.session_id,json_object('kind','team_member','task',json_object('project',t.project,'task_id',t.task_id),'session',json_object('session_id',m.session_id,
            'agent',m.agent,'runtime',m.runtime,'project',t.project),'role',m.role,'epoch',m.epoch,'team_revision',t.revision) FROM members m JOIN teams t ON t.task_key=m.task_key
        UNION ALL SELECT 7,id,json_object('kind','decision','id',id,'scope_key',scope_key,'decision_key',decision_key,
            'question',question,'recommendation',recommendation,'state',state,'revision',revision,'resolution',resolution,'created_at_ms',created_at,'resolved_at_ms',resolved_at,
            'session_id',session_id,'stop_id',stop_id,'category',category,'suggested_answer',suggested_answer,'later_until_ms',later_until) FROM chief_decisions WHERE ?4 OR state='open'
        UNION ALL SELECT 10,id,json_object('kind','delivery','id',id,'event_id',event_id,'recipient',recipient,'recipient_runtime',recipient_runtime,
            'recipient_epoch',recipient_epoch,'delivery_kind',kind,'envelope',json(envelope),'state',state,'native_id',native_id,'receipt',json(receipt),
            'started_at_ms',started_at,'finished_at_ms',finished_at) FROM chief_delivery_attempts WHERE ?4 OR state IN ('submitting','uncertain')
        UNION ALL SELECT 11,id,json_object('kind','event','id',id,'sequence',seq,'source',source,
            'event_kind',kind,'body',body,'created_at_ms',created_at) FROM chief_events WHERE ?4 OR id IN (SELECT event_id FROM chief_delivery_attempts WHERE state IN ('submitting','uncertain'))
        UNION ALL SELECT 12,identity,json_object('kind','admission','session',json(identity),'adapter',adapter,'evidence',evidence,'verified_at_ms',verified_at) FROM chief_admission_verifications WHERE ?4 OR identity IN (SELECT json_extract(value,'$.session') FROM json_each(?1))
        UNION ALL SELECT 13,id,json_object('kind','inbox','id',id,'chief_epoch',chief_epoch,'recipient',json(recipient_identity),
            'recipient_epoch',recipient_epoch,'envelope',json(envelope),'state',state,'next_at_ms',next_at,'attempts',attempts,'revision',attempts) FROM chief_inbox WHERE ?4 OR state IN ('queued','submitting','uncertain','failed')
        UNION ALL SELECT 14,token,json_object('kind','legacy_launch','id',token,'project',project,'task_id',task_id,'worker',worker,'state',state,'revision',revision,'created_at_ms',created_at) FROM legacy_launches WHERE ?4 OR state!='released'
        UNION ALL SELECT 15,'summary',json_object('kind','summary','state',coalesce((SELECT state FROM chief_summary_requests ORDER BY created_at DESC,rowid DESC LIMIT 1),'unavailable'),
            'request',json((SELECT json_object('id',id,'state',state,'created_at_ms',created_at,'deadline_ms',deadline) FROM chief_summary_requests ORDER BY created_at DESC,rowid DESC LIMIT 1)),
            'last_completed_at_ms',(SELECT completed_at FROM chief_summary_requests WHERE state='completed' ORDER BY completed_at DESC LIMIT 1),
            'previous_completed_at_ms',(SELECT completed_at FROM chief_summary_requests WHERE state='completed' ORDER BY completed_at DESC LIMIT 1 OFFSET 1),
            'last_result',json((SELECT response FROM chief_summary_requests WHERE state='completed' ORDER BY completed_at DESC LIMIT 1)))
        UNION ALL SELECT 16,printf('%019d%019d',9223372036854775807-created_at,9223372036854775807-answer_rowid),json_object('kind','answer','id',id,'stop_id',stop_id,
            'session_id',session_id,'project',project,'task_id',task_id,'question',question,'answer',answer,'reason',reason,'source',source,'delivery',delivery,
            'created_at_ms',created_at,'overrule',json(CASE WHEN overrule_id IS NULL THEN NULL ELSE json_object('id',overrule_id,'correction',correction,
                'delivery',overrule_delivery,'created_at_ms',overrule_created_at) END),
            'habits',json((SELECT coalesce(json_group_array(json_object('id',h.id,'text',h.text)),'[]') FROM (SELECT h.id,h.text FROM json_each(answer_habits) j JOIN chief_habits h ON h.id=j.value ORDER BY j.key) h)))
            FROM (SELECT a.rowid AS answer_rowid,a.id AS id,a.stop_id AS stop_id,a.session_id AS session_id,s.project AS project,s.task_id AS task_id,a.question AS question,
                    a.answer AS answer,a.reason AS reason,a.source AS source,a.delivery AS delivery,a.created_at AS created_at,a.habits AS answer_habits,
                    o.id AS overrule_id,o.correction AS correction,o.delivery AS overrule_delivery,o.created_at AS overrule_created_at
                FROM chief_answers a JOIN chief_stops s ON s.id=a.stop_id LEFT JOIN chief_overrules o ON o.answer_id=a.id
                ORDER BY a.created_at DESC,a.rowid DESC LIMIT 200)
        UNION ALL SELECT 17,printf('%019d%019d',9223372036854775807-created_at,9223372036854775807-rowid),json_object('kind','habit','id',id,'text',text,'project',project,
            'origin_project',origin_project,'source',source,'source_kind',source_kind,'reason',reason,'source_id',source_id,'state',state,'created_at_ms',created_at,'changed_at_ms',changed_at,
            'session_id',CASE source_kind WHEN 'overrule' THEN (SELECT o.session_id FROM chief_overrules o WHERE o.id=h.source_id)
                WHEN 'theo_answer' THEN (SELECT a.session_id FROM chief_answers a WHERE a.id=h.source_id)
                ELSE (SELECT s.session_id FROM chief_stops s WHERE s.id=h.source_id) END,
            'stopped_at_ms',CASE WHEN source_kind='theo_message' THEN (SELECT s.created_at FROM chief_stops s WHERE s.id=h.source_id) END)
            FROM (SELECT rowid,* FROM chief_habits ORDER BY created_at DESC,rowid DESC LIMIT 200) h
        ) SELECT record FROM records ORDER BY category,identity LIMIT ?2 OFFSET ?3")?;
    let encoded = statement.query_map(params![serde_json::to_string(live)?,limit as i64 + 1,after,audit],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    let records = encoded.iter().map(|record|Ok(serde_json::from_str::<Value>(record)?)).collect::<Result<Vec<_>,TeamError>>()?;
    let page = super::context_page(&records,limit)?;
    let count = page.as_array().expect("context page is array").len();
    Ok(json!({"records":page,"next":after.saturating_add(count as i64),"has_more":count<records.len(),
        "projection":"host_review","cursor_kind":"snapshot_offset"}))
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

/// Run on every open: the removed dispatch protocol must not keep tasks reserved or hold decisions
/// and deliveries open that nothing can act on any more.
pub fn clear_dispatch_leftovers(transaction: &rusqlite::Transaction<'_>) -> Result<(), TeamError> {
    transaction.execute("DELETE FROM scheduling_reservations WHERE owner='chief'", [])?;
    transaction.execute("UPDATE chief_decisions SET state='superseded',revision=revision+1 WHERE state='open'
        AND (work IS NOT NULL OR assignment IS NOT NULL OR scope_key LIKE 'host-assignment-%')", [])?;
    transaction.execute("UPDATE chief_decisions SET state='superseded',revision=revision+1 WHERE state='open'
        AND scope_key IN (SELECT 'host-delivery-'||id FROM chief_inbox WHERE state IN ('queued','submitting','uncertain')
            AND coalesce(json_extract(envelope,'$.kind'),'') NOT IN ('chief_wake','chief_answer','chief_form_reply','chief_correction','chief_message','chief_todo'))", [])?;
    transaction.execute("UPDATE chief_inbox SET state='cancelled' WHERE state IN ('queued','submitting','uncertain')
        AND coalesce(json_extract(envelope,'$.kind'),'') NOT IN ('chief_wake','chief_answer','chief_form_reply','chief_correction','chief_message','chief_todo')", [])?;
    transaction.execute("UPDATE chief_delivery_attempts SET state='cancelled' WHERE state IN ('submitting','uncertain')
        AND coalesce(kind,'') NOT IN ('chief_wake','chief_answer','chief_form_reply','chief_correction','chief_message','chief_todo')", [])?;
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
        if runtime != session.runtime || agent != session.agent { return Err(unauthorized()); }
        if serde_json::from_str::<crate::teams::TaskKey>(&key)? != *task {
            return Err(TeamError::new(ErrorCode::Unauthorized, "the session works in the team of another task"));
        }
    }
    Ok(())
}

fn require_unreserved(connection: &Connection, task: &crate::teams::TaskKey, worker: Option<&str>) -> Result<(), TeamError> {
    // Only a pending legacy launch may still pick any session of its project; observed "doing" work
    // without a session reserves its own task, and per-worker busy checks cover whoever runs it.
    let owner: Option<(String, String)> = connection.query_row(
        "SELECT owner,worker FROM scheduling_reservations WHERE ((project=?1 OR project='') AND task_id=?2)
            OR (worker<>'' AND worker=?3) OR ((project=?1 OR project='') AND worker='' AND owner='legacy') LIMIT 1",
        params![task.project, task.task_id, worker.unwrap_or("")], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    if let Some((owner, worker)) = owner { return Err(conflict(&format!("work already reserved by {owner}: {worker}"))); }
    Ok(())
}

/// A resumed conversation takes no new work, so the todo it is already doing (observed) is no
/// conflict: only a second resume, a launch aimed at it or a pending launch of its project is.
fn require_resumable(connection: &Connection, task: &crate::teams::TaskKey, session: &str) -> Result<(), TeamError> {
    let owner: Option<(String, String)> = connection.query_row(
        "SELECT owner,worker FROM scheduling_reservations WHERE ((project=?1 OR project='') AND task_id=?2)
            OR (worker=?3 AND owner<>'observed') OR ((project=?1 OR project='') AND worker='' AND owner='legacy') LIMIT 1",
        params![task.project, task.task_id, session], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    if let Some((owner, worker)) = owner { return Err(conflict(&format!("work already reserved by {owner}: {worker}"))); }
    Ok(())
}

fn insert_launch(connection: &Connection, task: &crate::teams::TaskKey, worker: Option<&str>, state: &str, now_ms: i64) -> Result<String, TeamError> {
    let token = crate::teams::store::identifier()?;
    connection.execute("INSERT INTO scheduling_reservations VALUES (?1,?2,?3,?4,'legacy')", params![task.project,task.task_id,worker.unwrap_or(""),token])?;
    connection.execute("INSERT INTO legacy_launches VALUES (?1,?2,?3,?4,?5,1,?6)",params![token,task.project,task.task_id,worker,state,now_ms])?;
    Ok(token)
}

/// Host-only reservation before any legacy send/spawn. A lost reply is not permission to retry.
pub fn reserve_legacy(connection: &mut Connection, task: &crate::teams::TaskKey, worker: Option<&str>, _now_ms: i64) -> Result<String, TeamError> {
    reserve_legacy_kind(connection,task,worker,false,_now_ms)
}

pub fn reserve_legacy_kind(connection: &mut Connection, task: &crate::teams::TaskKey, worker: Option<&str>, manual: bool, now_ms: i64) -> Result<String, TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    require_unreserved(&transaction, task, worker)?;
    let token = insert_launch(&transaction, task, worker, if manual {"manual"} else {"reserved"}, now_ms)?;
    transaction.commit()?;
    Ok(token)
}

/// Host-only reservation before resuming `session` (task `resume:<session>`).
pub fn reserve_resume(connection: &mut Connection, task: &crate::teams::TaskKey, session: &str, now_ms: i64) -> Result<String, TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    require_resumable(&transaction, task, session)?;
    let token = insert_launch(&transaction, task, Some(session), "reserved", now_ms)?;
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

/// How long a submitted launch keeps its todo reserved while the todo still reads To do: the queue
/// marks the todo in progress right after the launch, so a todo still To do past this went back.
pub const SUBMITTED_LAUNCH_GRACE_MS: i64 = 60_000;

/// Observe todo status in the shared scheduling reservations. A todo back in To do (its session
/// ended or gave it back) loses the reservation its old session held, or no session could ever
/// take it again; a launch still in flight or uncertain keeps it.
pub fn reconcile_todos(connection: &mut Connection, todos: &crate::todo::TodoList, now_ms: i64) -> Result<(), TeamError> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    // The reservations keep their first worker for dispatch; the supervision mirror always names the
    // session the todo itself names now.
    transaction.execute("DELETE FROM chief_supervised_tasks", [])?;
    for item in &todos.items {
        // Match team admission for clones/subdirectories; retain literal keys for removed checkouts.
        let project = crate::teams::canonical_project(&item.cwd()).unwrap_or_else(|_| format!("path:{}", item.cwd()));
        let task = item.id();
        if item.status() == "doing" {
            transaction.execute("INSERT OR REPLACE INTO chief_supervised_tasks VALUES (?1,?2,?3)", params![project,task,item.session()])?;
            transaction.execute("INSERT OR IGNORE INTO scheduling_reservations VALUES (?1,?2,?3,?4,'observed')",
                params![project,task,item.session(),crate::teams::store::identifier()?])?;
            transaction.execute("UPDATE scheduling_reservations SET worker=?3 WHERE project=?1 AND task_id=?2 AND worker=''", params![project,task,item.session()])?;
        } else if item.status() == "done" {
            crate::teams::store::release_completed_task(&transaction, Some(&project), &task)?;
            transaction.execute("DELETE FROM scheduling_reservations WHERE project=?1 AND task_id=?2 AND token NOT IN (SELECT token FROM legacy_launches WHERE state IN ('reserved','submitting','uncertain'))", params![project,task])?;
        } else if item.status() == "todo" {
            transaction.execute("DELETE FROM scheduling_reservations WHERE project=?1 AND task_id=?2 AND (owner='observed'
                OR token IN (SELECT token FROM legacy_launches WHERE state='submitted' AND created_at<=?3))",
                params![project,task,now_ms.saturating_sub(SUBMITTED_LAUNCH_GRACE_MS)])?;
        }
    }
    for (task, issue) in &todos.issues {
        if issue["status"] == "doing" {
            transaction.execute("INSERT OR REPLACE INTO chief_supervised_tasks VALUES ('',?1,?2)", params![task,issue["session"].as_str().unwrap_or("")])?;
            transaction.execute("INSERT INTO scheduling_reservations SELECT '',?1,?2,?3,'observed'
                WHERE NOT EXISTS(SELECT 1 FROM scheduling_reservations WHERE task_id=?1)",
                params![task,issue["session"].as_str().unwrap_or(""),crate::teams::store::identifier()?])?;
            transaction.execute("UPDATE scheduling_reservations SET worker=?2 WHERE task_id=?1 AND worker=''",
                params![task,issue["session"].as_str().unwrap_or("")])?;
        } else if issue["status"] == "done" {
            crate::teams::store::release_completed_task(&transaction, None, task)?;
            transaction.execute("DELETE FROM scheduling_reservations WHERE task_id=?1 AND token NOT IN (SELECT token FROM legacy_launches WHERE state IN ('reserved','submitting','uncertain'))", [task])?;
        }
    }
    transaction.execute("UPDATE legacy_launches SET state='released',revision=revision+1 WHERE state IN ('manual','submitted') AND NOT EXISTS(SELECT 1 FROM scheduling_reservations r WHERE r.token=legacy_launches.token)",[])?;
    transaction.commit()?;
    Ok(())
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
        ("chief_work_revisions", "approved", 256),
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
