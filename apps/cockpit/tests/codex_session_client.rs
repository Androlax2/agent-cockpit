mod common;
use common::sandbox::*;
use serde_json::{Value, json};
use std::fs;

const THREAD: &str = "aaaaaaaa-0000-4000-8000-000000000000";

fn sandbox(items: Value) -> Sandbox {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.home.join(".codex")).unwrap();
    fs::write(sandbox.home.join("thread-items.json"), items.to_string()).unwrap();
    write_executable(
        &sandbox.home.join(".local/npm/bin/codex"),
        r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
home = Path.home()
assert Path(os.environ['CODEX_HOME']) == home / '.codex'
assert sys.argv[1:] == ['app-server']
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    with open(os.environ['SHIM_LOG'], 'a') as log:
        log.write(method + '\n')
    if method == 'initialized': continue
    if method == 'initialize': result = {}
    elif method == 'thread/read':
        include_turns = request['params']['includeTurns']
        assert request['params']['threadId'] == 'aaaaaaaa-0000-4000-8000-000000000000'
        legacy = os.environ.get('SHIM_LEGACY') == '1'
        items = json.loads((home/'thread-items.json').read_text())
        result = {'thread':{'id': os.environ.get('SHIM_WRONG_THREAD','aaaaaaaa-0000-4000-8000-000000000000'),
            'model':'gpt-6.1-sol', 'modelProvider':'openai', 'reasoningEffort':'high',
            'historyMode':'legacy' if legacy else 'paginated',
            'turns':[{'id':'turn','status':os.environ.get('SHIM_TURN_STATUS','completed'),
                'error':{'codexErrorInfo':os.environ.get('SHIM_TURN_ERROR')}, 'items':[entry['item'] for entry in reversed(items)]}] if include_turns and legacy else []}}
    elif method == 'thread/items/list':
        if os.environ.get('SHIM_LEGACY') == '1':
            print(json.dumps({'id':request['id'],'error':{'code':-32600,'message':'thread/items/list is not supported yet'}}),flush=True)
            continue
        assert request['params']['sortDirection'] == 'desc'
        items = json.loads((home/'thread-items.json').read_text())
        start = int(request['params'].get('cursor', '0'))
        limit = request['params']['limit']
        result = {'data': items[start:start+limit], 'nextCursor':str(start+limit) if start+limit < len(items) else None}
    else: raise AssertionError('Forbidden mutating method: '+method)
    print(json.dumps({'id':request['id'],'result':result}), flush=True)
"#,
    );
    sandbox
}

fn read(sandbox: &Sandbox, thread: &str) -> std::process::Output {
    sandbox
        .command("python3", &[])
        .arg(waybar_scripts().join("codex-session.py"))
        .arg(thread)
        .output()
        .unwrap()
}

#[test]
fn session_export_returns_only_visible_text_and_the_original_model() {
    let sandbox = sandbox(json!([
        {"turnId":"turn", "item":{"type":"agentMessage", "id":"reply", "text":"Current result"}},
        {"turnId":"turn", "item":{"type":"reasoning", "id":"reason", "text":"private reasoning"}},
        {"turnId":"turn", "item":{"type":"commandExecution", "id":"tool", "aggregatedOutput":"private output"}},
        {"turnId":"turn", "item":{"type":"userMessage", "id":"request", "content":[{"type":"text", "text":"Finish this task"}]}}
    ]));
    let output = read(&sandbox, THREAD);
    assert!(output.status.success(), "{}", stderr(&output));
    let context: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(context["model"], "gpt-6.1-sol");
    assert_eq!(context["effort"], "high");
    assert_eq!(
        context["messages"],
        json!([
            {"id":"request", "role":"user", "text":"Finish this task"},
            {"id":"reply", "role":"assistant", "text":"Current result"}
        ])
    );
    assert_eq!(context["latest_user"], "Finish this task");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private"));
}

