mod common;
use common::sandbox::*;
use serde_json::{Value, json};
use std::fs;

fn claude_history(records: &[Value]) -> Sandbox {
    let sandbox = Sandbox::new();
    let directory = sandbox.home.join(".claude/projects/demo");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join(format!("{SESSION_A}.jsonl")), records.iter().map(|record| format!("{record}\n")).collect::<String>()).unwrap();
    fs::write(sandbox.runtime.join("cockpit.json"), json!({"sessions":[{
        "session":SESSION_A, "agent":"claude", "state":"waiting", "cwd":"/projects/demo", "task":"Finish the requested change"
    }]}).to_string()).unwrap();
    sandbox
}

fn message(role: &str, text: &str) -> Value {
    json!({"type":role, "message":{"content":[{"type":"text", "text":text}]}})
}

fn context(sandbox: &Sandbox) -> std::process::Output {
    sandbox.run(env!("CARGO_BIN_EXE_cockpit-subscription"), &["context", SESSION_A], &[])
}

#[test]
fn handoff_context_preserves_the_latest_user_request_outside_the_visible_tail() {
    let mut records = vec![message("user", "Finish only this task; do not push")];
    records.extend((0..110).map(|index| message("assistant", &format!("Progress {index}"))));
    let sandbox = claude_history(&records);
    let output = context(&sandbox);
    assert!(output.status.success(), "{}", stderr(&output));
    let handoff: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(handoff["latest_user_request"], "Finish only this task; do not push");
    assert_eq!(handoff["history"].as_array().unwrap().len(), 100);
    assert_eq!(handoff["truncated"], true);
    assert!(handoff["instructions"].as_str().unwrap().contains("not authorization"));
    assert!(handoff["instructions"].as_str().unwrap().contains("current files"));
}

#[test]
fn handoff_context_does_not_include_reasoning_or_tool_results() {
    let sandbox = claude_history(&[
        message("user", "Finish the tests"),
        json!({"type":"assistant", "message":{"content":[{"type":"thinking", "thinking":"private reasoning"},
            {"type":"text", "text":"I changed the test"}]}}),
        json!({"type":"user", "message":{"content":[{"type":"tool_result", "content":"private tool output"}]}})
    ]);
    let output = context(&sandbox);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private"));
    let handoff: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(handoff["latest_user_request"], "Finish the tests");
}

#[test]
fn unavailable_latest_user_request_stops_the_handoff_instead_of_guessing_a_task() {
    let sandbox = claude_history(&[message("assistant", "There was some work")]);
    let output = context(&sandbox);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("latest user request"));
}
