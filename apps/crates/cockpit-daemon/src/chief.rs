//! Chief routes reuse the team socket's kernel-peer and protected database boundary.

use crate::{registry::Registry, teams};
use cockpit_core::chief::{ChiefAgentAction, ChiefHostAction, WorkerState, store};
use cockpit_core::teams::store::storage_error;
use cockpit_core::teams::{
    ErrorCode, NativeCounts, TaskKey, TeamError, TeamReply, VerifiedSession,
};
use serde_json::Value;

#[derive(Clone)]
pub(crate) struct NativeTelemetryObservation {
    pub session: VerifiedSession,
    pub sequence: i64,
    pub counts: Option<NativeCounts>,
    pub received_at_ms: i64,
}

/// Peer verification establishes the source, not independent truth of these runtime reports.
pub(crate) fn native_telemetry(
    registry: &mut Registry,
    sender: &VerifiedSession,
    sequence: i64,
    counts: Option<NativeCounts>,
) -> Result<TeamReply, TeamError> {
    if sender.agent != "opencode"
        || cockpit_core::agents::of_session_id(&sender.session_id).map(|agent| agent.name)
            != Some("opencode")
    {
        return Err(TeamError::new(
            ErrorCode::Unauthorized,
            "native telemetry requires the reporting OpenCode runtime",
        ));
    }
    registry.chief_telemetry.retain(|id, observation| {
        registry
            .sessions
            .get(id)
            .is_some_and(|entry| entry.runtime == observation.session.runtime)
    });
    let previous_sequence = registry
        .chief_telemetry
        .get(&sender.session_id)
        .filter(|previous| previous.session == *sender)
        .map_or(0, |previous| previous.sequence);
    let valid_sequence = (1..=9_007_199_254_740_991).contains(&sequence);
    if valid_sequence && sequence <= previous_sequence {
        return Err(TeamError::new(
            ErrorCode::Conflict,
            "native telemetry sample is not newer",
        ));
    }
    if !valid_sequence
        || counts.as_ref().is_some_and(|counts| {
            [
                counts.running_sessions,
                counts.pending_inbox,
                counts.pending_permissions,
                counts.pending_forms,
            ]
            .into_iter()
            .any(|count| count > 10_000)
        })
    {
        // Invalidating a sample must not erase the ordering floor and make an old idle replay fresh.
        registry.chief_telemetry.insert(
            sender.session_id.clone(),
            NativeTelemetryObservation {
                session: sender.clone(),
                sequence: if valid_sequence {
                    sequence
                } else {
                    previous_sequence
                },
                counts: None,
                received_at_ms: teams::now_ms(),
            },
        );
        return Err(TeamError::new(
            ErrorCode::Invalid,
            "native telemetry exceeds its bounded schema",
        ));
    }
    registry.chief_telemetry.insert(
        sender.session_id.clone(),
        NativeTelemetryObservation {
            session: sender.clone(),
            sequence,
            counts,
            received_at_ms: teams::now_ms(),
        },
    );
    Ok(TeamReply {
        request_id: "native-telemetry".into(),
        result: Ok(serde_json::json!({"recorded":true})),
    })
}

/// Whether the queue may hand `project`'s work to `worker`, or to a fresh session when there is none;
/// otherwise the reason it may not. A fresh session waits until no session of the project is live,
/// unless the Chief asked for one (`is_chief_requested`).
pub(crate) fn legacy_auto_ready(
    registry: &mut Registry,
    worker: Option<&str>,
    project: &str,
    is_chief_requested: bool,
) -> Result<(), &'static str> {
    let now = teams::now_ms();
    if !cockpit_core::settings::is_on("todo-queue") {
        return Err("the todo queue is off");
    }
    if registry.system.gaming() {
        return Err("paused while gaming");
    }
    let Some((viewed, active, gaming, observed)) = &registry.chief_focus else {
        return Err("no cockpit focus report yet");
    };
    if *observed > now || now - observed > 30_000 {
        return Err("the cockpit focus report is stale");
    }
    if *gaming {
        return Err("paused while gaming");
    }
    if *active && viewed.is_empty() {
        return Err("the viewed session is unknown");
    }
    if registry.pending_host_request_for(worker, project) {
        return Err("an ask-to-run or guide request is waiting in this project");
    }
    let Some(worker) = worker else {
        if is_chief_requested {
            return Ok(());
        }
        let is_project_live = registry.sessions.values().any(|entry| {
            !entry.runtime.is_empty()
                && registry.system.runtime_is_live(&entry.runtime)
                && teams::canonical_project(&entry.cwd).is_ok_and(|current| current == project)
        });
        return if is_project_live {
            Err("a session of this project is running")
        } else {
            Ok(())
        };
    };
    if teams::sleep_operation_is_live(worker).unwrap_or(true) {
        return Err("the session is going to sleep");
    }
    if *active && viewed == worker {
        return Err("Theo is viewing the session");
    }
    if cockpit_core::prompts::count(worker) > 0 || cockpit_core::prompts::restart_wanted(worker) {
        return Err("a message or restart waits for the session");
    }
    let Some(entry) = registry.sessions.get(worker).filter(|entry| {
        !entry.runtime.is_empty() && registry.system.runtime_is_live(&entry.runtime)
    }) else {
        let is_asleep_idle = cockpit_core::sleep::asleep().iter().any(|note| {
            note.session == worker
                && !note.remote
                && now / 1000 - note.idle_since >= cockpit_core::queue::QUEUE_IDLE_S
                && teams::canonical_project(&note.cwd).is_ok_and(|current| current == project)
        });
        return if is_asleep_idle {
            Ok(())
        } else {
            Err("the session is neither live nor asleep long enough")
        };
    };
    let is_idle = entry.state == "waiting"
        && !entry.needs_input
        && !entry.terminal
        && entry.agents == 0
        && entry.background == 0
        && now / 1000 - entry.since as i64 >= cockpit_core::queue::QUEUE_IDLE_S;
    if is_idle {
        Ok(())
    } else {
        Err("the session is not idle")
    }
}