#[test]
fn legacy_history_uses_supported_read_only_turns_without_enabling_migration() {
    let sandbox = sandbox(json!([
        {"turnId":"turn","item":{"type":"agentMessage","id":"reply","text":"Current result"}},
        {"turnId":"turn","item":{"type":"reasoning","id":"private","text":"private reasoning"}},
        {"turnId":"turn","item":{"type":"userMessage","id":"request","content":[{"type":"text","text":"Finish only this task"}]}}
    ]));
    let output = sandbox
        .command("python3", &[("SHIM_LEGACY", "1")])
        .arg(waybar_scripts().join("codex-session.py"))
        .arg(THREAD)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let context: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(context["latest_user"], "Finish only this task");
    assert_eq!(
        context["messages"],
        json!([
            {"id":"request","role":"user","text":"Finish only this task"},
            {"id":"reply","role":"assistant","text":"Current result"}
        ])
    );
    assert!(
        !fs::read_to_string(&sandbox.log)
            .unwrap()
            .contains("thread/items/list")
    );
}

#[test]
fn only_a_verified_failed_quota_turn_marks_an_unassigned_codex_task_pending() {
    for (status, error, expected) in [
        ("failed", "usageLimitExceeded", true),
        ("failed", "rateLimitExceeded", true),
        ("completed", "usageLimitExceeded", false),
        ("interrupted", "usageLimitExceeded", false),
        ("failed", "serverError", false),
    ] {
        let sandbox = sandbox(json!([]));
        let output = sandbox
            .command(
                "python3",
                &[
                    ("SHIM_LEGACY", "1"),
                    ("SHIM_TURN_STATUS", status),
                    ("SHIM_TURN_ERROR", error),
                ],
            )
            .arg(waybar_scripts().join("codex-session.py"))
            .arg(THREAD)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["quota_stopped"],
            expected
        );
    }
}

#[test]
fn session_export_refuses_a_native_thread_mismatch() {
    let sandbox = sandbox(json!([]));
    let output = sandbox
        .command("python3", &[])
        .arg(waybar_scripts().join("codex-session.py"))
        .arg(THREAD)
        .env("SHIM_WRONG_THREAD", "bbbbbbbb-0000-4000-8000-000000000000")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("thread identity"));
}

#[test]
fn invalid_thread_ids_never_start_the_vendor_cli() {
    for invalid in [
        "../thread",
        "--last",
        "",
        "AAAAAAAA-0000-4000-8000-000000000000",
    ] {
        let sandbox = sandbox(json!([]));
        let output = read(&sandbox, invalid);
        assert!(!output.status.success());
        assert!(stderr(&output).contains("invalid native thread"));
        assert!(fs::read_to_string(&sandbox.log).unwrap().is_empty());
    }
}

#[test]
fn bounded_history_retains_a_user_request_older_than_a_hundred_visible_replies() {
    let mut items: Vec<Value> = (0..110)
        .map(|index| {
            json!({"turnId":"turn", "item":{
        "type":"agentMessage", "id":format!("reply-{index}"), "text":format!("Progress {index}")}})
        })
        .collect();
    items.push(json!({"turnId":"turn", "item":{"type":"userMessage", "id":"request", "content":[{"type":"text", "text":"Original restriction"}]}}));
    let sandbox = sandbox(json!(items));
    let output = read(&sandbox, THREAD);
    assert!(output.status.success(), "{}", stderr(&output));
    let context: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(context["messages"].as_array().unwrap().len(), 100);
    assert_eq!(context["latest_user"], "Original restriction");
    assert_eq!(context["truncated"], true);
}

#[test]
fn oversized_visible_text_is_bounded_at_a_unicode_boundary() {
    let sandbox = sandbox(json!([
        {"turnId":"turn", "item":{"type":"agentMessage", "id":"reply", "text":"é".repeat(140_000)}},
        {"turnId":"turn", "item":{"type":"userMessage", "id":"request", "content":[{"type":"text", "text":"Keep working"}]}}
    ]));
    let output = read(&sandbox, THREAD);
    assert!(output.status.success(), "{}", stderr(&output));
    let context: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        context["messages"][0]["text"].as_str().unwrap().len(),
        256 * 1024
    );
    assert_eq!(context["latest_user"], "Keep working");
    assert_eq!(context["truncated"], true);
}
