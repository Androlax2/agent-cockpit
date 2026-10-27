//! Daemon side of linked sessions: verified sender identity, then the protected store.

use crate::registry::Registry;
use cockpit::teams::{AgentAction, ErrorCode, HostAction, TeamError, TeamReply, VerifiedSession, store};
pub(crate) use cockpit::teams::canonical_project;
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_millis() as i64).unwrap_or(0)
}

/// The todo list with each item's cwd resolved to its canonical project, reread only when
/// todo.json changes. Every team request and tick reconciles it: a full reload with its project
/// lookups cost about 7 ms each, a few hundred times a minute.
#[derive(Default)]
pub(crate) struct ResolvedTodos {
    stamp: Option<(i64, i64, i64, i64, u64, u64)>,
    todos: cockpit::todo::TodoList,
    pub(crate) loads: u64,
}

impl ResolvedTodos {
    pub(crate) fn current(&mut self) -> &cockpit::todo::TodoList {
        let stamp = fs::metadata(cockpit::paths::todo()).ok()
            .map(|file| (file.mtime(), file.mtime_nsec(), file.ctime(), file.ctime_nsec(), file.len(), file.ino()));
        if self.loads == 0 || stamp.is_none() || stamp != self.stamp {
            self.todos = resolve_projects(cockpit::todo::TodoList::load());
            self.stamp = stamp;
            self.loads += 1;
        }
        &self.todos
    }
}

fn resolve_projects(mut todos: cockpit::todo::TodoList) -> cockpit::todo::TodoList {
    for item in &mut todos.items {
        if let Ok(project) = canonical_project(&item.cwd()) {
            item.0.insert("cwd".into(), json!(project.trim_start_matches("path:")));
        }
    }
    todos
}