pub(crate) fn workers(registry: &mut Registry) -> Result<Vec<WorkerState>, TeamError> {
    workers_at(registry, teams::now_ms(), true)
}

fn workers_at(
    registry: &mut Registry,
    now: i64,
    observe: bool,
) -> Result<Vec<WorkerState>, TeamError> {
    let live = teams::live_sessions(registry)?;
    workers_among(registry, live, now, observe)
}

/// Whether a game lets agent turns continue: Theo's setting allows it, the gamemode submap is off
/// and the host's game guard holds new heavy jobs, so what a turn starts waits or is pinned.
fn turns_continue_while_gaming(registry: &mut Registry) -> bool {
    match cockpit_core::settings::chief_turns_while_gaming() {
        Ok(true) => {}
        Ok(false) => return false,
        Err(error) => {
            crate::registry::log(&format!("{error}; agent turns wait during games"));
            return false;
        }
    }
    !registry.system.game_mode() && registry.system.game_guard_holds_heavy_jobs()
}

/// The workers among sessions the caller's own live_sessions() already verified: a team request
/// must not pay for that pass twice.
pub(crate) fn workers_among(
    registry: &mut Registry,
    live: Vec<VerifiedSession>,
    now: i64,
    observe: bool,
) -> Result<Vec<WorkerState>, TeamError> {
    let focus_gaming = matches!(&registry.chief_focus,
        Some((_, _, true, observed)) if *observed <= now && now - observed <= 30_000);
    let gaming = registry.system.gaming();
    let gaming_reason = if (gaming || focus_gaming) && turns_continue_while_gaming(registry) {
        store::GAMING_GUARDED
    } else {
        "gaming"
    };
    let connection = registry
        .teams
        .as_ref()
        .expect("live_sessions opens the protected store");
    let workers = live
        .into_iter()
        .map(|session| {
            let role = store::worker_role(connection, &session)?;
            let mut blocked_by = Vec::new();
            if teams::sleep_operation_is_live(&session.session_id)?
                || cockpit_core::teams::store::sleep::paused(connection, &session.session_id)?
            {
                blocked_by.push("host_sleep_in_progress".into());
            }
            if session.agent != "opencode" {
                blocked_by.push("native_admission_unverified_for_adapter".into());
            } else if !store::admission_verified(connection, &session)? {
                blocked_by.push("live_queued_admission_verification_required".into());
            }
            let mut has_native_observation = false;
            match registry.chief_native.get(&session.session_id) {
                Some((identity, idle, pending, observed))
                    if identity == &session && *observed <= now && now - observed <= 30_000 =>
                {
                    has_native_observation = true;
                    if !idle {
                        blocked_by.push("native_busy".into());
                    }
                    if *pending {
                        blocked_by.push("native_human_input_pending".into());
                    }
                }
                _ => {}
            }
            if let Some(observation) =
                registry
                    .chief_telemetry
                    .get(&session.session_id)
                    .filter(|observation| {
                        observation.session == session
                            && observation.received_at_ms <= now
                            && now - observation.received_at_ms <= 30_000
                    })
            {
                has_native_observation = true;
                match &observation.counts {
                    Some(counts) => {
                        if counts.running_sessions > 0 {
                            // A runtime waiting on a form still runs the session that asked: only a reply to
                            // that form may go past it, and only when no permission waits beside it.
                            let is_at_form =
                                counts.pending_forms > 0 && counts.pending_permissions == 0;
                            blocked_by.push(
                                if is_at_form {
                                    "native_busy_at_form"
                                } else {
                                    "native_busy"
                                }
                                .into(),
                            );
                        }
                        if counts.pending_inbox > 0 || counts.pending_permissions > 0 {
                            blocked_by.push("native_inbox_or_input_pending".into());
                        }
                        // Its own reason: a Chief form reply goes exactly to a session waiting on its form.
                        if counts.pending_forms > 0 {
                            blocked_by.push("native_form_pending".into());
                        }
                    }
                    None => blocked_by.push("runtime_native_sample_unavailable".into()),
                }
            }
            if !has_native_observation {
                blocked_by.push("fresh_native_availability_required".into());
                blocked_by.push(
                    if registry.chief_native.contains_key(&session.session_id)
                        || registry.chief_telemetry.contains_key(&session.session_id)
                    {
                        "native_observation_stale_or_identity_changed"
                    } else {
                        "native_observation_missing"
                    }
                    .into(),
                );
            }
            match &registry.chief_focus {
                Some((viewed, active, host_gaming, observed))
                    if *observed <= now && now - observed <= 30_000 =>
                {
                    if *active && viewed.is_empty() {
                        blocked_by.push("viewed_session_unknown".into());
                    }
                    if *active && *viewed == session.session_id {
                        blocked_by.push("viewed_session".into());
                    }
                    if *host_gaming {
                        blocked_by.push(gaming_reason.into());
                    }
                }
                _ => blocked_by.push("fresh_host_focus_required".into()),
            }
            if gaming {
                blocked_by.push(gaming_reason.into());
            }
            if cockpit_core::prompts::count(&session.session_id) > 0 {
                blocked_by.push("human_prompt_queued".into());
            }
            if cockpit_core::prompts::restart_wanted(&session.session_id) {
                blocked_by.push("restart_pending".into());
            }
            if store::legacy_worker_busy(connection, &session.session_id)? {
                blocked_by.push("legacy_work_in_progress".into());
            }
            if let Some(entry) = registry.sessions.get(&session.session_id) {
                if entry.needs_input {
                    blocked_by.push("human_input".into());
                }
                if now / 1000 - (entry.since as i64) < cockpit_core::queue::QUEUE_IDLE_S
                    && !store::is_chief(connection, &session.session_id)?
                {
                    blocked_by.push("idle_guard".into());
                }
                if entry.terminal || entry.agents > 0 || entry.background > 0 {
                    blocked_by.push("unavailable".into());
                }
                if registry.pending_host_request_for(Some(&session.session_id), &session.project) {
                    blocked_by.push("host_command_or_approval_pending".into());
                }
                match entry.state.as_str() {
                    "working" => blocked_by.push("busy".into()),
                    "waiting" => {}
                    "needs-you" => blocked_by.push("human_input".into()),
                    _ => blocked_by.push("unavailable".into()),
                }
            }
            Ok(WorkerState {
                session,
                role,
                ready: blocked_by.is_empty(),
                blocked_by,
                observed_at_ms: now,
            })
        })
        .collect::<Result<Vec<_>, TeamError>>()?;
    if observe {
        store::observe(
            registry.teams.as_mut().expect("opened above"),
            &workers,
            now,
        )?;
    }
    Ok(workers)
}

