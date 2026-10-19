//! Theo's list for Claude: items he types, per project, and a status for each item or GitHub issue
//! (todo, doing, done), in ~/.local/state/claude-sessions/todo.json. Sessions cannot write there
//! (greywall): claude-todo appends their changes to updates.jsonl, which apply_updates checks.
//!
//! An item has a title (its "text") and notes. A session can also propose one (claude-todo
//! propose, with why): it waits as "proposed" until Theo approves it (to do) or declines it; a
//! declined title is remembered, so the same idea is not proposed again.
//!
//! The queue: items Theo marked (Q) for Claude to take up on its own. When a session of the item's
//! project has waited idle at its prompt for a while, the column sends it the project's first
//! queued item (highest priority, then oldest), one at a time per session (App::dispatch_queue).

use crate::paths;
use crate::reviews::Issue;
use crate::util::{self, now_secs, project_name, repository_root};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fs;

pub const PRIORITY_LOW: i64 = 1;
pub const PRIORITY_NORMAL: i64 = 2;
pub const PRIORITY_HIGH: i64 = 3;
pub const PRIORITY_MAX: i64 = PRIORITY_NORMAL + 4; // + up to four times: ! !! !!! !!!!
const STATUSES: [&str; 3] = ["todo", "doing", "done"];
pub const PROPOSED: &str = "proposed";
const NOTES_MAX: usize = 4000;
const WHY_MAX: usize = 600;
const DECLINED_KEPT: usize = 200;
const DONE_KEPT_DAYS: i64 = 7;

fn typed_id() -> Regex { Regex::new(r"^[0-9a-f]{8}$").unwrap() }
fn issue_key() -> Regex { Regex::new(r"^[\w.-]+/[\w.-]+#\d+$").unwrap() }

/// Notes and reasons keep their line breaks; every other control character goes.
pub fn multiline(text: &str, limit: usize) -> String {
    let kept: String = text.chars().map(|c| if c == '\n' || !c.is_control() { c } else { ' ' }).collect();
    kept.trim().chars().take(limit).collect()
}

/// How two titles compare for "already declined": case and spacing aside.
fn normalized(title: &str) -> String {
    util::collapse(title).to_lowercase()
}

/// From an issue's labels: P0/P1/urgent/critical/high are high, P3/P4/low are low.
pub fn issue_priority(labels: &[String]) -> i64 {
    let high = Regex::new(r"(?i)\b(p0|p1|urgent|critical|high|priority[ :/_-]*(high|urgent|critical))\b").unwrap();
    let low = Regex::new(r"(?i)\b(p3|p4|low|priority[ :/_-]*low)\b").unwrap();
    if labels.iter().any(|label| high.is_match(label)) {
        PRIORITY_HIGH
    } else if labels.iter().any(|label| low.is_match(label)) {
        PRIORITY_LOW
    } else {
        PRIORITY_NORMAL
    }
}

/// "!" to "!!!!" above normal, "↓" below, nothing at normal.
pub fn priority_mark(priority: i64) -> String {
    match priority.cmp(&PRIORITY_NORMAL) {
        Ordering::Greater => "!".repeat((priority - PRIORITY_NORMAL) as usize),
        Ordering::Less => "↓".into(),
        Ordering::Equal => String::new(),
    }
}

/// A typed item, kept as its JSON object so fields it does not know survive a save.
#[derive(Clone, Debug, PartialEq)]
pub struct Item(pub Map<String, Value>);

impl Item {
    fn text_of(&self, key: &str) -> String {
        self.0.get(key).and_then(Value::as_str).unwrap_or("").to_string()
    }
    pub fn id(&self) -> String { self.text_of("id") }
    pub fn project(&self) -> String { self.text_of("project") }
    pub fn cwd(&self) -> String { self.text_of("cwd") }
    pub fn text(&self) -> String { self.text_of("text") }
    pub fn status(&self) -> String { let status = self.text_of("status"); if status.is_empty() { "todo".into() } else { status } }
    pub fn priority(&self) -> i64 { self.0.get("priority").and_then(Value::as_i64).unwrap_or(PRIORITY_NORMAL) }
    pub fn created(&self) -> i64 { self.0.get("created").and_then(Value::as_i64).unwrap_or(0) }
    /// Who filed it when not Theo: "session <id>" for claude-todo add.
    pub fn source(&self) -> String { self.text_of("source") }
    pub fn notes(&self) -> String { self.text_of("notes") }
    /// Why a session proposed it, in its own words.
    pub fn why(&self) -> String { self.text_of("why") }
    fn updated(&self) -> i64 { self.0.get("updated").and_then(Value::as_i64).unwrap_or(0) }
}

/// A row of the Todo panel: a typed item or an issue, with its status and priority.
#[derive(Clone, Debug)]
pub enum Entry {
    Typed(Item),
    Issue { issue: Issue, status: String, priority: i64 },
}

