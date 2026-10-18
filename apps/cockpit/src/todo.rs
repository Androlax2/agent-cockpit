//! Theo's list for Claude: items he types, per project, and a status for each item or GitHub issue
//! (todo, doing, done), in ~/.local/state/claude-sessions/todo.json. Sessions cannot write there
//! (greywall): claude-todo appends their changes to updates.jsonl, which apply_updates checks.

use crate::paths;
use crate::reviews::Issue;
use crate::util::{self, now_secs, project_name, repository_root};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::cmp::Ordering;
use std::fs;

pub const PRIORITY_LOW: i64 = 1;
pub const PRIORITY_NORMAL: i64 = 2;
pub const PRIORITY_HIGH: i64 = 3;
pub const PRIORITY_MAX: i64 = PRIORITY_NORMAL + 4; // + up to four times: ! !! !!! !!!!
const STATUSES: [&str; 3] = ["todo", "doing", "done"];
const DONE_KEPT_DAYS: i64 = 7;

fn typed_id() -> Regex { Regex::new(r"^[0-9a-f]{8}$").unwrap() }
fn issue_key() -> Regex { Regex::new(r"^[\w.-]+/[\w.-]+#\d+$").unwrap() }

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

/// The message for Claude, with the item's key: Claude marks it done with claude-todo.
pub fn with_todo_note(text: &str, key: &str) -> String {
    format!("{text} (todo {key}: when this is done, run claude-todo done {key})")
}

pub fn issue_prompt(issue: &Issue) -> String {
    format!("Work on GitHub issue #{} of this repository: {}. Read it with gh issue view {} first, then say how you will approach it before changing anything.",
            issue.number, issue.title, issue.number)
}

pub struct TodoList {
    pub items: Vec<Item>,
    pub issues: Map<String, Value>, // "owner/repo#12" -> {"status", "updated", "session"}
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
        Self { items, issues }
    }

    pub fn save(&mut self) {
        let oldest = now_secs() - DONE_KEPT_DAYS * 86400;
        self.items.retain(|item| item.status() != "done" || item.updated() >= oldest);
        self.issues.retain(|_, value| value.get("status").and_then(Value::as_str) != Some("done")
            || value.get("updated").and_then(Value::as_i64).unwrap_or(0) >= oldest);
        let items: Vec<Value> = self.items.iter().map(|item| Value::Object(item.0.clone())).collect();
        if let Err(error) = util::write_json(&paths::todo(), &json!({"items": items, "issues": self.issues})) {
            util::log_event("WARN", &format!("cannot save the todo list: {error}"));
        }
    }

    pub fn add(&mut self, project: &str, cwd: &str, text: &str) -> String {
        let id = util::token_hex(4);
        let text: String = util::collapse(text).chars().take(300).collect();
        let item = json!({"id": id, "project": project, "cwd": cwd, "priority": PRIORITY_NORMAL, "text": text, "created": now_secs(), "status": "todo"});
        self.items.push(Item(item.as_object().unwrap().clone()));
        self.save();
        id
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
            let Some(item) = self.items.iter_mut().find(|item| item.id() == key) else { return false };
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
            if action == "add" {
                let text = update.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let cwd = update.get("cwd").and_then(Value::as_str).unwrap_or("").to_string();
                if !text.is_empty() && std::path::Path::new(&cwd).is_dir() {
                    let id = self.add(&project_name(&repository_root(&cwd)), &cwd, &text);
                    util::log_event("INFO", &format!("todo {id} added by a session: {}", text.chars().take(60).collect::<String>()));
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
