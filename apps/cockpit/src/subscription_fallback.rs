//! Explicit subscription-routing decisions and bounded historical handoffs.

pub mod native;

use crate::{remote, sessions::Session};
use serde_json::json;
use crate::{agents, usage::SubscriptionCapacity, util};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs::{self, File, OpenOptions}, io::Write,
    os::{fd::AsRawFd, unix::fs::{DirBuilderExt, OpenOptionsExt}}, path::{Path, PathBuf}};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub variant: Option<String>,
    pub account: String,
}

impl Selection {
    fn valid(&self) -> bool {
        let plain = |value: &str| !value.is_empty() && value.len() <= 200 && !value.starts_with('-')
            && value.chars().all(|character| character.is_ascii_alphanumeric() || "._:/-".contains(character));
        matches!((self.agent.as_str(), self.provider.as_str()), ("claude", "anthropic") | ("codex", "openai")
            | ("opencode", "anthropic" | "openai"))
            && plain(&self.model) && self.variant.as_ref().is_none_or(|variant| plain(variant))
            && self.account.len() == 64 && self.account.bytes().all(|byte| byte.is_ascii_hexdigit())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub session: String,
    pub runtime: String,
    pub selection: Selection,
    pub native_thread: Option<String>,
}

impl Binding {
    fn valid(&self) -> bool {
        self.selection.valid() && agents::of_session_id(&self.session).is_some_and(|agent| agent.name == self.selection.agent)
            && !self.runtime.is_empty() && crate::sessions::valid_metadata_text(&self.runtime, 200)
            && if self.selection.agent == "codex" {
                self.native_thread.as_deref().and_then(|native| crate::codex::cockpit_id(native).ok()).as_deref() == Some(&self.session)
            } else { self.native_thread.is_none() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction { Fallback, Return }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage { Prepared, Retired, Submitted, Bound }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub origin: String,
    pub generation: String,
    pub source: Binding,
    #[serde(default)]
    pub source_repository: String,
    #[serde(default)]
    pub source_branch: String,
    pub target: Selection,
    pub direction: Direction,
    pub stage: Stage,
    pub replacement: Option<Binding>,
    #[serde(default)]
    pub retirement_started: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase { Active, Pending(Transition), Paused(String), Cancelled, Completed }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub original: Binding,
    pub current: Binding,
    pub directory: String,
    pub task: String,
    pub phase: Phase,
    pub updated_at: i64,
    #[serde(default)]
    pub admitted_generation: Option<String>,
    #[serde(default)]
    pub initial_admitted: bool,
    #[serde(default)]
    pub initial_started: bool,
    #[serde(default)]
    pub retired_sessions: Vec<String>,
    /// Why an automatic switch cannot happen yet, for the cockpit's message line.
    #[serde(default)]
    pub waiting: Option<String>,
}

impl Route {
    pub fn is_session_task(&self) -> bool {
        self.task == format!("native-session:{}",self.original.session)
    }
    pub fn awaiting_initial_admission(&self) -> bool {
        self.admitted_generation.is_some() && !self.initial_started
    }
    fn owns_session(&self, session: &str) -> bool {
        matches!(self.phase, Phase::Active | Phase::Pending(_) | Phase::Paused(_))
            && (self.current.session == session || self.retired_sessions.iter().any(|retired| retired == session)
                || (self.original.session == session && self.original.session != self.current.session)
                || matches!(&self.phase, Phase::Pending(pending) if pending.replacement.as_ref().is_some_and(|replacement| replacement.session == session)))
    }
}

#[derive(Clone)]
pub struct Observation {
    pub binding: Binding,
    pub session: Session,
    pub current_capacity: SubscriptionCapacity,
    pub original_capacity: SubscriptionCapacity,
    pub alternate: Option<(Selection, SubscriptionCapacity)>,
    pub has_pending_work: bool,
    pub prompt_clear: bool,
    pub manual_override: bool,
}

impl Observation {
    fn safe_boundary(&self) -> bool {
        let session = &self.session;
        self.prompt_clear && session.session == self.binding.session && session.agent == self.binding.selection.agent
            && session.state == "waiting" && !session.needs_attention() && session.ask_to_run != "running"
            && !session.terminal && !session.credits && !session.restarting && session.asleep.is_none()
            && !session.is_external() && session.agents == 0 && session.background == 0
    }
}

/// `returning`: the route already runs on its alternate. None when nothing is blocked.
pub fn waiting_reason(observed: &Observation, returning: bool) -> Option<String> {
    use SubscriptionCapacity::{Available, Exhausted};
    let reason = if returning && observed.original_capacity == Available {
        (!observed.safe_boundary()).then_some("waiting for the session to finish its turn before returning to the original model")
    } else if observed.current_capacity != Exhausted {
        None
    } else {
        match &observed.alternate {
            None => Some("the other subscription's account or quota could not be read"),
            Some((_, Exhausted)) => Some("both subscriptions are exhausted"),
            Some((_, Available)) => (!observed.safe_boundary())
                .then_some("waiting for the session to finish its turn and clear any typed draft"),
            Some((selection, _)) if selection.agent == "claude" => Some("Claude quota is unconfirmed or its paid extra usage is on"),
            Some(_) => Some("Codex quota is unconfirmed"),
        }
    };
    reason.map(str::to_string)
}

/// Input-row evidence only: callers must also verify the runtime, idle state and absence of dialogs/work.
/// Unknown layouts are unconfirmed; a cursor at Home is not proof that a draft is empty.
pub fn native_input_is_clear(agent: &str, line: &str, cursor_column: u16) -> bool {
    if cursor_column != 2 { return false; }
    match agent {
        "codex" => line == "\x1b[1m›\x1b[0m \x1b[2mAsk Codex to do anything\x1b[0m",
        "claude" => line == "❯ " || line == "❯\u{a0}",
        _ => false,
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutingState {
    version: u8,
    routes: BTreeMap<String, Route>,
}

pub struct RoutingStore {
    path: PathBuf,
    state: RoutingState,
    lock: File,
}

impl Drop for RoutingStore {
    fn drop(&mut self) {
        // Forked subprocesses can retain the open description until exec, even with CLOEXEC.
        if unsafe { libc::flock(self.lock.as_raw_fd(), libc::LOCK_UN) } != 0 {
            util::log_event("WARN", "could not release the subscription routing lock");
        }
    }
}

fn read_state(path: &Path) -> Result<RoutingState, String> {
    let state = match util::read_untrusted(path, 1024 * 1024 + 1) {
        Ok(bytes) if bytes.len() <= 1024 * 1024 => serde_json::from_slice::<RoutingState>(&bytes).map_err(|error| format!("invalid routing state: {error}"))?,
        Ok(_) => return Err("routing state exceeds 1 MiB".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => RoutingState { version: 1, routes: BTreeMap::new() },
        Err(error) => return Err(format!("cannot read routing state: {error}")),
    };
    validate_state(&state)?;
    Ok(state)
}

/// Writers publish by atomic rename; read-only dispatch holds must not seize their exclusive lock.
pub fn read_routes(path: &Path) -> Result<Vec<Route>, String> {
    Ok(read_state(path)?.routes.into_values().collect())
}

pub fn message_target(session: &str) -> Result<(String, bool), String> {
    let agent = agents::of_session_id(session).ok_or("invalid message session identity")?;
    if agent.name == "opencode" { return Ok((session.into(), false)); }
    let routes = read_routes(&crate::paths::subscription_routes())?;
    Ok(routes.iter().find(|route| route.owns_session(session)).map_or_else(|| (session.into(), false),
         |route| (route.current.session.clone(), route.awaiting_initial_admission() || matches!(route.phase, Phase::Pending(_) | Phase::Paused(_)))))
}

pub fn has_native_inbox(session: &str) -> Result<bool, String> {
    Ok(read_routes(&crate::paths::subscription_routes())?.iter().any(|route| route.current.session == session
        && route.current.selection.agent == "codex" && route.initial_admitted && matches!(route.phase, Phase::Active)))
}

impl RoutingStore {
    pub fn load(path: &Path) -> Result<Self, String> {
        let parent = path.parent().ok_or("routing path has no parent")?;
        fs::DirBuilder::new().recursive(true).mode(0o700).create(parent).map_err(|error| error.to_string())?;
        let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path.with_extension("lock")).map_err(|error| error.to_string())?;
        if !lock.metadata().map_err(|error| error.to_string())?.is_file() { return Err("routing lock is not a regular file".into()); }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("subscription routing is already being reconciled".into());
        }
        let state = read_state(path)?;
        Ok(Self { path: path.into(), state, lock })
    }

    pub fn route(&self, origin: &str) -> Option<&Route> { self.state.routes.get(origin) }

    pub fn routes(&self) -> impl Iterator<Item = &Route> { self.state.routes.values() }

    pub fn prompt_destination<'a>(&'a self, session: &'a str) -> &'a str {
        self.routes().find(|route| route.owns_session(session))
            .map_or(session, |route| route.current.session.as_str())
    }

    pub fn execution_allowed(&self, generation: &str, session: &str, runtime: &str) -> bool {
        self.state.routes.values().any(|route| route.admitted_generation.as_deref() == Some(generation)
            && route.current.session == session && route.current.runtime == runtime
            && (matches!(route.phase, Phase::Active) || route.initial_admitted && matches!(route.phase, Phase::Completed | Phase::Cancelled)))
    }

    pub fn track(&mut self, original: Binding, directory: &str, task: &str, now: i64) -> Result<(), String> {
        if self.state.routes.values().any(|route| route.current.session == original.session
            || (!matches!(route.phase, Phase::Cancelled | Phase::Completed) && route.retired_sessions.contains(&original.session))
            || matches!(&route.phase, Phase::Pending(pending) if pending.replacement.as_ref().is_some_and(|replacement| replacement.session == original.session))) {
            return Ok(());
        }
        let mut next = self.state.clone();
        next.routes.entry(original.session.clone()).or_insert_with(|| Route {
            current: original.clone(), original, directory: directory.into(), task: task.into(), phase: Phase::Active, updated_at: now,
            admitted_generation: None,
            initial_admitted: false,
            initial_started: false,
            waiting: None,
            retired_sessions: Vec::new(),
        });
        self.save(next)
    }

    pub fn reconcile(&mut self, observations: &[Observation], now: i64) -> Result<Vec<Transition>, String> {
        use SubscriptionCapacity::{Available, Exhausted};
        let mut next = self.state.clone();
        let mut transitions = Vec::new();
        for (origin, route) in &mut next.routes {
            let Some(observed) = observations.iter().find(|observed| observed.binding.session == route.current.session) else { continue };
            if matches!(route.phase, Phase::Cancelled | Phase::Completed) { continue; }
            if !observed.has_pending_work { route.phase = Phase::Completed; route.updated_at = now; continue; }
            if observed.manual_override || observed.binding != route.current {
                route.phase = Phase::Cancelled; route.updated_at = now; continue;
            }
            if matches!(route.phase, Phase::Pending(_)) || !observed.safe_boundary() || observed.session.cwd != route.directory { continue; }
            let returned = route.current.selection != route.original.selection;
            let target = if returned && observed.original_capacity == Available {
                Some((route.original.selection.clone(), Direction::Return))
            } else if observed.current_capacity == Exhausted {
                observed.alternate.as_ref().filter(|(selection, capacity)| *capacity == Available && selection.valid()
                    && selection.provider != route.current.selection.provider
                    && matches!((route.current.selection.agent.as_str(), selection.agent.as_str()),
                        ("claude", "codex") | ("codex", "claude") | ("opencode", "opencode")))
                    .map(|(selection, _)| (selection.clone(), Direction::Fallback))
            } else { None };
            if let Some((target, direction)) = target {
                let transition = Transition { origin: origin.clone(), generation: util::token_hex(16), source: route.current.clone(),
                    source_repository: observed.session.repository.clone(), source_branch: observed.session.branch.clone(),
                    target, direction, stage: Stage::Prepared, replacement: None, retirement_started: false };
                route.phase = Phase::Pending(transition.clone());
                route.updated_at = now;
                transitions.push(transition);
            }
        }
        self.save(next)?;
        Ok(transitions)
    }

    pub fn mark_retired(&mut self, origin: &str, generation: &str) -> Result<(), String> {
        self.advance(origin, generation, Stage::Prepared, Stage::Retired)
    }

    pub fn mark_retirement_started(&mut self, origin: &str, generation: &str) -> Result<(), String> {
        let mut next = self.state.clone();
        let pending = pending_mut(&mut next, origin, generation)?;
        if pending.stage != Stage::Prepared { return Err("subscription source is not prepared for retirement".into()); }
        pending.retirement_started = true;
        self.save(next)
    }

    /// True when the reason changed, so callers audit and show it only once.
    pub fn set_waiting(&mut self, origin: &str, reason: Option<&str>) -> Result<bool, String> {
        let mut next = self.state.clone();
        let route = next.routes.get_mut(origin).ok_or("no such subscription route")?;
        if route.waiting.as_deref() == reason { return Ok(false); }
        route.waiting = reason.map(str::to_string);
        self.save(next)?;
        Ok(true)
    }

    pub fn cancel(&mut self, origin: &str) -> Result<(), String> {
        let mut next = self.state.clone();
        let route = next.routes.get_mut(origin).ok_or("no such subscription route")?;
        route.phase = Phase::Cancelled;
        route.updated_at = util::now() as i64;
        self.save(next)
    }

    pub fn mark_submitted(&mut self, origin: &str, generation: &str) -> Result<(), String> {
        self.advance(origin, generation, Stage::Retired, Stage::Submitted)
    }

    fn advance(&mut self, origin: &str, generation: &str, expected: Stage, stage: Stage) -> Result<(), String> {
        let mut next = self.state.clone();
        let pending = pending_mut(&mut next, origin, generation)?;
        if pending.stage != expected { return Err("routing stage changed; reconcile before retrying".into()); }
        pending.stage = stage;
        self.save(next)
    }

    pub fn record_runtime(&mut self, generation: &str, replacement: Binding, directory: &str) -> Result<(), String> {
        if !replacement.valid() { return Err("invalid replacement runtime identity".into()); }
        let mut next = self.state.clone();
        let route = next.routes.values_mut().find(|route| matches!(&route.phase, Phase::Pending(pending) if pending.generation == generation))
            .ok_or("no pending routing generation")?;
        let Phase::Pending(pending) = &mut route.phase else { unreachable!() };
        if pending.stage == Stage::Bound && pending.replacement.as_ref() == Some(&replacement) && directory == route.directory {
            return Ok(());
        }
        if pending.stage != Stage::Submitted || directory != route.directory || replacement.selection != pending.target
            || (pending.direction == Direction::Return && replacement.session != route.original.session) {
            return Err("replacement does not match the pending route".into());
        }
        pending.replacement = Some(replacement);
        pending.stage = Stage::Bound;
        self.save(next)
    }

    pub fn finish(&mut self, origin: &str, generation: &str) -> Result<(), String> {
        let mut next = self.state.clone();
        let pending = pending_mut(&mut next, origin, generation)?;
        if pending.stage != Stage::Bound { return Err("replacement runtime has not been verified".into()); }
        let replacement = pending.replacement.clone().ok_or("replacement runtime is missing")?;
        let route = next.routes.get_mut(origin).unwrap();
        route.retired_sessions.retain(|session| *session != replacement.session);
        if route.current.session != replacement.session && !route.retired_sessions.contains(&route.current.session) {
            route.retired_sessions.push(route.current.session.clone());
        }
        route.current = replacement;
        route.admitted_generation = Some(generation.into());
        route.initial_admitted = false;
        route.initial_started = false;
        route.phase = Phase::Active;
        self.save(next)
    }

    pub fn confirm_initial_admission(&mut self, origin: &str, generation: &str) -> Result<(), String> {
        let mut next = self.state.clone();
        let route = next.routes.get_mut(origin).ok_or("no such subscription route")?;
        if !matches!(route.phase, Phase::Active) || route.admitted_generation.as_deref() != Some(generation) {
            return Err("automatic subscription admission was cancelled".into());
        }
        route.initial_admitted = true;
        self.save(next)
    }

    pub fn start_initial_turn(&mut self, origin: &str, generation: &str) -> Result<(), String> {
        let mut next = self.state.clone();
        let route = next.routes.get_mut(origin).ok_or("no such subscription route")?;
        if !route.initial_admitted || !matches!(route.phase,Phase::Active) || route.admitted_generation.as_deref() != Some(generation) {
            return Err("initial subscription turn is no longer authorized".into());
        }
        route.initial_started = true;
        self.save(next)
    }

    fn save(&mut self, next: RoutingState) -> Result<(), String> {
        validate_state(&next)?;
        if next == self.state { return Ok(()); }
        let bytes = serde_json::to_vec(&next).map_err(|error| error.to_string())?;
        if bytes.len() > 1024 * 1024 { return Err("routing state exceeds 1 MiB".into()); }
        let temporary = self.path.with_extension(format!("tmp-{}", util::token_hex(12)));
        let mut file = OpenOptions::new().create_new(true).write(true).mode(0o600).open(&temporary).map_err(|error| error.to_string())?;
        let result = (|| -> std::io::Result<()> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            File::open(self.path.parent().unwrap())?.sync_all()
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(temporary);
            return Err(format!("cannot persist routing state: {error}"));
        }
        self.state = next;
        Ok(())
    }
}

fn pending_mut<'a>(state: &'a mut RoutingState, origin: &str, generation: &str) -> Result<&'a mut Transition, String> {
    match state.routes.get_mut(origin).map(|route| &mut route.phase) {
        Some(Phase::Pending(pending)) if pending.generation == generation => Ok(pending),
        _ => Err("routing generation is not pending".into()),
    }
}

fn validate_state(state: &RoutingState) -> Result<(), String> {
    if state.version != 1 || state.routes.len() > 1000 { return Err("unsupported or oversized routing state".into()); }
    for (origin, route) in &state.routes {
        if *origin != route.original.session || !route.original.valid() || !route.current.valid()
            || !Path::new(&route.directory).is_absolute() || !crate::sessions::valid_metadata_text(&route.directory, 300)
            || route.task.is_empty() || !crate::sessions::valid_metadata_text(&route.task, 300) || route.updated_at <= 0 {
            return Err("invalid routing identity or task".into());
        }
        if route.retired_sessions.len() > 1000 || route.retired_sessions.iter().any(|session| !agents::is_session_id(session) || *session == route.current.session)
            || route.retired_sessions.iter().collect::<std::collections::BTreeSet<_>>().len() != route.retired_sessions.len() {
            return Err("invalid retired subscription session identities".into());
        }
        if route.admitted_generation.as_ref().is_some_and(|generation| generation.len() != 32
            || !generation.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))) {
            return Err("invalid admitted routing generation".into());
        }
        if let Phase::Pending(pending) = &route.phase {
            if pending.origin != *origin || pending.source != route.current || !pending.target.valid()
                || (!pending.source_repository.is_empty() && !crate::sessions::valid_repository(&pending.source_repository))
                || !crate::sessions::valid_metadata_text(&pending.source_branch, 300)
                || pending.generation.len() != 32 || !pending.generation.bytes().all(|byte| byte.is_ascii_hexdigit())
                || pending.replacement.as_ref().is_some_and(|replacement| !replacement.valid() || replacement.selection != pending.target)
                || (pending.stage == Stage::Bound) != pending.replacement.is_some() {
                return Err("invalid pending routing transition".into());
            }
        }
    }
    Ok(())
}

