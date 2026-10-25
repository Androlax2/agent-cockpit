//! Codex's native thread identity and non-authorizing lifecycle reports.

use crate::audit;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

/// The CLI's read-only history API handles both legacy rollouts and paginated threads.
pub fn session_snapshot(native_thread: &str) -> Result<Value, String> {
    cockpit_id(native_thread)?;
    let output = Command::new("python3").arg(crate::paths::codex_session_script()).arg(native_thread)
        .output().map_err(|error| format!("cannot read Codex conversation: {error}"))?;
    if !output.status.success() { return Err("Codex conversation history is unavailable".into()); }
    if output.stdout.len() > 1024 * 1024 { return Err("Codex conversation history exceeds the safe size".into()); }
    let snapshot: Value = serde_json::from_slice(&output.stdout).map_err(|_| "Codex conversation history is invalid")?;
    if snapshot["thread"].as_str() != Some(native_thread) { return Err("Codex thread identity mismatch".into()); }
    let messages = snapshot["messages"].as_array().ok_or("Codex conversation messages are unavailable")?;
    if messages.iter().any(|message| !matches!(message["role"].as_str(), Some("user" | "assistant"))
        || !message["text"].is_string() || !message["id"].is_string()) {
        return Err("Codex conversation contains an invalid visible message".into());
    }
    Ok(snapshot)
}

/// Namespace native threads so a Codex UUID cannot alias a Claude conversation. Only a lowercase
/// UUID is a thread: anything else (a leading "-") would reach `codex resume` as an option.
pub fn cockpit_id(native_thread: &str) -> Result<String, String> {
    let is_uuid = native_thread.len() == 36 && native_thread.bytes().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => byte == b'-',
        _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
    });
    if !is_uuid {
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
        "TaskLabel" | "TaskStep" => None,
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
    if event == "TaskStep" {
        let doing = payload.get("doing").and_then(Value::as_str).ok_or("missing doing-now status")?;
        report["doing"] = json!(crate::sessions::validate_doing(doing)?);
    }
    if event == "PostToolUse" {
        if let Some(summary) = payload.get("tool_name").and_then(Value::as_str)
            .and_then(|tool| crate::sessions::tool_summary(tool, payload.get("tool_input"))) {
            report["last_tool"] = json!(summary);
        }
    }
    Ok(Some(report))
}