impl Entry {
    pub fn key(&self) -> String {
        match self {
            Entry::Typed(item) => item.id(),
            Entry::Issue { issue, .. } => issue.key(),
        }
    }
    pub fn status(&self) -> String {
        match self {
            Entry::Typed(item) => item.status(),
            Entry::Issue { status, .. } => status.clone(),
        }
    }
    pub fn priority(&self) -> i64 {
        match self {
            Entry::Typed(item) => item.priority(),
            Entry::Issue { priority, .. } => *priority,
        }
    }
}

/// Done last; otherwise high priority first, in progress before to do, typed items before issues,
/// then oldest typed and most recently updated issues first.
pub fn todo_order(a: &Entry, b: &Entry) -> Ordering {
    let head = |entry: &Entry| (entry.status() == "done", -entry.priority(), entry.status() != "doing");
    head(a).cmp(&head(b)).then_with(|| match (a, b) {
        (Entry::Typed(x), Entry::Typed(y)) => x.created().cmp(&y.created()),
        (Entry::Typed(_), Entry::Issue { .. }) => Ordering::Less,
        (Entry::Issue { .. }, Entry::Typed(_)) => Ordering::Greater,
        (Entry::Issue { issue: x, .. }, Entry::Issue { issue: y, .. }) => y.updated.cmp(&x.updated),
    })
}

/// The queue's order: highest priority first, then the oldest.
pub fn queue_order(a: &Entry, b: &Entry) -> Ordering {
    let created = |entry: &Entry| match entry { Entry::Typed(item) => item.created(), Entry::Issue { .. } => 0 };
    (-a.priority(), created(a)).cmp(&(-b.priority(), created(b)))
}

/// The message for Claude, with the item's key: Claude marks it done with claude-todo.
pub fn with_todo_note(text: &str, key: &str) -> String {
    // The text can become claude's first argument (claude-sessions new): one starting with "-"
    // would be read as an option, such as --append-system-prompt=….
    let text = if text.starts_with('-') { format!("Task: {text}") } else { text.to_string() };
    format!("{text} (todo {key}: when this is done, run claude-todo done {key})")
}

pub fn issue_prompt(issue: &Issue) -> String {
    format!("Work on GitHub issue #{} of this repository: {}. Read it with gh issue view {} first, then say how you will approach it before changing anything.",
            issue.number, without_controls(&issue.title), issue.number)
}

/// Text typed into a session goes through tmux send-keys: a control character there is a key
/// (Escape, Ctrl+C, Shift+Tab's sequence), and the column draws none of them. Gone before storing.
pub fn without_controls(text: &str) -> String {
    text.chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect()
}

pub struct TodoList {
    pub items: Vec<Item>,
    pub issues: Map<String, Value>, // "owner/repo#12" -> {"status", "updated", "session"}
    /// Keys of the items and issues queued for Claude. Apart from the items: an issue's entry is
    /// rewritten whole on each status change.
    pub queued: BTreeSet<String>,
    /// Titles Theo declined when a session proposed them, newest last.
    pub declined: Vec<String>,
}

