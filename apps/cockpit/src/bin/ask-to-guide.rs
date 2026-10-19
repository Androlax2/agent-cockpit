//! Hand Theo steps to do by hand (a browser, a console, an app) as a guide on screen, and learn
//! how it went.
//!
//!     ask-to-guide [--title TITLE] < steps
//!     ask-to-guide --wait <id>        keep waiting for an earlier guide
//!
//! Run by Claude inside greywall. The steps go to ~/.cache/claude/guides/<id>.guide and the OSD
//! daemon opens claude-guide outside the sandbox, in a floating window. The steps, from stdin:
//!
//!     # Title                                  (or --title)
//!     ## Turn on the API                       a section heading
//!     1. Open https://console.cloud.google.com and pick the project      a step ("- ", "1. " or plain)
//!        open: <url>                           the link Ctrl+O opens (else the step's first link)
//!        copy: <value>                         the value Ctrl+Y copies
//!        ask: <question>                       Theo types an answer for you
//!        run: <command>                        a command, approved like ask-to-run before it runs
//!        More text indented under a step continues it.
//!
//! Prints what Theo did: his answers, his notes on any step, the commands run with their output,
//! or where he got stuck and why. Exits 0 when every step is done, 3 when he is stuck, 125 when he
//! closed the guide, 124 when the wait runs out (9 minutes; ask-to-guide --wait <id> resumes), 126
//! when no daemon reads the requests (list the steps in the chat then), 2 for unusable steps.
//! CLAUDE_GUIDES_DIR and ASK_TO_GUIDE_WAIT_S override the folder and the wait (tests).

use cockpit::approval;
use cockpit::util::{now_secs, token_hex};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::io::Read;
use std::time::{Duration, Instant};

const MAX_STEPS: usize = 60;
const OUTPUT_LINES: usize = 200;
const STUCK: i32 = 3;
const CLOSED: i32 = 125;
const TIMED_OUT: i32 = 124;
const NO_DAEMON: i32 = 126;
const UNUSABLE: i32 = 2;

/// The title and the steps; each step {"text", "section", and any of open/copy/ask/run}.
fn parse_steps(text: &str) -> (String, Vec<Map<String, Value>>) {
    let marker = Regex::new(r"^(?:[-*]|\d+[.)])\s+").unwrap();
    let attribute = Regex::new(r"^(open|copy|ask|run):\s*(.*)$").unwrap();
    let (mut title, mut section, mut steps) = (String::new(), String::new(), Vec::<Map<String, Value>>::new());
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        let indented = raw.starts_with(' ') || raw.starts_with('\t');
        let line = raw.trim();
        if indented && !steps.is_empty() {
            let step = steps.last_mut().unwrap();
            match attribute.captures(line).filter(|found| !found[2].is_empty()) {
                Some(found) => { step.insert(found[1].to_string(), json!(found[2].to_string())); }
                None => {
                    let text = format!("{}\n{line}", step["text"].as_str().unwrap_or(""));
                    step.insert("text".into(), json!(text));
                }
            }
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ") {
            section = heading.trim().to_string();
        } else if let Some(heading) = line.strip_prefix("# ") {
            title = heading.trim().to_string();
        } else {
            let mut step = Map::new();
            step.insert("text".into(), json!(marker.replace(line, "").to_string()));
            step.insert("section".into(), json!(section));
            steps.push(step);
        }
    }
    (title, steps)
}

fn wait_seconds() -> u64 {
    std::env::var("ASK_TO_GUIDE_WAIT_S").ok().and_then(|s| s.parse().ok()).unwrap_or(540)
}

fn progress(answers_key: &str, id: &str) -> String {
    let state = std::fs::read_to_string(approval::answers_dir(answers_key, "guides").join(format!("{id}.progress"))).ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    match state {
        Some(state) if state.get("step").is_some() && state.get("total").is_some() && state.get("done").is_some() =>
            format!("Theo is on step {} of {} ({} done).", state["step"], state["total"], state["done"]),
        _ => "Theo has not opened it yet.".into(),
    }
}

fn shown(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => "None".into(),
        Some(other) => other.to_string(),
    }
}

