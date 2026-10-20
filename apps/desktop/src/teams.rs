//! Daemon side of linked sessions: verified sender identity, then the protected store.

use crate::registry::Registry;
use cockpit::teams::{AgentAction, ErrorCode, HostAction, TeamError, TeamReply, VerifiedSession, store};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_millis() as i64).unwrap_or(0)
}

fn unauthorized() -> TeamError {
    TeamError::new(ErrorCode::Unauthorized, "this process is not a verified live cockpit session")
}

/// The checkout's real host path: a parallel clone counts as its repository's primary checkout
/// (`~/IdeaProjects/<repo>`, from the host-made clones layout). Never read from repository files,
/// which a session can write.
pub fn canonical_project(cwd: &str) -> Result<String, TeamError> {
    if cwd.is_empty() {
        return Err(TeamError::new(ErrorCode::Invalid, "session has no working directory"));
    }
    let repository = crate::registry::parallel_clone_repository(cwd);
    let primary = cockpit::paths::home().join("IdeaProjects").join(&repository);
    let root = if !repository.is_empty() && primary.is_dir() { primary } else { PathBuf::from(cockpit::util::repository_root(cwd)) };
    Ok(format!("path:{}", fs::canonicalize(&root)?.display()))
}

/// Opens the store on first use; a restarted daemon takes each member's launch back from it while
/// that launch still runs (runtime_is_live re-checks boot, pid and start time).
fn open_and_restore(registry: &mut Registry) -> Result<bool, TeamError> {
    let is_opened = registry.teams.is_none();
    if is_opened {
        registry.teams = Some(store::open(&cockpit::paths::team_database())?);
    }
    let missing: Vec<String> = registry.sessions.iter().filter(|(_, entry)| entry.runtime.is_empty()).map(|(id, _)| id.clone()).collect();
    for session in missing {
        let Some(runtime) = store::member_runtime(registry.teams.as_ref().expect("opened above"), &session)? else { continue };
        if registry.system.runtime_is_live(&runtime) {
            if let Some(entry) = registry.sessions.get_mut(&session) { entry.runtime = runtime; }
        }
    }
    Ok(is_opened)
}

pub(crate) fn live_sessions(registry: &mut Registry) -> Result<Vec<VerifiedSession>, TeamError> {
    let is_opened = open_and_restore(registry)?;
    let rows: Vec<(String, String, String, String)> = registry.sessions.iter()
        .filter(|(_, entry)| !entry.runtime.is_empty() && !entry.cwd.is_empty())
        .map(|(id, entry)| (id.clone(), entry.agent.clone(), entry.cwd.clone(), entry.runtime.clone())).collect();
    let live: Vec<VerifiedSession> = rows.into_iter().filter(|(_, _, _, runtime)| registry.system.runtime_is_live(runtime))
        .filter_map(|(session_id, agent, cwd, runtime)| Some(VerifiedSession { session_id, agent, project: canonical_project(&cwd).ok()?, runtime }))
        .collect();
    if is_opened {
        store::recover(registry.teams.as_mut().expect("opened above"), &live, now_ms())?;
    }
    Ok(live)
}

fn verified_sender(registry: &mut Registry, peer: u32, live: &[VerifiedSession]) -> Result<VerifiedSession, TeamError> {
    let runtime = registry.system.sender_runtime(peer).ok_or_else(unauthorized)?;
    let matches: Vec<&VerifiedSession> = live.iter().filter(|session| session.runtime == runtime).collect();
    match matches.as_slice() {
        [session] => Ok((*session).clone()),
        _ => Err(unauthorized()),
    }
}

fn database<'a>(registry: &'a mut Registry, _live: &[VerifiedSession]) -> Result<&'a mut store::Connection, TeamError> {
    registry.teams.as_mut().ok_or_else(|| TeamError::new(ErrorCode::Unavailable, "team store not open"))
}

fn record(kind: &str, sender: Option<&VerifiedSession>, is_ok: bool) {
    cockpit::audit::record("daemon", &format!("team.{kind}"), if is_ok { "ok" } else { "refused" },
        json!({"session": sender.map(|sender| sender.session_id.as_str()), "agent": sender.map(|sender| sender.agent.as_str())}));
}

