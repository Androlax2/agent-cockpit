//! Sandbox-safe collaboration actions. Host approval controls are deliberately absent.

use cockpit::teams::{AgentAction, client};
use cockpit::approval::escape_hiding;
use std::io::{self, Read};

const HELP: &str = r#"cockpit-team context            your task, your role and your teammates' session ids
cockpit-team <action> < action.json   propose, recruit, send, assess or handoff; the JSON's "action" names the same one

Recipients are session ids from `context`, never role names. Use each submission_key once
(letters, digits, - or _): resending the same one is a no-op. Task is the object `context` shows.

A notice:
  {"action":"send","message":{"task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"submission_key":"note-1","recipient":"ses_3f2a9c0b1ffeQkTz8mWn4rJd2L","kind":"notice","body":"The frame is ready."}}
A clarification (at most three rounds; answer with kind "reply" and the thread_id you got):
  {"action":"send","message":{"task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"submission_key":"ask-1","recipient":"aaaaaaaa-0000-4000-8000-000000000000","kind":"clarification","body":"Which frame is approved?"}}
Whether the reply resolved your question:
  {"action":"assess","task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"thread_id":"<thread_id>","resolved":true}
Missing teammates (Theo approves; they start in his default agent, Designer and Developer in your
folder; a QA recruit means a fresh QA at each handoff). Your own role, then each new one with its brief:
  {"action":"recruit","recruitment":{"task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"submission_key":"recruit-1","role":"developer","recruits":[{"role":"designer","brief":"Design the jobs list screen in FigCraft."},{"role":"qa","brief":"Check contrast and keyboard use."}],"reason":"The UI has no design yet."}}
The Developer's finished work (requests QA once):
  {"action":"handoff","handoff":{"task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"submission_key":"handoff-1","branch":"feature/x","commit":"<full 40-character commit id>","checkout":"/home/theo/IdeaProjects/app","summary":"What changed.","tests":"cargo test: 57 passed","figcraft":[]}}

Link requests require Theo's approval. Collaboration never grants tool or scope permissions.
"#;

fn action(name: &str) -> Result<AgentAction, String> {
    if name == "context" { return Ok(AgentAction::Context); }
    if name == "receive" { return Ok(AgentAction::Receive); }
    if !matches!(name, "propose" | "send" | "receipt" | "assess" | "handoff" | "recruit") {
        return Err(format!("{name:?} is not an agent action; use context, propose, recruit, send, assess, handoff, receive or receipt"));
    }
    let mut input = Vec::new();
    io::stdin().take(client::REQUEST_MAX as u64 + 1).read_to_end(&mut input).map_err(|error| error.to_string())?;
    if input.len() > client::REQUEST_MAX { return Err("team request exceeds 32 KiB".into()); }
    let action: AgentAction = serde_json::from_slice(&input).map_err(|error| format!("invalid team action JSON: {error}"))?;
    let actual = match &action {
        AgentAction::Context => "context", AgentAction::Propose { .. } => "propose", AgentAction::Send { .. } => "send",
        AgentAction::Receive => "receive", AgentAction::Receipt { .. } => "receipt", AgentAction::Assess { .. } => "assess",
        AgentAction::Handoff { .. } => "handoff", AgentAction::Recruit { .. } => "recruit",
    };
    if name != actual { return Err("command and JSON action do not match".into()); }
    Ok(action)
}

fn is_claude_session_id(text: &str) -> bool {
    text.len() == 36 && text.bytes().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => byte == b'-',
        _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
    })
}

fn poll_interval() -> std::time::Duration {
    std::time::Duration::from_millis(std::env::var("COCKPIT_TEAM_POLL_MS").ok().and_then(|value| value.parse().ok()).unwrap_or(3000))
}

/// Claude's background hook (asyncRewake, at SessionStart and Stop): waits for this session's next
/// approved message, which the daemon hands out only while the session is idle, then wakes Claude
/// with it (exit 2, the text on stderr). The receipt follows the text, so a crash in between leaves
/// the delivery uncertain, never falsely received. One watcher per session: a newer one replaces it.
fn watch() -> i32 {
    use cockpit::teams::{AgentAction, TransportOutcome};
    let mut payload = String::new();
    let _ = io::stdin().take(65_536).read_to_string(&mut payload);
    let session = serde_json::from_str::<serde_json::Value>(&payload).ok()
        .and_then(|payload| payload["session_id"].as_str().map(str::to_string)).unwrap_or_default();
    if !is_claude_session_id(&session) {
        eprintln!("cockpit-team watch: the hook payload has no Claude session id");
        return 1;
    }
    let Some(home) = std::env::var_os("HOME") else { return 1 };
    let marker = std::path::Path::new(&home).join(".cache/claude").join(format!("team-watch-{session}.pid"));
    let own = std::process::id().to_string();
    if std::fs::create_dir_all(marker.parent().unwrap_or(&marker)).and_then(|_| std::fs::write(&marker, &own)).is_err() {
        eprintln!("cockpit-team watch: cannot write {}", marker.display());
        return 1;
    }
    let is_current = || std::fs::read_to_string(&marker).is_ok_and(|current| current == own);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(86_400);
    while is_current() && std::time::Instant::now() < deadline {
        // Only the launch's own current session takes: not a nested `claude -p` sharing the launch,
        // nor this session once /clear has given the launch a new one.
        let is_confirmed = match client::request(&AgentAction::Context).and_then(|reply| reply.result) {
            Ok(context) if context["session_id"] == session.as_str() => true,
            Ok(_) => return 0,
            Err(_) => false,
        };
        if is_confirmed {
            if let Ok(delivery) = client::request(&AgentAction::Receive).and_then(|reply| reply.result) {
                if let Some(dispatch_id) = delivery.get("dispatch_id").and_then(serde_json::Value::as_str) {
                    eprintln!("{}", delivery["text"].as_str().unwrap_or_default());
                    let receipt = AgentAction::Receipt { dispatch_id: dispatch_id.into(),
                        outcome: TransportOutcome::Received { native_id: format!("claude-wake:{dispatch_id}") } };
                    let _ = client::request(&receipt);
                    return 2;
                }
            }
        }
        std::thread::sleep(poll_interval());
    }
    0
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
    if arguments == ["watch"] {
        std::process::exit(watch());
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