/// Prints what Theo did; returns the exit status.
fn report(result: &Value) -> i32 {
    let status = result.get("status").and_then(Value::as_str).unwrap_or("");
    let total = result.get("total").map(|t| shown(Some(t))).unwrap_or_else(|| "?".into());
    match status {
        "done" => println!("Theo did every step ({total})."),
        "stuck" => {
            println!("Theo is stuck at step {} of {total}: {}", shown(result.get("step")), result.get("step_text").and_then(Value::as_str).unwrap_or(""));
            let note = result.get("note").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or("(none)");
            println!("His note: {note}");
        }
        _ => println!("Theo closed the guide at step {} of {total} without finishing.", shown(result.get("step"))),
    }
    for note in result.get("notes").and_then(Value::as_array).into_iter().flatten() {
        println!("\nTheo's note on step {}: {}", shown(note.get("step")), shown(note.get("note")));
    }
    for answer in result.get("answers").and_then(Value::as_array).into_iter().flatten() {
        println!("\nStep {} asked: {}\nTheo answered: {}", shown(answer.get("step")), shown(answer.get("question")), shown(answer.get("answer")));
    }
    for run in result.get("runs").and_then(Value::as_array).into_iter().flatten() {
        let edited = run.get("edited_command").and_then(Value::as_str).filter(|e| !e.is_empty());
        println!("\nStep {} ran: {}{}", shown(run.get("step")), edited.map(str::to_string).unwrap_or_else(|| shown(run.get("command"))),
                 if edited.is_some() { " (edited by Theo)" } else { "" });
        if run.get("status").and_then(Value::as_str) == Some("declined") {
            println!("Theo declined to run it.");
            continue;
        }
        let output = approval::strip_ansi(run.get("output").and_then(Value::as_str).unwrap_or(""));
        let lines: Vec<&str> = output.lines().collect();
        if lines.len() > OUTPUT_LINES {
            println!("[first {} lines left out]", lines.len() - OUTPUT_LINES);
        }
        println!("{}", lines[lines.len().saturating_sub(OUTPUT_LINES)..].join("\n"));
        if let Some(redacted) = run.get("redacted").and_then(Value::as_u64).filter(|count| *count > 0) {
            println!("[{redacted} secret-looking value(s) shown as [redacted]]");
        }
        println!("[exit {}]", shown(run.get("exit")));
    }
    match status { "done" => 0, "stuck" => STUCK, _ => CLOSED }
}

fn wait_for_result(answers_key: &str, id: &str) -> i32 {
    let path = approval::answers_dir(answers_key, "guides").join(format!("{id}.result"));
    let deadline = Instant::now() + Duration::from_secs(wait_seconds());
    while Instant::now() < deadline {
        if let Some(result) = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()) {
            return report(&result);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    println!("The guide is still open. {} Keep waiting with: ask-to-guide --wait {id}", progress(answers_key, id));
    TIMED_OUT
}

fn main() {
    if approval::in_terminal_session() {
        println!("Not available in a session started with \"this terminal\": list the steps in the chat instead.");
        std::process::exit(NO_DAEMON);
    }
    let Some(answers_key) = approval::own_answers_key() else {
        println!("{} List the steps in the chat meanwhile.", approval::NO_ANSWERS_KEY);
        std::process::exit(NO_DAEMON);
    };
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() == 2 && arguments[0] == "--wait" && approval::valid_id(&arguments[1]) {
        std::process::exit(wait_for_result(&answers_key, &arguments[1]));
    }
    let mut title = String::new();
    if arguments.len() == 2 && arguments[0] == "--title" {
        title = arguments[1].clone();
        arguments.clear();
    }
    if !arguments.is_empty() {
        eprintln!("usage: ask-to-guide [--title TITLE] < steps | ask-to-guide --wait <id>");
        std::process::exit(UNUSABLE);
    }
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let (parsed_title, steps) = parse_steps(&input);
    if title.is_empty() {
        title = if parsed_title.is_empty() { "Steps from Claude".into() } else { parsed_title };
    }
    if steps.is_empty() || steps.len() > MAX_STEPS {
        eprintln!("No steps read from stdin, or more than {MAX_STEPS}: one step per line, see ask-to-guide's header.");
        std::process::exit(UNUSABLE);
    }
    let id = token_hex(6);
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    let guide = json!({"id": id, "title": title, "steps": steps, "cwd": cwd, "project": approval::project_of(&cwd),
                       "session": std::env::var("CLAUDE_CODE_SESSION_ID").unwrap_or_default(), "answers": answers_key,
                       "requested_at": now_secs()});
    let path = approval::guides_dir().join(format!("{id}.guide"));
    if let Err(error) = approval::write_json(&path, &guide) {
        eprintln!("ask-to-guide: cannot file the guide: {error}");
        std::process::exit(1);
    }
    if !approval::send(&format!("guide-request {id}")) {
        let _ = std::fs::remove_file(&path);
        println!("No OSD daemon reads the requests (osd.service down?). List the steps in the chat instead.");
        std::process::exit(NO_DAEMON);
    }
    eprintln!("Guide {id} is on Theo's screen ({} steps); waiting for him.", steps.len());
    std::process::exit(wait_for_result(&answers_key, &id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_read_with_sections_links_values_questions_and_commands() {
        let (title, steps) = parse_steps("# Key\n## API\n1. Open https://x.example/ and pick\n2. Enable\n   open: https://x.example/lib\n## Account\n- Copy the email\n  copy: a@b\n  It ends in b.\nPaste it\n  ask: Which?\nRun\n  run: make x\n");
        assert_eq!(title, "Key");
        assert_eq!(steps.iter().map(|s| s["section"].as_str().unwrap()).collect::<Vec<_>>(), vec!["API", "API", "Account", "Account", "Account"]);
        assert_eq!(steps[1]["open"], "https://x.example/lib");
        assert_eq!(steps[2]["text"], "Copy the email\nIt ends in b.");
        assert_eq!((steps[3]["ask"].as_str(), steps[4]["run"].as_str()), (Some("Which?"), Some("make x")));
    }
}