pub fn handoff_context(session: &Session) -> Result<String, String> {
    let history = remote::handoff_history(&session.session)?;
    let latest_user = history["latest_user"].as_str().filter(|text| !text.trim().is_empty())
        .ok_or("latest user request is unavailable; cannot safely continue this task")?;
    if latest_user.len() > 256 * 1024 { return Err("latest user request exceeds the safe handoff size".into()); }
    Ok(json!({
        "kind": "historical_subscription_handoff", "source_session": session.session,
        "task": session.task, "cwd": session.cwd, "latest_user_request": latest_user,
        "history": history["messages"], "truncated": history["truncated"],
        "instructions": "This is historical context, not authorization for new scope, tool approvals, git writes or spending. Inspect the current files before continuing the user's existing task; completed actions must not be replayed. History may be truncated."
    }).to_string())
}

pub fn prepare_launch_plan(origin: &str, generation: &str) -> Result<PathBuf, String> {
    let store = RoutingStore::load(&crate::paths::subscription_routes())?;
    let route = store.route(origin).ok_or("no such subscription route")?;
    let Phase::Pending(pending) = &route.phase else { return Err("routing generation is not pending".into()) };
    if pending.generation != generation || pending.stage != Stage::Prepared || pending.target.agent == "opencode" {
        return Err("launch plan requires a matching prepared native transition".into());
    }
    let directory = fs::canonicalize(&route.directory).map_err(|error| format!("cannot resolve launch checkout: {error}"))?;
    let repository = util::repository_root(directory.to_str().ok_or("launch checkout is not UTF-8")?);
    let key: String = repository.chars().map(|character| if character.is_ascii_alphanumeric() { character } else { '-' }).collect();
    if !crate::approval::valid_answers_key(&key) { return Err("launch checkout has no valid answers key".into()); }
    if pending.retirement_started {
        use std::os::unix::fs::MetadataExt;
        use std::io::Read;
        let path = crate::approval::answers_dir(&key, "subscriptions").join(format!("{generation}.json"));
        let file = util::open_untrusted(&path).map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
        if bytes.len() > 1024 * 1024 || metadata.mode() & 0o777 != 0o600 || metadata.uid() != unsafe { libc::geteuid() } {
            return Err("retirement recovery plan is not bounded and private".into());
        }
        let plan: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "invalid retirement recovery plan")?;
        if plan["version"] != 1 || plan["generation"] != generation || plan["directory"] != json!(directory)
            || plan["selection"] != json!(pending.target) || plan["context"]["source_session"] != pending.source.session
            || plan["context"]["kind"] != "historical_subscription_handoff" {
            return Err("retirement recovery plan does not match the persisted intent".into());
        }
        return Ok(path);
    }
    let session = crate::sessions::reported().into_iter().find(|session| session.session == pending.source.session
        && session.agent == pending.source.selection.agent && session.cwd == route.directory).ok_or("source session or checkout changed")?;
    let context: serde_json::Value = serde_json::from_str(&handoff_context(&session)?).map_err(|_| "invalid historical handoff context")?;
    let native_thread = if pending.direction == Direction::Return { route.original.native_thread.as_deref() } else { None };
    let session_id = if pending.target.agent == "claude" {
        Some(if pending.direction == Direction::Return { route.original.session.clone() }
            else { format!("{}-{}-{}-{}-{}", &generation[..8], &generation[8..12], &generation[12..16], &generation[16..20], &generation[20..]) })
    } else { None };
    let bytes = serde_json::to_vec(&json!({"version":1,"generation":generation,"directory":directory,
        "selection":pending.target,"native_thread":native_thread,"session_id":session_id,
        "resume":pending.direction == Direction::Return,"context":context})).map_err(|error| error.to_string())?;
    if bytes.len() > 1024 * 1024 { return Err("launch plan exceeds 1 MiB".into()); }
    let folder = crate::approval::answers_dir(&key, "subscriptions");
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder).map_err(|error| error.to_string())?;
    let path = folder.join(format!("{generation}.json"));
    let temporary = folder.join(format!(".plan-{}", util::token_hex(12)));
    let mut file = OpenOptions::new().create_new(true).write(true).mode(0o600).open(&temporary).map_err(|error| error.to_string())?;
    let result = (|| -> Result<(), String> {
        file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|error| error.to_string())?;
        // Publish without replacing a plan whose admission status might already be uncertain.
        match fs::hard_link(&temporary, &path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                use std::io::Read;
                use std::os::unix::fs::MetadataExt;
                let existing = util::open_untrusted(&path).map_err(|error| error.to_string())?;
                let metadata = existing.metadata().map_err(|error| error.to_string())?;
                let mut previous = Vec::new();
                existing.take(1024 * 1024 + 1).read_to_end(&mut previous).map_err(|error| error.to_string())?;
                if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o777 != 0o600 || previous != bytes {
                    return Err("existing protected launch plan conflicts with this transition".into());
                }
            }
            Err(error) => return Err(format!("cannot publish protected launch plan: {error}")),
        }
        File::open(&folder).and_then(|folder| folder.sync_all()).map_err(|error| error.to_string())
    })();
    fs::remove_file(&temporary).map_err(|error| format!("cannot remove temporary launch plan: {error}"))?;
    result?;
    Ok(path)
}