impl TodoList {
    pub fn load() -> Self {
        let data = util::read_json(&paths::todo()).unwrap_or(json!({}));
        // The first format: items only, "sent" for handed to Claude.
        let data = if data.is_array() { json!({"items": data}) } else { data };
        let mut items = Vec::new();
        for item in data.get("items").and_then(Value::as_array).into_iter().flatten() {
            let Some(object) = item.as_object() else { continue };
            let mut object = object.clone();
            let valid = object.get("text").and_then(Value::as_str).is_some_and(|text| !text.is_empty())
                && object.get("id").and_then(Value::as_str).is_some_and(|id| typed_id().is_match(id));
            if !valid {
                continue;
            }
            if !object.contains_key("status") {
                let sent = object.remove("sent").and_then(|sent| sent.as_bool()).unwrap_or(false);
                object.insert("status".into(), json!(if sent { "doing" } else { "todo" }));
            }
            items.push(Item(object));
        }
        let issues = data.get("issues").and_then(Value::as_object).map(|issues| {
            issues.iter().filter(|(key, value)| issue_key().is_match(key) && value.is_object()).map(|(k, v)| (k.clone(), v.clone())).collect()
        }).unwrap_or_default();
        let queued = data.get("queued").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str)
            .filter(|key| typed_id().is_match(key) || issue_key().is_match(key)).map(str::to_string).collect();
        let declined = data.get("declined").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        Self { items, issues, queued, declined }
    }

    /// Queues the item or issue, or takes it out of the queue; true when it is now queued.
    pub fn toggle_queued(&mut self, key: &str) -> bool {
        let queued = if self.queued.remove(key) { false } else { self.queued.insert(key.to_string()) };
        self.save();
        queued
    }

    pub fn is_queued(&self, key: &str) -> bool {
        self.queued.contains(key)
    }

    /// Whether a session already works on an item it was given (queued or handed by Theo).
    pub fn busy(&self, session: &str) -> bool {
        self.items.iter().any(|item| item.status() == "doing" && item.text_of("session") == session)
            || self.issues.values().any(|value| value.get("status").and_then(Value::as_str) == Some("doing")
                                         && value.get("session").and_then(Value::as_str) == Some(session))
    }

    pub fn save(&mut self) {
        let oldest = now_secs() - DONE_KEPT_DAYS * 86400;
        self.items.retain(|item| item.status() != "done" || item.updated() >= oldest);
        self.issues.retain(|_, value| value.get("status").and_then(Value::as_str) != Some("done")
            || value.get("updated").and_then(Value::as_i64).unwrap_or(0) >= oldest);
        // A typed item that is gone or done leaves the queue; an issue leaves it once done.
        let (items, issues) = (&self.items, &self.issues);
        self.queued.retain(|key| if issue_key().is_match(key) {
            issues.get(key).and_then(|value| value.get("status")).and_then(Value::as_str) != Some("done")
        } else {
            items.iter().any(|item| &item.id() == key && item.status() != "done")
        });
        let items: Vec<Value> = self.items.iter().map(|item| Value::Object(item.0.clone())).collect();
        if let Err(error) = util::write_json(&paths::todo(), &json!({"items": items, "issues": self.issues, "queued": self.queued, "declined": self.declined})) {
            util::log_event("WARN", &format!("cannot save the todo list: {error}"));
        }
    }

    pub fn add(&mut self, project: &str, cwd: &str, text: &str) -> String {
        let id = self.add_unsaved(project, cwd, text);
        self.save();
        id
    }

    fn add_unsaved(&mut self, project: &str, cwd: &str, text: &str) -> String {
        let id = util::token_hex(4);
        let text: String = util::collapse(&without_controls(text)).chars().take(300).collect();
        let item = json!({"id": id, "project": project, "cwd": cwd, "priority": PRIORITY_NORMAL, "text": text, "created": now_secs(), "status": "todo"});
        self.items.push(Item(item.as_object().unwrap().clone()));
        id
    }

    /// Title and notes, as Theo wrote them in his editor.
    pub fn edit(&mut self, id: &str, title: &str, notes: &str) -> bool {
        let title: String = util::collapse(&without_controls(title)).chars().take(300).collect();
        if title.is_empty() {
            return false;
        }
        let Some(item) = self.items.iter_mut().find(|item| item.id() == id) else { return false };
        item.0.insert("text".into(), json!(title));
        item.0.insert("notes".into(), json!(multiline(notes, NOTES_MAX)));
        self.save();
        true
    }

    /// A proposal Theo wants: it joins the list to do.
    pub fn approve(&mut self, id: &str) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id() == id && item.status() == PROPOSED) else { return false };
        item.0.insert("status".into(), json!("todo"));
        item.0.insert("updated".into(), json!(now_secs()));
        self.save();
        true
    }

    /// A proposal Theo does not want: gone, and its title remembered so it is not proposed again.
    pub fn decline(&mut self, id: &str) -> bool {
        let Some(position) = self.items.iter().position(|item| item.id() == id && item.status() == PROPOSED) else { return false };
        let item = self.items.remove(position);
        self.declined.push(item.text());
        let excess = self.declined.len().saturating_sub(DECLINED_KEPT);
        self.declined.drain(..excess);
        self.save();
        true
    }

    pub fn was_declined(&self, title: &str) -> bool {
        let title = normalized(title);
        self.declined.iter().any(|declined| normalized(declined) == title)
    }

    pub fn remove(&mut self, id: &str) {
        self.items.retain(|item| item.id() != id);
        self.save();
    }

    pub fn change_priority(&mut self, id: &str, step: i64) {
        for item in self.items.iter_mut().filter(|item| item.id() == id) {
            let priority = (item.priority() + step).clamp(PRIORITY_LOW, PRIORITY_MAX);
            item.0.insert("priority".into(), json!(priority));
        }
        self.save();
    }

    pub fn status_of(&self, key: &str) -> String {
        if issue_key().is_match(key) {
            return self.issues.get(key).and_then(|value| value.get("status")).and_then(Value::as_str).unwrap_or("todo").to_string();
        }
        self.items.iter().find(|item| item.id() == key).map(Item::status).unwrap_or_else(|| "todo".into())
    }

    /// Status of a typed item (its id) or an issue ("owner/repo#12"); false when unknown.
    pub fn set_status(&mut self, key: &str, status: &str, session: &str) -> bool {
        if !STATUSES.contains(&status) {
            return false;
        }
        if issue_key().is_match(key) {
            self.issues.insert(key.into(), json!({"status": status, "updated": now_secs(), "session": session}));
        } else {
            // A proposal becomes work only through Theo's approval, never a session's status change.
            let Some(item) = self.items.iter_mut().find(|item| item.id() == key && item.status() != PROPOSED) else { return false };
            let session = if session.is_empty() { item.text_of("session") } else { session.to_string() };
            item.0.insert("status".into(), json!(status));
            item.0.insert("updated".into(), json!(now_secs()));
            item.0.insert("session".into(), json!(session));
        }
        self.save();
        true
    }

    /// The changes sessions asked for (claude-todo), each checked; returns how many applied.
    pub fn apply_updates(&mut self) -> usize {
        let path = paths::todo_updates();
        if !path.exists() {
            return 0;
        }
        let taken = path.with_extension(format!("jsonl.{}", std::process::id()));
        let lines = match fs::rename(&path, &taken).and_then(|_| fs::read_to_string(&taken)) {
            Ok(text) => text,
            Err(_) => return 0,
        };
        let _ = fs::remove_file(&taken);
        let mut applied = 0;
        for line in lines.lines() {
            let Ok(update) = serde_json::from_str::<Value>(line) else { continue };
            let Some(update) = update.as_object() else { continue };
            let action = update.get("action").and_then(Value::as_str).unwrap_or("");
            let session: String = update.get("session").and_then(Value::as_str).unwrap_or("").chars().take(40).collect();
            if action == "add" || action == "propose" {
                let text = update.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let cwd = update.get("cwd").and_then(Value::as_str).unwrap_or("").to_string();
                let field = |key: &str, limit: usize| multiline(update.get(key).and_then(Value::as_str).unwrap_or(""), limit);
                if action == "propose" && self.was_declined(&text) {
                    util::log_event("INFO", &format!("todo proposal already declined, dropped: {}", text.chars().take(60).collect::<String>()));
                    continue;
                }
                if !text.is_empty() && std::path::Path::new(&cwd).is_dir() {
                    let id = self.add_unsaved(&project_name(&repository_root(&cwd)), &cwd, &text);
                    // Any sandbox can file an item under any project: the panel says it came from a
                    // session, so Theo reads it before handing it on.
                    if let Some(item) = self.items.iter_mut().find(|item| item.id() == id) {
                        item.0.insert("source".into(), json!(if session.is_empty() { "a session".to_string() } else { format!("session {session}") }));
                        item.0.insert("notes".into(), json!(field("notes", NOTES_MAX)));
                        if action == "propose" {
                            item.0.insert("status".into(), json!(PROPOSED));
                            item.0.insert("why".into(), json!(field("why", WHY_MAX)));
                        }
                    }
                    self.save();
                    util::log_event("INFO", &format!("todo {id} {} by a session: {}", if action == "propose" { "proposed" } else { "added" },
                                                     text.chars().take(60).collect::<String>()));
                    applied += 1;
                }
            } else if STATUSES.contains(&action) {
                let id = update.get("id").and_then(Value::as_str).unwrap_or("");
                if self.set_status(id, action, &session) {
                    util::log_event("INFO", &format!("todo {id} marked {action} by a session"));
                    applied += 1;
                }
            }
        }
        applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_starting_with_a_dash_is_never_an_option() {
        assert!(with_todo_note("--append-system-prompt=x", "ab12cd34").starts_with("Task: --append"));
    }

    #[test]
    fn control_characters_never_reach_a_session() {
        let mut todos = TodoList { items: Vec::new(), issues: Map::new(), queued: BTreeSet::new(), declined: Vec::new() };
        let id = todos.add_unsaved("p", "/p", "fix typo\x1b[Z\x03\x03 now");
        let text = todos.items.iter().find(|item| item.id() == id).unwrap().text();
        assert!(!text.chars().any(char::is_control), "{text:?}");
    }

    #[test]
    fn notes_keep_their_lines_and_lose_other_controls() {
        assert_eq!(multiline("first\nsecond\x1b[2J", 100), "first\nsecond [2J");
    }

    #[test]
    fn priority_marks_go_up_to_four() {
        assert_eq!(priority_mark(PRIORITY_NORMAL + 1), "!");
        assert_eq!(priority_mark(PRIORITY_MAX), "!!!!");
        assert_eq!(priority_mark(PRIORITY_LOW), "↓");
        assert_eq!(priority_mark(PRIORITY_NORMAL), "");
    }

    #[test]
    fn labels_set_an_issues_priority() {
        assert_eq!(issue_priority(&["P1".into()]), PRIORITY_HIGH);
        assert_eq!(issue_priority(&["priority: low".into()]), PRIORITY_LOW);
        assert_eq!(issue_priority(&["bug".into()]), PRIORITY_NORMAL);
    }
}
