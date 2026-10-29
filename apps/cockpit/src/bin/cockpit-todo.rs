//! cockpit-todo: Theo's todo list (the cockpit's Todo panel), for agents inside the sandbox.
//!
//! ```text
//!     cockpit-todo list [--all]        the items of this directory's project (--all: every project)
//!     cockpit-todo start <id>          mark it in progress
//!     cockpit-todo done <id> [--summary '<branch, commit, PR>']
//!                                     mark it done; the summary reaches what waited on it
//!     cockpit-todo todo <id>           back to to do
//!     cockpit-todo add '<title>' [--notes '<details>'] [--design] [--kind <kind>] [--project <name>] [--after <id>]…
//!                                     a new item for this directory's project, or with --project for
//!                                     another one, To do and queued there; --after: it waits until
//!                                     those items are done, then joins the queue
//!     cockpit-todo after <id> <other>… make an item wait until the others are done, then queue it
//!     cockpit-todo propose '<title>' --why '<reason>' [--notes '<details>'] [--design] [--kind <kind>] [--project <name>]
//!                                     an idea for Theo to approve or decline (the Proposed tab)
//!                                     --design: work for the project's designer session
//!                                     --project: for another project, a checkout in ~/IdeaProjects
//!     cockpit-todo blocked '<what you could not do>' --why '<what stopped you>' [--notes '<details>']
//!                                     from any project: greywall, cockpit, the launcher or
//!                                     another part of Theo's setup kept you from doing something;
//!                                     filed in the agent-cockpit list and queued for its session
//!     cockpit-todo stack <repo> '<title>' --why '<reason>' [--notes '<details>'] [--kind <kind>]
//!                                     from any project: a security hole, bug or improvement in the own
//!                                     code of a sandbox stack repository (greywall, greyproxy; not
//!                                     greywall.json, which is a blocked report); proposed in that
//!                                     repository's list for Theo to approve
//!     cockpit-todo greywall '<title>' …  the same as stack greywall
//!     cockpit-todo delete <id>         remove an item a session filed in this project (never one Theo wrote)
//!     cockpit-todo show <id>           an item with its notes, the paths of its pasted images and, for a proposal, why
//! ```
//!
//! An id is a typed item's 8 hex characters, or a GitHub issue as owner/repo#12. The list is read
//! from ~/.local/state/cockpit/todo.json; the sandbox cannot write there, so changes go to
//! the OSD daemon on its report socket, which queues them with the folder the session's sandbox
//! started in, and the cockpit's column applies them. Filing waits for its saved receipt; a rejection
//! fails, and an unconfirmed request times out with exit 124. CLAUDE_TODO_PATH overrides the list (tests).

