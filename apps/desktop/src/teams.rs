//! Daemon side of linked sessions: verified sender identity, then the protected store.

use crate::registry::Registry;
use cockpit::teams::{AgentAction, ErrorCode, HostAction, TeamError, TeamReply, VerifiedSession, store};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const GIT_CONFIG_MAX: u64 = 65536;

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_millis() as i64).unwrap_or(0)
}

fn unauthorized() -> TeamError {
    TeamError::new(ErrorCode::Unauthorized, "this process is not a verified live cockpit session")
}

fn git_config_path(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git.join("config"));
    }
    let text = cockpit::util::read_untrusted_text(&dot_git, 4096).ok()?;
    let gitdir = PathBuf::from(text.lines().next()?.trim().strip_prefix("gitdir:")?.trim());
    let gitdir = if gitdir.is_absolute() { gitdir } else { root.join(gitdir) };
    let common = cockpit::util::read_untrusted_text(&gitdir.join("commondir"), 4096).ok().map(|common| {
        let common = PathBuf::from(common.trim());
        if common.is_absolute() { common } else { gitdir.join(common) }
    });
    Some(common.unwrap_or(gitdir).join("config"))
}

/// Read as text only: repository configuration is untrusted and must never run on the host.
fn origin_url(config: &Path) -> Option<String> {
    let text = cockpit::util::read_untrusted_text(config, GIT_CONFIG_MAX).ok()?;
    let mut is_origin = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            is_origin = line == "[remote \"origin\"]";
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        if is_origin && key.trim() == "url" {
            let url = value.trim().trim_matches('"');
            if url.is_empty() || url.len() > 250 || url.chars().any(char::is_control) {
                return None;
            }
            return Some(without_credentials(url));
        }
    }
    None
}

fn without_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let host_end = rest.find('/').unwrap_or(rest.len());
    match rest[..host_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => url.to_string(),
    }
}

fn canonical_at(root: &Path, depth: usize) -> Result<String, TeamError> {
    let root = fs::canonicalize(root)?;
    if let Some(url) = git_config_path(&root).and_then(|config| origin_url(&config)) {
        let local = Path::new(&url);
        if !local.is_absolute() {
            return Ok(format!("remote:{url}"));
        }
        if depth < 4 && local.exists() {
            return canonical_at(local, depth + 1);
        }
    }
    Ok(format!("path:{}", root.display()))
}

/// Same repository across separate checkouts; the same directory name alone is never enough.
pub fn canonical_project(cwd: &str) -> Result<String, TeamError> {
    if cwd.is_empty() {
        return Err(TeamError::new(ErrorCode::Invalid, "session has no working directory"));
    }
    canonical_at(Path::new(&cockpit::util::repository_root(cwd)), 0)
}

pub(crate) fn live_sessions(registry: &mut Registry) -> Vec<VerifiedSession> {
    let rows: Vec<(String, String, String, String)> = registry.sessions.iter()
        .filter(|(_, entry)| !entry.runtime.is_empty() && !entry.cwd.is_empty())
        .map(|(id, entry)| (id.clone(), entry.agent.clone(), entry.cwd.clone(), entry.runtime.clone())).collect();
    rows.into_iter().filter(|(_, _, _, runtime)| registry.system.runtime_is_live(runtime))
        .filter_map(|(session_id, agent, cwd, runtime)| Some(VerifiedSession { session_id, agent, project: canonical_project(&cwd).ok()?, runtime }))
        .collect()
}

fn verified_sender(registry: &mut Registry, peer: u32, live: &[VerifiedSession]) -> Result<VerifiedSession, TeamError> {
    let runtime = registry.system.sender_runtime(peer).ok_or_else(unauthorized)?;
    let matches: Vec<&VerifiedSession> = live.iter().filter(|session| session.runtime == runtime).collect();
    match matches.as_slice() {
        [session] => Ok((*session).clone()),
        _ => Err(unauthorized()),
    }
}

fn database<'a>(registry: &'a mut Registry, live: &[VerifiedSession]) -> Result<&'a mut store::Connection, TeamError> {
    if registry.teams.is_none() {
        let mut connection = store::open(&cockpit::paths::team_database())?;
        store::recover(&mut connection, live, now_ms())?;
        registry.teams = Some(connection);
    }
    Ok(registry.teams.as_mut().expect("opened above"))
}

fn record(kind: &str, sender: Option<&VerifiedSession>, is_ok: bool) {
    cockpit::audit::record("daemon", &format!("team.{kind}"), if is_ok { "ok" } else { "refused" },
        json!({"session": sender.map(|sender| sender.session_id.as_str()), "agent": sender.map(|sender| sender.agent.as_str())}));
}

pub fn request(registry: &mut Registry, peer: u32, action: AgentAction) -> Result<TeamReply, TeamError> {
    let live = live_sessions(registry);
    let sender = match verified_sender(registry, peer, &live) {
        Ok(sender) => sender,
        Err(error) => {
            record("rejected", None, false);
            return Err(error);
        }
    };
    let kind = match &action {
        AgentAction::Context => "context", AgentAction::Propose { .. } => "propose", AgentAction::Send { .. } => "send",
        AgentAction::Receive => "receive", AgentAction::Receipt { .. } => "receipt",
    };
    // The daemon's own view of the recipient decides whether it may be woken: never the adapter's claim.
    let turn = registry.sessions.get(&sender.session_id)
        .filter(|entry| !entry.needs_input && entry.state != "needs-you")
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

/// Host UI only: reached through a socket no sandbox can open.
pub fn host_control(registry: &mut Registry, action: &HostAction) -> Result<TeamReply, TeamError> {
    let live = live_sessions(registry);
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
