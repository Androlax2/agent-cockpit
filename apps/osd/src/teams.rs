//! Daemon side of linked sessions: verified sender identity, then the protected store.

use crate::registry::Registry;
use cockpit::teams::{AgentAction, ErrorCode, HostAction, TeamError, TeamReply, VerifiedSession, store};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
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
    reconcile_sleep(registry)?;
    // Continuation must commit before startup recovery sees a changed member runtime.
    for session in &live {
        let Some(entry) = registry.sessions.get(&session.session_id) else { continue };
        if entry.resume_token.is_empty() { continue; }
        let token = entry.resume_token.clone();
        let Ok(checkout) = fs::canonicalize(&entry.cwd) else { continue };
        let checkout = checkout.to_string_lossy().into_owned();
        let connection = registry.teams.as_mut().expect("opened above");
        let Some(permit) = store::sleep::permit(connection, &token)? else { continue };
        let is_acknowledgement = permit.state == "bound" && permit.replacement_runtime.as_deref() == Some(&session.runtime);
        if is_acknowledgement || (permit.note_recorded && !registry.system.runtime_is_live(&permit.old_runtime)
            && matching_sleep_note(&permit, now_ms())) {
            store::sleep::bind(connection, &token, session, &checkout, now_ms())?;
        }
    }
    if is_opened {
        store::recover(registry.teams.as_mut().expect("opened above"), &live, now_ms())?;
    }
    // A session the cockpit started for an approved team takes its role at its first verified request.
    let tokens: Vec<(String, VerifiedSession)> = live.iter().filter_map(|session| registry.sessions.get(&session.session_id)
        .filter(|entry| !entry.spawn_token.is_empty()).map(|entry| (entry.spawn_token.clone(), session.clone()))).collect();
    for (token, session) in tokens {
        // One session that cannot be bound must not stop every team request: logged, and tried again.
        if let Err(error) = store::bind_spawn(registry.teams.as_mut().expect("opened above"), &token, &session, now_ms()) {
            crate::registry::log(&format!("team start for {} not bound: {error}", session.session_id));
        }
    }
    Ok(live)
}

