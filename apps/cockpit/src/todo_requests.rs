//! Filing receipts and exact-request approval, kept outside the sender's writable sandbox.

use crate::{approval, paths, util};
use serde_json::{Map, Value, json};
use std::{fs, io::Write, path::PathBuf};

pub fn receipt_path(request: &Map<String, Value>) -> Option<PathBuf> {
    let id = request.get("request_id")?.as_str().filter(|id| approval::valid_id(id))?;
    let key = request.get("receipt_key")?.as_str().filter(|key| approval::valid_answers_key(key))?;
    Some(approval::answers_dir(key, "todos").join(format!("{id}.json")))
}

pub fn hold(request: &mut Map<String, Value>, started_in: &str, key: &str) -> Result<(), String> {
    request.remove("host_approved");
    request.remove("receipt_key");
    request.insert("started_in".into(), json!(started_in));
    let Some(id) = request.get("request_id").and_then(Value::as_str).map(str::to_string) else { return Ok(()) };
    if !approval::valid_id(&id) || !approval::valid_answers_key(key) {
        return Err("invalid todo request id or answers key".into());
    }
    if !matches!(request.get("action").and_then(Value::as_str), Some("add" | "propose")) {
        return Err("only add/propose take a filing request id".into());
    }
    request.insert("receipt_key".into(), json!(key));
    request.insert("at".into(), json!(util::now_secs()));
    let path = approval::held_request(&id, "todo");
    fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
    if let Ok(previous) = fs::read_to_string(&path) {
        let previous = serde_json::from_str::<Value>(&previous).map_err(|error| format!("invalid retained request: {error}"))?;
        if let Some(at) = previous.get("at") { request.insert("at".into(), at.clone()); }
        return if previous == json!(request) { Ok(()) }
            else { Err("todo request id is already owned by a different request".into()) };
    }
    let mut total = 0;
    let mut own = 0;
    for entry in fs::read_dir(path.parent().unwrap()).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.path().extension().is_none_or(|extension| extension != "todo") { continue; }
        let Some(previous) = util::read_json(&entry.path()) else { continue };
        if previous.get("at").and_then(Value::as_i64).is_some_and(|at| util::now_secs() - at > 86_400) {
            if let Some(fields) = previous.as_object() && let Some(receipt) = receipt_path(fields) {
                let _ = fs::remove_file(receipt);
            }
            if let Some(old_id) = previous.get("request_id").and_then(Value::as_str).filter(|id| approval::valid_id(id)) {
                let _ = fs::remove_file(approval::held_request(old_id, "todo-approved"));
            }
            fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
            continue;
        }
        total += 1;
        if previous["started_in"] == started_in { own += 1; }
    }
    if total >= 1024 || own >= 64 { return Err("too many retained todo requests; retry after their one-day retention".into()); }
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|error| error.to_string())?;
    if let Err(error) = writeln!(file, "{}", json!(request)) {
        let _ = fs::remove_file(&path);
        return Err(format!("cannot retain todo request: {error}"));
    }
    Ok(())
}

pub fn is_approved(request: &Map<String, Value>) -> bool {
    let Some(id) = request.get("request_id").and_then(Value::as_str).filter(|id| approval::valid_id(id)) else { return false };
    request.get("host_approved") == Some(&json!(true))
        && util::read_json(&approval::held_request(id, "todo-approved")) == Some(json!(request))
}

pub fn finish(request: &Value, outcome: &str, item: Option<&crate::todo::Item>, queued: bool, reason: &str) {
    let Some(fields) = request.as_object() else { return };
    let Some(path) = receipt_path(fields) else { return };
    let id = fields["request_id"].as_str().unwrap();
    if !is_retained(request) { return; }
    if outcome != "accepted" && util::read_json(&path).is_some_and(|receipt| receipt["outcome"] == "accepted") { return; }
    let state = item.map(|item| if queued { "queued" } else if !item.after().is_empty() { "waiting" } else { "saved" });
    let receipt = json!({"request_id": id, "outcome": outcome, "id": item.map(|item| item.id()),
                         "project": item.map(|item| item.project()).or_else(|| fields.get("project").and_then(Value::as_str).map(str::to_string)),
                         "state": state, "status": item.map(|item| item.status()), "reason": reason});
    if let Err(error) = util::write_json(&path, &receipt) {
        util::log_event("WARN", &format!("todo receipt {id} could not be written: {error}"));
    }
}

pub fn is_retained(request: &Value) -> bool {
    let Some(fields) = request.as_object() else { return false };
    let Some(id) = fields.get("request_id").and_then(Value::as_str).filter(|id| approval::valid_id(id)) else { return false };
    is_approved(fields) || util::read_json(&approval::held_request(id, "todo")) == Some(request.clone())
}

/// Called on the host through ask-to-run; only the protected daemon copy supplies task content.
pub fn approve(id: &str) -> Result<(), String> {
    if !approval::valid_id(id) { return Err("invalid todo request id".into()); }
    let mut request = util::read_json(&approval::held_request(id, "todo"))
        .and_then(|value| value.as_object().cloned()).ok_or("no retained todo request on this host")?;
    if request.get("action").and_then(Value::as_str) != Some("add") || request.get("project").and_then(Value::as_str).is_none() {
        return Err("only an exact cross-project add can be approved".into());
    }
    if request.get("at").and_then(Value::as_i64).is_some_and(|at| util::now_secs() - at > 86_400) {
        return Err("todo approval request expired; file a fresh request".into());
    }
    let path = receipt_path(&request).ok_or("request has no trusted receipt destination")?;
    if util::read_json(&path).is_some_and(|receipt| receipt["outcome"] == "accepted") { return Ok(()); }
    request.insert("host_approved".into(), json!(true));
    util::write_json(&approval::held_request(id, "todo-approved"), &json!(request)).map_err(|error| error.to_string())?;
    let queue = paths::todo_updates();
    if fs::metadata(&queue).is_ok_and(|metadata| metadata.len() >= 1 << 20) { return Err("todo queue is full".into()); }
    let mut file = fs::OpenOptions::new().create(true).append(true).open(queue).map_err(|error| error.to_string())?;
    finish(&json!(request), "pending", None, false, "host approved; awaiting persistence");
    writeln!(file, "{}", json!(request)).map_err(|error| error.to_string())
}