/// The active grant's epoch when the report-socket sender is the verified chief runtime. Any doubt
/// (no store, no unique live session, a store error) means no chief: the ordinary limits apply.
pub(crate) fn active_epoch_of(registry: &mut Registry, peer: u32) -> Option<String> {
    if registry.teams.is_none() && !cockpit_core::paths::team_database().is_file() {
        return None;
    }
    let result = teams::live_sessions(registry).and_then(|live| {
        let sender = teams::verified_sender(registry, peer, &live)?;
        store::active_epoch(
            registry
                .teams
                .as_ref()
                .expect("live_sessions opens the protected store"),
            &sender,
        )
    });
    match result {
        Ok(epoch) => epoch,
        Err(error) if error.code == ErrorCode::Unauthorized => None,
        Err(error) => {
            crate::registry::log(&format!(
                "chief grant lookup for a todo filing failed, ordinary limits apply: {}",
                error.message
            ));
            None
        }
    }
}

pub fn request(
    registry: &mut Registry,
    peer: u32,
    request: ChiefAgentAction,
) -> Result<TeamReply, TeamError> {
    let live = workers(registry)?;
    let sessions = live
        .iter()
        .map(|worker| worker.session.clone())
        .collect::<Vec<_>>();
    let sender = teams::verified_sender(registry, peer, &sessions)?;
    let reply = store::agent(
        registry.teams.as_mut().expect("opened above"),
        &sender,
        &request,
        &live,
        teams::now_ms(),
    )?;
    let dispatch_policy = reply
        .result
        .as_ref()
        .ok()
        .and_then(|value| value.get("dispatch_policy"))
        .cloned()
        .unwrap_or_default();
    if let ChiefAgentAction::StartSession {
        todo,
        project,
        reason,
    } = &request
    {
        let refusal = reply
            .result
            .as_ref()
            .err()
            .map(|error| error.message.as_str());
        cockpit_core::audit::record(
            "daemon",
            "chief.session_start_requested",
            if refusal.is_none() { "ok" } else { "refused" },
            serde_json::json!({"session": sender.session_id, "task_id": todo, "project": project, "reason": reason, "refusal": refusal, "dispatch_policy": dispatch_policy}),
        );
        if refusal.is_none() {
            cockpit_core::util::log_as(
                "daemon",
                "INFO",
                &format!("chief: asked for a fresh session for {todo} in {project}: {reason}"),
            );
        }
    }
    let coordination = match &request {
        ChiefAgentAction::GiveTodo {
            todo,
            session,
            reason,
        } => Some((
            "chief.todo_gift_requested",
            serde_json::json!({"todo":todo,"session":session,"reason":reason,"dispatch_policy":dispatch_policy}),
        )),
        ChiefAgentAction::Message {
            session,
            text,
            reason,
        } => Some((
            "chief.message_requested",
            serde_json::json!({"session":session,"text_sha256":cockpit_core::audit::sha256_hex(text.as_bytes()),"reason":reason}),
        )),
        _ => None,
    };
    if let Some((event, mut fields)) = coordination {
        fields["chief"] = serde_json::json!(sender.session_id);
        fields["refusal"] =
            serde_json::json!(reply.result.as_ref().err().map(|error| &error.message));
        cockpit_core::audit::record(
            "daemon",
            event,
            if reply.result.is_ok() {
                "ok"
            } else {
                "refused"
            },
            fields,
        );
    }
    Ok(reply)
}

