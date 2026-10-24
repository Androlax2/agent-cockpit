//! Chief routes reuse the team socket's kernel-peer and protected database boundary.

use crate::{registry::Registry, teams};
use cockpit::chief::{ChiefAgentAction, ChiefHostAction, WorkerState, store};
use cockpit::teams::{TeamError, TeamReply, VerifiedSession, NativeCounts, ErrorCode};
use serde_json::Value;

#[derive(Clone)]
pub(crate) struct NativeTelemetryObservation {
    pub session: VerifiedSession,
    pub sequence: i64,
    pub counts: Option<NativeCounts>,
    pub received_at_ms: i64,
}

/// Peer verification establishes the source, not independent truth of these runtime reports.
pub(crate) fn native_telemetry(registry: &mut Registry, sender: &VerifiedSession, sequence: i64, counts: Option<NativeCounts>) -> Result<TeamReply, TeamError> {
    if sender.agent != "opencode" || cockpit::agents::of_session_id(&sender.session_id).map(|agent|agent.name) != Some("opencode") {
        return Err(TeamError::new(ErrorCode::Unauthorized,"native telemetry requires the reporting OpenCode runtime"));
    }
    registry.chief_telemetry.retain(|id, observation| registry.sessions.get(id).is_some_and(|entry|entry.runtime==observation.session.runtime));
    let previous_sequence = registry.chief_telemetry.get(&sender.session_id).filter(|previous|previous.session==*sender).map_or(0,|previous|previous.sequence);
    let valid_sequence = (1..=9_007_199_254_740_991).contains(&sequence);
    if valid_sequence && sequence <= previous_sequence {
        return Err(TeamError::new(ErrorCode::Conflict,"native telemetry sample is not newer"));
    }
    if !valid_sequence || counts.as_ref().is_some_and(|counts|
        [counts.running_sessions,counts.pending_inbox,counts.pending_permissions,counts.pending_forms].into_iter().any(|count|count>10_000)) {
        // Invalidating a sample must not erase the ordering floor and make an old idle replay fresh.
        registry.chief_telemetry.insert(sender.session_id.clone(),NativeTelemetryObservation {
            session:sender.clone(), sequence:if valid_sequence { sequence } else { previous_sequence },counts:None,received_at_ms:teams::now_ms(),
        });
        return Err(TeamError::new(ErrorCode::Invalid,"native telemetry exceeds its bounded schema"));
    }
    registry.chief_telemetry.insert(sender.session_id.clone(),NativeTelemetryObservation {
        session:sender.clone(), sequence, counts, received_at_ms:teams::now_ms(),
    });
    Ok(TeamReply { request_id:"native-telemetry".into(),result:Ok(serde_json::json!({"recorded":true})) })
}

pub(crate) fn legacy_auto_ready(registry: &mut Registry, worker: Option<&str>, project: &str) -> bool {
    let now = teams::now_ms();
    if !cockpit::settings::is_on("todo-queue") || registry.system.gaming() { return false; }
    let Some((viewed, active, gaming, observed)) = &registry.chief_focus else { return false };
    if *observed > now || now - observed > 30_000 || *gaming || (*active && viewed.is_empty()) { return false; }
    if registry.pending_host_request_for(worker,project) { return false; }
    let Some(worker) = worker else {
        return !registry.sessions.values().any(|entry| !entry.runtime.is_empty() && registry.system.runtime_is_live(&entry.runtime)
            && teams::canonical_project(&entry.cwd).is_ok_and(|current| current == project));
    };
    if teams::sleep_operation_is_live(worker).unwrap_or(true) { return false; }
    if (*active && viewed == worker) || cockpit::prompts::count(worker) > 0 || cockpit::prompts::restart_wanted(worker) { return false; }
    let Some(entry) = registry.sessions.get(worker).filter(|entry| !entry.runtime.is_empty() && registry.system.runtime_is_live(&entry.runtime)) else {
        return cockpit::sleep::asleep().iter().any(|note| note.session == worker && !note.remote
            && now / 1000 - note.idle_since >= cockpit::queue::QUEUE_IDLE_S
            && teams::canonical_project(&note.cwd).is_ok_and(|current|current==project));
    };
    entry.state == "waiting" && !entry.needs_input && !entry.terminal && entry.agents == 0 && entry.background == 0
        && now / 1000 - entry.since as i64 >= cockpit::queue::QUEUE_IDLE_S
}

pub(crate) fn workers(registry: &mut Registry) -> Result<Vec<WorkerState>, TeamError> {
    workers_at(registry,teams::now_ms(),true)
}

