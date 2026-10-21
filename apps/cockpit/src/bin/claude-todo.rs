//! claude-todo: Theo's todo list (the cockpit's Todo panel), for Claude inside the sandbox.
//!
//!     claude-todo list [--all]        the items of this directory's project (--all: every project)
//!     claude-todo start <id>          mark it in progress
//!     claude-todo done <id>           mark it done
//!     claude-todo todo <id>           back to to do
//!     claude-todo add '<title>' [--notes '<details>'] [--design] [--kind <kind>]
//!                                     a new item for this directory's project
//!     claude-todo propose '<title>' --why '<reason>' [--notes '<details>'] [--design] [--kind <kind>] [--project <name>]
//!                                     an idea for Theo to approve or decline (the Proposed tab)
//!                                     --design: work for the project's designer session
//!                                     --project: for another project, a checkout in ~/IdeaProjects;
//!                                     add stays limited to this directory's project (security audit M4)
//!     claude-todo blocked '<what you could not do>' --why '<what stopped you>' [--notes '<details>']
//!                                     from any project: greywall, claude-sessions, the launcher or
//!                                     another part of Theo's setup kept you from doing something;
//!                                     filed in the dotfiles list and queued for its session
//!     claude-todo stack <repo> '<title>' --why '<reason>' [--notes '<details>'] [--kind <kind>]
//!                                     from any project: a security hole, bug or improvement in the own
//!                                     code of a sandbox stack repository (greywall, greyproxy; not
//!                                     greywall.json, which is a blocked report); proposed in that
//!                                     repository's list for Theo to approve
//!     claude-todo greywall '<title>' …  the same as stack greywall
//!     claude-todo delete <id>         remove an item a session filed in this project (never one Theo wrote)
//!     claude-todo show <id>           an item with its notes, the paths of its pasted images and, for a proposal, why
//!
//! An id is a typed item's 8 hex characters, or a GitHub issue as owner/repo#12. The list is read
//! from ~/.local/state/claude-sessions/todo.json; the sandbox cannot write there, so changes go to
//! the OSD daemon on its report socket, which queues them with the folder the session's sandbox
//! started in, and the cockpit's column applies them. CLAUDE_TODO_PATH overrides the list (tests).

use cockpit::{approval, paths, todo_images};
use cockpit::todo::{Category, Item, STACK_REPOS};
use cockpit::util::{collapse, now_secs, project_name, repository_root};
use regex::Regex;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

const USAGE: &str = "    claude-todo list [--all]        the items of this directory's project (--all: every project)
    claude-todo start <id>          mark it in progress
    claude-todo done <id>           mark it done
    claude-todo todo <id>           back to to do
    claude-todo add '<title>' [--notes '<details>'] [--design] [--kind <kind>]
    claude-todo propose '<title>' --why '<reason>' [--notes '<details>'] [--design] [--kind <kind>] [--project <name>]
                                    an idea for Theo to approve or decline; one Theo declined is not filed again
                                    --design: work for the project's designer session (a gap in the design)
                                    --kind: task (default), feature, bug, improvement, chore, research
                                    --project: work for another project (its folder name in ~/IdeaProjects):
                                    proposed in that project's list; add files only in this directory's project
    claude-todo blocked '<what you could not do>' --why '<what stopped you>' [--notes '<details>']
                                    the setup (greywall, claude-sessions, the launcher…) blocked you:
                                    filed in the dotfiles list and queued for its session
    claude-todo stack <repo> '<title>' --why '<reason>' [--notes '<details>'] [--kind <kind>]
                                    a security hole, bug or improvement in the own code of a sandbox stack
                                    repository, from any project: <repo> is the one the code is in (greywall,
                                    greyproxy); proposed in its list for Theo to approve. A path or host
                                    greywall.json denies you is a blocked report, not this
    claude-todo greywall '<title>' …  the same as stack greywall
    claude-todo delete <id>         remove an item a session filed in this project (never one Theo wrote)
    claude-todo show <id>           an item with its notes and the paths of its pasted images";

fn todo_path() -> PathBuf {
    std::env::var("CLAUDE_TODO_PATH").map(PathBuf::from).unwrap_or_else(|_| paths::todo())
}

/// The project of the directory: its git checkout (a parallel clone counts as its repository).
fn project() -> String {
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    project_name(&repository_root(&cwd))
}

/// The change, handed to the OSD daemon on its report socket: it adds the folder this session's
/// sandbox started in, which decides the project (security audit 2026-10-19, M4).
fn append(mut update: serde_json::Map<String, Value>) -> Result<(), String> {
    update.insert("session".into(), json!(cockpit::agents::current_session_id()?));
    update.insert("at".into(), json!(now_secs()));
    approval::file_request("todo-update", &Value::Object(update)).map_err(|refusal| match refusal {
        approval::Refusal::Unreachable(reason) => format!("cannot reach the OSD daemon: {reason}"),
        approval::Refusal::Refused(reason) => format!("the OSD daemon refused it: {reason}"),
    })
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
    println!("Kind: {}", category_label(item));
    for (label, key) in [("Why", "why"), ("Notes", "notes")] {
        let text = field(key);
        if !text.is_empty() {
            println!("\n{label}:\n{text}");
        }
    }
    let images = item.as_object().map(|fields| Item(fields.clone()).images()).unwrap_or_default();
    if !images.is_empty() {
        let list_folder = todo_path().parent().map(PathBuf::from).unwrap_or_default();
        println!("\nImages (open each with your file reader):");
        for (number, file) in images {
            println!("{} {}", todo_images::marker(number), todo_images::path_in(&list_folder, &file).display());
        }
    }
    0
}

