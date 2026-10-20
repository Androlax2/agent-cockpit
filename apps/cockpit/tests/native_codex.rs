use cockpit::codex::{cockpit_id, translate_hook};
use serde_json::json;
mod common;
use common::sandbox::Sandbox;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::process::Stdio;

const NATIVE_THREAD: &str = "aaaaaaaa-0000-4000-8000-000000000000";
const COCKPIT_THREAD: &str = "cdx_c214e8c0c9458f09ea4300974071bcad955995188f33264e341800f8a2bb5fcb";

#[test]
fn codex_ids_do_not_alias_claude_uuids() {
    let id = cockpit_id(NATIVE_THREAD).unwrap();
    assert_ne!(id, NATIVE_THREAD);
    assert_eq!(id, COCKPIT_THREAD);
    assert!(id.starts_with("cdx_"));
    assert_eq!(id.len(), 68);
    assert_eq!(cockpit_id(NATIVE_THREAD).unwrap(), id);
    assert_ne!(cockpit_id("bbbbbbbb-0000-4000-8000-000000000000").unwrap(), id);
}

#[test]
fn native_thread_ids_reject_unsafe_or_oversized_values() {
    for invalid in ["".to_string(), "x".repeat(36), "../thread".into(), "thread\nother".into(), "thread name".into(),
                    "--dangerously-bypass-approvals-and-sandbox".into(), "-aaaaaaa-0000-4000-8000-000000000000".into(),
                    "AAAAAAAA-0000-4000-8000-000000000000".into()] {
        assert!(cockpit_id(&invalid).is_err(), "accepted {invalid:?}");
    }
    assert!(cockpit_id("01a02010-e5b0-75f1-b3b7-3e0c54879f59").is_ok());
}

#[test]
fn hook_translation_retains_native_thread_and_agent() {
    let report = translate_hook(&json!({"hook_event_name": "UserPromptSubmit", "session_id": NATIVE_THREAD,
        "cwd": "/projects/demo", "prompt": "private prompt"})).unwrap().unwrap();
    assert_eq!(report["agent"], "codex");
    assert_eq!(report["native_thread"], NATIVE_THREAD);
    assert_eq!(report["project"], "demo");
    assert_eq!(report["cwd"], "/projects/demo");
    assert_eq!(report["state"], "working");
    assert!(!report.to_string().contains("private prompt"));
}

#[test]
fn subagent_events_do_not_replace_root_card() {
    for event in ["SubagentStart", "SubagentStop"] {
        assert_eq!(translate_hook(&json!({"hook_event_name": event, "session_id": NATIVE_THREAD,
            "cwd": "/projects/demo", "agent_id": "child-thread"})).unwrap(), None);
    }
}

#[test]
fn unknown_native_events_do_not_report_idle() {
    assert!(translate_hook(&json!({"hook_event_name": "FutureEvent", "session_id": NATIVE_THREAD,
        "cwd": "/projects/demo"})).is_err());
}

#[test]
fn lifecycle_states_do_not_mistake_start_or_interrupt_for_idle() {
    for (event, state) in [("SessionStart", "working"), ("PermissionRequest", "needs-you"),
        ("Interrupt", "needs-you"), ("SessionEnd", "gone"), ("Stop", "waiting")] {
        let report = translate_hook(&json!({"hook_event_name": event, "session_id": NATIVE_THREAD,
            "cwd": "/projects/demo"})).unwrap().unwrap();
        assert_eq!(report["state"], state, "{event}");
    }
}

#[test]
fn a_previous_stop_continuation_does_not_hide_a_completed_turn() {
    let report = translate_hook(&json!({"hook_event_name": "Stop", "session_id": NATIVE_THREAD,
        "cwd": "/projects/demo", "stop_hook_active": true})).unwrap().unwrap();
    assert_eq!(report["state"], "waiting");
}