pub fn host(registry: &mut Registry, action: &ChiefHostAction) -> Result<TeamReply, TeamError> {
    // Host recovery controls must remain reachable when observation hits a decision-capacity fault.
    let live = workers_at(registry, teams::now_ms(), false)?;
    if let ChiefHostAction::Recover {
        request_id,
        expected_epoch,
        ..
    } = action
    {
        let connection = registry.teams.as_ref().expect("opened above");
        let replay = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chief_actions WHERE request_id=?1)",
                [request_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_error)?;
        if !replay {
            let previous: Option<String>=connection.query_row("SELECT (SELECT runtime FROM chief_grants WHERE epoch=?1 AND state IN ('active','paused'))",[expected_epoch],|row|row.get(0)).map_err(storage_error)?;
            if previous
                .as_deref()
                .is_some_and(|runtime| registry.system.runtime_is_live(runtime))
            {
                return Ok(TeamReply {
                    request_id: request_id.clone(),
                    result: Err(TeamError::new(
                        cockpit_core::teams::ErrorCode::Conflict,
                        "the previous Chief launcher is still alive; recovery is refused",
                    )),
                });
            }
        }
    }
    if let ChiefHostAction::Enable {
        request_id,
        session,
    } = action
    {
        let connection = registry.teams.as_ref().expect("opened above");
        // Preserve authentication errors; a valid enable still cannot activate unverified admission.
        let replay = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chief_actions WHERE request_id=?1)",
                [request_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_error)?;
        if !replay
            && live.iter().any(|worker| worker.session == *session)
            && !store::is_product_worker(connection, &session.session_id)?
        {
            let mut verified = false;
            for worker in &live {
                if worker.session.agent == "opencode"
                    && store::admission_verified(connection, &worker.session)?
                {
                    verified = true;
                }
            }
            if !verified {
                return Ok(TeamReply {
                    request_id: request_id.clone(),
                    result: Err(TeamError::new(
                        cockpit_core::teams::ErrorCode::Unavailable,
                        cockpit_core::chief::ADMISSION_REQUIRED,
                    )),
                });
            }
        }
    }
    store::host(
        registry.teams.as_mut().expect("opened above"),
        action,
        &live,
        teams::now_ms(),
    )
}

pub fn snapshot(
    registry: &mut Registry,
    after: i64,
    limit: usize,
    generation: Option<&str>,
) -> Result<Value, TeamError> {
    let sessions = teams::polled_live_sessions(registry)?;
    let live = workers_among(registry, sessions, teams::now_ms(), false)?;
    let mut snapshot = store::snapshot_page(
        registry.teams.as_ref().expect("opened above"),
        &live,
        after,
        limit,
        generation,
        teams::now_ms(),
    )?;
    let now = teams::now_ms();
    for record in snapshot["records"]
        .as_array_mut()
        .expect("host snapshot records")
    {
        if record["kind"] != "worker" {
            continue;
        }
        let session: VerifiedSession = serde_json::from_value(record["session"].clone())?;
        let host =
            registry
                .chief_native
                .get(&session.session_id)
                .filter(|(identity, _, _, observed)| {
                    identity == &session && *observed <= now && now - *observed <= 30_000
                });
        let report = registry
            .chief_telemetry
            .get(&session.session_id)
            .filter(|observation| {
                observation.session == session
                    && observation.received_at_ms <= now
                    && now - observation.received_at_ms <= 30_000
            });
        record["native_observation"] = serde_json::json!({
            "source":match (host.is_some(),report.is_some()) {(true,true)=>"host_native_and_runtime_report",(true,false)=>"host_native",(false,true)=>"runtime_report",_=>"unavailable"},
            "host":host.map(|(_,idle,pending,observed)|serde_json::json!({"idle":idle,"pending_input":pending,"observed_at_ms":observed})),
            "runtime_report":report.map(|observation|serde_json::json!({"sequence":observation.sequence,"counts":observation.counts,"received_at_ms":observation.received_at_ms,
                "provenance":"runtime_report_not_independently_proven"})),
        });
    }
    // Provenance must fit the same reply budget as the protected records it annotates.
    let records = snapshot["records"]
        .as_array()
        .expect("host snapshot records");
    let page = cockpit_core::chief::context_page(records, limit)?;
    let count = page.as_array().expect("bounded page").len();
    if count < records.len() {
        snapshot["next"] = serde_json::json!(after.saturating_add(count as i64));
        snapshot["has_more"] = serde_json::json!(true);
    }
    snapshot["records"] = page;
    snapshot["coordination_error"] = serde_json::json!(registry.chief_fault);
    snapshot["session_names"] = session_names(
        registry,
        &live,
        snapshot["records"].as_array().expect("bounded page"),
    );
    Ok(snapshot)
}

