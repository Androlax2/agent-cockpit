//! User-owned continuity metadata. Mutations persist before they replace the live state.

use crate::{agents, sessions::{Session, TurnResult, valid_metadata_text}, util, audit};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, fs, io::{self, Write}, os::unix::fs::OpenOptionsExt, path::{Path, PathBuf}};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RECORDS: usize = 10_000;
mod tasks;
pub use tasks::{TaskSnapshot, TaskAssignment, TaskRecord, PullRequestMemory, TrailTarget, task_key};
mod deferrals;
pub use deferrals::{DeferralTarget, Deferral};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionMemory {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub result: Option<TurnResult>,
    #[serde(default)]
    pub question: Option<Update>,
    #[serde(default)]
    reviewed_result: Option<String>,
    #[serde(default)]
    reviewed_question: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl SessionMemory {
    /// A path that cannot be kept verbatim is dropped (no resume offered) rather than altered.
    fn remember(&mut self, session: &Session) {
        self.title = display_text(&util::collapse(if session.label.is_empty() { &session.title } else { &session.label }), 300);
        self.cwd = if valid_metadata_text(&session.cwd, 300) { session.cwd.clone() } else { String::new() };
    }
}

/// Display text from sources that allow anything (transcript titles, task notes, GitHub titles),
/// made storable: the saved file is still checked strictly when it is loaded.
pub(crate) fn display_text(text: &str, limit: usize) -> String {
    display_lines(text, limit).replace('\n', " ")
}

pub(crate) fn display_lines(text: &str, limit: usize) -> String {
    text.replace("#{", "# {").replace("#(", "# (").chars()
        .map(|character| if character.is_control() && character != '\n' { ' ' } else { character }).take(limit).collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateKind { Result, Question }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Update {
    pub session: String,
    pub identity: String,
    pub kind: UpdateKind,
    pub observed_at: i64,
    pub text: String,
}

pub fn question_text(session: &Session) -> Option<&str> {
    if !session.needs_input { return None; }
    let text = if !session.full_request.is_empty() { &session.full_request }
        else if !session.request.is_empty() { &session.request } else { &session.detail };
    (!text.trim().is_empty()).then_some(text)
}

pub fn question_identity(session: &Session) -> Option<String> {
    question_text(session).map(|text| format!("question:{}", audit::sha256_hex(text.as_bytes())))
}

fn valid_question(text: &str) -> bool {
    text.chars().count() <= 16_384 && !text.chars().any(|character| character.is_control() && character != '\n')
        && !text.contains("#(") && !text.contains("#{")
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct State {
    version: u64,
    #[serde(default)]
    sessions: BTreeMap<String, SessionMemory>,
    #[serde(default)]
    tasks: BTreeMap<String, TaskRecord>,
    #[serde(default)]
    prs: BTreeMap<String, PullRequestMemory>,
    #[serde(default)]
    deferrals: BTreeMap<String, Deferral>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl State {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 { return Err(format!("unsupported continuity version {}", self.version)); }
        if self.sessions.len() > MAX_RECORDS { return Err("too many continuity sessions".into()); }
        self.validate_tasks()?;
        self.validate_deferrals()?;
        for (id, session) in &self.sessions {
            if !agents::is_session_id(id) || !valid_metadata_text(&session.title, 300)
                || !valid_metadata_text(&session.cwd, 300) {
                return Err("invalid continuity session metadata".into());
            }
            if let Some(note) = &session.note { validate_note(note)?; }
            if let Some(result) = &session.result { result.validate()?; }
            if let Some(question) = &session.question {
                if question.session != *id || question.kind != UpdateKind::Question || !valid_question(&question.text)
                    || question.observed_at <= 0 || question.identity != format!("question:{}", audit::sha256_hex(question.text.as_bytes())) {
                    return Err("invalid saved question".into());
                }
            }
            if session.reviewed_result.as_ref().is_some_and(|id| !crate::sessions::valid_turn_id(id))
                || session.reviewed_question.as_ref().is_some_and(|id| !valid_metadata_text(id, 73)) {
                return Err("invalid saved acknowledgement".into());
            }
        }
        Ok(())
    }
}

pub struct Continuity {
    path: PathBuf,
    state: State,
}

fn validate_note(note: &str) -> Result<(), String> {
    if !valid_metadata_text(note, 280) { return Err("invalid return note: at most 280 characters, without controls or tmux escapes".into()); }
    Ok(())
}

impl Continuity {
    pub fn load(path: &Path) -> Result<Self, String> {
        let state = match util::read_untrusted(path, MAX_BYTES + 1) {
            Ok(bytes) => {
                if bytes.len() as u64 > MAX_BYTES { return Err("continuity file exceeds 16 MiB; preserve it and reduce archived records".into()); }
                serde_json::from_slice::<State>(&bytes).map_err(|error| format!("invalid continuity file: {error}"))?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => State { version: 1, sessions: BTreeMap::new(), tasks: BTreeMap::new(), prs: BTreeMap::new(), deferrals: BTreeMap::new(), extra: Map::new() },
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        state.validate()?;
        Ok(Self { path: path.into(), state })
    }

    pub fn note(&self, session: &str) -> Option<&str> {
        self.state.sessions.get(session)?.note.as_deref()
    }

    pub fn memories(&self) -> &BTreeMap<String, SessionMemory> { &self.state.sessions }

    pub fn observe_sessions(&mut self, sessions: &[Session], now: i64) -> Result<bool, String> {
        self.change(|state| {
            state.refresh_question_deferrals(sessions, now);
            for (id, memory) in &mut state.sessions {
                if !sessions.iter().any(|session| session.session == *id && question_text(session).is_some()) {
                    memory.question = None;
                    memory.reviewed_question = None;
                }
            }
            for session in sessions.iter().filter(|session| agents::is_session_id(&session.session)) {
                if session.last_result.is_none() && question_text(session).is_none() && !state.sessions.contains_key(&session.session) { continue; }
                let memory = state.sessions.entry(session.session.clone()).or_default();
                memory.remember(session);
                if let Some(result) = &session.last_result { memory.result = Some(result.clone()); }
                if let Some(text) = question_text(session) {
                    let identity = question_identity(session).unwrap();
                    if memory.question.as_ref().is_none_or(|question| question.identity != identity) {
                        memory.question = Some(Update { session: session.session.clone(), identity,
                            kind: UpdateKind::Question, observed_at: now, text: text.into() });
                        memory.reviewed_question = None;
                    }
                }
            }
            Ok(())
        })
    }

    pub fn updates(&self) -> Vec<Update> {
        let mut updates = Vec::new();
        for (session, memory) in &self.state.sessions {
            if let Some(result) = &memory.result {
                if memory.reviewed_result.as_deref() != Some(&result.id) && !result.outcome.is_empty() {
                    updates.push(Update { session: session.clone(), identity: format!("result:{}", result.id),
                        kind: UpdateKind::Result, observed_at: result.completed_at, text: result.outcome.clone() });
                }
            }
            if let Some(question) = &memory.question {
                if memory.reviewed_question.as_deref() != Some(&question.identity) { updates.push(question.clone()); }
            }
        }
        updates.sort_by(|left, right| right.observed_at.cmp(&left.observed_at).then(left.session.cmp(&right.session)));
        updates
    }

    pub fn reviewed(&mut self, session: &str, identities: &[String]) -> Result<bool, String> {
        self.change(|state| {
            let Some(memory) = state.sessions.get_mut(session) else { return Ok(()) };
            for identity in identities {
                if let Some(result) = &memory.result {
                    if *identity == format!("result:{}", result.id) { memory.reviewed_result = Some(result.id.clone()); }
                }
                if let Some(question) = &memory.question {
                    if *identity == question.identity { memory.reviewed_question = Some(question.identity.clone()); }
                }
            }
            Ok(())
        })
    }

    pub fn set_note(&mut self, session: &Session, note: Option<&str>) -> Result<bool, String> {
        if let Some(note) = note { validate_note(note)?; }
        self.change(|state| {
            let memory = state.sessions.entry(session.session.clone()).or_default();
            memory.remember(session);
            memory.note = note.map(str::to_string);
            Ok(())
        })
    }

    fn change(&mut self, mutate: impl FnOnce(&mut State) -> Result<(), String>) -> Result<bool, String> {
        let mut next = self.state.clone();
        mutate(&mut next)?;
        next.validate()?;
        if next == self.state { return Ok(false); }
        self.save(&next).map_err(|error| format!("cannot save continuity: {error}"))?;
        self.state = next;
        Ok(true)
    }

    fn save(&self, state: &State) -> io::Result<()> {
        let bytes = serde_json::to_vec(state)?;
        if bytes.len() as u64 > MAX_BYTES { return Err(io::Error::other("continuity file exceeds 16 MiB")); }
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(io::Error::other("continuity path became a symlink; original preserved"));
        }
        if let Some(parent) = self.path.parent() { fs::create_dir_all(parent)?; }
        let temporary = self.path.with_extension(format!("tmp-{}", util::token_hex(12)));
        let result = (|| {
            let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)
        })();
        if result.is_err() { let _ = fs::remove_file(&temporary); }
        result
    }
}
