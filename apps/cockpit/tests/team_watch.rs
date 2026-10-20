//! `cockpit-team watch`: Claude's background (asyncRewake) hook that wakes an idle session with
//! its next approved team message. Exit 2 wakes Claude with stderr; anything else stays silent.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::io::Write;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

fn watch(sandbox: &Sandbox, session: &str) -> Child {
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[("COCKPIT_TEAM_POLL_MS", "20")]).arg("watch")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(json!({"hook_event_name": "Stop", "session_id": session}).to_string().as_bytes()).unwrap();
    child
}

fn exit_within(child: &mut Child, limit: Duration) -> Option<i32> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() { return status.code(); }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

fn actions(requests: &TeamRequests) -> Vec<Value> {
    requests.lock().unwrap().iter().map(|request| request["action"].clone()).collect()
}

#[test]
fn an_approved_message_wakes_the_idle_session_and_is_acknowledged() {
    let sandbox = Sandbox::new();
    let team = sandbox.team_socket_as(SESSION_A, json!({"dispatch_id": "d1", "message_id": "m1", "text": "[Cockpit team message m1] The frame is ready."}));
    let mut child = watch(&sandbox, SESSION_A);
    assert_eq!(exit_within(&mut child, Duration::from_secs(3)), Some(2), "exit 2 is what wakes Claude");
    let output = child.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("The frame is ready."));
    let requests = team.lock().unwrap().clone();
    let receipt = requests.iter().find(|request| request["action"] == "receipt").expect("no receipt");
    assert_eq!((receipt["dispatch_id"].clone(), receipt["outcome"]["outcome"].clone()), (json!("d1"), json!("received")));
}

#[test]
fn a_newer_watcher_of_the_same_session_replaces_the_older_one() {
    let sandbox = Sandbox::new();
    let _team = sandbox.team_socket_as(SESSION_A, Value::Null);
    let mut older = watch(&sandbox, SESSION_A);
    std::thread::sleep(Duration::from_millis(200));
    let mut newer = watch(&sandbox, SESSION_A);
    assert_eq!(exit_within(&mut older, Duration::from_secs(2)), Some(0));
    assert_eq!(exit_within(&mut newer, Duration::from_millis(300)), None, "the newer one keeps watching");
    newer.kill().unwrap();
}

#[test]
fn a_watcher_that_is_not_its_launchs_session_takes_nothing() {
    // A nested `claude -p` shares its parent's launch: the daemon would hand it the parent's messages.
    let sandbox = Sandbox::new();
    let team = sandbox.team_socket_as(SESSION_B, json!({"dispatch_id": "d1", "message_id": "m1", "text": "hi"}));
    let mut child = watch(&sandbox, SESSION_A);
    assert_eq!(exit_within(&mut child, Duration::from_secs(2)), Some(0));
    assert_eq!(actions(&team), [json!("context")]);
}

#[test]
fn a_hook_payload_without_a_valid_session_id_is_refused() {
    for session in ["", "../x", "not-a-session"] {
        let sandbox = Sandbox::new();
        let team = sandbox.team_socket_as(SESSION_A, Value::Null);
        let mut child = watch(&sandbox, session);
        assert_eq!(exit_within(&mut child, Duration::from_secs(2)), Some(1), "{session:?}");
        assert!(actions(&team).is_empty());
    }
}
