//! Explicit subscription-routing decisions and bounded historical handoffs.

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
    pub target: Selection,
    pub direction: Direction,
    pub stage: Stage,
    pub replacement: Option<Binding>,
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
        let state = match util::read_untrusted(path, 1024 * 1024 + 1) {
            Ok(bytes) if bytes.len() <= 1024 * 1024 => serde_json::from_slice::<RoutingState>(&bytes).map_err(|error| format!("invalid routing state: {error}"))?,
            Ok(_) => return Err("routing state exceeds 1 MiB".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => RoutingState { version: 1, routes: BTreeMap::new() },
            Err(error) => return Err(format!("cannot read routing state: {error}")),
        };
        validate_state(&state)?;
        Ok(Self { path: path.into(), state, lock })
    }

    pub fn route(&self, origin: &str) -> Option<&Route> { self.state.routes.get(origin) }

    pub fn track(&mut self, original: Binding, directory: &str, task: &str, now: i64) -> Result<(), String> {
        if self.state.routes.values().any(|route| route.current.session == original.session
            || matches!(&route.phase, Phase::Pending(pending) if pending.replacement.as_ref().is_some_and(|replacement| replacement.session == original.session))) {
            return Ok(());
        }
        let mut next = self.state.clone();
        next.routes.entry(original.session.clone()).or_insert_with(|| Route {
            current: original.clone(), original, directory: directory.into(), task: task.into(), phase: Phase::Active, updated_at: now,
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
                    target, direction, stage: Stage::Prepared, replacement: None };
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
        route.current = replacement;
        route.phase = Phase::Active;
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
        if let Phase::Pending(pending) = &route.phase {
            if pending.origin != *origin || pending.source != route.current || !pending.target.valid()
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