/// Called under the routing lock: durable publication precedes removal from the ordinary queue.
pub fn prepare_admission_inbox(route: &Route, generation: &str) -> Result<PathBuf, String> {
    if route.admitted_generation.as_deref() != Some(generation) { return Err("inbox requires a bound generation".into()); }
    let repository = util::repository_root(&route.directory);
    let key: String = repository.chars().map(|character| if character.is_ascii_alphanumeric() { character } else { '-' }).collect();
    if !crate::approval::valid_answers_key(&key) { return Err("invalid subscription inbox project".into()); }
    let folder = crate::approval::answers_dir(&key, "subscriptions");
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder).map_err(|error| error.to_string())?;
    let path = folder.join(format!("{generation}.inbox.json"));
    let messages: BTreeMap<String, String> = match util::read_untrusted(&path, 256 * 1024 + 1) {
        Ok(bytes) if bytes.len() <= 256 * 1024 => serde_json::from_slice(&bytes).map_err(|_| "invalid subscription inbox")?,
        Ok(_) => return Err("subscription inbox exceeds 256 KiB".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            for retired in &route.retired_sessions { crate::prompts::transfer_subscription_prompts(retired, &route.current.session)?; }
            let mut messages = BTreeMap::new();
            for queued in crate::prompts::pending(&route.current.session) {
                let bytes = util::read_untrusted(&queued, (crate::prompts::PROMPT_MAX * 4 + 1) as u64).map_err(|error| error.to_string())?;
                if bytes.len() > crate::prompts::PROMPT_MAX * 4 { return Err("queued instruction exceeds its bound".into()); }
                let text = String::from_utf8(bytes).map_err(|_| "queued instruction is not UTF-8")?;
                messages.insert(queued.file_name().and_then(|name| name.to_str()).ok_or("invalid queued instruction name")?.to_string(), text);
            }
            let bytes = serde_json::to_vec(&messages).map_err(|error| error.to_string())?;
            if bytes.len() > 256 * 1024 { return Err("subscription inbox exceeds 256 KiB".into()); }
            let temporary = folder.join(format!(".inbox-{}", util::token_hex(12)));
            let mut file = OpenOptions::new().create_new(true).write(true).mode(0o600).open(&temporary).map_err(|error| error.to_string())?;
            file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|error| error.to_string())?;
            fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
            File::open(&folder).and_then(|folder| folder.sync_all()).map_err(|error| error.to_string())?;
            messages
        },
        Err(error) => return Err(error.to_string()),
    };
    let queue = crate::prompts::folder(&route.current.session).ok_or("invalid inbox owner")?;
    for (name, text) in messages {
        if !name.strip_suffix(".txt").is_some_and(|stamp| !stamp.is_empty() && stamp.bytes().all(|byte| byte.is_ascii_digit())) {
            return Err("invalid queued instruction name".into());
        }
        let queued = queue.join(name);
        match util::read_untrusted(&queued, (crate::prompts::PROMPT_MAX * 4 + 1) as u64) {
            Ok(bytes) if bytes == text.as_bytes() => fs::remove_file(queued).map_err(|error| error.to_string())?,
            Ok(_) => return Err("queued instruction changed during admission".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.to_string()),
        }
    }
    if queue.exists() { File::open(queue).and_then(|queue| queue.sync_all()).map_err(|error| error.to_string())?; }
    Ok(path)
}

/// Hook failures must stop dispatch: a nonzero hook exit alone is not a native prompt gate.
pub fn prompt_gate(generation: &str) -> serde_json::Value {
    if admission_confirmed(generation,false) { return json!({}); }
    eprintln!("cockpit-subscription: execution paused until the host confirms replacement ownership");
    json!({"continue":false,"stopReason":"Subscription replacement ownership is not confirmed; the task remains paused."})
}

pub fn initial_prompt_gate(generation: &str) -> serde_json::Value {
    if admission_confirmed(generation,true) { return json!({}); }
    json!({"continue":false,"stopReason":"Initial subscription task start is not confirmed; the task remains paused."})
}

fn admission_confirmed(generation: &str,initial: bool) -> bool {
    let valid = generation.len() == 32 && generation.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    let kind = if initial {"subscription-initial-gate"} else {"subscription-gate"};
    let request = format!("{kind} {}", json!({"generation":generation}));
    valid && crate::approval::ask_daemon(&request, std::time::Duration::from_millis(500)).is_ok_and(|reply| reply == "ok")
}

pub fn await_prompt_gate(generation: &str) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if admission_confirmed(generation,false) { return json!({}); }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    prompt_gate(generation)
}
