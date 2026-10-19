//! claude-todo: Theo's todo list (the cockpit's Todo panel), for Claude inside the sandbox.
//!
//!     claude-todo list [--all]        the items of this directory's project (--all: every project)
//!     claude-todo start <id>          mark it in progress
//!     claude-todo done <id>           mark it done
//!     claude-todo todo <id>           back to to do
//!     claude-todo add '<title>' [--notes '<details>'] [--design]
//!                                     a new item for this directory's project
//!     claude-todo propose '<title>' --why '<reason>' [--notes '<details>'] [--design]
//!                                     an idea for Theo to approve or decline (the Proposed tab)
//!                                     --design: work for the project's designer session
//!     claude-todo show <id>           an item with its notes and, for a proposal, why
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
    claude-todo add '<title>' [--notes '<details>'] [--design]
    claude-todo propose '<title>' --why '<reason>' [--notes '<details>'] [--design]
                                    an idea for Theo to approve or decline; one Theo declined is not filed again
                                    --design: work for the project's designer session (a gap in the design)
    claude-todo show <id>           an item with its notes";

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
    match status { "doing" => "◐", "done" => "✓", "proposed" => "?", _ => "•" }
}

fn read_list() -> Value {
    let data = fs::read_to_string(todo_path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(json!({}));
    if data.is_array() { json!({"items": data, "issues": {}}) } else { data }
}

fn show(id: &str) -> i32 {
    let data = read_list();
    let Some(item) = data.get("items").and_then(Value::as_array).into_iter().flatten().find(|item| item.get("id").and_then(Value::as_str) == Some(id)) else {
        eprintln!("claude-todo: no item {id}");
        return 1;
    };
    let field = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    println!("{} {id}  {}  {}: {}", glyph(&field("status")), field("status"), field("project"), field("text"));
    for (label, key) in [("Why", "why"), ("Notes", "notes")] {
        let text = field(key);
        if !text.is_empty() {
            println!("\n{label}:\n{text}");
        }
    }
    0
}

/// The value after `--flag` in the arguments, if any.
fn option(arguments: &[&str], flag: &str) -> Option<String> {
    arguments.iter().position(|word| *word == flag).and_then(|at| arguments.get(at + 1)).map(|value| value.to_string())
}

fn file(action: &str, title: &str, why: Option<String>, notes: Option<String>, is_design: bool) -> i32 {
    if action == "propose" && read_list().get("declined").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str)
        .any(|declined| collapse(declined).to_lowercase() == collapse(title).to_lowercase()) {
        println!("Theo already declined this idea; it is not filed again.");
        return 0;
    }
    let mut update = serde_json::Map::new();
    update.insert("action".into(), json!(action));
    update.insert("text".into(), json!(collapse(title).chars().take(300).collect::<String>()));
    update.insert("cwd".into(), json!(std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default()));
    if is_design {
        update.insert("kind".into(), json!("design"));
    }
    for (key, value) in [("why", why), ("notes", notes)] {
        if let Some(value) = value {
            update.insert(key.into(), json!(value.chars().take(4000).collect::<String>()));
        }
    }
    match append(update) {
        Ok(()) if action == "propose" => { println!("Proposed to Theo in {}'s list (Theo approves or declines it).", project()); 0 }
        Ok(()) => { println!("Added to {}'s list.", project()); 0 }
        Err(error) => { eprintln!("claude-todo: {error}"); 1 }
    }
}

fn list(everything: bool) -> i32 {
    let data = read_list();
    let here = project();
    let mut shown = 0;
    for item in data.get("items").and_then(Value::as_array).into_iter().flatten() {
        let item_project = item.get("project").and_then(Value::as_str).unwrap_or("");
        if !everything && item_project != here {
            continue;
        }
        let status = item.get("status").and_then(Value::as_str).map(str::to_string)
            .unwrap_or_else(|| if item.get("sent").and_then(Value::as_bool).unwrap_or(false) { "doing".into() } else { "todo".into() });
        let has_notes = item.get("notes").and_then(Value::as_str).is_some_and(|notes| !notes.is_empty());
        println!("{} {}  {status:<8}  {item_project}: {}{}", glyph(&status), item.get("id").and_then(Value::as_str).unwrap_or(""),
                 item.get("text").and_then(Value::as_str).unwrap_or(""), if has_notes { "  (notes: claude-todo show)" } else { "" });
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
    let is_design = arguments.iter().any(|word| word == "--design");
    let words: Vec<&str> = arguments.iter().map(String::as_str).filter(|word| *word != "--design").collect();
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
        ["show", id] if key.is_match(id) => show(id),
        ["add", title, rest @ ..] if !title.trim().is_empty() && (rest.is_empty() || (rest.len() == 2 && rest[0] == "--notes")) =>
            file("add", title, None, option(rest, "--notes"), is_design),
        ["propose", title, rest @ ..] if !title.trim().is_empty() && option(rest, "--why").is_some_and(|why| !why.trim().is_empty())
                                         && rest.len() % 2 == 0 && rest.chunks(2).all(|pair| pair[0] == "--why" || pair[0] == "--notes") =>
            file("propose", title, option(rest, "--why"), option(rest, "--notes"), is_design),
        _ => { eprintln!("{USAGE}"); 2 }
    };
    std::process::exit(status);
}