fn category_label(value: &Value) -> String {
    let Some(fields) = value.as_object() else { return "Unknown".into() };
    let item = Item(fields.clone());
    item.category().map(|category| category.label().to_string())
        .unwrap_or_else(|| format!("Unknown: {}", value["category"]))
}

fn file_arguments(action: &str, repo: Option<&str>, title: &str, arguments: &[&str]) -> i32 {
    let (mut why, mut notes, mut category, mut target) = (None, None, None, None);
    let mut is_design = false;
    let mut remaining = arguments;
    while let Some((&flag, rest)) = remaining.split_first() {
        if flag == "--design" && !matches!(action, "blocked" | "stack") && !is_design {
            is_design = true;
            remaining = rest;
            continue;
        }
        let Some((&value, rest)) = rest.split_first() else { eprintln!("{USAGE}"); return 2 };
        match flag {
            "--why" if action != "add" && why.is_none() => why = Some(value.to_string()),
            "--notes" if notes.is_none() => notes = Some(value.to_string()),
            "--project" if action == "propose" && target.is_none() => target = Some(value.to_string()),
            "--kind" if action != "blocked" && category.is_none() => {
                let Some(parsed) = Category::parse(value) else { eprintln!("claude-todo: invalid kind {value:?}\n{USAGE}"); return 2 };
                category = Some(parsed);
            }
            _ => { eprintln!("{USAGE}"); return 2 }
        }
        remaining = rest;
    }
    if action != "add" && !why.as_ref().is_some_and(|reason| !reason.trim().is_empty()) {
        eprintln!("{USAGE}");
        return 2;
    }
    file(action, repo, title, why, notes, is_design, category, target)
}

fn file(action: &str, repo: Option<&str>, title: &str, why: Option<String>, notes: Option<String>, is_design: bool, category: Option<Category>, target: Option<String>) -> i32 {
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
    if let Some(category) = category {
        update.insert("category".into(), json!(category.as_str()));
    }
    if let Some(target) = &target {
        update.insert("project".into(), json!(target));
    }
    if let Some(repo) = repo {
        update.insert("repo".into(), json!(repo));
    }
    for (key, value) in [("why", why), ("notes", notes)] {
        if let Some(value) = value {
            update.insert(key.into(), json!(value.chars().take(4000).collect::<String>()));
        }
    }
    match append(update) {
        Ok(()) if action == "propose" => { println!("Proposed to Theo in {}'s list (Theo approves or declines it).", target.unwrap_or_else(project)); 0 }
        Ok(()) if action == "stack" => { println!("Proposed to Theo in {}'s list (Theo approves or declines it).", repo.unwrap_or_default()); 0 }
        Ok(()) if action == "blocked" => { println!("Reported in the dotfiles list and queued for its session. Tell Theo what you could not do; don't work around it."); 0 }
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
        println!("{} {}  {status:<8}  [{}] {item_project}: {}{}", glyph(&status), item.get("id").and_then(Value::as_str).unwrap_or(""), category_label(item),
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
    let item_key = Regex::new(r"^[0-9a-f]{8}$").unwrap();
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let status = match words.as_slice() {
        ["--help" | "-h" | "help", ..] | ["add" | "propose" | "blocked" | "stack" | "greywall", "--help" | "-h", ..] | ["stack", _, "--help" | "-h", ..] => { println!("{USAGE}"); 0 }
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
        ["delete", id] if item_key.is_match(id) => {
            let mut update = serde_json::Map::new();
            update.insert("action".into(), json!("delete"));
            update.insert("id".into(), json!(id));
            match append(update) {
                Ok(()) => { println!("{id} deleted (the column applies it; only items a session filed in this project)."); 0 }
                Err(error) => { eprintln!("claude-todo: {error}"); 1 }
            }
        }
        ["show", id] if key.is_match(id) => show(id),
        [action @ ("add" | "propose" | "blocked"), title, rest @ ..] if !title.trim().is_empty() => file_arguments(action, None, title, rest),
        ["stack", repo, title, rest @ ..] if STACK_REPOS.contains(repo) && !title.trim().is_empty() => file_arguments("stack", Some(repo), title, rest),
        ["stack", repo, ..] if !STACK_REPOS.contains(repo) => {
            eprintln!("claude-todo: {repo:?} is not a stack repository (one of: {}); work for another project goes through propose --project\n{USAGE}", STACK_REPOS.join(", "));
            2
        }
        ["greywall", title, rest @ ..] if !title.trim().is_empty() => file_arguments("stack", Some("greywall"), title, rest),
        _ => { eprintln!("{USAGE}"); 2 }
    };
    std::process::exit(status);
}