/// `<label> · <project>` for every live session and every session a record in the page refers to.
/// A session the registry does not know, or that has no label, is left out so callers fall back to the id.
fn session_names(registry: &Registry, live: &[WorkerState], records: &[Value]) -> Value {
    let mut names = serde_json::Map::new();
    let referenced = records.iter().filter_map(|record| {
        record["session_id"]
            .as_str()
            .or_else(|| record["session"]["session_id"].as_str())
    });
    let live_ids = live.iter().map(|worker| worker.session.session_id.as_str());
    for session_id in referenced.chain(live_ids) {
        let Some(entry) = registry.sessions.get(session_id) else {
            continue;
        };
        if entry.label.is_empty() {
            continue;
        }
        let name = if entry.project.is_empty() {
            entry.label.clone()
        } else {
            format!("{} · {}", entry.label, entry.project)
        };
        names.insert(session_id.to_string(), Value::String(name));
    }
    Value::Object(names)
}

/// The state boundary at which the session is stopped for the Chief, if it is. A question and a
/// permission prompt both show as `needs-you`; only fresh native telemetry, received after the stop
/// began and reporting no open permission, tells them apart, so a missing, stale or older report
/// means no stop.
pub(crate) fn stop_boundary(
    registry: &Registry,
    session: &VerifiedSession,
    now_ms: i64,
) -> Option<String> {
    let boundary = state_boundary(registry, &session.session_id)?;
    let entry = &registry.sessions[&session.session_id];
    if entry.state == "waiting" {
        return Some(boundary);
    }
    // `since` is whole seconds: the stop may have begun up to a second after it.
    let stopped_by_ms = (entry.since as i64).saturating_add(1).saturating_mul(1000);
    registry
        .chief_telemetry
        .get(&session.session_id)
        .filter(|observation| {
            observation.session == *session
                && observation.received_at_ms <= now_ms
                && now_ms - observation.received_at_ms <= 30_000
                && observation.received_at_ms >= stopped_by_ms
        })
        .and_then(|observation| observation.counts.as_ref())
        .is_some_and(|counts| counts.pending_permissions == 0)
        .then_some(boundary)
}

/// The session's `state:since` boundary while it waits or asks, whatever telemetry says (or fails to).
///
/// `since` is whole seconds because the snapshot keeps it that way: a boundary built from the
/// fractional value would change when the daemon restarts, and the same stop would be recorded twice.
fn state_boundary(registry: &Registry, session_id: &str) -> Option<String> {
    let entry = registry.sessions.get(session_id)?;
    matches!(entry.state.as_str(), "waiting" | "needs-you")
        .then(|| format!("{}:{}", entry.state, entry.since as i64))
}

