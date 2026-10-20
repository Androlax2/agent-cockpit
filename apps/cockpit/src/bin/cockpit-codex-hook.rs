//! Native Codex lifecycle → authenticated cockpit reports. Never decides permissions.

use cockpit::{approval, codex, paths};
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

fn report() -> Result<(), String> {
    let mut input = Vec::new();
    io::stdin().take(HOOK_MAX_BYTES + 1).read_to_end(&mut input).map_err(|error| error.to_string())?;
    if input.len() as u64 > HOOK_MAX_BYTES {
        return Err("Codex hook payload exceeds 64 KiB".into());
    }
    let payload = serde_json::from_slice(&input).map_err(|error| format!("invalid Codex hook JSON: {error}"))?;
    if let Some(mut report) = codex::translate_hook(&payload)? {
        update_question(&payload, &mut report)?;
        report["terminal"] = serde_json::json!(std::env::var("CLAUDE_SESSIONS_TERMINAL").is_ok_and(|value| value == "1"));
        approval::file_request("session-request", &report).map_err(|refusal| {
            let (approval::Refusal::Unreachable(reason) | approval::Refusal::Refused(reason)) = refusal;
            format!("cannot report Codex lifecycle: {reason}")
        })?;
    }
    Ok(())
}

fn main() {
    match report() {
        Ok(()) => println!("{{}}"),
        Err(error) => {
            eprintln!("cockpit-codex-hook: {error}");
            std::process::exit(1);
        }
    }
}