fn workers_at(registry: &mut Registry, now: i64, observe: bool) -> Result<Vec<WorkerState>, TeamError> {
    let live = teams::live_sessions(registry)?;
    let gaming = registry.system.gaming();
    let connection = registry.teams.as_ref().expect("live_sessions opens the protected store");
    let workers = live.into_iter().map(|session| {
        let role = store::worker_role(connection, &session)?;
        let mut blocked_by = Vec::new();
        if teams::sleep_operation_is_live(&session.session_id)? || cockpit::teams::store::sleep::paused(connection, &session.session_id)? {
            blocked_by.push("host_sleep_in_progress".into());
        }
        if session.agent != "opencode" {
            blocked_by.push("native_admission_unverified_for_adapter".into());
        } else if !store::admission_verified(connection, &session)? {
            blocked_by.push("live_queued_admission_verification_required".into());
        }
        let mut has_native_observation = false;
        match registry.chief_native.get(&session.session_id) {
            Some((identity, idle, pending, observed)) if identity == &session && *observed <= now && now - observed <= 30_000 => {
                has_native_observation = true;
                if !idle { blocked_by.push("native_busy".into()); }
                if *pending { blocked_by.push("native_human_input_pending".into()); }
            },
            _ => {},
        }
        if let Some(observation) = registry.chief_telemetry.get(&session.session_id)
            .filter(|observation| observation.session == session && observation.received_at_ms <= now && now-observation.received_at_ms <= 30_000) {
            has_native_observation = true;
            match &observation.counts {
                Some(counts) => {
                    if counts.running_sessions > 0 { blocked_by.push("native_busy".into()); }
                    if counts.pending_inbox > 0 || counts.pending_permissions > 0 || counts.pending_forms > 0 {
                        blocked_by.push("native_inbox_or_input_pending".into());
                    }
                },
                None => blocked_by.push("runtime_native_sample_unavailable".into()),
            }
        }
        if !has_native_observation {
            blocked_by.push("fresh_native_availability_required".into());
            blocked_by.push(if registry.chief_native.contains_key(&session.session_id) || registry.chief_telemetry.contains_key(&session.session_id) {
                "native_observation_stale_or_identity_changed"
            } else {"native_observation_missing"}.into());
        }
        match &registry.chief_focus {
            Some((viewed, active, host_gaming, observed)) if *observed <= now && now - observed <= 30_000 => {
                if *active && viewed.is_empty() { blocked_by.push("viewed_session_unknown".into()); }
                if *active && *viewed == session.session_id { blocked_by.push("viewed_session".into()); }
                if *host_gaming { blocked_by.push("gaming".into()); }
            },
            _ => blocked_by.push("fresh_host_focus_required".into()),
        }
        if gaming { blocked_by.push("gaming".into()); }
        if cockpit::prompts::count(&session.session_id) > 0 { blocked_by.push("human_prompt_queued".into()); }
        if cockpit::prompts::restart_wanted(&session.session_id) { blocked_by.push("restart_pending".into()); }
        if store::legacy_worker_busy(connection, &session.session_id)? { blocked_by.push("legacy_work_in_progress".into()); }
        if let Some(entry) = registry.sessions.get(&session.session_id) {
            if entry.needs_input { blocked_by.push("human_input".into()); }
            if now / 1000 - (entry.since as i64) < cockpit::queue::QUEUE_IDLE_S { blocked_by.push("idle_guard".into()); }
            if entry.terminal || entry.agents > 0 || entry.background > 0 { blocked_by.push("unavailable".into()); }
            if registry.pending_host_request_for(Some(&session.session_id),&session.project) {
                blocked_by.push("host_command_or_approval_pending".into());
            }
            match entry.state.as_str() {
                "working" => blocked_by.push("busy".into()),
                "waiting" => {},
                "needs-you" => blocked_by.push("human_input".into()),
                _ => blocked_by.push("unavailable".into()),
            }
        }
        Ok(WorkerState { session, role, ready: blocked_by.is_empty(), blocked_by, observed_at_ms: now })
    }).collect::<Result<Vec<_>,TeamError>>()?;
    if observe {store::observe(registry.teams.as_mut().expect("opened above"),&workers,now)?;}
    Ok(workers)
}

pub fn request(registry: &mut Registry, peer: u32, request: ChiefAgentAction) -> Result<TeamReply, TeamError> {
    let live = workers(registry)?;
    let sessions = live.iter().map(|worker| worker.session.clone()).collect::<Vec<_>>();
    let sender = teams::verified_sender(registry, peer, &sessions)?;
    store::agent(registry.teams.as_mut().expect("opened above"), &sender, &request, &live, teams::now_ms())
}