/// Attention alone is deferred; admission and delivery still see the real blocked session state.
pub(crate) fn question_grace_deadlines(
    registry: &mut Registry,
    now_ms: i64,
) -> Result<std::collections::HashMap<String, i64>, TeamError> {
    let mut deadlines = std::collections::HashMap::new();
    if registry.chief_fault.is_some() {
        return Ok(deadlines);
    }
    if !registry.sessions.values().any(|entry| {
        entry.agent == "opencode"
            && entry.state == "needs-you"
            && (0..cockpit_core::chief::QUESTION_GRACE_S)
                .contains(&(now_ms / 1000).saturating_sub(entry.since as i64))
    }) {
        return Ok(deadlines);
    }
    let Some(connection) = registry.teams.as_ref() else {
        return Ok(deadlines);
    };
    let chiefs = connection.prepare("SELECT g.session_id,g.agent,g.project,g.runtime FROM chief_grants g JOIN chief_coordination c USING(epoch)
        WHERE g.state='active' AND c.failed=0").map_err(storage_error)?.query_map([], |row| Ok(VerifiedSession {
            session_id: row.get(0)?, agent: row.get(1)?, project: row.get(2)?, runtime: row.get(3)?,
        })).map_err(storage_error)?
        .collect::<Result<Vec<_>, _>>().map_err(storage_error)?;
    let Some(chief) = chiefs.first() else {
        return Ok(deadlines);
    };
    let has_native_proof =
        registry
            .chief_native
            .get(&chief.session_id)
            .is_some_and(|(identity, _, _, at)| {
                identity == chief && *at <= now_ms && now_ms - at <= 30_000
            })
            || registry
                .chief_telemetry
                .get(&chief.session_id)
                .is_some_and(|sample| {
                    sample.session == *chief
                        && sample.counts.is_some()
                        && sample.received_at_ms <= now_ms
                        && now_ms - sample.received_at_ms <= 30_000
                });
    if !has_native_proof
        || !store::admission_verified(connection, chief)?
        || !registry
            .sessions
            .get(&chief.session_id)
            .is_some_and(|entry| entry.runtime == chief.runtime && entry.agent == "opencode")
        || !registry.system.runtime_is_live(&chief.runtime)
    {
        return Ok(deadlines);
    }
    let now = now_ms / 1000;
    for (session, entry) in &registry.sessions {
        if session == &chief.session_id
            || entry.agent != "opencode"
            || entry.state != "needs-you"
            || entry.runtime.is_empty()
        {
            continue;
        }
        let since = entry.since as i64;
        let until = since.saturating_add(cockpit_core::chief::QUESTION_GRACE_S);
        if now < since || now >= until {
            continue;
        }
        let counts = registry
            .chief_telemetry
            .get(session)
            .filter(|sample| {
                sample.session.runtime == entry.runtime
                    && sample.received_at_ms <= now_ms
                    && now_ms - sample.received_at_ms <= 30_000
                    && sample.received_at_ms >= since.saturating_add(1).saturating_mul(1000)
            })
            .and_then(|sample| sample.counts.as_ref());
        // Notification identity wins over cached counts: a form sample cannot classify a later permission.
        if counts.is_some_and(|counts| counts.pending_permissions > 0)
            || !(entry.needs_input || entry.question_form)
        {
            continue;
        }
        let boundary = format!("needs-you:{since}");
        if registry
            .chief_stop_failures
            .get(session)
            .is_some_and(|(failed, _)| failed == &boundary)
        {
            continue;
        }
        let needs_theo: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM chief_stops s
            WHERE s.session_id=?1 AND s.runtime=?2 AND s.boundary=?3 AND (s.state IN ('escalated','superseded')
                OR EXISTS(SELECT 1 FROM chief_answers a WHERE a.stop_id=s.id AND a.delivery IN ('uncertain','cancelled'))
                OR EXISTS(SELECT 1 FROM chief_inbox i WHERE json_extract(i.envelope,'$.stop')=s.id AND i.state IN ('failed','uncertain','cancelled'))))",
            (session, &entry.runtime, &boundary), |row| row.get(0)).map_err(storage_error)?;
        if !needs_theo {
            deadlines.insert(session.clone(), until);
        }
    }
    Ok(deadlines)
}

/// A stop whose bundle failed is retried at most this often; each try opens the conversation database
/// on the tick thread.
const STOP_BUNDLE_RETRY_MS: i64 = 60_000;

fn observe_stops(
    registry: &mut Registry,
    live: &[WorkerState],
    now_ms: i64,
) -> Result<(), TeamError> {
    observe_stops_with(registry, live, now_ms, cockpit_core::todo::TodoList::load)
}