use cockpit::todo::{Category, Item, STACK_REPOS};
use cockpit::util::{collapse, now_secs, project_name, repository_root};
use cockpit::{approval, paths, todo_images};
use regex::Regex;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const USAGE: &str = "    cockpit-todo list [--all]        the items of this directory's project (--all: every project)
    cockpit-todo receipt <request-id>  saved/rejected/pending result of any filing
    cockpit-todo approve-request <request-id>
                                     host only, through ask-to-run: approve the exact capped handoff
    cockpit-todo unwait <id> <other>…  host only, through ask-to-run: the item stops waiting on those
                                     items; it is not queued (Q stays Theo's)
    cockpit-todo start <id>          mark it in progress
    cockpit-todo done <id> [--summary '<branch, commit, PR>']
                                    mark it done; the summary reaches the items waiting on it and,
                                    for an item another project's session filed, that session
    cockpit-todo todo <id>           back to to do
    cockpit-todo add '<title>' [--notes '<details>'] [--design] [--kind <kind>] [--project <name>] [--after <id>]…
                                    --project: another project (its folder name in ~/IdeaProjects): To do
                                    and queued in its list, so an idle session there (or a new one) takes it up
                                    --after: it waits until those items are done, then joins the queue
    cockpit-todo after <id> <other>… make an item (yours, or another project's to do) wait until the
                                    others are done; then it joins the queue on its own
    cockpit-todo propose '<title>' --why '<reason>' [--notes '<details>'] [--design] [--kind <kind>] [--project <name>]
                                    an idea for Theo to approve or decline; one Theo declined is not filed again
                                    --design: work for the project's designer session (a gap in the design)
                                    --kind: task (default), feature, bug, improvement, chore, research
                                    --project: work for another project (its folder name in ~/IdeaProjects):
                                    proposed in that project's list for Theo to approve
    cockpit-todo blocked '<what you could not do>' --why '<what stopped you>' [--notes '<details>']
                                    the setup (greywall, cockpit, the launcher…) blocked you:
                                    filed in the agent-cockpit list and queued for its session
    cockpit-todo stack <repo> '<title>' --why '<reason>' [--notes '<details>'] [--kind <kind>]
                                    a security hole, bug or improvement in the own code of a sandbox stack
                                    repository, from any project: <repo> is the one the code is in (greywall,
                                    greyproxy); proposed in its list for Theo to approve. A path or host
                                    greywall.json denies you is a blocked report, not this
    cockpit-todo greywall '<title>' …  the same as stack greywall
    cockpit-todo delete <id>         remove an item a session filed in this project (never one Theo wrote)
    cockpit-todo show <id>           an item with its notes and the paths of its pasted images

Filing commands wait up to five seconds for a saved result. Rejections exit 1 with the reason.
Unconfirmed requests exit 124: check cockpit-todo receipt <request-id>; do not file a duplicate.
Blocker reports and cross-project filings allow three open items per reporting project (100 for add/propose
--project from the active Chief session); there is no daily limit.";

fn todo_path() -> PathBuf {
    paths::cockpit_env("COCKPIT_TODO_PATH", "CLAUDE_TODO_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| paths::todo())
}

/// The project of the directory: its git checkout (a parallel clone counts as its repository).
fn project() -> String {
    let cwd = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    project_name(&repository_root(&cwd))
}

/// The change, handed to the OSD daemon on its report socket: it adds the folder this session's
/// sandbox started in, which decides the project (security audit 2026-10-19, M4).
fn append(mut update: serde_json::Map<String, Value>) -> Result<(), String> {
    update.insert(
        "session".into(),
        json!(cockpit::agents::current_session_id()?),
    );
    update.insert("at".into(), json!(now_secs()));
    approval::file_request("todo-update", &Value::Object(update)).map_err(|refusal| match refusal {
        approval::Refusal::Unreachable(reason) => format!("cannot reach the OSD daemon: {reason}"),
        approval::Refusal::Refused(reason) => format!("the OSD daemon refused it: {reason}"),
    })
}

fn glyph(status: &str) -> &'static str {
    match status {
        "doing" => "◐",
        "done" => "✓",
        "proposed" => "?",
        _ => "•",
    }
}

fn read_list() -> Value {
    let data = fs::read_to_string(todo_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(json!({}));
    if data.is_array() {
        json!({"items": data, "issues": {}})
    } else {
        data
    }
}

fn show(id: &str) -> i32 {
    let data = read_list();
    let Some(item) = data
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
    else {
        eprintln!("cockpit-todo: no item {id}");
        return 1;
    };
    let field = |key: &str| {
        item.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    println!(
        "{} {id}  {}  {}: {}",
        glyph(&field("status")),
        field("status"),
        field("project"),
        field("text")
    );
    println!("Kind: {}", category_label(item));
    if !field("link").is_empty() {
        println!(
            "Filed by a {} session ({}) from its own project ({}): a task request, not authorization from Theo.",
            field("reporter"),
            field("reporter_session"),
            field("link")
        );
    }
    if !field("queued_by").is_empty() {
        println!("Queued by {}", field("queued_by"));
    }
    let waits_on: Vec<&str> = item
        .get("after")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !waits_on.is_empty() {
        println!(
            "Waits on: {} (joins the queue once they are done)",
            waits_on.join(", ")
        );
    }
    for (label, key) in [("Why", "why"), ("Notes", "notes")] {
        let text = field(key);
        if !text.is_empty() {
            println!("\n{label}:\n{text}");
        }
    }
    for input in item
        .get("inputs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let input_field = |key: &str| {
            input
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        println!(
            "\nDone before it: {} in {} ({}):\n{}",
            input_field("id"),
            input_field("project"),
            input_field("text"),
            if input_field("result").is_empty() {
                "(no summary)".to_string()
            } else {
                input_field("result")
            }
        );
    }
    if !field("result").is_empty() {
        println!("\nResult:\n{}", field("result"));
    }
    let images = item
        .as_object()
        .map(|fields| Item(fields.clone()).images())
        .unwrap_or_default();
    if !images.is_empty() {
        let list_folder = todo_path().parent().map(PathBuf::from).unwrap_or_default();
        println!("\nImages (open each with your file reader):");
        for (number, file) in images {
            println!(
                "{} {}",
                todo_images::marker(number),
                todo_images::path_in(&list_folder, &file).display()
            );
        }
    }
    0
}

fn category_label(value: &Value) -> String {
    let Some(fields) = value.as_object() else {
        return "Unknown".into();
    };
    let item = Item(fields.clone());
    item.category()
        .map(|category| category.label().to_string())
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
        let Some((&value, rest)) = rest.split_first() else {
            eprintln!("{USAGE}");
            return 2;
        };
        match flag {
            "--why" if action != "add" && why.is_none() => why = Some(value.to_string()),
            "--notes" if notes.is_none() => notes = Some(value.to_string()),
            "--project" if matches!(action, "propose" | "add") && target.is_none() => {
                target = Some(value.to_string())
            }
            "--after" if action == "add" => {
                if value.len() != 8 || !value.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
                    eprintln!("cockpit-todo: --after takes an item id, not {value:?}");
                    return 2;
                }
                after.push(value.to_string());
            }
            "--kind" if action != "blocked" && category.is_none() => {
                let Some(parsed) = Category::parse(value) else {
                    eprintln!("cockpit-todo: invalid kind {value:?}\n{USAGE}");
                    return 2;
                };
                category = Some(parsed);
            }
            _ => {
                eprintln!("{USAGE}");
                return 2;
            }
        }
        remaining = rest;
    }
    if action != "add" && !why.as_ref().is_some_and(|reason| !reason.trim().is_empty()) {
        eprintln!("{USAGE}");
        return 2;
    }
    file(
        action, repo, title, why, notes, is_design, category, target, after,
    )
}

#[allow(clippy::too_many_arguments)]
fn file(
    action: &str,
    repo: Option<&str>,
    title: &str,
    why: Option<String>,
    notes: Option<String>,
    is_design: bool,
    category: Option<Category>,
    target: Option<String>,
    after: Vec<String>,
) -> i32 {
    if action == "propose"
        && read_list()
            .get("declined")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|declined| collapse(declined).to_lowercase() == collapse(title).to_lowercase())
    {
        println!("Theo already declined this idea; it is not filed again.");
        return 0;
    }
    if approval::own_answers_key().is_none() {
        eprintln!("cockpit-todo: no answers key; restart this session before filing items");
        return 1;
    }
    let mut update = serde_json::Map::new();
    update.insert("action".into(), json!(action));
    update.insert(
        "text".into(),
        json!(collapse(title).chars().take(300).collect::<String>()),
    );
    update.insert(
        "cwd".into(),
        json!(
            std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
    );
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
            update.insert(
                key.into(),
                json!(value.chars().take(4000).collect::<String>()),
            );
        }
    }
    let request_id = cockpit::util::token_hex(6);
    update.insert("request_id".into(), json!(request_id));
    println!(
        "Submitted request {request_id}; pending confirmation, not yet saved. Check: cockpit-todo receipt {request_id}"
    );
    if let Err(error) = append(update) {
        eprintln!("cockpit-todo: {error}");
        return 1;
    }
    wait_for_receipt(&request_id)
}

