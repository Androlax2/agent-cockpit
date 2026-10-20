use super::{session_row, text};
use crate::{agents, paths};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

const SOURCE_MAX: usize = 4 * 1024 * 1024;
const TEXT_MAX: usize = 256 * 1024;
const MESSAGES_MAX: usize = 100;

/// Visible conversation text for an active session, without reasoning or tool payloads.
pub fn conversation(id: &str) -> Result<Value, String> {
    if !agents::is_session_id(id) { return Err("no such session".into()); }
    let row = session_row(id).ok_or("no such session")?;
    let (messages, truncated) = match agents::get(&text(&row, "agent")).name {
        "claude" => claude_messages(id),
        "opencode" => opencode_messages(id),
        _ => Err("conversation history is not supported for this agent".into()),
    }?;
    let mut messages: VecDeque<Value> = messages.into();
    let mut truncated = truncated;
    while messages.len() > MESSAGES_MAX {
        messages.pop_front();
        truncated = true;
    }
    let mut bytes: usize = messages.iter().map(|message| message["text"].as_str().unwrap().len()).sum();
    while bytes > TEXT_MAX && messages.len() > 1 {
        bytes -= messages.pop_front().unwrap()["text"].as_str().unwrap().len();
        truncated = true;
    }
    if bytes > TEXT_MAX {
        let message = messages.front_mut().unwrap();
        let content = message["text"].as_str().unwrap();
        let mut start = content.len() - TEXT_MAX;
        while !content.is_char_boundary(start) { start += 1; }
        message["text"] = json!(&content[start..]);
        truncated = true;
    }
    Ok(json!({"messages": messages, "truncated": truncated}))
}

fn visible_text(content: &Value) -> String {
    if let Some(content) = content.as_str() { return content.to_string(); }
    content.as_array().map(|blocks| blocks.iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>().join("\n\n")).unwrap_or_default()
}

fn claude_messages(id: &str) -> Result<(Vec<Value>, bool), String> {
    let root = paths::transcripts();
    let entries = fs::read_dir(&root).map_err(|_| "conversation history is unavailable")?;
    let path = entries.filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path().join(format!("{id}.jsonl")))
        .find(|path| path.symlink_metadata().is_ok()).ok_or("conversation history is unavailable")?;
    let mut file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path).map_err(|_| "conversation history could not be read")?;
    let metadata = file.metadata().map_err(|_| "conversation history could not be read")?;
    if !metadata.is_file() { return Err("conversation history is not a regular file".into()); }
    let length = metadata.len();
    let truncated = length > SOURCE_MAX as u64;
    let start = length.saturating_sub(SOURCE_MAX as u64);
    file.seek(SeekFrom::Start(start)).map_err(|_| "conversation history could not be read")?;
    let mut bytes = Vec::new();
    file.take(SOURCE_MAX as u64).read_to_end(&mut bytes).map_err(|_| "conversation history could not be read")?;
    let mut lines = bytes.split(|byte| *byte == b'\n');
    if truncated { lines.next(); }
    let mut messages = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.is_empty() { continue; }
        // A currently streaming JSONL record can end halfway through a write.
        let record: Value = match serde_json::from_slice(line) {
            Ok(record) => record,
            Err(_) if !bytes.ends_with(b"\n") && bytes.ends_with(line) => break,
            Err(_) => return Err("conversation history contains an invalid record".into()),
        };
        let role = record.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(role, "user" | "assistant") || record.get("isMeta").and_then(Value::as_bool) == Some(true) { continue; }
        let content = visible_text(&record["message"]["content"]);
        if content.trim().is_empty() { continue; }
        messages.push(json!({"id": record.get("uuid").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("{start}:{index}")),
            "role": role, "text": content}));
    }
    Ok((messages, truncated))
}

fn opencode_messages(id: &str) -> Result<(Vec<Value>, bool), String> {
    let database = Connection::open_with_flags(paths::opencode_database(), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "conversation history is unavailable")?;
    database.busy_timeout(Duration::from_millis(200)).map_err(|_| "conversation history is unavailable")?;
    let mut statement = database.prepare(
        "select id, type, substr(data, 1, ?2) from session_message
         where session_id = ?1 and type in ('user', 'assistant') order by seq desc limit 101")
        .map_err(|_| "conversation history format is unavailable")?;
    let rows = statement.query_map(rusqlite::params![id, (SOURCE_MAX + 1) as i64], |row|
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))
        .map_err(|_| "conversation history could not be read")?;
    let mut messages = Vec::new();
    let mut consumed = 0;
    let mut truncated = false;
    for row in rows {
        let (id, role, data) = row.map_err(|_| "conversation history could not be read")?;
        consumed += data.len();
        if consumed > SOURCE_MAX { truncated = true; break; }
        let record: Value = serde_json::from_str(&data).map_err(|_| "conversation history contains an invalid record")?;
        let content = visible_text(if role == "user" { &record["text"] } else { &record["content"] });
        if !content.trim().is_empty() { messages.push(json!({"id": id, "role": role, "text": content})); }
    }
    // The source query is bounded even when it contains tool-only assistant records.
    if messages.len() == 101 { truncated = true; }
    messages.reverse();
    Ok((messages, truncated))
}
