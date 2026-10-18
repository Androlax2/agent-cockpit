//! claude-todo: Theo's todo list (the cockpit's Todo panel), for Claude inside the sandbox.
//!
//!     claude-todo list [--all]        the items of this directory's project (--all: every project)
//!     claude-todo start <id>          mark it in progress
//!     claude-todo done <id>           mark it done
//!     claude-todo todo <id>           back to to do
//!     claude-todo add '<text>'        a new item for this directory's project
//!
//! An id is a typed item's 8 hex characters, or a GitHub issue as owner/repo#12. The list is read
//! from ~/.local/state/claude-sessions/todo.json; the sandbox cannot write there, so changes are
//! appended to ~/.cache/claude/todo/updates.jsonl and the cockpit's column applies them.
//! CLAUDE_TODO_PATH and CLAUDE_TODO_UPDATES override both paths (tests).

use cockpit::paths;
use cockpit::util::{collapse, now_secs, project_name, repository_root};
use regex::Regex;
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

const USAGE: &str = "    claude-todo list [--all]        the items of this directory's project (--all: every project)
    claude-todo start <id>          mark it in progress
    claude-todo done <id>           mark it done
    claude-todo todo <id>           back to to do
    claude-todo add '<text>'        a new item for this directory's project";

fn todo_path() -> PathBuf {
    std::env::var("CLAUDE_TODO_PATH").map(PathBuf::from).unwrap_or_else(|_| paths::todo())
}

/// The project of the directory: its git checkout (a parallel clone counts as its repository).
fn project() -> String {
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    project_name(&repository_root(&cwd))
}

fn append(mut update: serde_json::Map<String, Value>) -> std::io::Result<()> {
    let path = paths::todo_updates();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    update.insert("session".into(), json!(std::env::var("CLAUDE_CODE_SESSION_ID").unwrap_or_default()));
    update.insert("at".into(), json!(now_secs()));
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", Value::Object(update))
}

fn glyph(status: &str) -> &'static str {
    match status { "doing" => "◐", "done" => "✓", _ => "•" }
}

fn list(everything: bool) -> i32 {
    let data = fs::read_to_string(todo_path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(json!({}));
    let data = if data.is_array() { json!({"items": data, "issues": {}}) } else { data };
    let here = project();
    let mut shown = 0;
    for item in data.get("items").and_then(Value::as_array).into_iter().flatten() {
        let item_project = item.get("project").and_then(Value::as_str).unwrap_or("");
        if !everything && item_project != here {
            continue;
        }
        let status = item.get("status").and_then(Value::as_str).map(str::to_string)
            .unwrap_or_else(|| if item.get("sent").and_then(Value::as_bool).unwrap_or(false) { "doing".into() } else { "todo".into() });
        println!("{} {}  {status:<5}  {item_project}: {}", glyph(&status), item.get("id").and_then(Value::as_str).unwrap_or(""),
                 item.get("text").and_then(Value::as_str).unwrap_or(""));
        shown += 1;
    }
    for (key, value) in data.get("issues").and_then(Value::as_object).into_iter().flatten() {
        let status = value.get("status").and_then(Value::as_str).unwrap_or("");
        println!("{} {key}  {status}", glyph(status));
        shown += 1;
    }
    if shown == 0 {
        println!("{}", if everything { "The list is empty.".to_string() } else { format!("Nothing on the list for {here}.") });
    }
    0
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let key = Regex::new(r"^(?:[0-9a-f]{8}|[\w.-]+/[\w.-]+#\d+)$").unwrap();
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let status = match words.as_slice() {
        [] | ["list", ..] => list(words.contains(&"--all")),
        [action @ ("start" | "done" | "todo"), id] if key.is_match(id) => {
            let status = if *action == "start" { "doing" } else { action };
            let mut update = serde_json::Map::new();
            update.insert("action".into(), json!(status));
            update.insert("id".into(), json!(id));
            match append(update) {
                Ok(()) => {
                    println!("{id} marked {} (the column applies it).", if *action == "start" { "in progress" } else { action });
                    0
                }
                Err(error) => { eprintln!("claude-todo: {error}"); 1 }
            }
        }
        ["add", text] if !text.trim().is_empty() => {
            let mut update = serde_json::Map::new();
            update.insert("action".into(), json!("add"));
            update.insert("text".into(), json!(collapse(text).chars().take(300).collect::<String>()));
            update.insert("cwd".into(), json!(std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default()));
            match append(update) {
                Ok(()) => { println!("Added to {}'s list.", project()); 0 }
                Err(error) => { eprintln!("claude-todo: {error}"); 1 }
            }
        }
        _ => { eprintln!("{USAGE}"); 2 }
    };
    std::process::exit(status);
}