/// Withdraws Theo's corrections whose session is gone or restarted, closes the stops of sessions that
/// moved on (cancelling queued answers only for sessions that are gone), and, while a Chief is enabled,
/// records a stop for every OpenCode session other than the Chief that sits at a stop boundary while
/// its todo is in progress, or that asks for an answer (or waits on its question form) with no todo in progress.
/// A stop whose bundle cannot be built is not recorded; it is retried after a backoff and logged once
/// per boundary. The todo list is read only when some session is at a stop boundary.
///
/// Moving on means a new `state:since` boundary, a state that is no stop, or a session that is gone.
/// Missing or stale telemetry only keeps a new stop from being recorded: it is no sign the agent
/// moved on, and an answer still needs fresh telemetry to be delivered.
pub(crate) fn observe_stops_with(
    registry: &mut Registry,
    live: &[WorkerState],
    now_ms: i64,
    load_todos: impl FnOnce() -> cockpit_core::todo::TodoList,
) -> Result<(), TeamError> {
    store::cancel_stranded_corrections(
        registry
            .teams
            .as_mut()
            .expect("workers_at opens the protected store"),
        live,
        now_ms,
    )?;
    let connection = registry
        .teams
        .as_ref()
        .expect("workers_at opens the protected store");
    for session_id in store::sessions_with_open_stops(connection)? {
        if !live
            .iter()
            .any(|worker| worker.session.session_id == session_id)
        {
            store::close_stops_of_gone_session(connection, &session_id)?;
            continue;
        }
        store::supersede_stops(
            connection,
            &session_id,
            state_boundary(registry, &session_id).as_deref(),
        )?;
    }
    let Some(chief) = store::chief_session(connection)? else {
        return Ok(());
    };
    let candidates: Vec<(VerifiedSession, String)> = live
        .iter()
        .filter(|worker| worker.session.agent == "opencode" && worker.session.session_id != chief)
        .filter_map(|worker| {
            Some((
                worker.session.clone(),
                stop_boundary(registry, &worker.session, now_ms)?,
            ))
        })
        .collect();
    let todos = (!candidates.is_empty()).then(load_todos);
    // A session with no todo in progress is stopped for the Chief only while it asks for an answer
    // (cockpit-needs-input or its own question form): one that merely finished its turn has nothing to answer.
    let stopped: Vec<(VerifiedSession, String, Option<cockpit_core::todo::Item>)> = candidates
        .into_iter()
        .filter_map(|(session, boundary)| {
            let item = todos
                .as_ref()?
                .items
                .iter()
                .find(|item| item.status() == "doing" && item.session() == session.session_id)
                .cloned();
            let asks_for_answer = registry
                .sessions
                .get(&session.session_id)
                .is_some_and(|entry| {
                    entry.state == "needs-you" && (entry.needs_input || entry.question_form)
                });
            (item.is_some() || asks_for_answer).then_some((session, boundary, item))
        })
        .collect();
    let connection = registry
        .teams
        .as_mut()
        .expect("workers_at opens the protected store");
    for (session, boundary, item) in &stopped {
        if store::stop_recorded(connection, &session.session_id, boundary)? {
            continue;
        }
        let failure = registry
            .chief_stop_failures
            .get(&session.session_id)
            .filter(|(failed_boundary, _)| failed_boundary == boundary)
            .map(|(_, at)| *at);
        if failure.is_some_and(|at| (0..STOP_BUNDLE_RETRY_MS).contains(&(now_ms - at))) {
            continue;
        }
        let (instructions, warnings) = project_instructions(&session.project);
        if failure.is_none() {
            for warning in warnings {
                cockpit_core::util::log_as(
                    "daemon",
                    "WARN",
                    &format!(
                        "chief stop for {} at {boundary}: {warning}",
                        session.session_id
                    ),
                );
            }
        }
        match cockpit_core::chief::bundle::stop_bundle(
            &cockpit_core::paths::opencode_database(),
            &session.session_id,
            item.as_ref(),
            instructions,
        ) {
            Ok(bundle) => {
                let task = TaskKey {
                    project: session.project.clone(),
                    task_id: item
                        .as_ref()
                        .map(cockpit_core::todo::Item::id)
                        .unwrap_or_default(),
                };
                store::record_stop(connection, session, boundary, &task, &bundle, now_ms)?;
            }
            Err(error) => {
                if failure.is_none() {
                    cockpit_core::util::log_as(
                        "daemon",
                        "WARN",
                        &format!(
                            "chief stop bundle for {} at {boundary} not built: {}",
                            session.session_id, error.message
                        ),
                    );
                }
                registry
                    .chief_stop_failures
                    .insert(session.session_id.clone(), (boundary.clone(), now_ms));
            }
        }
    }
    registry
        .chief_stop_failures
        .retain(|session_id, (failed_boundary, _)| {
            stopped.iter().any(|(session, boundary, _)| {
                session.session_id == *session_id && boundary == failed_boundary
            })
        });
    Ok(())
}

/// The instruction files at the root a canonical project key (`path:<abs>`) names. Any other key names
/// no directory: nothing is read and the caller logs the warning.
pub(crate) fn project_instructions(
    project: &str,
) -> (
    Vec<cockpit_core::chief::bundle::InstructionFile>,
    Vec<String>,
) {
    match project.strip_prefix("path:") {
        Some(root) => cockpit_core::chief::bundle::instruction_files(std::path::Path::new(root)),
        None => (
            Vec::new(),
            vec![format!(
                "project key {project} is no path; instruction files not read"
            )],
        ),
    }
}

