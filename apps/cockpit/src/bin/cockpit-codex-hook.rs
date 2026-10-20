//! Native Codex lifecycle → authenticated cockpit reports. Never decides permissions.

use cockpit::{approval, codex, paths, teams};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Read};

const HOOK_MAX_BYTES: u64 = 65536;

fn question(text: &str) -> Result<&str, String> {
    if text.trim().is_empty() || text.chars().count() > 300 || text.chars().any(char::is_control)
        || text.contains("#(") || text.contains("#{") {
        return Err("question must be 1–300 characters without controls or tmux formats".into());
    }
    Ok(text)
}

fn update_question(payload: &Value, report: &mut Value) -> Result<(), String> {
    let session = report["session"].as_str().ok_or("missing translated session")?;
    let cache = paths::statusline_dir();
    let pending = cache.join(format!("input-{session}.question.pending"));
    let current = cache.join(format!("input-{session}.question"));
    match payload["hook_event_name"].as_str() {
        Some("NeedsInput") => {
            let text = question(payload["message"].as_str().ok_or("missing question")?)?;
            fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
            fs::write(&pending, text).map_err(|error| error.to_string())?;
            match fs::remove_file(current) {
                Ok(()) => {},
                Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                Err(error) => return Err(error.to_string()),
            }
        },
        Some("SessionStart" | "UserPromptSubmit" | "SessionEnd") => {
            for path in [pending, current] {
                match fs::remove_file(path) {
                    Ok(()) => {},
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                    Err(error) => return Err(error.to_string()),
                }
            }
            report["needs_input"] = json!(false);
        },
        Some("Stop") if report["state"] == "waiting" => {
            match fs::rename(pending, &current) {
                Ok(()) => {},
                Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                Err(error) => return Err(error.to_string()),
            }
            match fs::read_to_string(current) {
                Ok(text) => {
                    report["message"] = json!(question(&text)?);
                    report["state"] = json!("needs-you");
                    report["needs_input"] = json!(true);
                    report["sticky"] = json!(true);
                },
                Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                Err(error) => return Err(error.to_string()),
            }
        },
        _ => {},
    }
    Ok(())
}

/// At the end of an idle-bound turn, the next approved team message, if the daemon allows one now.
/// Team messaging being unavailable never breaks lifecycle reporting.
fn team_delivery() -> Option<Value> {
    match teams::client::request(&teams::AgentAction::Receive).and_then(|reply| reply.result) {
        Ok(delivery) if delivery.get("dispatch_id").and_then(Value::as_str).is_some() => Some(delivery),
        Ok(_) => None,
        Err(error) => {
            eprintln!("cockpit-codex-hook: no team delivery: {error}");
            None
        },
    }
}

fn report() -> Result<Option<Value>, String> {
    let mut input = Vec::new();
    io::stdin().take(HOOK_MAX_BYTES + 1).read_to_end(&mut input).map_err(|error| error.to_string())?;
    if input.len() as u64 > HOOK_MAX_BYTES {
        return Err("Codex hook payload exceeds 64 KiB".into());
    }
    let payload = serde_json::from_slice(&input).map_err(|error| format!("invalid Codex hook JSON: {error}"))?;
    let Some(mut report) = codex::translate_hook(&payload)? else { return Ok(None) };
    update_question(&payload, &mut report)?;
    let delivery = (payload["hook_event_name"] == "Stop" && report["state"] == "waiting").then(team_delivery).flatten();
    if delivery.is_some() {
        report["state"] = json!("working");
    }
    report["terminal"] = json!(std::env::var("CLAUDE_SESSIONS_TERMINAL").is_ok_and(|value| value == "1"));
    approval::file_request("session-request", &report).map_err(|refusal| {
        let (approval::Refusal::Unreachable(reason) | approval::Refusal::Refused(reason)) = refusal;
        format!("cannot report Codex lifecycle: {reason}")
    })?;
    Ok(delivery)
}

fn main() {
    match report() {
        Ok(None) => println!("{{}}"),
        Ok(Some(delivery)) => {
            // Codex continues the turn with `reason` as its next prompt: printed before the receipt,
            // so a crash in between leaves the delivery uncertain, never falsely received.
            println!("{}", json!({"decision": "block", "reason": delivery["text"]}));
            let _ = io::Write::flush(&mut io::stdout());
            let dispatch_id = delivery["dispatch_id"].as_str().unwrap_or_default().to_string();
            let receipt = teams::AgentAction::Receipt { dispatch_id: dispatch_id.clone(),
                outcome: teams::TransportOutcome::Received { native_id: format!("codex-stop:{dispatch_id}") } };
            if let Err(error) = teams::client::request(&receipt).and_then(|reply| reply.result) {
                eprintln!("cockpit-codex-hook: team receipt not recorded: {error}");
            }
        },
        Err(error) => {
            eprintln!("cockpit-codex-hook: {error}");
            std::process::exit(1);
        }
    }
}
