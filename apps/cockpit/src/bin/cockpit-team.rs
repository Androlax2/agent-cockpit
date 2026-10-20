//! Sandbox-safe collaboration actions. Host approval controls are deliberately absent.

use cockpit::teams::{AgentAction, client};
use cockpit::approval::escape_hiding;
use std::io::{self, Read};

const HELP: &str = r#"cockpit-team context            your task, your role and your teammates' session ids
cockpit-team <action> < action.json   propose, send, assess or handoff; the JSON's "action" names the same one

Recipients are session ids from `context`, never role names. Use each submission_key once
(letters, digits, - or _): resending the same one is a no-op. Task is the object `context` shows.

A notice:
  {"action":"send","message":{"task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"submission_key":"note-1","recipient":"ses_3f2a9c0b1ffeQkTz8mWn4rJd2L","kind":"notice","body":"The frame is ready."}}
A clarification (at most three rounds; answer with kind "reply" and the thread_id you got):
  {"action":"send","message":{"task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"submission_key":"ask-1","recipient":"aaaaaaaa-0000-4000-8000-000000000000","kind":"clarification","body":"Which frame is approved?"}}
Whether the reply resolved your question:
  {"action":"assess","task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"thread_id":"<thread_id>","resolved":true}
The Developer's finished work (requests QA once):
  {"action":"handoff","handoff":{"task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"submission_key":"handoff-1","branch":"feature/x","commit":"<full 40-character commit id>","checkout":"/home/theo/IdeaProjects/app","summary":"What changed.","tests":"cargo test: 57 passed","figcraft":[]}}

Link requests require Theo's approval. Collaboration never grants tool or scope permissions.
"#;

fn action(name: &str) -> Result<AgentAction, String> {
    if name == "context" { return Ok(AgentAction::Context); }
    if name == "receive" { return Ok(AgentAction::Receive); }
    if !matches!(name, "propose" | "send" | "receipt" | "assess" | "handoff") {
        return Err(format!("{name:?} is not an agent action; use context, propose, send, assess, handoff, receive or receipt"));
    }
    let mut input = Vec::new();
    io::stdin().take(client::REQUEST_MAX as u64 + 1).read_to_end(&mut input).map_err(|error| error.to_string())?;
    if input.len() > client::REQUEST_MAX { return Err("team request exceeds 32 KiB".into()); }
    let action: AgentAction = serde_json::from_slice(&input).map_err(|error| format!("invalid team action JSON: {error}"))?;
    let actual = match &action {
        AgentAction::Context => "context", AgentAction::Propose { .. } => "propose", AgentAction::Send { .. } => "send",
        AgentAction::Receive => "receive", AgentAction::Receipt { .. } => "receipt", AgentAction::Assess { .. } => "assess",
        AgentAction::Handoff { .. } => "handoff",
    };
    if name != actual { return Err("command and JSON action do not match".into()); }
    Ok(action)
}

/// Claude's Stop hook, once the turn end is reported: the next approved message, if any, as the
/// hook's decision to continue. Printed before the receipt, so a crash in between leaves the
/// delivery uncertain, never falsely received. Prints nothing when there is none.
fn turn_end() {
    use cockpit::teams::{AgentAction, TransportOutcome};
    let delivery = match client::request(&AgentAction::Receive).and_then(|reply| reply.result) {
        Ok(delivery) => delivery,
        Err(error) => {
            eprintln!("cockpit-team turn-end: no team delivery: {}", escape_hiding(&error.to_string(), "", ""));
            return;
        },
    };
    let Some(dispatch_id) = delivery.get("dispatch_id").and_then(serde_json::Value::as_str) else { return };
    println!("{}", serde_json::json!({"decision": "block", "reason": delivery["text"]}));
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let receipt = AgentAction::Receipt { dispatch_id: dispatch_id.into(),
        outcome: TransportOutcome::Received { native_id: format!("claude-stop:{dispatch_id}") } };
    if let Err(error) = client::request(&receipt).and_then(|reply| reply.result) {
        eprintln!("cockpit-team turn-end: team receipt not recorded: {}", escape_hiding(&error.to_string(), "", ""));
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments == ["prepare"] {
        if let Err(error) = client::prepare() {
            eprintln!("cockpit-team: {}", escape_hiding(&error.to_string(), "", ""));
            std::process::exit(1);
        }
        return;
    }
    if arguments == ["turn-end"] {
        turn_end();
        return;
    }
    if arguments == ["--help"] || arguments.is_empty() {
        print!("{HELP}");
        return;
    }
    if arguments.len() != 1 {
        eprintln!("usage: cockpit-team context | propose|send < tagged-action.json");
        std::process::exit(2);
    }
    let action = action(&arguments[0]).unwrap_or_else(|error| {
        eprintln!("cockpit-team: {}", escape_hiding(&error, "", ""));
        std::process::exit(2);
    });
    match client::request(&action).and_then(|reply| reply.result) {
        Ok(result) => println!("{result}"),
        Err(error) => {
            eprintln!("cockpit-team: {}", escape_hiding(&error.to_string(), "", ""));
            std::process::exit(1);
        }
    }
}