pub fn tick(registry: &mut Registry, now_ms: i64) -> Result<(), TeamError> {
    if registry.teams.is_none() && !cockpit_core::paths::team_database().is_file() {
        return Ok(());
    }
    let observation = workers_at(registry, now_ms, true);
    let Some(connection) = registry.teams.as_mut() else {
        return observation.map(|_| ());
    };
    let pending = connection.prepare("SELECT a.decision,d.category FROM chief_attention a JOIN chief_decisions d ON d.id=a.decision WHERE d.state='open' ORDER BY a.created_at,a.decision LIMIT ?1").map_err(storage_error)?
        .query_map([cockpit_core::chief::MAX_OPEN_DECISIONS as i64],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?))).map_err(storage_error)?.collect::<Result<Vec<_>,_>>().map_err(storage_error)?;
    for (decision, is_visible) in &mut registry.chief_attention_presentations {
        if *is_visible && !pending.iter().any(|(open, _)| open == decision) {
            registry
                .banners
                .drop_banner(&format!("notice:chief-decision-{decision}"));
            *is_visible = false;
        }
    }
    for (decision, category) in pending {
        // Durable pending attention survives restarts; presentation history only deduplicates this daemon.
        if registry
            .chief_attention_presentations
            .contains_key(&decision)
        {
            continue;
        }
        let notice = match category.as_deref() {
            Some(category) => crate::registry::Notice {id:format!("chief-decision-{decision}"),title:format!("Chief · {} question needs you",category.replace('_'," ")),
                message:"A stopped agent waits for your answer. Review the escalation in the chief view. No permission has been granted.".into()},
            None => crate::registry::Notice {id:format!("chief-decision-{decision}"),title:"Chief · host decision needed".into(),message:"Review the protected chief decisions. No permission has been granted.".into()},
        };
        registry.banners.raise_notice(&notice);
        registry
            .chief_attention_presentations
            .insert(decision.clone(), true);
        connection
            .execute(
                "UPDATE chief_attention SET notified_at=coalesce(notified_at,?2) WHERE decision=?1",
                (decision.as_str(), now_ms),
            )
            .map_err(storage_error)?;
    }
    if let Ok(live) = &observation {
        advance_startup(registry, live)?;
        observe_stops(registry, live, now_ms)?;
    }
    observation.map(|_| ())
}

fn advance_startup(registry: &mut Registry, live: &[WorkerState]) -> Result<(), TeamError> {
    let connection = registry.teams.as_ref().expect("opened by observation");
    let Some(startup) = store::startup(connection)? else {
        return Ok(());
    };
    let state = startup["state"].as_str().expect("stored startup state");
    if !matches!(state, "starting" | "checking") {
        return Ok(());
    }
    let attempt = startup["id"].as_str().expect("stored startup id");
    let session = startup["session_id"]
        .as_str()
        .expect("stored startup session");
    let directory = startup["directory"]
        .as_str()
        .expect("stored startup directory");
    if state == "starting" {
        // Claim before dispatch: a daemon crash here leaves Checking, never a second process.
        if connection.execute("UPDATE chief_startup SET state='checking',revision=revision+1 WHERE id=?1 AND state='starting'", [attempt]).map_err(storage_error)? == 0 { return Ok(()); }
        if let Err(error) = registry.system.start_chief(session, directory) {
            startup_outcome(registry, attempt, "failed", Some(&error))?;
        }
        return Ok(());
    }
    if let Some(result) = registry.system.chief_launch_result(session) {
        match result {
            Ok((0, _)) => {}
            Ok((code, error)) => {
                let error = format!("Chief launcher exited with status {code}: {error}");
                // Exit 2 is reserved for pre-launch failures. Any later failure is uncertain.
                let absent = !live
                    .iter()
                    .any(|worker| worker.session.session_id == session)
                    && !registry.system.live_session_ids().contains(session);
                let next = if code == 2 && absent {
                    "failed"
                } else {
                    "checking"
                };
                startup_outcome(registry, attempt, next, Some(&error))?;
                if next == "failed" {
                    return Ok(());
                }
            }
            Err(error) => startup_outcome(registry, attempt, "checking", Some(&error))?,
        }
    }
    let Some(worker) = live
        .iter()
        .find(|worker| worker.session.session_id == session)
    else {
        return Ok(());
    };
    if worker.session.agent != "opencode" || worker.session.project != format!("path:{directory}") {
        return startup_outcome(
            registry,
            attempt,
            "checking",
            Some("The reported Chief session does not match its reserved agent and folder"),
        );
    }
    // Started from one action, the Chief needs its own session's verification, not any other session's.
    if !store::admission_verified(connection, &worker.session)? {
        return startup_outcome(
            registry,
            attempt,
            "checking",
            Some(cockpit_core::chief::ADMISSION_REQUIRED),
        );
    }
    let action = ChiefHostAction::Enable {
        request_id: format!("chief-start-{attempt}"),
        session: worker.session.clone(),
    };
    match host(registry, &action)?.result {
        // A replayed success whose grant was disabled meanwhile still settles as activated: the store reports it ended.
        Ok(_) => {
            registry
                .teams
                .as_ref()
                .expect("opened above")
                .execute(
                    "UPDATE chief_startup SET identity=?2 WHERE id=?1",
                    (attempt, serde_json::to_string(&worker.session)?),
                )
                .map_err(storage_error)?;
            startup_outcome(registry, attempt, "active", None)
        }
        Err(error) => startup_outcome(registry, attempt, "checking", Some(&error.message)),
    }
}

fn startup_outcome(
    registry: &Registry,
    attempt: &str,
    state: &str,
    error: Option<&str>,
) -> Result<(), TeamError> {
    registry
        .teams
        .as_ref()
        .expect("startup store opened")
        .execute(
            "UPDATE chief_startup SET state=?2,error=?3,revision=revision+1
        WHERE id=?1 AND state='checking' AND (state!=?2 OR error IS NOT ?3)",
            (attempt, state, error),
        )
        .map_err(storage_error)?;
    Ok(())
}
