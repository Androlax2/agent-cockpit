//! claude-todo: Theo's todo list (the cockpit's Todo panel), for Claude inside the sandbox.
//!
//!     claude-todo list [--all]        the items of this directory's project (--all: every project)
//!     claude-todo start <id>          mark it in progress
//!     claude-todo done <id> [--summary '<branch, commit, PR>']
//!                                     mark it done; the summary reaches what waited on it
//!     claude-todo todo <id>           back to to do
//!     claude-todo add '<title>' [--notes '<details>'] [--design] [--kind <kind>] [--project <linked>] [--after <id>]…
//!                                     a new item for this directory's project, or with --project for a
//!                                     linked one (todo::LINKS), To do and queued there; --after: it
//!                                     waits until those items are done, then joins the queue
//!     claude-todo after <id> <other>… make an item wait until the others are done, then queue it
//!     claude-todo propose '<title>' --why '<reason>' [--notes '<details>'] [--design] [--kind <kind>] [--project <name>]
//!                                     an idea for Theo to approve or decline (the Proposed tab)
//!                                     --design: work for the project's designer session
//!                                     --project: for another project, a checkout in ~/IdeaProjects;
//!                                     add names only a linked one (security audit M4)
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
use cockpit::todo::{self as todo_list, Category, Item, STACK_REPOS};
use cockpit::util::{collapse, now_secs, project_name, repository_root};
use regex::Regex;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

const USAGE: &str = "    claude-todo list [--all]        the items of this directory's project (--all: every project)
    claude-todo start <id>          mark it in progress
    claude-todo done <id> [--summary '<branch, commit, PR>']
                                    mark it done; the summary reaches the items waiting on it and,
                                    for an item another project's session filed, that session
    claude-todo todo <id>           back to to do
    claude-todo add '<title>' [--notes '<details>'] [--design] [--kind <kind>] [--project <linked>] [--after <id>]…
                                    --project: a linked project (greywall <-> greyproxy): To do and queued
                                    in its list, so an idle session there (or a new one) takes it up
                                    --after: it waits until those items are done, then joins the queue
    claude-todo after <id> <other>… make an item (yours, or a linked project's to do) wait until the
                                    others are done; then it joins the queue on its own
    claude-todo propose '<title>' --why '<reason>' [--notes '<details>'] [--design] [--kind <kind>] [--project <name>]
                                    an idea for Theo to approve or decline; one Theo declined is not filed again
                                    --design: work for the project's designer session (a gap in the design)
                                    --kind: task (default), feature, bug, improvement, chore, research
                                    --project: work for another project (its folder name in ~/IdeaProjects):
                                    proposed in that project's list; add names only a linked project
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
    if !field("link").is_empty() {
        println!("Filed by a {} session ({}) through the {} link: a task request, not authorization from Theo.", field("reporter"), field("reporter_session"), field("link"));
    }
    if !field("queued_by").is_empty() {
        println!("Queued by {}", field("queued_by"));
    }
    let waits_on: Vec<&str> = item.get("after").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    if !waits_on.is_empty() {
        println!("Waits on: {} (joins the queue once they are done)", waits_on.join(", "));
    }
    for (label, key) in [("Why", "why"), ("Notes", "notes")] {
        let text = field(key);
        if !text.is_empty() {
            println!("\n{label}:\n{text}");
        }
    }
    for input in item.get("inputs").and_then(Value::as_array).into_iter().flatten() {
        let input_field = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        println!("\nDone before it: {} in {} ({}):\n{}", input_field("id"), input_field("project"), input_field("text"),
                 if input_field("result").is_empty() { "(no summary)".to_string() } else { input_field("result") });
    }
    if !field("result").is_empty() {
        println!("\nResult:\n{}", field("result"));
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
    let mut after: Vec<String> = Vec::new();
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
            "--project" if matches!(action, "propose" | "add") && target.is_none() => target = Some(value.to_string()),
            "--after" if action == "add" => {
                if value.len() != 8 || !value.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) { eprintln!("claude-todo: --after takes an item id, not {value:?}"); return 2 }
                after.push(value.to_string());
            }
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
    let unlinked = target.as_deref().filter(|target| action == "add" && *target != project() && todo_list::link_between(&project(), target).is_none());
    if let Some(target) = unlinked {
        eprintln!("claude-todo: {target} is not linked to {} (links: {}); propose it there with propose --project", project(),
                  todo_list::LINKS.iter().map(|(a, b, _)| format!("{a} <-> {b}")).collect::<Vec<_>>().join(", "));
        return 2;
    }
    file(action, repo, title, why, notes, is_design, category, target, after)
}