fn unauthorized() -> TeamError {
    TeamError::new(ErrorCode::Unauthorized, "this process is not a verified live cockpit session")
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
        let connection = registry.teams.as_ref().expect("opened above");
        let mut runtimes = store::approved_runtimes(connection, &session)?;
        let member_runtime = store::member_runtime(connection, &session)?;
        let chief_runtime = cockpit::chief::store::runtime(connection, &session)?;
        if member_runtime.as_ref().zip(chief_runtime.as_ref()).is_some_and(|(member, chief)| member != chief) { continue; }
        if let Some(runtime) = chief_runtime { runtimes.push(runtime); }
        runtimes.sort();
        runtimes.dedup();
        let live: Vec<_> = runtimes.into_iter().filter(|runtime| registry.system.runtime_is_live(runtime)).collect();
        let [runtime] = live.as_slice() else { continue };
        if let Some(entry) = registry.sessions.get_mut(&session) { entry.runtime = runtime.clone(); }
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
        start_spawns(registry)?;
    }
    cockpit::chief::store::observe_resumed(registry.teams.as_ref().expect("opened above"), &live)?;
    let todos = registry.resolved_todos.current();
    cockpit::chief::store::reconcile_todos(registry.teams.as_mut().expect("opened above"), todos, now_ms())?;
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
        // A recorded operation that ended without exiting its old runtime is abandoned too.
        if is_old_live || !permit.note_recorded || !matching_sleep_note(&permit, now_ms()) {
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

pub(crate) fn sleep_operation_is_live(session: &str) -> std::io::Result<bool> {
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

/// Only existing launch directories reach the store; a deleted subfolder can still identify a project.
fn folders(registry: &Registry, live: &[VerifiedSession]) -> std::collections::HashMap<String, String> {
    live.iter().filter_map(|session| registry.sessions.get(&session.session_id).filter(|entry| std::path::Path::new(&entry.cwd).is_dir())
        .map(|entry| (session.session_id.clone(), entry.cwd.clone()))).collect()
}

/// Starts queued recruits and handoffs without waiting, and closes replaced QA sessions.
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

pub(crate) fn verified_sender(registry: &mut Registry, peer: u32, live: &[VerifiedSession]) -> Result<VerifiedSession, TeamError> {
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
    if let AgentAction::Chief { request } = action {
        return crate::chief::request(registry, peer, request);
    }
    let live = live_sessions(registry)?;
    let sender = match verified_sender(registry, peer, &live) {
        Ok(sender) => sender,
        Err(error) => {
            record("rejected", None, false);
            return Err(error);
        }
    };
    if let AgentAction::NativeTelemetry { sequence, counts } = &action {
        return crate::chief::native_telemetry(registry, &sender, *sequence, counts.clone());
    }
    let kind = match &action {
        AgentAction::Context => "context", AgentAction::Propose { .. } => "propose", AgentAction::Send { .. } => "send",
        AgentAction::Receive => "receive", AgentAction::Receipt { .. } => "receipt", AgentAction::Assess { .. } => "assess",
        AgentAction::Handoff { .. } => "handoff", AgentAction::Recruit { .. } => "recruit",
        AgentAction::Chief { .. } | AgentAction::NativeTelemetry { .. } => return Err(TeamError::new(ErrorCode::Unavailable, "action is unavailable here")),
        AgentAction::LinkPropose { .. } => "link_propose", AgentAction::LinkSend { .. } => "link_send", AgentAction::LinkAssess { .. } => "link_assess",
    };
    // The host chooses the default agent; the requesting session cannot choose one.
    let default_agent = if matches!(action, AgentAction::Recruit { .. }) { registry.system.default_agent() } else { None };
    let directories = if matches!(action, AgentAction::Recruit { .. }) { folders(registry, &live) } else { Default::default() };
    // The Developer's real folder, where a fresh QA's copy comes from (never the handoff's own claim).
    let developer_folder = registry.sessions.get(&sender.session_id).map(|entry| entry.cwd.clone()).filter(|cwd| !cwd.is_empty());
    // The daemon's own view of the recipient decides whether it may be woken: never the adapter's claim.
    let turn = registry.sessions.get(&sender.session_id)
        .filter(|entry| !entry.needs_input && entry.state == "waiting")
        .map(|entry| format!("{}:{}", entry.state, entry.since));
    // A Chief answer also reaches a session stopped at its own question, and only at the stop it answers:
    // the idle period as every inbox names it, and the stop boundary the answer was written for.
    let answer_turn = registry.sessions.get(&sender.session_id)
        .filter(|entry| entry.state == "waiting" || (entry.state == "needs-you" && entry.needs_input))
        .and_then(|entry| Some((format!("{}:{}", entry.state, entry.since), crate::chief::stop_boundary(registry, &sender, now_ms())?)));
    // A Chief form reply reaches only a session waiting on a form: `needs-you` without a saved question,
    // where `stop_boundary`'s fresh telemetry rules out a permission prompt.
    let form_turn = registry.sessions.get(&sender.session_id)
        .filter(|entry| entry.state == "needs-you" && !entry.needs_input)
        .and_then(|entry| Some((format!("{}:{}", entry.state, entry.since), crate::chief::stop_boundary(registry, &sender, now_ms())?)));
    let chief_live = if matches!(action, AgentAction::Receive) { crate::chief::workers_among(registry, live.clone(), now_ms(), true)? } else { Vec::new() };
    let is_subscription_held = matches!(action, AgentAction::Receive)
        && cockpit::subscription_fallback::message_target(&sender.session_id)
            .map_err(|error| TeamError::new(ErrorCode::Unavailable, format!("subscription messaging is paused: {error}")))?.1;
    if sender.agent == "codex" {
        if let AgentAction::Receipt { dispatch_id, outcome } = &action {
            if dispatch_id.starts_with("prompt_") {
                if !matches!(outcome, cockpit::teams::TransportOutcome::Received { .. }) {
                    return Err(TeamError::new(ErrorCode::Conflict,"native user message delivery is uncertain; reconcile before retrying"));
                }
                cockpit::prompts::receipt_native(&sender.session_id,&sender.runtime,dispatch_id)
                    .map_err(|error| TeamError::new(ErrorCode::Storage,error))?;
                return Ok(TeamReply { request_id:dispatch_id.clone(),result:Ok(Value::Null) });
            }
        }
        if matches!(action,AgentAction::Receive) && !is_subscription_held && turn.is_some() {
            let routes = cockpit::subscription_fallback::RoutingStore::load(&cockpit::paths::subscription_routes())
                .map_err(|error| TeamError::new(ErrorCode::Storage,error))?;
            if routes.routes().any(|route| route.current.session == sender.session_id && route.current.runtime == sender.runtime
                && route.initial_admitted && matches!(route.phase,cockpit::subscription_fallback::Phase::Active)) {
                let session = registry.snapshot_rows().iter().find(|row| row["session"] == sender.session_id)
                    .and_then(cockpit::sessions::Session::from_value).ok_or_else(unauthorized)?;
                if session.needs_attention() || session.agents != 0 || session.background != 0 || session.ask_to_run == "running"
                    || !registry.system.native_input_clear(&session) {
                    return Ok(TeamReply { request_id:"receive".into(),result:Ok(Value::Null) });
                }
                if let Some(delivery) = cockpit::prompts::claim_native(&sender.session_id,&sender.runtime)
                    .map_err(|error| TeamError::new(ErrorCode::Storage,error))? {
                    if let Some(entry) = registry.sessions.get_mut(&sender.session_id) { entry.state = "working".into(); }
                    registry.publish();
                    return Ok(TeamReply { request_id:"receive".into(),result:Ok(delivery) });
                }
                if cockpit::prompts::folder(&sender.session_id).is_some_and(|folder| folder.join("native-delivery.json").exists()) {
                    return Ok(TeamReply { request_id:"receive".into(),result:Ok(Value::Null) });
                }
            }
        }
    }
    let result = database(registry, &live).and_then(|connection| match &action {
        AgentAction::Context => Ok(TeamReply { request_id: "context".into(), result: Ok(store::context(connection, &sender)?) }),
        AgentAction::Chief { .. } | AgentAction::NativeTelemetry { .. } => Err(TeamError::new(ErrorCode::Unavailable, "action is unavailable here")),
        AgentAction::Propose { proposal } => store::propose(connection, &sender, proposal, &live, now_ms()),
        AgentAction::LinkPropose { proposal } => cockpit::teams::links::propose(connection, &sender, proposal, &live, now_ms()),
        AgentAction::LinkSend { message } => cockpit::teams::links::accept(connection, &sender, message, &live, now_ms()),
        AgentAction::LinkAssess { link_id, thread_id, resolved } => cockpit::teams::links::assess(connection, &sender, link_id, thread_id, *resolved),
        AgentAction::Send { .. } => store::accept(connection, &sender, &action, now_ms()),
        AgentAction::Receive => {
            store::expire(connection, now_ms())?;
            cockpit::chief::store::observe(connection, &chief_live, now_ms())?;
            if is_subscription_held { return Ok(TeamReply { request_id:"receive".into(),result:Ok(Value::Null) }); }
            // The native Receive handoff shares the TODO closure boundary.
            let mut dispatch = match &turn { Some(turn) => store::claim(connection, &sender, turn, &live, now_ms())?, None => None };
            // Theo's correction goes out before any Chief answer.
            if let (None, Some((idle_turn, boundary))) = (&dispatch, &answer_turn) {
                dispatch = cockpit::chief::store::claim_correction(connection, &sender, idle_turn, boundary, now_ms())?;
            }
            if let (None, Some((idle_turn, boundary))) = (&dispatch, &answer_turn) {
                dispatch = cockpit::chief::store::claim_answer(connection, &sender, idle_turn, boundary, now_ms())?;
            }
            if let (None, Some((idle_turn, boundary))) = (&dispatch, &form_turn) {
                dispatch = cockpit::chief::store::claim_form_reply(connection, &sender, idle_turn, boundary, now_ms())?;
            }
            if let (None, Some(turn)) = (&dispatch, &turn) {
                dispatch = cockpit::chief::store::claim(connection, &sender, turn, now_ms())?;
            }
            Ok(TeamReply { request_id: "receive".into(), result: Ok(dispatch.map_or(Value::Null, |dispatch| json!({
                "dispatch_id": dispatch.id, "message_id": dispatch.message_id,
                "text": cockpit::teams::envelope_text(&dispatch.envelope), "envelope": dispatch.envelope}))) })
        },
        AgentAction::Assess { .. } => store::assess(connection, &sender, &action, now_ms()),
        AgentAction::Handoff { .. } => store::handoff_in(connection, &sender, &action, developer_folder.as_deref(), now_ms()),
        AgentAction::Recruit { recruitment } => {
            store::recruit(connection, &sender, recruitment, default_agent.as_deref(), &live, &directories, now_ms())
        },
        AgentAction::Receipt { dispatch_id, outcome } => {
            if cockpit::chief::store::owns_dispatch(connection, dispatch_id)? {
                cockpit::chief::store::finish_for(connection, &sender, dispatch_id, outcome, now_ms())?;
            } else {
                store::finish_for(connection, &sender, dispatch_id, outcome, now_ms())?;
            }
            Ok(TeamReply { request_id: dispatch_id.clone(), result: Ok(json!({"status": "recorded"})) })
        },
    });
    if !matches!(kind, "context" | "receive") || result.as_ref().is_ok_and(|reply| reply.result.as_ref().is_ok_and(|value| !value.is_null())) {
        record(kind, Some(&sender), result.as_ref().is_ok_and(|reply| reply.result.is_ok()));
    }
    if matches!(kind, "handoff" | "recruit") && result.as_ref().is_ok_and(|reply| reply.result.is_ok()) {
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
        sessions.push(json!({"session_id": session.session_id, "agent": session.agent, "project": session.project, "runtime": session.runtime,
            "project_name": entry.map(|entry| entry.project.as_str()).unwrap_or(""), "label": entry.map(|entry| entry.label.as_str()).unwrap_or(""),
            "state": entry.map(|entry| entry.state.as_str()).unwrap_or(""), "cwd": entry.map(|entry| entry.cwd.as_str()).unwrap_or("")}));
    }
    let mut state = store::snapshot(database(registry, &live)?, None, None, 100)?;
    state["sessions"] = json!(sessions);
    state["chief_session"] = json!(cockpit::chief::store::chief_session(registry.teams.as_ref().expect("opened above"))?);
    let mut sleeps = Vec::new();
    for permit in store::sleep::prepared_records(registry.teams.as_ref().expect("opened above"))? {
        let mut status = store::sleep::status(registry.teams.as_ref().expect("opened above"), &permit.session_id, &permit.token)?;
        status["session_id"] = json!(permit.session_id);
        sleeps.push(status);
    }
    state["sleeps"] = json!(sleeps);
    state["chief_answers"] = json!(cockpit::chief::store::answer_counts(registry.teams.as_ref().expect("opened above"))?);
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
        cockpit::teams::HostRequest::Chief { action } => crate::chief::host(registry, &action),
        cockpit::teams::HostRequest::ChiefSnapshot { after, limit, generation } => Ok(TeamReply { request_id: "chief-snapshot".into(), result: Ok(crate::chief::snapshot(registry, after, limit, generation.as_deref())?) }),
        cockpit::teams::HostRequest::ChiefAudit { after, limit, generation } => {
            let live=crate::chief::workers(registry)?;
            Ok(TeamReply {request_id:"chief-audit".into(),result:cockpit::chief::store::audit_page(registry.teams.as_ref().ok_or_else(||TeamError::new(ErrorCode::Unavailable,"chief storage unavailable"))?,&live,after,limit,generation.as_deref(),now_ms())})
        },
        cockpit::teams::HostRequest::ReserveManual {task_id,directory} => {
            live_sessions(registry)?;
            let task=cockpit::teams::TaskKey {project:canonical_project(&directory)?,task_id};
            task.validate()?;
            let todos=cockpit::todo::TodoList::load();
            let item=todos.items.iter().find(|item|item.id()==task.task_id);
            if todos.status_of(&task.task_id)!="todo" || match item {
                Some(item)=>canonical_project(&item.cwd())?!=task.project,
                None=>!task.task_id.contains('#'),
            } {return Err(unauthorized());}
            let token=cockpit::chief::store::reserve_legacy_kind(registry.teams.as_mut().unwrap(),&task,None,true,now_ms())?;
            Ok(TeamReply {request_id:"reserve-manual".into(),result:Ok(json!({"reservation":token}))})
        },
        cockpit::teams::HostRequest::ReleaseManual {task_id,directory,token} => {
            live_sessions(registry)?;
            let project=canonical_project(&directory)?;
            if cockpit::todo::TodoList::load().status_of(&task_id)=="doing" {return Err(TeamError::new(ErrorCode::Conflict,"undo the manual status before releasing it"));}
            cockpit::chief::store::release_manual(registry.teams.as_mut().unwrap(),&project,&task_id,&token)?;
            Ok(TeamReply {request_id:"release-manual".into(),result:Ok(json!({"released_manual_only":true}))})
        },
        cockpit::teams::HostRequest::SettleReservation {token,expected_revision,outcome} => {
            live_sessions(registry)?;
            Ok(TeamReply {request_id:"settle-reservation".into(),result:cockpit::chief::store::settle_launch(registry.teams.as_mut().unwrap(),&token,expected_revision,&outcome)})
        },
        cockpit::teams::HostRequest::ReserveTask { task_id, directory, worker, automatic } => {
            let live = live_sessions(registry)?;
            let task = cockpit::teams::TaskKey { project: canonical_project(&directory)?, task_id };
            task.validate()?;
            if automatic && !crate::chief::legacy_auto_ready(registry,worker.as_deref(),&task.project) {
                return Err(TeamError::new(ErrorCode::Conflict,"automatic legacy admission is no longer ready"));
            }
            let todos = cockpit::todo::TodoList::load();
            let item = todos.items.iter().find(|item| item.id() == task.task_id);
            if !task.task_id.contains('#') && item.is_none() { return Err(unauthorized()); }
            if let Some(item) = item {
                if canonical_project(&item.cwd())? != task.project || item.status() != "todo" || item.is_waiting() {
                    return Err(TeamError::new(ErrorCode::Conflict, "task is not unassigned ready work in this project"));
                }
            } else if todos.status_of(&task.task_id) != "todo" { return Err(unauthorized()); }
            if let Some(id) = &worker {
                let target = live.iter().find(|session| &session.session_id == id && session.project == task.project);
                let role = if let Some(target) = target {
                    cockpit::chief::store::require_non_chief(registry.teams.as_ref().unwrap(), &target.session_id)?;
                    cockpit::chief::store::require_task_mapping(registry.teams.as_ref().unwrap(), target, &task)?;
                    cockpit::chief::store::worker_role(registry.teams.as_ref().unwrap(), target)?
                } else {
                    // Only the existing host sleep ledger can authorize a legacy wake; not a dead card or agent claim.
                    let asleep = cockpit::sleep::asleep().into_iter().any(|note| &note.session == id
                        && cockpit::agents::of_session_id(id).is_some_and(|agent|agent.name==note.agent)
                        && canonical_project(&note.cwd).is_ok_and(|project|project==task.project));
                    if !asleep || cockpit::chief::store::is_product_worker(registry.teams.as_ref().unwrap(),id)? { return Err(unauthorized()); }
                    cockpit::chief::store::require_non_chief(registry.teams.as_ref().unwrap(), id)?;
                    None
                };
                let designer = role == Some(cockpit::teams::Role::Designer) || (role.is_none() && cockpit::sessions::role_of(id) == "designer");
                if designer != item.is_some_and(|item| item.is_design()) { return Err(unauthorized()); }
            }
            let token = cockpit::chief::store::reserve_legacy(registry.teams.as_mut().unwrap(), &task, worker.as_deref(), now_ms())?;
            Ok(TeamReply { request_id: "reserve-task".into(), result: Ok(json!({"reservation":token})) })
        },
        cockpit::teams::HostRequest::ReserveResume { session, directory } => {
            let live = live_sessions(registry)?;
            if cockpit::agents::of_session_id(&session).is_none() || live.iter().any(|worker| worker.session_id == session) {
                return Err(TeamError::new(ErrorCode::Conflict,"conversation is already live or has no valid native identity"));
            }
            let task = cockpit::teams::TaskKey { project: canonical_project(&directory)?, task_id: format!("resume:{session}") };
            let token = cockpit::chief::store::reserve_resume(registry.teams.as_mut().unwrap(), &task, &session, now_ms())?;
            Ok(TeamReply { request_id: "reserve-resume".into(), result: Ok(json!({"reservation":token})) })
        },
        cockpit::teams::HostRequest::ChiefFocus { viewed, workspace_active, gaming } => {
            registry.chief_focus = Some((viewed, workspace_active, gaming, now_ms()));
            Ok(TeamReply { request_id: "chief-focus".into(), result: Ok(json!({"recorded":true})) })
        },
        cockpit::teams::HostRequest::ChiefNativeObservation { session, idle, pending_input, observed_at_ms } => {
            if !live_sessions(registry)?.contains(&session) || session.agent != "opencode"
                || cockpit::agents::of_session_id(&session.session_id).map(|agent|agent.name) != Some("opencode") { return Err(unauthorized()); }
            let now = now_ms();
            if observed_at_ms > now || now.saturating_sub(observed_at_ms) > 30_000 {
                return Err(TeamError::new(ErrorCode::Unavailable,"native host observation is expired or from the future"));
            }
            registry.chief_native.insert(session.session_id.clone(), (session, idle, pending_input, observed_at_ms));
            Ok(TeamReply { request_id: "chief-native-observation".into(), result: Ok(json!({"recorded":true})) })
        },
        cockpit::teams::HostRequest::VerifyChiefAdmission { session, adapter, evidence } => {
            if !live_sessions(registry)?.contains(&session) || session.agent != "opencode"
                || cockpit::agents::of_session_id(&session.session_id).map(|agent|agent.name) != Some("opencode") || adapter != "opencode-queued-v1"
                || evidence.trim().is_empty() || evidence.len() > 2000 || evidence.chars().any(char::is_control) { return Err(unauthorized()); }
            cockpit::chief::store::verify_admission(registry.teams.as_mut().unwrap(), &session, &adapter, &evidence, now_ms())?;
            Ok(TeamReply { request_id: "verify-chief-admission".into(), result: Ok(json!({"recorded":true})) })
        },
        cockpit::teams::HostRequest::SubscriptionSource { session } => {
            let entry = registry.sessions.get(&session).cloned();
            let value = if entry.as_ref().is_some_and(|entry| !entry.runtime.is_empty()
                && registry.system.runtime_is_live(&entry.runtime)) {
                let mut row = registry.snapshot_rows().into_iter().find(|row| row["session"] == session).unwrap_or(Value::Null);
                let entry = entry.unwrap();
                row["runtime"] = json!(entry.runtime);
                row["subscription_account"] = json!(registry.system.runtime_subscription_account(&entry.runtime));
                let member = VerifiedSession { session_id:session.clone(), agent:entry.agent, runtime:entry.runtime,
                    project:canonical_project(&entry.cwd)? };
                open_and_restore(registry)?;
                row["team_task"] = store::context(registry.teams.as_ref().expect("opened above"), &member)?["task"]["task_id"].clone();
                row
            } else { Value::Null };
            Ok(TeamReply { request_id: session, result: Ok(value) })
        },
        cockpit::teams::HostRequest::SubscriptionRuntime { generation } => {
            let runtime = registry.subscription_runtime(&generation)
                .map_err(|error| TeamError::new(ErrorCode::Conflict, error))?;
            Ok(TeamReply { request_id: generation, result: Ok(runtime) })
        },
        cockpit::teams::HostRequest::SubscriptionBind { origin, generation } => bind_subscription(registry, &origin, &generation),
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
            if sleep_operation_is_live(&session_id)? {
                return Err(TeamError::new(ErrorCode::Conflict, "sleep operation still has live exit helpers"));
            }
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

fn bind_subscription(registry: &mut Registry, origin: &str, generation: &str) -> Result<TeamReply, TeamError> {
    use cockpit::subscription_fallback::{Binding, Phase, RoutingStore};
    let conflict = |message: &str| TeamError::new(ErrorCode::Conflict, message);
    let storage = |message: String| TeamError::new(ErrorCode::Storage, message);
    let mut routes = RoutingStore::load(&cockpit::paths::subscription_routes()).map_err(storage)?;
    let route = routes.route(origin).cloned().ok_or_else(|| conflict("no such subscription route"))?;
    let receipt = registry.subscription_runtime(generation).map_err(|error| conflict(&error))?;
    let session = receipt["session"].as_str().ok_or_else(|| conflict("subscription replacement has not reported"))?;
    let runtime = receipt["runtime"].as_str().ok_or_else(|| conflict("subscription replacement has no verified runtime"))?;
    if matches!(route.phase, Phase::Active) && route.admitted_generation.as_deref() == Some(generation)
        && route.current.session == session && route.current.runtime == runtime && receipt["cwd"].as_str() == Some(&route.directory) {
        return Ok(TeamReply { request_id: generation.into(), result: Ok(json!({"session":session,"bound":true})) });
    }
    let Phase::Pending(pending) = &route.phase else { return Err(conflict("subscription route is not pending")) };
    if pending.generation != generation { return Err(conflict("subscription routing generation changed")); }
    let mut source_runtimes = vec![pending.source.runtime.clone()];
    if let Some(entry) = registry.sessions.get(&pending.source.session) { source_runtimes.push(entry.runtime.clone()); }
    if source_runtimes.iter().any(|runtime| !runtime.is_empty() && registry.system.runtime_is_live(runtime)) {
        return Err(conflict("subscription source has not retired or was manually restarted"));
    }
    if receipt["agent"].as_str() != Some(&pending.target.agent) || receipt["cwd"].as_str() != Some(&route.directory) {
        return Err(conflict("subscription runtime does not match the planned agent and checkout"));
    }
    let replacement = registry.snapshot_rows().iter().find(|row| row["session"] == session)
        .and_then(cockpit::sessions::Session::from_value).ok_or_else(|| conflict("replacement metadata is unavailable"))?;
    if replacement.cwd != route.directory || replacement.branch != pending.source_branch || replacement.repository != pending.source_repository {
        return Err(conflict("replacement task metadata does not match the retired source"));
    }
    let binding = Binding { session:session.into(),runtime:runtime.into(),selection:pending.target.clone(),
        native_thread:receipt["native_thread"].as_str().filter(|native| !native.is_empty()).map(str::to_string) };
    routes.record_runtime(generation, binding, &route.directory).map_err(storage)?;
    let source = cockpit::sessions::Session { session:pending.source.session.clone(),agent:pending.source.selection.agent.clone(),
        cwd:route.directory.clone(),branch:pending.source_branch.clone(),repository:pending.source_repository.clone(), ..Default::default() };
    // Taken before any store commits: a busy TODO writer fails the bind closed with nothing moved.
    let todo_guard = cockpit::todo::lock_mutations().map_err(|error| storage(format!("cannot serialize subscription task transfer: {error}")))?;
    let mut todos = cockpit::todo::TodoList::load_checked().map_err(storage)?;
    if !route.is_session_task() && todos.status_of(&route.task) != "doing" { return Err(conflict("subscription task is missing or no longer in progress")); }
    let mut continuity = cockpit::continuity::Continuity::load(&cockpit::paths::continuity()).map_err(storage)?;
    let project = canonical_project(&route.directory)?;
    let task = cockpit::teams::TaskKey { project:project.clone(),task_id:route.task.clone() };
    let source_member = VerifiedSession { session_id:source.session.clone(),agent:source.agent.clone(),project:project.clone(),runtime:pending.source.runtime.clone() };
    let replacement_member = VerifiedSession { session_id:session.into(),agent:pending.target.agent.clone(),project,runtime:runtime.into() };
    open_and_restore(registry)?;
    let connection = registry.teams.as_ref().expect("opened above");
    let source_context = store::context(connection, &source_member)?;
    let replacement_context = store::context(connection, &replacement_member)?;
    let owner = todos.items.iter().find(|item| item.id() == route.task).map(|item| item.session())
        .or_else(|| todos.issues.get(&route.task).and_then(|issue| issue["session"].as_str()).map(str::to_string));
    let has_other_team_owner = owner.as_deref().is_some_and(|owner| owner != source.session && owner != session
        && [&source_context, &replacement_context].iter().any(|context| context["task"] == json!(task)
            && context["members"].as_array().is_some_and(|members| members.iter().any(|member| member["session_id"] == owner))));
    let has_todo_transfer = if has_other_team_owner {
        if todos.status_of(&route.task) != "doing" { return Err(conflict("subscription team task is no longer in progress")); }
        false
    } else { todos.transfer_subscription_task(&route.task, &source.session, session).map_err(storage)? };
    store::replace_subscription_member(registry.teams.as_mut().expect("opened above"), generation, &task, &source_member, &replacement_member)?;
    continuity.transfer_subscription_task(&route.task, &source, &replacement, now_ms() / 1000).map_err(storage)?;
    if has_todo_transfer { todos.save_guarded(&todo_guard)?; }
    drop(todo_guard);
    cockpit::prompts::transfer_subscription_prompts(&source.session, session).map_err(storage)?;
    routes.finish(origin, generation).map_err(storage)?;
    cockpit::audit::record("daemon", "subscription.switched", "ok", json!({"origin":origin, "generation":generation,
        "task":route.task, "direction":format!("{:?}", pending.direction), "from_agent":pending.source.selection.agent,
        "to_agent":pending.target.agent, "to_model":pending.target.model, "session":session}));
    registry.publish();
    Ok(TeamReply { request_id:generation.into(),result:Ok(json!({"session":session,"bound":true})) })
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