#[test]
fn native_hooks_require_an_absolute_safe_working_directory() {
    for cwd in ["", "relative/demo", "/projects/\u{1b}demo", "/projects/#{pane_id}"] {
        assert!(translate_hook(&json!({"hook_event_name": "SessionStart", "session_id": NATIVE_THREAD,
            "cwd": cwd})).is_err(), "accepted {cwd:?}");
    }
}

#[test]
fn native_permission_hook_reports_attention_without_approving() {
    let sandbox = Sandbox::new();
    let socket_dir = sandbox.runtime.join("claude-report");
    std::fs::create_dir_all(&socket_dir).unwrap();
    let listener = UnixListener::bind(socket_dir.join("osd-report.sock")).unwrap();
    let receiver = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        stream.read_to_string(&mut request).unwrap();
        stream.write_all(b"ok\n").unwrap();
        request
    });
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-codex-hook"), &[])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(json!({"hook_event_name": "PermissionRequest", "session_id": NATIVE_THREAD,
        "cwd": "/projects/demo", "tool_input": {"command": "private command"}}).to_string().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(), json!({}));
    let request = receiver.join().unwrap();
    let report: serde_json::Value = serde_json::from_str(request.strip_prefix("session-request ").unwrap().trim()).unwrap();
    assert_eq!(report["state"], "needs-you");
    assert_eq!(report["agent"], "codex");
    assert!(!request.contains("private command"));
    assert!(!request.contains("permissionDecision"));
}

#[test]
fn native_hook_fails_visibly_when_report_is_not_accepted() {
    let sandbox = Sandbox::new();
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-codex-hook"), &[])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(json!({"hook_event_name": "Stop", "session_id": NATIVE_THREAD,
        "cwd": "/projects/demo"}).to_string().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot report Codex lifecycle"));
    assert!(output.stdout.is_empty());
}

#[test]
fn native_codex_is_an_explicit_agent_not_a_claude_uuid_alias() {
    let agent = cockpit::agents::find("codex").expect("native Codex must be registered");
    assert_eq!(agent.name, "codex");
    assert_eq!(cockpit::agents::of_session_id(COCKPIT_THREAD).map(|agent| agent.name), Some("codex"));
    assert_eq!(cockpit::agents::of_session_id(NATIVE_THREAD).map(|agent| agent.name), Some("claude"));
}

#[test]
fn native_codex_background_starts_its_own_sandbox_launcher() {
    let sandbox = Sandbox::new();
    sandbox.shim("tmux", "");
    let output = sandbox.run(common::sandbox::bin().join("claude-sessions"),
        &["--agent", "codex", "background", sandbox.root.to_str().unwrap(), "review this task"], &[]);
    assert!(output.status.success(), "{}", common::sandbox::stderr(&output));
    let calls = sandbox.calls("tmux");
    let launch = calls.iter().find(|call| call.contains("new-session")).expect("no native Codex launch");
    assert!(launch.contains("claude-retry-codex-"), "{launch}");
    assert!(launch.contains("greywall-agents/codex"), "{launch}");
    assert!(launch.ends_with("review this task"), "{launch}");
    assert!(!launch.contains("--last"));
}

#[test]
fn native_codex_restore_uses_only_the_exact_verified_native_thread() {
    let sandbox = Sandbox::new();
    sandbox.shim("tmux", "");
    let state = sandbox.state.join("claude-sessions");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("recent-sessions.json"), json!({COCKPIT_THREAD: {"native_thread": NATIVE_THREAD}}).to_string()).unwrap();
    let script = r#"source "$1"; RECENT_SESSIONS="$XDG_STATE_HOME/claude-sessions/recent-sessions.json"; detached_size() { echo '200 50'; }; require_plain_directory() { :; }; session_field() { return 0; }; agent_codex_restore "$2" "$3""#;
    let output = sandbox.command("bash", &[]).args(["-c", script, "bash"])
        .arg(common::sandbox::bin().join("claude-sessions-agents/codex.sh")).arg(&sandbox.root).arg(COCKPIT_THREAD).output().unwrap();
    assert!(output.status.success(), "{}", common::sandbox::stderr(&output));
    assert!(sandbox.calls("tmux").iter().any(|call| call.ends_with(&format!("resume {NATIVE_THREAD}"))));
}