pub fn host(registry: &mut Registry, action: &ChiefHostAction) -> Result<TeamReply, TeamError> {
    // Host recovery controls must remain reachable when observation hits a decision-capacity fault.
    let live = workers_at(registry,teams::now_ms(),false)?;
    if let ChiefHostAction::Enable { request_id, session } = action {
        let connection = registry.teams.as_ref().expect("opened above");
        // Preserve authentication errors; a valid enable still cannot activate unverified admission.
        let replay = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_actions WHERE request_id=?1)", [request_id], |row| row.get::<_,bool>(0))?;
        if !replay && live.iter().any(|worker| worker.session == *session) && !store::is_product_worker(connection,&session.session_id)? {
            let mut verified = false;
            for worker in &live {
                if worker.session.agent == "opencode" && store::admission_verified(connection,&worker.session)? { verified = true; }
            }
            if !verified { return Ok(TeamReply { request_id:request_id.clone(), result:Err(TeamError::new(
                cockpit::teams::ErrorCode::Unavailable,"host live OpenCode draft/queue verification is required before chief activation")) }); }
        }
    }
    store::host(registry.teams.as_mut().expect("opened above"), action, &live, teams::now_ms())
}

pub fn snapshot(registry: &mut Registry, after: i64, limit: usize, generation: Option<&str>) -> Result<Value, TeamError> {
    let live = workers_at(registry,teams::now_ms(),false)?;
    let mut snapshot = store::snapshot_page(registry.teams.as_ref().expect("opened above"), &live, after, limit, generation, teams::now_ms())?;
    let now = teams::now_ms();
    for record in snapshot["records"].as_array_mut().expect("host snapshot records") {
        if record["kind"] != "worker" { continue; }
        let session: VerifiedSession = serde_json::from_value(record["session"].clone())?;
        let host = registry.chief_native.get(&session.session_id).filter(|(identity,_,_,observed)|identity==&session && *observed<=now && now-*observed<=30_000);
        let report = registry.chief_telemetry.get(&session.session_id).filter(|observation|observation.session==session && observation.received_at_ms<=now && now-observation.received_at_ms<=30_000);
        record["native_observation"] = serde_json::json!({
            "source":match (host.is_some(),report.is_some()) {(true,true)=>"host_native_and_runtime_report",(true,false)=>"host_native",(false,true)=>"runtime_report",_=>"unavailable"},
            "host":host.map(|(_,idle,pending,observed)|serde_json::json!({"idle":idle,"pending_input":pending,"observed_at_ms":observed})),
            "runtime_report":report.map(|observation|serde_json::json!({"sequence":observation.sequence,"counts":observation.counts,"received_at_ms":observation.received_at_ms,
                "provenance":"runtime_report_not_independently_proven"})),
        });
    }
    // Provenance must fit the same reply budget as the protected records it annotates.
    let records = snapshot["records"].as_array().expect("host snapshot records");
    let page = cockpit::chief::context_page(records,limit)?;
    let count = page.as_array().expect("bounded page").len();
    if count < records.len() {
        snapshot["next"] = serde_json::json!(after.saturating_add(count as i64));
        snapshot["has_more"] = serde_json::json!(true);
    }
    snapshot["records"] = page;
    snapshot["coordination_error"] = serde_json::json!(registry.chief_fault);
    Ok(snapshot)
}

pub fn tick(registry: &mut Registry, now_ms: i64) -> Result<(), TeamError> {
    if registry.teams.is_none() && !cockpit::paths::team_database().is_file() {return Ok(())}
    let observation = workers_at(registry,now_ms,true);
    let Some(connection) = registry.teams.as_mut() else {return observation.map(|_|())};
    let pending = connection.prepare("SELECT a.decision FROM chief_attention a JOIN chief_decisions d ON d.id=a.decision WHERE d.state='open' ORDER BY a.created_at,a.decision LIMIT ?1")?
        .query_map([cockpit::chief::MAX_OPEN_DECISIONS as i64],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for (decision,is_visible) in &mut registry.chief_attention_presentations {
        if *is_visible && !pending.contains(decision) {
            registry.banners.drop_banner(&format!("notice:chief-decision-{decision}"));
            *is_visible=false;
        }
    }
    for decision in pending {
        // Durable pending attention survives restarts; presentation history only deduplicates this daemon.
        if registry.chief_attention_presentations.contains_key(&decision) {continue;}
        registry.banners.raise_notice(&crate::registry::Notice {id:format!("chief-decision-{decision}"),title:"Chief · host decision needed".into(),message:"Review the protected chief decisions. No permission has been granted.".into()});
        registry.chief_attention_presentations.insert(decision.clone(),true);
        connection.execute("UPDATE chief_attention SET notified_at=coalesce(notified_at,?2) WHERE decision=?1",(decision.as_str(),now_ms))?;
    }
    observation.map(|_|())
}