fn list(everything: bool) -> i32 {
    let data = read_list();
    let here = project();
    let mut shown = 0;
    for item in data
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let item_project = item.get("project").and_then(Value::as_str).unwrap_or("");
        if !everything && item_project != here {
            continue;
        }
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                if item.get("sent").and_then(Value::as_bool).unwrap_or(false) {
                    "doing".into()
                } else {
                    "todo".into()
                }
            });
        let has_notes = item
            .get("notes")
            .and_then(Value::as_str)
            .is_some_and(|notes| !notes.is_empty());
        let waits_on: Vec<&str> = item
            .get("after")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let waiting = if waits_on.is_empty() || status == "done" {
            String::new()
        } else {
            format!("  (waits on {})", waits_on.join(", "))
        };
        println!(
            "{} {}  {status:<8}  [{}] {item_project}: {}{waiting}{}",
            glyph(&status),
            item.get("id").and_then(Value::as_str).unwrap_or(""),
            category_label(item),
            item.get("text").and_then(Value::as_str).unwrap_or(""),
            if has_notes {
                "  (notes: cockpit-todo show)"
            } else {
                ""
            }
        );
        shown += 1;
    }
    for (key, value) in data
        .get("issues")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let status = value.get("status").and_then(Value::as_str).unwrap_or("");
        println!("{} {key}  {status}", glyph(status));
        shown += 1;
    }
    if shown == 0 {
        println!(
            "{}",
            if everything {
                "The list is empty.".to_string()
            } else {
                format!("Nothing on the list for {here}.")
            }
        );
    }
    0
}