#[test]
fn a_recorded_native_thread_that_is_a_flag_is_never_passed_to_codex() {
    let sandbox = Sandbox::new();
    sandbox.shim("tmux", "");
    let flag = "--dangerously-bypass-approvals-and-sandbox";
    let digest = cockpit::audit::sha256_hex(flag.as_bytes());
    let forged_id = format!("cdx_{digest}");
    let state = sandbox.state.join("claude-sessions");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("recent-sessions.json"), json!({&forged_id: {"native_thread": flag}}).to_string()).unwrap();
    let script = r#"source "$1"; RECENT_SESSIONS="$XDG_STATE_HOME/claude-sessions/recent-sessions.json"; detached_size() { echo '200 50'; }; require_plain_directory() { :; }; session_field() { return 0; }; agent_codex_restore "$2" "$3""#;
    let output = sandbox.command("bash", &[]).args(["-c", script, "bash"])
        .arg(common::sandbox::bin().join("claude-sessions-agents/codex.sh")).arg(&sandbox.root).arg(&forged_id).output().unwrap();
    assert!(!output.status.success());
    assert!(sandbox.calls("tmux").is_empty(), "{:?}", sandbox.calls("tmux"));
}

#[test]
fn native_codex_missing_thread_refuses_instead_of_resuming_last() {
    let sandbox = Sandbox::new();
    sandbox.shim("tmux", "");
    let script = r#"source "$1"; RECENT_SESSIONS="$XDG_STATE_HOME/claude-sessions/recent-sessions.json"; session_field() { return 0; }; agent_codex_restore "$2" "$3""#;
    let output = sandbox.command("bash", &[]).args(["-c", script, "bash"])
        .arg(common::sandbox::bin().join("claude-sessions-agents/codex.sh")).arg(&sandbox.root).arg(COCKPIT_THREAD).output().unwrap();
    assert!(!output.status.success());
    assert!(common::sandbox::stderr(&output).contains("no verified native Codex thread"));
    assert!(sandbox.calls("tmux").is_empty());
}

#[test]
fn native_codex_todo_updates_are_attributed_to_its_namespaced_session() {
    let sandbox = Sandbox::new();
    let daemon = common::sandbox::ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let output = sandbox.command(env!("CARGO_BIN_EXE_claude-todo"), &[("CLAUDE_SESSIONS_AGENT", "codex"), ("CODEX_THREAD_ID", NATIVE_THREAD)])
        .env_remove("CLAUDE_CODE_SESSION_ID").args(["start", "0123abcd"]).current_dir(&sandbox.root).output().unwrap();
    assert!(output.status.success(), "{}", common::sandbox::stderr(&output));
    let recorded = daemon.received(std::time::Duration::from_secs(1));
    let update: serde_json::Value = serde_json::from_str(recorded[0].strip_prefix("todo-update ").unwrap()).unwrap();
    assert_eq!(update["session"], COCKPIT_THREAD);
}

#[test]
fn native_codex_run_requests_keep_user_approval_and_own_session() {
    let sandbox = Sandbox::new();
    let daemon = common::sandbox::ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let output = sandbox.command(env!("CARGO_BIN_EXE_ask-to-run"), &[("CLAUDE_SESSIONS_AGENT", "codex"),
        ("CODEX_THREAD_ID", NATIVE_THREAD), ("ASK_TO_RUN_WAIT_S", "0")])
        .env_remove("CLAUDE_CODE_SESSION_ID").args(["--why", "Inspect the kernel version.", "uname -r"]).current_dir(&sandbox.root).output().unwrap();
    assert_eq!(output.status.code(), Some(124));
    let recorded = daemon.received(std::time::Duration::from_secs(1));
    let request: serde_json::Value = serde_json::from_str(recorded[0].strip_prefix("run-request ").unwrap()).unwrap();
    assert_eq!(request["session"], COCKPIT_THREAD);
    assert_eq!(request["command"], "uname -r");
    assert!(!request.get("approved").is_some());
}