pub fn request(registry: &mut Registry, peer: u32, action: AgentAction) -> Result<TeamReply, TeamError> {
    let live = live_sessions(registry)?;
    let sender = match verified_sender(registry, peer, &live) {
        Ok(sender) => sender,
        Err(error) => {
            record("rejected", None, false);
            return Err(error);
        }
    };
    let kind = match &action {
        AgentAction::Context => "context", AgentAction::Propose { .. } => "propose", AgentAction::Send { .. } => "send",
        AgentAction::Receive => "receive", AgentAction::Receipt { .. } => "receipt", AgentAction::Assess { .. } => "assess",
        AgentAction::Handoff { .. } => "handoff",
    };
    // The daemon's own view of the recipient decides whether it may be woken: never the adapter's claim.
    let turn = registry.sessions.get(&sender.session_id)
        .filter(|entry| !entry.needs_input && entry.state == "waiting")
        .map(|entry| format!("{}:{}", entry.state, entry.since));
    let result = database(registry, &live).and_then(|connection| match &action {
        AgentAction::Context => Ok(TeamReply { request_id: "context".into(), result: Ok(store::context(connection, &sender)?) }),
        AgentAction::Propose { proposal } => store::propose(connection, &sender, proposal, &live, now_ms()),
        AgentAction::Send { .. } => store::accept(connection, &sender, &action, now_ms()),
        AgentAction::Receive => {
            store::expire(connection, now_ms())?;
            let Some(turn) = turn else { return Ok(TeamReply { request_id: "receive".into(), result: Ok(Value::Null) }) };
            let dispatch = store::claim(connection, &sender, &turn, now_ms())?;
            Ok(TeamReply { request_id: "receive".into(), result: Ok(dispatch.map_or(Value::Null, |dispatch| json!({
                "dispatch_id": dispatch.id, "message_id": dispatch.message_id,
                "text": cockpit::teams::envelope_text(&dispatch.envelope), "envelope": dispatch.envelope}))) })
        },
        AgentAction::Assess { .. } => store::assess(connection, &sender, &action, now_ms()),
        AgentAction::Handoff { .. } => store::handoff(connection, &sender, &action, now_ms()),
        AgentAction::Receipt { dispatch_id, outcome } => {
            store::finish_for(connection, &sender, dispatch_id, outcome, now_ms())?;
            Ok(TeamReply { request_id: dispatch_id.clone(), result: Ok(json!({"status": "recorded"})) })
        },
    });
    if !matches!(kind, "context" | "receive") || result.as_ref().is_ok_and(|reply| reply.result.as_ref().is_ok_and(|value| !value.is_null())) {
        record(kind, Some(&sender), result.as_ref().is_ok_and(|reply| reply.result.is_ok()));
    }
    result
}

/// The host socket is for programs in the daemon's own PID namespace: every sandbox has its own.
pub fn host_peer_allowed(table: &dyn crate::registry::Processes, pid: &str) -> bool {
    let own = table.pid_namespace("self");
    own.is_some() && table.pid_namespace(pid) == own
}

/// What the cockpit's Team view shows: the store's state plus the verified live sessions to pick.
pub fn host_snapshot(registry: &mut Registry) -> Result<Value, TeamError> {
    let live = live_sessions(registry)?;
    let mut sessions = Vec::new();
    for session in &live {
        let needs_channel = session.agent == "claude" && !registry.system.runtime_has_team_channel(&session.runtime);
        let entry = registry.sessions.get(&session.session_id);
        sessions.push(json!({"session_id": session.session_id, "agent": session.agent, "project": session.project,
            "project_name": entry.map(|entry| entry.project.as_str()).unwrap_or(""), "label": entry.map(|entry| entry.label.as_str()).unwrap_or(""),
            "state": entry.map(|entry| entry.state.as_str()).unwrap_or(""), "needs_team_channel": needs_channel}));
    }
    let mut state = store::snapshot(database(registry, &live)?, None, None, 100)?;
    state["sessions"] = json!(sessions);
    Ok(state)
}

pub fn host(registry: &mut Registry, request: cockpit::teams::HostRequest) -> Result<TeamReply, TeamError> {
    match request {
        cockpit::teams::HostRequest::Control { action } => host_control(registry, &action),
        cockpit::teams::HostRequest::Snapshot => Ok(TeamReply { request_id: "snapshot".into(), result: Ok(host_snapshot(registry)?) }),
    }
}

/// Host UI only: reached through a socket no sandbox can open.
pub fn host_control(registry: &mut Registry, action: &HostAction) -> Result<TeamReply, TeamError> {
    let live = live_sessions(registry)?;
    if let HostAction::Approve { proposal_id, .. } = action {
        let members = store::proposal_members(database(registry, &live)?, proposal_id)?;
        let missing: Vec<String> = members.into_iter()
            .filter(|(_, agent, runtime)| agent == "claude" && !registry.system.runtime_has_team_channel(runtime))
            .map(|(session, _, _)| session).collect();
        if !missing.is_empty() {
            let commands: Vec<String> = missing.iter().map(|session| format!("claude-sessions team-channel {session}")).collect();
            return Err(TeamError::new(ErrorCode::Conflict, format!(
                "Claude members must first restart into the team channel ({}), then review the request again", commands.join("; "))));
        }
    }
    let result = database(registry, &live).and_then(|connection| store::control(connection, action, &live, now_ms()));
    record("host_control", None, result.as_ref().is_ok_and(|reply| reply.result.is_ok()));
    result
}
