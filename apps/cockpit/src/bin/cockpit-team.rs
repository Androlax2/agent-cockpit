//! Peer-authenticated agent actions and a separate, host-socket-only chief control mode.

use cockpit::teams::{AgentAction, client};
use cockpit::approval::escape_hiding;
use std::io::{self, Read};

const HELP: &str = r#"cockpit-team context            your task, your role and your teammates' session ids
cockpit-team <action> < action.json   propose, recruit, send, assess or handoff; the JSON's "action" names the same one
cockpit-team chief < action.json      nested {"action":"chief","request":{...}} agent request
cockpit-team chief-host < request.json   host only: {"request":"chief","action":{...}} or chief_snapshot

Recipients are session ids from `context`, never role names. Use each submission_key once
(letters, digits, - or _): resending the same one is a no-op. Task is the object `context` shows.

A notice:
  {"action":"send","message":{"task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"submission_key":"note-1","recipient":"ses_3f2a9c0b1ffeQkTz8mWn4rJd2L","kind":"notice","body":"The frame is ready."}}
A clarification (at most three rounds; answer with kind "reply" and the thread_id you got):
  {"action":"send","message":{"task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"submission_key":"ask-1","recipient":"aaaaaaaa-0000-4000-8000-000000000000","kind":"clarification","body":"Which frame is approved?"}}
Whether the reply resolved your question:
  {"action":"assess","task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"thread_id":"<thread_id>","resolved":true}
Missing teammates start automatically in every project, using Theo's default agent, Designer and
Developer in your folder; a QA recruit means a fresh QA at each handoff (context shows
"qa_starts_at_handoff": true once planned; do not recruit QA again). Keep briefs within the team's
original task; recruitment grants no additional permissions. Your own role, then each new one with its brief:
  {"action":"recruit","recruitment":{"task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"submission_key":"recruit-1","role":"developer","recruits":[{"role":"designer","brief":"Design the jobs list screen in FigCraft."},{"role":"qa","brief":"Check contrast and keyboard use."}],"reason":"The UI has no design yet."}}
The Developer's finished work (requests QA once):
  {"action":"handoff","handoff":{"task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"submission_key":"handoff-1","branch":"feature/x","commit":"<full 40-character commit id>","checkout":"/home/theo/IdeaProjects/app","summary":"What changed.","tests":"cargo test: 57 passed","figcraft":[]}}

Link requests require Theo's approval. Collaboration never grants tool or scope permissions.

Cross-project links (no team membership required): `context` also shows links.approved and your
own links.proposals status receipts. Propose only an already-known peer session; pending requests
reveal no peer facts. links.omitted_history counts older terminal records left out to fit the reply;
active links and pending requests take priority. Use link-propose, link-send or link-assess with the matching snake_case JSON:
  {"action":"link_propose","proposal":{"submission_key":"joint-1","task":{"project":"path:/home/theo/IdeaProjects/app","task_id":"5e1f7e57"},"peer_session_id":"ses_3f2a9c0b1ffeQkTz8mWn4rJd2L","purpose":"Coordinate the shared work."}}
Every message needs an explicit link_id from links.approved; the daemon chooses the other participant:
  {"action":"link_send","message":{"link_id":"<approved link_id>","submission_key":"link-note-1","kind":"notice","body":"The shared interface is ready."}}
Use kind clarification to ask; answer with kind reply and the received thread_id (at most three rounds).
  {"action":"link_assess","link_id":"<approved link_id>","thread_id":"<thread_id>","resolved":true}
Marking the shared TODO done closes the link permanently; reopening the TODO does not restore it.
Links carry coordination only and cannot relay Theo's permission, authorize tools or Git writes,
change scope, recruit teammates or hand off work. Ask Theo directly for authorization.
"#;

fn action(name: &str) -> Result<AgentAction, String> {
    if name == "context" { return Ok(AgentAction::Context); }
    if name == "receive" { return Ok(AgentAction::Receive); }
    if !matches!(name, "chief" | "native-telemetry" | "propose" | "send" | "receipt" | "assess" | "handoff" | "recruit" | "link-propose" | "link-send" | "link-assess") {
        return Err(format!("{name:?} is not an agent action; use context, propose, recruit, send, assess, handoff, receive, receipt, link-propose, link-send or link-assess"));
    }
    let mut input = Vec::new();
    io::stdin().take(client::REQUEST_MAX as u64 + 1).read_to_end(&mut input).map_err(|error| error.to_string())?;
    if input.len() > client::REQUEST_MAX { return Err("team request exceeds 32 KiB".into()); }
    let action: AgentAction = serde_json::from_slice(&input).map_err(|error| format!("invalid team action JSON: {error}"))?;
    let actual = match &action {
        AgentAction::Context => "context", AgentAction::Propose { .. } => "propose", AgentAction::Send { .. } => "send",
        AgentAction::Receive => "receive", AgentAction::Receipt { .. } => "receipt", AgentAction::Assess { .. } => "assess",
        AgentAction::Handoff { .. } => "handoff", AgentAction::Recruit { .. } => "recruit",
        AgentAction::Chief { .. } => "chief",
        AgentAction::NativeTelemetry { .. } => "native-telemetry",
        AgentAction::LinkPropose { .. } => "link-propose", AgentAction::LinkSend { .. } => "link-send", AgentAction::LinkAssess { .. } => "link-assess",
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

fn sleep_request(arguments: &[String]) -> Result<cockpit::teams::HostRequest, String> {
    use cockpit::teams::HostRequest;
    if arguments.len() != 2 || cockpit::agents::of_session_id(&arguments[1]).is_none() {
        return Err("sleep commands need one valid session id; tokens belong in stdin JSON".into());
    }
    let session_id = arguments[1].clone();
    if arguments[0] == "sleep-prepare" { return Ok(HostRequest::PrepareSleep { session_id }); }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Token { token: String }
    let mut input = Vec::new();
    io::stdin().take(client::REQUEST_MAX as u64 + 1).read_to_end(&mut input).map_err(|_| "cannot read sleep token JSON")?;
    if input.len() > client::REQUEST_MAX { return Err("team request exceeds 32 KiB".into()); }
    let payload: Token = serde_json::from_slice(&input).map_err(|_| "invalid sleep token JSON")?;
    let token = payload.token;
    if token.len() != 32 || !token.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
        return Err("invalid sleep token".into());
    }
    Ok(match arguments[0].as_str() {
        "sleep-recorded" => HostRequest::SleepRecorded { session_id, token },
        "sleep-cancel" => HostRequest::CancelSleep { session_id, token },
        "sleep-begin-wake" => HostRequest::BeginWake { session_id, token },
        "sleep-status" => HostRequest::SleepStatus { session_id, token },
        _ => return Err("unknown sleep command".into()),
    })
}

/// Claude's background hook (asyncRewake, at SessionStart and Stop): waits for this session's next
/// approved message, which the daemon hands out only while the session is idle, then wakes Claude
/// with it (exit 2, the text on stderr). This cannot prove acceptance after the hook exits: chief
/// scheduling keeps Claude unavailable. One watcher per session: a newer one replaces it.
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
                    let text = format!("{}\n", delivery["text"].as_str().unwrap_or_default());
                    let mut stderr = io::stderr();
                    let output = io::Write::write_all(&mut stderr, text.as_bytes()).and_then(|_| io::Write::flush(&mut stderr));
                    if let Err(error) = output {
                        let receipt = AgentAction::Receipt { dispatch_id:dispatch_id.into(),
                            outcome:TransportOutcome::Uncertain { reason:format!("native wake output failed: {error}") } };
                        if let Err(error) = client::request(&receipt).and_then(|reply| reply.result) {
                            cockpit::util::log_event("WARN", &format!("native wake uncertain receipt not recorded: {error}"));
                        }
                        return 1;
                    }
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
    if arguments.first().is_some_and(|verb| verb.starts_with("sleep-")) {
        let request = sleep_request(&arguments).unwrap_or_else(|error| { eprintln!("cockpit-team: {error}"); std::process::exit(2) });
        match client::host_request(&request).and_then(|reply| reply.result) {
            Ok(result) => println!("{result}"),
            Err(error) => { eprintln!("cockpit-team: {}", escape_hiding(&error.to_string(), "", "")); std::process::exit(1); },
        }
        return;
    }
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
    if arguments[0] == "chief-host" {
        let result = (|| {
            let mut input = Vec::new();
            io::stdin().take(client::REQUEST_MAX as u64 + 1).read_to_end(&mut input)?;
            if input.len() > client::REQUEST_MAX { return Err(cockpit::teams::TeamError::new(cockpit::teams::ErrorCode::Limit, "chief host request exceeds 32 KiB")); }
            let request: cockpit::teams::HostRequest = serde_json::from_slice(&input)?;
            if !matches!(request, cockpit::teams::HostRequest::Chief { .. } | cockpit::teams::HostRequest::ChiefSnapshot { .. } | cockpit::teams::HostRequest::ChiefAudit { .. }
                | cockpit::teams::HostRequest::VerifyChiefAdmission { .. } | cockpit::teams::HostRequest::ChiefNativeObservation { .. }
                | cockpit::teams::HostRequest::ChiefFocus { .. }) {
                return Err(cockpit::teams::TeamError::new(cockpit::teams::ErrorCode::Invalid, "chief-host requires a chief host request"));
            }
            if matches!(&request,cockpit::teams::HostRequest::Chief {action:cockpit::chief::ChiefHostAction::ReconcileDelivery {..}
                | cockpit::chief::ChiefHostAction::ReconcileLaunch {..} | cockpit::chief::ChiefHostAction::SettleStartup {..}}) {
                use std::io::{BufRead,Write};
                eprintln!("Host confirmation of exact stopped-target evidence:\n{}",escape_hiding(&serde_json::to_string_pretty(&request)?,"",""));
                let mut terminal = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty")?;
                terminal.write_all(b"Confirm exactly this host reconciliation? [y/N] ")?;
                terminal.flush()?;
                let mut answer = String::new();
                std::io::BufReader::new(terminal).read_line(&mut answer)?;
                if !matches!(answer.trim(),"y"|"Y") { return Err(cockpit::teams::TeamError::new(cockpit::teams::ErrorCode::Unauthorized,"host declined confirmation")); }
            }
            client::host_request(&request)?.result
        })();
        match result {
            Ok(result) => println!("{result}"),
            Err(error) => { eprintln!("cockpit-team: {}", escape_hiding(&error.to_string(), "", "")); std::process::exit(1); },
        }
        return;
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