#[test]
fn native_codex_guides_are_attributed_to_the_same_cockpit_session() {
    let sandbox = Sandbox::new();
    let daemon = common::sandbox::ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_ask-to-guide"), &[("CLAUDE_SESSIONS_AGENT", "codex"),
        ("CODEX_THREAD_ID", NATIVE_THREAD), ("ASK_TO_GUIDE_WAIT_S", "0")])
        .env_remove("CLAUDE_CODE_SESSION_ID").env_remove("CLAUDE_SESSIONS_TERMINAL")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"# Check the team\nInspect the linked sessions\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(124));
    let recorded = daemon.received(std::time::Duration::from_secs(1));
    let request: serde_json::Value = serde_json::from_str(recorded[0].strip_prefix("guide-request ").unwrap()).unwrap();
    assert_eq!(request["session"], COCKPIT_THREAD);
}

#[test]
fn native_codex_design_role_uses_its_own_session_file() {
    let sandbox = Sandbox::new();
    let output = sandbox.command(env!("CARGO_BIN_EXE_claude-design"), &[("CLAUDE_SESSIONS_AGENT", "codex"), ("CODEX_THREAD_ID", NATIVE_THREAD)])
        .env_remove("CLAUDE_CODE_SESSION_ID").args(["role", "developer"]).output().unwrap();
    assert!(output.status.success(), "{}", common::sandbox::stderr(&output));
    assert_eq!(std::fs::read_to_string(sandbox.home.join(format!(".cache/claude/designs/{COCKPIT_THREAD}.role"))).unwrap(), "developer");
}

#[test]
fn native_codex_task_label_targets_its_own_card() {
    let sandbox = Sandbox::new();
    let daemon = common::sandbox::ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    sandbox.shim("cockpit-codex-hook", &format!("exec '{}'", env!("CARGO_BIN_EXE_cockpit-codex-hook")));
    let output = sandbox.command(common::sandbox::bin().join("claude-session-label"),
        &[("CLAUDE_SESSIONS_AGENT", "codex"), ("CODEX_THREAD_ID", NATIVE_THREAD)])
        .env_remove("CLAUDE_CODE_SESSION_ID").arg("Implement native Codex sessions").current_dir(&sandbox.root).output().unwrap();
    assert!(output.status.success(), "{}", common::sandbox::stderr(&output));
    let recorded = daemon.received(std::time::Duration::from_secs(1));
    let report: serde_json::Value = serde_json::from_str(recorded[0].strip_prefix("session-request ").unwrap()).unwrap();
    assert_eq!(report["session"], COCKPIT_THREAD);
    assert_eq!(report["label"], "Implement native Codex sessions");
    assert!(report.get("state").is_none());
}

