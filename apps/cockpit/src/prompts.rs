//! Prompts queued for a session while it works: sent one at a time, each when a turn ends, so a
//! thought written mid-turn waits instead of interrupting Claude. One file per prompt in
//! <state>/prompt-queue/<session>/<nanoseconds>.txt, written by the column (m) and by
//! `claude-sessions queue-prompt` (Alt+Q in the session pane); the column sends and deletes them.

use crate::paths;
use std::fs;
use std::path::PathBuf;

pub const PROMPT_MAX: usize = 4000;

/// An id some agent reports (agents::AGENTS): the queue's folder is named after it.
fn valid_session(session: &str) -> bool {
    crate::agents::is_session_id(session)
}

pub fn folder(session: &str) -> Option<PathBuf> {
    valid_session(session).then(|| paths::state_dir().join("prompt-queue").join(session))
}

/// Queues one prompt: one line, no control characters, at most PROMPT_MAX characters.
pub fn add(session: &str, text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > PROMPT_MAX || text.chars().any(char::is_control) {
        return Err("a queued prompt is one line, without control characters".into());
    }
    let folder = folder(session).ok_or("not a session id")?;
    fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|age| age.as_nanos()).unwrap_or(0);
    fs::write(folder.join(format!("{stamp}.txt")), text).map_err(|error| error.to_string())
}

/// The session's queued prompts, oldest first.
pub fn pending(session: &str) -> Vec<PathBuf> {
    let Some(folder) = folder(session) else { return Vec::new() };
    let mut files: Vec<PathBuf> = fs::read_dir(folder).into_iter().flatten().flatten().map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "txt")).collect();
    files.sort();
    files
}

pub fn count(session: &str) -> usize {
    pending(session).len()
}

/// The oldest prompt's text and file, to delete once sent.
pub fn oldest(session: &str) -> Option<(String, PathBuf)> {
    let path = pending(session).into_iter().next()?;
    let text = fs::read_to_string(&path).ok()?;
    Some((text.trim().to_string(), path))
}

pub fn clear(session: &str) {
    if let Some(folder) = folder(session) {
        let _ = fs::remove_dir_all(folder);
    }
}

/// Sessions to restart once their turn ends (craft-open's --session): the marker files in
/// <state>/restart-when-idle, each named after its session.
pub fn restart_wanted(session: &str) -> bool {
    valid_session(session) && paths::state_dir().join("restart-when-idle").join(session).exists()
}

pub fn restart_done(session: &str) {
    if valid_session(session) {
        let _ = fs::remove_file(paths::state_dir().join("restart-when-idle").join(session));
    }
}
