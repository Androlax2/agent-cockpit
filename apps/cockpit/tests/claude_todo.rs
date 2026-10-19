//! claude-todo, the command sessions use inside the sandbox to update Theo's todo list: reads the
//! list, hands its changes to the OSD daemon on the report socket.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

fn claude_todo(sandbox: &Sandbox, arguments: &[&str], cwd: &Path) -> Output {
    let todo = sandbox.state.join("todo.json").display().to_string();
    sandbox.command(bin().join("claude-todo"), &[("CLAUDE_TODO_PATH", &todo), ("CLAUDE_CODE_SESSION_ID", SESSION_A)])
        .args(arguments).current_dir(cwd).output().unwrap()
}

/// The daemon's report socket, answering every update `reply`.
fn daemon(sandbox: &Sandbox, reply: &str) -> ReportSocket {
    ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), reply)
}

/// The updates the daemon received.
fn updates(daemon: &ReportSocket) -> Vec<Value> {
    daemon.received(Duration::from_millis(500)).iter()
        .map(|line| serde_json::from_str(line.strip_prefix("todo-update ").unwrap_or_else(|| panic!("not a todo update: {line}"))).unwrap()).collect()
}

#[test]
fn start_and_done_append_an_update_with_the_session() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    assert_eq!(claude_todo(&sandbox, &["start", "0123abcd"], &sandbox.root).status.code(), Some(0));
    assert_eq!(claude_todo(&sandbox, &["done", "o/runway#12"], &sandbox.root).status.code(), Some(0));
    let recorded: Vec<(Value, Value, Value)> = updates(&daemon).into_iter()
        .map(|update| (update["action"].clone(), update["id"].clone(), update["session"].clone())).collect();
    assert_eq!(recorded, [(json!("doing"), json!("0123abcd"), json!(SESSION_A)), (json!("done"), json!("o/runway#12"), json!(SESSION_A))]);
}

#[test]
fn a_malformed_id_is_refused() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    assert_eq!(claude_todo(&sandbox, &["done", "rm -rf"], &sandbox.root).status.code(), Some(2));
    assert!(updates(&daemon).is_empty());
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

#[test]
fn propose_files_an_idea_with_why_and_notes() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    let output = claude_todo(&sandbox, &["propose", "Cache the usage fetch", "--why", "it runs every minute", "--notes", "use the ETag"], &sandbox.root);
    assert_eq!(output.status.code(), Some(0));
    let update = &updates(&daemon)[0];
    assert_eq!((update["action"].as_str(), update["why"].as_str(), update["notes"].as_str()),
               (Some("propose"), Some("it runs every minute"), Some("use the ETag")));
}

#[test]
fn propose_without_a_reason_is_refused() {
    let sandbox = Sandbox::new();
    assert_eq!(claude_todo(&sandbox, &["propose", "Cache it"], &sandbox.root).status.code(), Some(2));
}

#[test]
fn an_idea_theo_declined_is_not_proposed_again() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    fs::write(sandbox.state.join("todo.json"), json!({"items": [], "declined": ["Rewrite in Go"]}).to_string()).unwrap();
    let output = claude_todo(&sandbox, &["propose", "rewrite in go", "--why", "faster"], &sandbox.root);
    assert!(stdout(&output).contains("already declined"), "{}", stdout(&output));
    assert!(updates(&daemon).is_empty());
}

#[test]
fn design_marks_an_item_for_the_designer_session() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    assert_eq!(claude_todo(&sandbox, &["add", "Empty state of the order list", "--design", "--notes", "no frame for 0 orders"], &sandbox.root).status.code(), Some(0));
    assert_eq!(claude_todo(&sandbox, &["propose", "Error state", "--why", "the API fails often", "--design"], &sandbox.root).status.code(), Some(0));
    let filed = updates(&daemon);
    assert_eq!(filed.iter().map(|update| update["kind"].as_str()).collect::<Vec<_>>(), [Some("design"), Some("design")]);
    assert_eq!(filed[0]["notes"].as_str(), Some("no frame for 0 orders"));
}

#[test]
fn an_item_without_design_has_no_kind() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    claude_todo(&sandbox, &["add", "Bump the deps"], &sandbox.root);
    assert_eq!(updates(&daemon)[0].get("kind"), None);
}

#[test]
fn blocked_files_a_report_with_what_stopped_the_session() {
    let sandbox = Sandbox::new();
    let daemon = daemon(&sandbox, "ok");
    let output = claude_todo(&sandbox, &["blocked", "Push the release tag", "--why", "greywall hides ~/.ssh"], &sandbox.root);
    assert_eq!(output.status.code(), Some(0));
    let update = &updates(&daemon)[0];
    assert_eq!((update["action"].as_str(), update["why"].as_str()), (Some("blocked"), Some("greywall hides ~/.ssh")));
}

#[test]
fn blocked_without_what_stopped_it_is_refused() {
    let sandbox = Sandbox::new();
    assert_eq!(claude_todo(&sandbox, &["blocked", "Push the release tag"], &sandbox.root).status.code(), Some(2));
}

#[test]
fn an_update_the_daemon_refuses_says_why() {
    let sandbox = Sandbox::new();
    let _daemon = daemon(&sandbox, "refused: the todo queue is full");
    let output = claude_todo(&sandbox, &["done", "0123abcd"], &sandbox.root);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("the todo queue is full"), "{}", stderr(&output));
}

#[test]
fn without_a_daemon_an_update_fails_loudly() {
    let sandbox = Sandbox::new();
    let output = claude_todo(&sandbox, &["done", "0123abcd"], &sandbox.root);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("cannot reach the OSD daemon"), "{}", stderr(&output));
}