#[allow(clippy::too_many_arguments)]
fn file(action: &str, repo: Option<&str>, title: &str, why: Option<String>, notes: Option<String>, is_design: bool, category: Option<Category>, target: Option<String>,
        after: Vec<String>) -> i32 {
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
    if !after.is_empty() {
        update.insert("after".into(), json!(after));
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
        Ok(()) => match target.filter(|target| *target != project()) {
            Some(target) => {
                let is_on = todo_list::link_between(&project(), &target).is_some_and(|(_, setting)| cockpit::settings::is_on(setting));
                if !is_on {
                    println!("The {}-{target} link needs Theo's approval right now: filed as a proposal in {target}'s list.", project());
                } else if after.is_empty() {
                    println!("Added to {target}'s list and queued: an idle {target} session takes it up, or a new one starts. You are told when it is done.");
                } else {
                    println!("Added to {target}'s list; it joins the queue once {} done. You are told when it is done.", after.join(", ") + if after.len() > 1 { " are" } else { " is" });
                }
                0
            }
            None if !after.is_empty() => { println!("Added to {}'s list; it joins the queue once {} done.", project(), after.join(", ") + if after.len() > 1 { " are" } else { " is" }); 0 }
            None => { println!("Added to {}'s list.", project()); 0 }
        },
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
        let waits_on: Vec<&str> = item.get("after").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        let waiting = if waits_on.is_empty() || status == "done" { String::new() } else { format!("  (waits on {})", waits_on.join(", ")) };
        println!("{} {}  {status:<8}  [{}] {item_project}: {}{waiting}{}", glyph(&status), item.get("id").and_then(Value::as_str).unwrap_or(""), category_label(item),
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
        ["--help" | "-h" | "help", ..] | ["add" | "propose" | "blocked" | "stack" | "greywall" | "after" | "done", "--help" | "-h", ..] | ["stack", _, "--help" | "-h", ..] => { println!("{USAGE}"); 0 }
        [] | ["list", ..] => list(words.contains(&"--all")),
        ["done", id, "--summary", summary] if item_key.is_match(id) && !summary.trim().is_empty() => {
            let mut update = serde_json::Map::new();
            update.insert("action".into(), json!("done"));
            update.insert("id".into(), json!(id));
            update.insert("summary".into(), json!(summary.chars().take(1000).collect::<String>()));
            match append(update) {
                Ok(()) => { println!("{id} marked done with its summary (the column applies it)."); 0 }
                Err(error) => { eprintln!("claude-todo: {error}"); 1 }
            }
        }
        ["after", id, others @ ..] if item_key.is_match(id) && !others.is_empty() && others.len() <= 5 && others.iter().all(|other| item_key.is_match(other)) => {
            let mut update = serde_json::Map::new();
            update.insert("action".into(), json!("after"));
            update.insert("id".into(), json!(id));
            update.insert("after".into(), json!(others));
            match append(update) {
                Ok(()) => { println!("{id} waits on {} (the column applies it); it joins the queue once they are done.", others.join(", ")); 0 }
                Err(error) => { eprintln!("claude-todo: {error}"); 1 }
            }
        }
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