fn ended(session: &str) -> bool {
    match fs::read_to_string(cockpit::paths::ended_sessions()) {
        Ok(text) => text.lines().any(|line| line.trim() == session),
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Pin the private directory, then open its file without following a final symlink. Linux flags
/// match the daemon's existing direct O_NONBLOCK use; no extra dependency is needed.
fn matching_sleep_note(permit: &store::sleep::SleepPermit, now: i64) -> bool {
    if cockpit::agents::of_session_id(&permit.session_id).is_none() || permit.expires_at <= now || ended(&permit.session_id) {
        return false;
    }
    const O_NOFOLLOW: i32 = 0x20000;
    const O_DIRECTORY: i32 = 0x10000;
    const O_NONBLOCK: i32 = 0x800;
    let read_note = || -> std::io::Result<Option<Value>> {
        let directory = fs::OpenOptions::new().read(true).custom_flags(O_NOFOLLOW | O_DIRECTORY)
            .open(cockpit::paths::asleep_dir())?;
        let directory_metadata = directory.metadata()?;
        let owner = fs::metadata("/proc/self")?.uid();
        if directory_metadata.uid() != owner || directory_metadata.mode() & 0o077 != 0 { return Ok(None); }
        let file = fs::OpenOptions::new().read(true).custom_flags(O_NOFOLLOW | O_NONBLOCK)
            .open(format!("/proc/self/fd/{}/{}", directory.as_raw_fd(), permit.session_id))?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.uid() != owner || metadata.mode() & 0o077 != 0 || metadata.nlink() != 1
            || metadata.len() > cockpit::teams::client::REQUEST_MAX as u64 { return Ok(None); }
        let mut input = Vec::new();
        file.take(cockpit::teams::client::REQUEST_MAX as u64 + 1).read_to_end(&mut input)?;
        if input.len() > cockpit::teams::client::REQUEST_MAX { return Ok(None); }
        Ok(serde_json::from_slice(&input).ok())
    };
    let note = match read_note() {
        Ok(Some(note)) => note,
        _ => return false,
    };
    let slept_at = note["slept_at"].as_i64().and_then(|seconds| seconds.checked_mul(1000));
    let lifetime = cockpit::sleep::EXPIRY_S * 1000;
    note["session"].as_str() == Some(&permit.session_id) && note["agent"].as_str() == Some(&permit.agent)
        && note["team_resume"].as_str() == Some(&permit.token)
        && note["cwd"].as_str().and_then(|cwd| fs::canonicalize(cwd).ok()).as_deref() == Some(std::path::Path::new(&permit.checkout))
        && slept_at.is_some_and(|slept_at| slept_at >= permit.expires_at - lifetime - 1000
            && slept_at <= now + 1000 && now.saturating_sub(slept_at) < lifetime)
}

fn reconcile_sleep(registry: &mut Registry) -> Result<(), TeamError> {
    let connection = registry.teams.as_mut().expect("opened above");
    store::sleep::expire_wakes(connection, now_ms())?;
    for permit in store::sleep::prepared_records(connection)? {
        let is_old_live = registry.system.runtime_is_live(&permit.old_runtime);
        // The inherited kernel lock outlives a killed manager while an exit helper still runs.
        // Never reopen delivery on a clock while that operation can still exit the old runtime.
        if is_old_live && sleep_operation_is_live(&permit.session_id)? { continue; }
        // Recorded notes remain the durable sleep barrier, even if process teardown lags the script.
        if !permit.note_recorded || !matching_sleep_note(&permit, now_ms()) {
            store::sleep::cancel(connection, &permit.session_id, &permit.token)?;
        }
    }
    Ok(())
}

fn sleep_operation_file(session: &str) -> std::io::Result<Option<fs::File>> {
    const O_NOFOLLOW: i32 = 0x20000;
    const O_DIRECTORY: i32 = 0x10000;
    const O_NONBLOCK: i32 = 0x800;
    let directory = match fs::OpenOptions::new().read(true).custom_flags(O_NOFOLLOW | O_DIRECTORY)
        .open(cockpit::paths::runtime_dir().join("cockpit.sleep-operations")) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let owner = fs::metadata("/proc/self")?.uid();
    let metadata = directory.metadata()?;
    if metadata.uid() != owner || metadata.mode() & 0o077 != 0 {
        return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    }
    let file = match fs::OpenOptions::new().read(true).custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(format!("/proc/self/fd/{}/{session}", directory.as_raw_fd())) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != owner || metadata.mode() & 0o077 != 0 || metadata.nlink() != 1 {
        return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    }
    Ok(Some(file))
}

fn sleep_operation_is_live(session: &str) -> std::io::Result<bool> {
    let Some(file) = sleep_operation_file(session)? else { return Ok(false) };
    match file.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

fn verify_sleep_operation(peer: u32, session: &str) -> Result<(), TeamError> {
    if !host_peer_allowed(&crate::registry::Proc, &peer.to_string()) { return Err(unauthorized()); }
    let file = sleep_operation_file(session)?.ok_or_else(unauthorized)?;
    let metadata = file.metadata()?;
    // fdinfo reports locks attached to this open description, including inherited flock locks.
    // A PID, environment claim or merely opening somebody else's locked file is not ownership.
    for descriptor in fs::read_dir(format!("/proc/{peer}/fd"))? {
        let descriptor = descriptor?;
        let Ok(candidate) = fs::metadata(descriptor.path()) else { continue };
        if candidate.dev() != metadata.dev() || candidate.ino() != metadata.ino() { continue; }
        let Ok(info) = fs::read_to_string(format!("/proc/{peer}/fdinfo/{}", descriptor.file_name().to_string_lossy())) else { continue };
        if info.lines().any(|line| line.starts_with("lock:") && line.contains(" FLOCK ") && line.contains(" WRITE ")) {
            return Ok(());
        }
    }
    Err(unauthorized())
}

fn start_sleep_wakes(registry: &mut Registry) -> Result<(), TeamError> {
    reconcile_sleep(registry)?;
    let connection = registry.teams.as_mut().expect("opened above");
    for permit in store::sleep::pending_wakes(connection, now_ms())? {
        if !permit.note_recorded || registry.system.runtime_is_live(&permit.old_runtime)
            || !matching_sleep_note(&permit, now_ms()) { continue; }
        if store::sleep::reserve_wake(connection, &permit.session_id, &permit.token, now_ms())?
            && !registry.system.start_detached(&["cockpit", "wake", &permit.session_id]) {
            store::sleep::mark_failed(connection, &permit.session_id, &permit.token, "could not start cockpit wake")?;
        }
    }
    Ok(())
}

/// The live sessions' folders: approved recruits start in the asker's.
fn folders(registry: &Registry, live: &[VerifiedSession]) -> std::collections::HashMap<String, String> {
    live.iter().filter_map(|session| registry.sessions.get(&session.session_id).filter(|entry| !entry.cwd.is_empty())
        .map(|entry| (session.session_id.clone(), entry.cwd.clone()))).collect()
}

/// Starts what approvals and handoffs queued (never waiting for it), and closes replaced QA sessions.
/// A start is recorded before the next pass, so nothing is started twice.
pub fn start_spawns(registry: &mut Registry) -> Result<(), TeamError> {
    let Some(connection) = registry.teams.as_mut() else { return Ok(()) };
    store::expire_spawns(connection, now_ms())?;
    let pending = store::pending_spawns(connection)?;
    let superseded = store::superseded_qa(connection)?;
    for spawn in pending {
        let role = match spawn.role { cockpit::teams::Role::Designer => "designer", cockpit::teams::Role::Developer => "developer", cockpit::teams::Role::Qa => "qa" };
        let commit = spawn.commit_id.as_deref().unwrap_or("-");
        // Recorded before the start: a failure after it can never start the same teammate twice.
        store::mark_spawn_started(registry.teams.as_ref().expect("opened above"), &spawn.token, now_ms())?;
        if !registry.system.start_detached(&["cockpit", "--agent", &spawn.agent, "team-spawn", &spawn.token, role, &spawn.directory, commit, &spawn.brief]) {
            store::mark_spawn_failed(registry.teams.as_ref().expect("opened above"), &spawn.token)?;
        }
    }
    for session in superseded {
        // Only once the exit could be started; otherwise the next pass tries again.
        if registry.system.start_detached(&["cockpit", "exit", &session]) {
            store::mark_qa_exited(registry.teams.as_ref().expect("opened above"), &session)?;
        }
    }
    Ok(())
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
        AgentAction::Handoff { .. } => "handoff", AgentAction::Recruit { .. } => "recruit",
    };
    // Recruits run in the default agent as it stands now; Theo sees and approves that choice.
    let default_agent = if matches!(action, AgentAction::Recruit { .. }) { registry.system.default_agent() } else { None };
    // The Developer's real folder, where a fresh QA's copy comes from (never the handoff's own claim).
    let developer_folder = registry.sessions.get(&sender.session_id).map(|entry| entry.cwd.clone()).filter(|cwd| !cwd.is_empty());
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
        AgentAction::Handoff { .. } => store::handoff_in(connection, &sender, &action, developer_folder.as_deref(), now_ms()),
        AgentAction::Recruit { recruitment } => {
            let agent = default_agent.as_deref().ok_or_else(|| TeamError::new(ErrorCode::Unavailable, "cannot read the default agent from the cockpit settings"))?;
            store::recruit(connection, &sender, recruitment, agent, &live, now_ms())
        },
        AgentAction::Receipt { dispatch_id, outcome } => {
            store::finish_for(connection, &sender, dispatch_id, outcome, now_ms())?;
            Ok(TeamReply { request_id: dispatch_id.clone(), result: Ok(json!({"status": "recorded"})) })
        },
    });
    if !matches!(kind, "context" | "receive") || result.as_ref().is_ok_and(|reply| reply.result.as_ref().is_ok_and(|value| !value.is_null())) {
        record(kind, Some(&sender), result.as_ref().is_ok_and(|reply| reply.result.is_ok()));
    }
    if kind == "handoff" && result.as_ref().is_ok_and(|reply| reply.result.is_ok()) {
        start_spawns(registry)?;
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
    // The cockpit asks every 10 s: starts that never reported turn failed here, and a start queued
    // before a daemon restart goes out.
    start_spawns(registry)?;
    start_sleep_wakes(registry)?;
    let mut sessions = Vec::new();
    for session in &live {
        let entry = registry.sessions.get(&session.session_id);
        sessions.push(json!({"session_id": session.session_id, "agent": session.agent, "project": session.project,
            "project_name": entry.map(|entry| entry.project.as_str()).unwrap_or(""), "label": entry.map(|entry| entry.label.as_str()).unwrap_or(""),
            "state": entry.map(|entry| entry.state.as_str()).unwrap_or(""), "cwd": entry.map(|entry| entry.cwd.as_str()).unwrap_or("")}));
    }
    let mut state = store::snapshot(database(registry, &live)?, None, None, 100)?;
    state["sessions"] = json!(sessions);
    let mut sleeps = Vec::new();
    for permit in store::sleep::prepared_records(registry.teams.as_ref().expect("opened above"))? {
        let mut status = store::sleep::status(registry.teams.as_ref().expect("opened above"), &permit.session_id, &permit.token)?;
        status["session_id"] = json!(permit.session_id);
        sleeps.push(status);
    }
    state["sleeps"] = json!(sleeps);
    Ok(state)
}

pub fn host(registry: &mut Registry, peer: u32, request: cockpit::teams::HostRequest) -> Result<TeamReply, TeamError> {
    if let cockpit::teams::HostRequest::PrepareSleep { session_id } = &request {
        if cockpit::agents::of_session_id(session_id).is_none() { return Err(unauthorized()); }
        verify_sleep_operation(peer, session_id)?;
    }
    match request {
        cockpit::teams::HostRequest::Control { action } => host_control(registry, &action),
        cockpit::teams::HostRequest::Snapshot => Ok(TeamReply { request_id: "snapshot".into(), result: Ok(host_snapshot(registry)?) }),
        request => sleep_control(registry, request),
    }
}

fn sleep_control(registry: &mut Registry, request: cockpit::teams::HostRequest) -> Result<TeamReply, TeamError> {
    use cockpit::teams::HostRequest;
    let is_recording = matches!(&request, HostRequest::SleepRecorded { .. });
    let live = live_sessions(registry)?;
    let session_id = match &request {
        HostRequest::PrepareSleep { session_id } | HostRequest::CancelSleep { session_id, .. }
        | HostRequest::SleepRecorded { session_id, .. } | HostRequest::BeginWake { session_id, .. }
        | HostRequest::SleepStatus { session_id, .. } => session_id,
        _ => unreachable!(),
    };
    if cockpit::agents::of_session_id(session_id).is_none() {
        return Err(TeamError::new(ErrorCode::Invalid, "invalid sleep session id"));
    }
    let result = match request {
        HostRequest::PrepareSleep { session_id } => {
            let member = live.iter().find(|member| member.session_id == session_id).ok_or_else(unauthorized)?;
            let entry = registry.sessions.get(&session_id).ok_or_else(unauthorized)?;
            let checkout = fs::canonicalize(&entry.cwd)?.to_string_lossy().into_owned();
            let token = store::sleep::prepare(registry.teams.as_mut().expect("opened above"), member, &checkout, now_ms())?;
            json!({"token":token})
        },
        HostRequest::CancelSleep { session_id, token } => {
            store::sleep::cancel(registry.teams.as_mut().expect("opened above"), &session_id, &token)?;
            json!({"state":"cancelled"})
        },
        HostRequest::SleepStatus { session_id, token } => store::sleep::status(registry.teams.as_ref().expect("opened above"), &session_id, &token)?,
        HostRequest::SleepRecorded { session_id, token } | HostRequest::BeginWake { session_id, token } => {
            let connection = registry.teams.as_mut().expect("opened above");
            let permit = store::sleep::permit(connection, &token)?.filter(|permit| permit.session_id == session_id)
                .ok_or_else(|| TeamError::new(ErrorCode::NotFound, "sleep continuation not found"))?;
            if !matching_sleep_note(&permit, now_ms()) {
                return Err(TeamError::new(ErrorCode::Conflict, "matching protected sleep note required"));
            }
            // Both transitions are host-only. Record while the old process lives; launch only after it died.
            if is_recording {
                if !registry.system.runtime_is_live(&permit.old_runtime) { return Err(unauthorized()); }
                store::sleep::record_note(connection, &session_id, &token)?;
                json!({"state":"prepared"})
            } else {
                if !permit.note_recorded || registry.system.runtime_is_live(&permit.old_runtime) {
                    return Err(TeamError::new(ErrorCode::Conflict, "sleep note is not recorded or old runtime is still live"));
                }
                store::sleep::begin_wake(connection, &session_id, &token, now_ms())?;
                store::sleep::status(connection, &session_id, &token)?
            }
        },
        _ => unreachable!(),
    };
    Ok(TeamReply { request_id: "sleep".into(), result: Ok(result) })
}

/// Host UI only: reached through a socket no sandbox can open.
pub fn host_control(registry: &mut Registry, action: &HostAction) -> Result<TeamReply, TeamError> {
    let live = live_sessions(registry)?;
    let directories = folders(registry, &live);
    let result = database(registry, &live).and_then(|connection| store::control_with(connection, action, &live, &directories, now_ms()));
    let is_ok = result.as_ref().is_ok_and(|reply| reply.result.is_ok());
    record("host_control", None, is_ok);
    if is_ok && matches!(action, HostAction::Approve { .. }) {
        start_spawns(registry)?;
    }
    result
}
