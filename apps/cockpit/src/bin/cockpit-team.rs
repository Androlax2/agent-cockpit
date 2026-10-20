//! Sandbox-safe collaboration actions. Host approval controls are deliberately absent.

use cockpit::teams::{AgentAction, client};
use cockpit::approval::escape_hiding;
use std::io::{self, Read};

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

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments == ["prepare"] {
        if let Err(error) = client::prepare() {
            eprintln!("cockpit-team: {}", escape_hiding(&error.to_string(), "", ""));
            std::process::exit(1);
        }
        return;
    }
    if arguments == ["channel"] {
        if let Err(error) = cockpit::teams::channel::run() {
            eprintln!("cockpit-team channel: {error}");
            std::process::exit(1);
        }
        return;
    }
    if arguments == ["--help"] || arguments.is_empty() {
        println!("cockpit-team context\ncockpit-team propose|send < tagged-action.json\nLink requests require Theo's approval. Collaboration never grants tool or scope permissions.");
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