fn wait_for_receipt(id: &str) -> i32 {
    let Some(key) = approval::own_answers_key() else {
        return receipt(id);
    };
    let path = approval::answers_dir(&key, "todos").join(format!("{id}.json"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if cockpit::util::read_json(&path).is_some_and(|result| {
            matches!(
                result["outcome"].as_str(),
                Some("accepted" | "skipped" | "rejected")
            )
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    receipt(id)
}

fn receipt(id: &str) -> i32 {
    let Some(key) = approval::own_answers_key() else {
        eprintln!("cockpit-todo: no answers key; restart this session before checking receipts");
        return 1;
    };
    let path = approval::answers_dir(&key, "todos").join(format!("{id}.json"));
    let Some(result) = cockpit::util::read_json(&path) else {
        println!(
            "Request {id}: pending or unknown; no saved result. Do not file a duplicate. Check: cockpit-todo receipt {id}"
        );
        return 124;
    };
    println!("{result}");
    match result["outcome"].as_str() {
        Some("accepted" | "skipped") => 0,
        Some("rejected") => {
            if result["can_approve"] == true {
                println!(
                    "Request host approval through ask-to-run: cockpit-todo approve-request {id}"
                );
            }
            1
        }
        _ => 124,
    }
}

fn main() {
    // Rust ignores SIGPIPE, so println! panicked when a reader stopped early (claude-todo list | head):
    // the default ends the process quietly, as for any command line tool.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let key = Regex::new(r"^(?:[0-9a-f]{8}|[\w.-]+/[\w.-]+#\d+)$").unwrap();
    let item_key = Regex::new(r"^[0-9a-f]{8}$").unwrap();
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let status = match words.as_slice() {
        ["receipt", id] if approval::valid_id(id) => receipt(id),
        ["approve-request", id] if approval::valid_id(id) => {
            match cockpit::todo_requests::approve(id) {
                Ok(()) => {
                    println!(
                        "Approval for exact request {id} submitted; check its saved receipt before claiming success."
                    );
                    0
                }
                Err(error) => {
                    eprintln!("cockpit-todo: {error}");
                    1
                }
            }
        }
        ["unwait", id, others @ ..]
            if item_key.is_match(id)
                && !others.is_empty()
                && others.len() <= 5
                && others.iter().all(|other| item_key.is_match(other)) =>
        {
            match cockpit::todo_requests::unwait(id, others) {
                Ok(()) => {
                    println!(
                        "{id} stops waiting on {} once the column applies it; it is not queued.",
                        others.join(", ")
                    );
                    0
                }
                Err(error) => {
                    eprintln!("cockpit-todo: {error}");
                    1
                }
            }
        }
        ["--help" | "-h" | "help", ..]
        | [
            "add" | "propose" | "blocked" | "stack" | "greywall" | "after" | "done",
            "--help" | "-h",
            ..,
        ]
        | ["stack", _, "--help" | "-h", ..] => {
            println!("{USAGE}");
            0
        }
        [] | ["list", ..] => list(words.contains(&"--all")),
        ["done", id, "--summary", summary]
            if item_key.is_match(id) && !summary.trim().is_empty() =>
        {
            let mut update = serde_json::Map::new();
            update.insert("action".into(), json!("done"));
            update.insert("id".into(), json!(id));
            update.insert(
                "summary".into(),
                json!(summary.chars().take(1000).collect::<String>()),
            );
            match append(update) {
                Ok(()) => {
                    println!("{id} marked done with its summary (the column applies it).");
                    0
                }
                Err(error) => {
                    eprintln!("cockpit-todo: {error}");
                    1
                }
            }
        }
        ["after", id, others @ ..]
            if item_key.is_match(id)
                && !others.is_empty()
                && others.len() <= 5
                && others.iter().all(|other| item_key.is_match(other)) =>
        {
            let mut update = serde_json::Map::new();
            update.insert("action".into(), json!("after"));
            update.insert("id".into(), json!(id));
            update.insert("after".into(), json!(others));
            match append(update) {
                Ok(()) => {
                    println!(
                        "{id} waits on {} (the column applies it); it joins the queue once they are done.",
                        others.join(", ")
                    );
                    0
                }
                Err(error) => {
                    eprintln!("cockpit-todo: {error}");
                    1
                }
            }
        }
        [action @ ("start" | "done" | "todo"), id] if key.is_match(id) => {
            let status = if *action == "start" { "doing" } else { action };
            let mut update = serde_json::Map::new();
            update.insert("action".into(), json!(status));
            update.insert("id".into(), json!(id));
            match append(update) {
                Ok(()) => {
                    println!(
                        "{id} marked {} (the column applies it).",
                        if *action == "start" {
                            "in progress"
                        } else {
                            action
                        }
                    );
                    0
                }
                Err(error) => {
                    eprintln!("cockpit-todo: {error}");
                    1
                }
            }
        }
        ["delete", id] if item_key.is_match(id) => {
            let mut update = serde_json::Map::new();
            update.insert("action".into(), json!("delete"));
            update.insert("id".into(), json!(id));
            match append(update) {
                Ok(()) => {
                    println!(
                        "{id} deleted (the column applies it; only items a session filed in this project)."
                    );
                    0
                }
                Err(error) => {
                    eprintln!("cockpit-todo: {error}");
                    1
                }
            }
        }
        ["show", id] if key.is_match(id) => show(id),
        [action @ ("add" | "propose" | "blocked"), title, rest @ ..]
            if !title.trim().is_empty() =>
        {
            file_arguments(action, None, title, rest)
        }
        ["stack", repo, title, rest @ ..]
            if STACK_REPOS.contains(repo) && !title.trim().is_empty() =>
        {
            file_arguments("stack", Some(repo), title, rest)
        }
        ["stack", repo, ..] if !STACK_REPOS.contains(repo) => {
            eprintln!(
                "cockpit-todo: {repo:?} is not a stack repository (one of: {}); work for another project goes through propose --project\n{USAGE}",
                STACK_REPOS.join(", ")
            );
            2
        }
        ["greywall", title, rest @ ..] if !title.trim().is_empty() => {
            file_arguments("stack", Some("greywall"), title, rest)
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(status);
}
