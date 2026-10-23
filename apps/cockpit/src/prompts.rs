//! Prompts queued for a session while it works: sent one at a time, each when a turn ends, so a
//! thought written mid-turn waits instead of interrupting Claude. One file per prompt in
//! <state>/prompt-queue/<session>/<nanoseconds>.txt, written by the column (m) and by
//! `cockpit queue-prompt` (Alt+Q in the session pane); the column sends and deletes them.

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

/// Queues one prompt, retaining line breaks but no other controls, at most PROMPT_MAX characters.
pub fn add(session: &str, text: &str) -> Result<(), String> {
    let normalized = text.replace("\r\n", "\n");
    let text = normalized.trim();
    if text.is_empty() || text.chars().count() > PROMPT_MAX || text.chars().any(|character| character.is_control() && character != '\n') {
        return Err(format!("a queued prompt is at most {PROMPT_MAX} characters, without control characters other than line breaks"));
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

/// Both sessions must remain routing-held until this transfer and ownership binding finish.
pub fn transfer_subscription_prompts(source: &str, replacement: &str) -> Result<usize, String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let source_folder = folder(source).ok_or("invalid source prompt queue")?;
    let target_folder = folder(replacement).filter(|_| source != replacement).ok_or("invalid replacement prompt queue")?;
    let metadata = match fs::symlink_metadata(&source_folder) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_dir() { return Err("source prompt queue is not a plain directory".into()); }
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&target_folder).map_err(|error| error.to_string())?;
    if !fs::symlink_metadata(&target_folder).map_err(|error| error.to_string())?.is_dir() {
        return Err("replacement prompt queue is not a plain directory".into());
    }
    let target_directory = fs::File::open(&target_folder).map_err(|error| error.to_string())?;
    let source_directory = fs::File::open(&source_folder).map_err(|error| error.to_string())?;
    let mut transferred = 0;
    for path in pending(source) {
        let file = crate::util::open_untrusted(&path).map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if metadata.len() > (PROMPT_MAX * 4) as u64 { return Err("queued prompt exceeds the safe transfer size".into()); }
        file.sync_all().map_err(|error| error.to_string())?;
        let target = target_folder.join(path.file_name().ok_or("queued prompt has no filename")?);
        match fs::hard_link(&path, &target) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.to_string()),
        }
        let published = fs::symlink_metadata(&target).map_err(|error| error.to_string())?;
        let current = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if !published.is_file() || published.ino() != metadata.ino() || published.dev() != metadata.dev()
            || current.ino() != metadata.ino() || current.dev() != metadata.dev() {
            return Err("queued prompt changed or conflicts with the replacement queue".into());
        }
        // A crash after publication leaves the same inode at both names, safely recognizable on retry.
        target_directory.sync_all().map_err(|error| error.to_string())?;
        fs::remove_file(&path).map_err(|error| error.to_string())?;
        source_directory.sync_all().map_err(|error| error.to_string())?;
        transferred += 1;
    }
    Ok(transferred)
}

/// The oldest prompt's text and file, to delete once sent.
pub fn oldest(session: &str) -> Option<(String, PathBuf)> {
    let path = pending(session).into_iter().next()?;
    let text = fs::read_to_string(&path).ok()?;
    Some((text.trim().to_string(), path))
}

pub fn claim_native(session: &str, runtime: &str) -> Result<Option<serde_json::Value>, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let folder = folder(session).ok_or("invalid native message recipient")?;
    let outbox = folder.join("native-delivery.json");
    if outbox.exists() { return Ok(None); }
    let Some(queued) = pending(session).into_iter().next() else { return Ok(None) };
    let bytes = crate::util::read_untrusted(&queued,(PROMPT_MAX * 4 + 1) as u64).map_err(|error| error.to_string())?;
    if bytes.len() > PROMPT_MAX * 4 { return Err("native message exceeds its safe size".into()); }
    let text = String::from_utf8(bytes).map_err(|_| "native message is not UTF-8")?;
    let dispatch_id = format!("prompt_{}",crate::util::token_hex(16));
    let delivery = serde_json::json!({"dispatch_id":dispatch_id,"text":text,"runtime":runtime,
        "queued":queued.file_name().and_then(|name| name.to_str()).ok_or("invalid native message name")?});
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&outbox).map_err(|error| error.to_string())?;
    file.write_all(delivery.to_string().as_bytes()).and_then(|_| file.sync_all()).map_err(|error| error.to_string())?;
    fs::File::open(&folder).and_then(|folder| folder.sync_all()).map_err(|error| error.to_string())?;
    fs::remove_file(queued).map_err(|error| error.to_string())?;
    fs::File::open(&folder).and_then(|folder| folder.sync_all()).map_err(|error| error.to_string())?;
    Ok(Some(serde_json::json!({"dispatch_id":dispatch_id,"text":text})))
}

pub fn receipt_native(session: &str, runtime: &str, dispatch_id: &str) -> Result<(), String> {
    let folder = folder(session).ok_or("invalid native message recipient")?;
    let outbox = folder.join("native-delivery.json");
    let bytes = crate::util::read_untrusted(&outbox,32768).map_err(|error| error.to_string())?;
    let saved: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "invalid native message receipt state")?;
    if saved["dispatch_id"] != dispatch_id || saved["runtime"] != runtime { return Err("native message receipt belongs to another delivery or runtime".into()); }
    let name = saved["queued"].as_str().ok_or("native message has no source name")?;
    if !name.strip_suffix(".txt").is_some_and(|stamp| !stamp.is_empty() && stamp.bytes().all(|byte| byte.is_ascii_digit())) {
        return Err("invalid native message source name".into());
    }
    match fs::remove_file(folder.join(name)) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.to_string()),
    }
    fs::remove_file(outbox).map_err(|error| error.to_string())?;
    fs::File::open(folder).and_then(|folder| folder.sync_all()).map_err(|error| error.to_string())
}

pub fn clear(session: &str) {
    if let Some(folder) = folder(session) {
        let _ = fs::remove_dir_all(folder);
    }
}

/// Sessions to restart once their turn ends (craft-open's --session, restart-all's busy ones): the
/// marker files in <state>/restart-when-idle, each named after its session.
pub fn want_restart(session: &str) -> Result<(), String> {
    if !valid_session(session) {
        return Err("not a session id".into());
    }
    let markers = paths::state_dir().join("restart-when-idle");
    fs::create_dir_all(&markers).and_then(|_| fs::write(markers.join(session), "")).map_err(|error| error.to_string())
}

pub fn restart_wanted(session: &str) -> bool {
    valid_session(session) && paths::state_dir().join("restart-when-idle").join(session).exists()
}

pub fn restart_done(session: &str) {
    if valid_session(session) {
        let _ = fs::remove_file(paths::state_dir().join("restart-when-idle").join(session));
    }
}