#[test]
fn native_codex_question_becomes_answerable_only_after_the_turn_ends() {
    let sandbox = Sandbox::new();
    let daemon = common::sandbox::ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    sandbox.shim("cockpit-codex-hook", &format!("exec '{}'", env!("CARGO_BIN_EXE_cockpit-codex-hook")));
    let output = sandbox.command(common::sandbox::bin().join("claude-needs-input"),
        &[("CLAUDE_SESSIONS_AGENT", "codex"), ("CODEX_THREAD_ID", NATIVE_THREAD)])
        .env_remove("CLAUDE_CODE_SESSION_ID").arg("Which task should QA review?").current_dir(&sandbox.root).output().unwrap();
    assert!(output.status.success(), "{}", common::sandbox::stderr(&output));
    let recorded = daemon.received(std::time::Duration::from_secs(1));
    let first: serde_json::Value = serde_json::from_str(recorded[0].strip_prefix("session-request ").unwrap()).unwrap();
    assert_eq!(first["state"], "working");
    assert_ne!(first["needs_input"], true);
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-codex-hook"), &[])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(json!({"hook_event_name": "Stop", "session_id": NATIVE_THREAD,
        "cwd": sandbox.root}).to_string().as_bytes()).unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
    let recorded = daemon.received(std::time::Duration::from_secs(1));
    let last: serde_json::Value = serde_json::from_str(recorded.last().unwrap().strip_prefix("session-request ").unwrap()).unwrap();
    assert_eq!(last["state"], "needs-you");
    assert_eq!(last["needs_input"], true);
    assert_eq!(last["message"], "Which task should QA review?");
}

fn stop_hook(sandbox: &Sandbox) -> std::process::Output {
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-codex-hook"), &[])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(json!({"hook_event_name": "Stop", "session_id": NATIVE_THREAD,
        "cwd": sandbox.root}).to_string().as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn a_finished_codex_turn_continues_with_the_next_approved_team_message() {
    let sandbox = Sandbox::new();
    let reports = common::sandbox::ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let team = sandbox.team_socket(json!({"dispatch_id": "d1", "message_id": "m1", "text": "[Cockpit team message m1] The frame is ready."}));
    let output = stop_hook(&sandbox);
    assert!(output.status.success(), "{}", common::sandbox::stderr(&output));
    let printed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(printed["decision"], "block");
    assert!(printed["reason"].as_str().unwrap().contains("The frame is ready."));
    let filed: Vec<serde_json::Value> = reports.received(std::time::Duration::from_secs(1)).iter()
        .map(|line| serde_json::from_str(line.strip_prefix("session-request ").unwrap()).unwrap()).collect();
    let states: Vec<_> = filed.iter().map(|report| report["state"].clone()).collect();
    assert_eq!(states, [json!("waiting"), json!("working")], "the turn end is reported before the next message is taken");
    let requests = team.lock().unwrap().clone();
    assert_eq!(requests.last().unwrap()["action"], "receipt");
    assert_eq!(requests.last().unwrap()["outcome"]["outcome"], "received");
    assert_eq!(requests.last().unwrap()["dispatch_id"], "d1");
}

#[test]
fn a_finished_codex_turn_without_team_messages_stays_idle() {
    let sandbox = Sandbox::new();
    let reports = common::sandbox::ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let _team = sandbox.team_socket(serde_json::Value::Null);
    let output = stop_hook(&sandbox);
    assert!(output.status.success(), "{}", common::sandbox::stderr(&output));
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(), json!({}));
    let report: serde_json::Value = serde_json::from_str(reports.received(std::time::Duration::from_secs(1))[0].strip_prefix("session-request ").unwrap()).unwrap();
    assert_eq!(report["state"], "waiting");
}

#[test]
fn failed_native_codex_restart_does_not_focus_an_unknown_session() {
    let sandbox = Sandbox::new();
    sandbox.shim("tmux", "exit 1");
    let script = r#"source "$1"; session_field() { echo aaaaaaaa-0000-4000-8000-000000000000; }; exit_session() { :; }; require_plain_directory() { :; }; detached_size() { echo '200 50'; }; view_session() { echo 'unexpected view'; }; label_panes() { :; }; agent_codex_restart "$2" "$3""#;
    let output = sandbox.command("bash", &[]).args(["-c", script, "bash"])
        .arg(common::sandbox::bin().join("claude-sessions-agents/codex.sh")).arg(COCKPIT_THREAD).arg(&sandbox.root).output().unwrap();
    assert!(!output.status.success());
    assert!(!common::sandbox::stdout(&output).contains("unexpected view"));
}
