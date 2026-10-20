//! Codex's native thread identity and non-authorizing lifecycle reports.

use crate::audit;
use serde_json::{Value, json};
use std::path::Path;

/// Namespace native threads so a Codex UUID cannot alias a Claude conversation.
pub fn cockpit_id(native_thread: &str) -> Result<String, String> {
    if native_thread.is_empty() || native_thread.len() > 256
        || !native_thread.bytes().all(|character| character.is_ascii_alphanumeric() || matches!(character, b'-' | b'_')) {
        return Err("invalid Codex thread id".into());
    }
    Ok(format!("cdx_{}", audit::sha256_hex(native_thread.as_bytes())))
}

/// Ignore child lifecycle events; unknown events cannot establish root readiness.
pub fn translate_hook(payload: &Value) -> Result<Option<Value>, String> {
    let event = payload.get("hook_event_name").and_then(Value::as_str).ok_or("missing Codex hook event")?;
    if matches!(event, "SubagentStart" | "SubagentStop") {
        return Ok(None);
    }
    let native_thread = payload.get("session_id").and_then(Value::as_str).ok_or("missing Codex thread id")?;
    let session = cockpit_id(native_thread)?;
    let cwd = payload.get("cwd").and_then(Value::as_str).ok_or("missing Codex working directory")?;
    if !Path::new(cwd).is_absolute() || cwd.len() > 300 || cwd.chars().any(char::is_control)
        || cwd.contains("#(") || cwd.contains("#{") {
        return Err("invalid Codex working directory".into());
    }
    let project = Path::new(cwd).file_name().and_then(|name| name.to_str()).ok_or("Codex working directory has no project name")?;
    let state = match event {
        "SessionStart" | "UserPromptSubmit" | "PostToolUse" | "NeedsInput" => Some("working"),
        "PermissionRequest" | "Interrupt" => Some("needs-you"),
        "Stop" => Some("waiting"),
        "SessionEnd" => Some("gone"),
        "TaskLabel" => None,
        _ => return Err(format!("unsupported Codex hook event {event:?}")),
    };
    let mut report = json!({"session": session, "native_thread": native_thread, "agent": "codex", "cwd": cwd, "project": project});
    if let Some(state) = state {
        report["state"] = json!(state);
    }
    if event == "TaskLabel" {
        let label = payload.get("label").and_then(Value::as_str).ok_or("missing task label")?;
        report["label"] = json!(crate::sessions::validate_task_label(label)?);
    }
    Ok(Some(report))
}
