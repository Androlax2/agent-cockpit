//! User-owned continuity metadata. Mutations persist before they replace the live state.

use crate::{agents, sessions::{Session, valid_metadata_text}, util};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, fs, io::{self, Write}, os::unix::fs::OpenOptionsExt, path::{Path, PathBuf}};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RECORDS: usize = 10_000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionMemory {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct State {
    version: u64,
    #[serde(default)]
    sessions: BTreeMap<String, SessionMemory>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl State {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 { return Err(format!("unsupported continuity version {}", self.version)); }
        if self.sessions.len() > MAX_RECORDS { return Err("too many continuity sessions".into()); }
        for (id, session) in &self.sessions {
            if !agents::is_session_id(id) || !valid_metadata_text(&session.title, 300)
                || !valid_metadata_text(&session.cwd, 300) {
                return Err("invalid continuity session metadata".into());
            }
            if let Some(note) = &session.note { validate_note(note)?; }
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
            Err(error) if error.kind() == io::ErrorKind::NotFound => State { version: 1, sessions: BTreeMap::new(), extra: Map::new() },
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        state.validate()?;
        Ok(Self { path: path.into(), state })
    }

    pub fn note(&self, session: &str) -> Option<&str> {
        self.state.sessions.get(session)?.note.as_deref()
    }

    pub fn memories(&self) -> &BTreeMap<String, SessionMemory> { &self.state.sessions }

    pub fn set_note(&mut self, session: &Session, note: Option<&str>) -> Result<bool, String> {
        if let Some(note) = note { validate_note(note)?; }
        self.change(|state| {
            let memory = state.sessions.entry(session.session.clone()).or_default();
            memory.title = util::collapse(if session.label.is_empty() { &session.title } else { &session.label });
            memory.cwd = session.cwd.clone();
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
