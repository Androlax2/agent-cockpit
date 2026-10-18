//! claude-todo, the command sessions use inside the sandbox to update Theo's todo list: reads the
//! list, appends updates.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Output;

fn claude_todo(sandbox: &Sandbox, arguments: &[&str], cwd: &Path) -> Output {
    let todo = sandbox.state.join("todo.json").display().to_string();
    let updates = sandbox.root.join("updates.jsonl").display().to_string();
    sandbox.command(bin().join("claude-todo"), &[("CLAUDE_TODO_PATH", &todo), ("CLAUDE_TODO_UPDATES", &updates), ("CLAUDE_CODE_SESSION_ID", SESSION_A)])
        .args(arguments).current_dir(cwd).output().unwrap()
}

fn updates(sandbox: &Sandbox) -> Vec<Value> {
    fs::read_to_string(sandbox.root.join("updates.jsonl")).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

#[test]
fn start_and_done_append_an_update_with_the_session() {
    let sandbox = Sandbox::new();
    assert_eq!(claude_todo(&sandbox, &["start", "0123abcd"], &sandbox.root).status.code(), Some(0));
    assert_eq!(claude_todo(&sandbox, &["done", "o/runway#12"], &sandbox.root).status.code(), Some(0));
    let recorded: Vec<(Value, Value, Value)> = updates(&sandbox).into_iter()
        .map(|update| (update["action"].clone(), update["id"].clone(), update["session"].clone())).collect();
    assert_eq!(recorded, [(json!("doing"), json!("0123abcd"), json!(SESSION_A)), (json!("done"), json!("o/runway#12"), json!(SESSION_A))]);
}

#[test]
fn a_malformed_id_is_refused() {
    let sandbox = Sandbox::new();
    assert_eq!(claude_todo(&sandbox, &["done", "rm -rf"], &sandbox.root).status.code(), Some(2));
    assert!(!sandbox.root.join("updates.jsonl").exists());
}

#[test]
fn list_shows_the_items_of_this_directorys_repository() {
    let sandbox = Sandbox::new();
    let checkout = sandbox.root.join("runway");
    fs::create_dir_all(checkout.join(".git")).unwrap();
    let todo = json!({"items": [
        {"id": "0123abcd", "project": "runway", "text": "deploy", "status": "doing"},
        {"id": "4567cdef", "project": "homelab", "text": "postgres", "status": "todo"}], "issues": {}});
    fs::write(sandbox.state.join("todo.json"), todo.to_string()).unwrap();
    let listed = stdout(&claude_todo(&sandbox, &["list"], &checkout));
    assert!(listed.contains("0123abcd"), "{listed}");
    assert!(!listed.contains("4567cdef"), "{listed}");
    let listed_all = stdout(&claude_todo(&sandbox, &["list", "--all"], &checkout));
    assert!(listed_all.contains("4567cdef"), "{listed_all}");
}
