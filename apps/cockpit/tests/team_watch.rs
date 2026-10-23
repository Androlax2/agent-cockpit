//! `cockpit-team watch`: Claude's background (asyncRewake) hook that wakes an idle session with
//! its next approved team message. Exit 2 wakes Claude with stderr; anything else stays silent.

mod common;
#[path = "common/link_history.rs"]
mod link_history;
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

#[test]
fn chief_envelopes_use_the_same_claude_native_wake_and_receipt_once() {
    for kind in ["chief_assignment", "chief_coordinate"] {
        let sandbox = Sandbox::new();
        let team = sandbox.team_socket_as(SESSION_A, json!({"dispatch_id": "chief_d1", "message_id": "assignment1",
            "text": "Protected approval; commentary is not authority", "envelope": {"kind": kind}}));
        let mut child = watch(&sandbox, SESSION_A);
        assert_eq!(exit_within(&mut child, Duration::from_secs(3)), Some(2));
        let output = child.wait_with_output().unwrap();
        assert!(String::from_utf8_lossy(&output.stderr).contains("commentary is not authority"));
        let requests = team.lock().unwrap();
        let receipts = requests.iter().filter(|request| request["action"] == "receipt").collect::<Vec<_>>();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0]["dispatch_id"], "chief_d1");
        assert_eq!(receipts[0]["outcome"]["native_id"], "claude-wake:chief_d1");
    }
}

#[test]
fn chief_claude_failed_wake_output_never_sends_a_received_receipt() {
    let sandbox = Sandbox::new();
    let requests = sandbox.team_socket_as(SESSION_A, json!({"dispatch_id":"failed_wake", "message_id":"assignment1",
        "text":"approved task", "envelope":{"kind":"chief_assignment"}}));
    let full = std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[("COCKPIT_TEAM_POLL_MS","20")]).arg("watch")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::from(full)).spawn().unwrap();
    child.stdin.take().unwrap().write_all(json!({"hook_event_name":"Stop", "session_id":SESSION_A}).to_string().as_bytes()).unwrap();
    assert_eq!(exit_within(&mut child, Duration::from_secs(3)), Some(1));
    let requests = requests.lock().unwrap();
    let receipt = requests.iter().find(|request| request["action"] == "receipt").expect("uncertain receipt");
    assert_eq!(receipt["outcome"]["outcome"], "uncertain");
}

#[test]
fn link_messages_wake_only_the_verified_root_once_with_stable_receipts() {
    for teamed in [false, true] {
        let sandbox = Sandbox::new();
        let members = if teamed { json!([{"session_id":SESSION_A,"role":"developer","agent":"claude"}]) } else { json!([]) };
        let requests = sandbox.team_socket_with_context(json!({"session_id":SESSION_A,"members":members}), link_delivery());
        let mut child = watch(&sandbox, SESSION_A);
        assert_eq!(exit_within(&mut child, Duration::from_secs(3)), Some(2));
        let output = child.wait_with_output().unwrap();
        let text = stderr(&output);
        assert!(text.starts_with("[Cockpit link message m-link"), "{text}");
        assert!(text.contains("Link: link-123") && text.contains("1d3fd318") && text.contains("path:/projects/peer"));
        assert_eq!(actions(&requests), [json!("context"), json!("receive"), json!("receipt")]);
        assert_eq!(requests.lock().unwrap()[2], json!({"action":"receipt","dispatch_id":"d-link","outcome":{"outcome":"received","native_id":"claude-wake:d-link"}}));
    }
    let sandbox = Sandbox::new();
    let requests = sandbox.team_socket_as(SESSION_B, link_delivery());
    let mut nested = watch(&sandbox, SESSION_A);
    assert_eq!(exit_within(&mut nested, Duration::from_secs(2)), Some(0));
    assert_eq!(actions(&requests), [json!("context")]);
}

#[test]
fn terminal_link_history_does_not_block_a_real_queued_team_wake() {
    use cockpit::teams::{self, AgentAction, HostAction, LinkProposal, Member, MessageKind, Proposal, Role, Submission, TaskKey, VerifiedSession, store};
    use std::io::Read;
    use std::os::unix::net::UnixListener;
    if std::env::var_os("TEAM_WATCH_HISTORY_CHILD").is_none() {
        let sandbox = Sandbox::new();
        let output = sandbox.command(std::env::current_exe().unwrap(), &[("TEAM_WATCH_HISTORY_CHILD", "1")])
            .args(["--exact", "terminal_link_history_does_not_block_a_real_queued_team_wake", "--nocapture"]).output().unwrap();
        assert!(output.status.success(), "{}\n{}", stdout(&output), stderr(&output));
        return;
    }
    let sandbox = Sandbox::new();
    let directory = cockpit::paths::home().join("IdeaProjects/project");
    std::fs::create_dir_all(directory.join(".git")).unwrap();
    let project = format!("path:{}", directory.display());
    let mut todos = cockpit::todo::TodoList::load();
    let task = TaskKey { project:project.clone(), task_id:todos.add("project", directory.to_str().unwrap(), "Joint work") };
    let live: Vec<_> = [SESSION_A, SESSION_B].iter().map(|session| VerifiedSession { session_id:(*session).into(), agent:"claude".into(),
        project:project.clone(), runtime:format!("runtime-{session}") }).collect();
    let mut connection = store::open(&sandbox.root.join("protected/teams.sqlite")).unwrap();
    let history = LinkProposal { submission_key:"history".into(), task:task.clone(), peer_session_id:SESSION_B.into(), purpose:"Joint work".into() };
    link_history::populate(&mut connection, &live, &history);
    let proposal = Proposal { submission_key:"team".into(), task:task.clone(), reason:"Own team".into(),
        members:vec![Member { session_id:SESSION_A.into(), role:Role::Developer }, Member { session_id:SESSION_B.into(), role:Role::Designer }] };
    let pending = store::propose(&mut connection, &live[0], &proposal, &live, 6_000_000).unwrap().result.unwrap();
    store::control(&mut connection, &HostAction::Approve { request_id:"team".into(), proposal_id:pending["proposal_id"].as_str().unwrap().into(),
        expected_revision:1, mapping_hash:pending["mapping_hash"].as_str().unwrap().into() }, &live, 6_000_001).unwrap();
    let accepted = store::accept(&mut connection, &live[1], &AgentAction::Send { message:Submission { task, submission_key:"new-note".into(),
        recipient:SESSION_A.into(), kind:MessageKind::Notice, body:"Team wake still works".into(), thread_id:None, handoff_id:None } }, 6_000_002).unwrap().result.unwrap();
    link_history::fill_reply_boundary(&mut connection, &live, &history);
    let source = sandbox.root.join("team.sock");
    let listener = UnixListener::bind(&source).unwrap();
    listener.set_nonblocking(true).unwrap();
    teams::client::publish_socket(&source, sandbox.answers("teams").parent().unwrap()).unwrap();
    let mut child = watch(&sandbox, SESSION_A);
    let mut received = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok((mut stream, _)) = listener.accept() {
            stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
            let mut input = String::new();
            stream.read_to_string(&mut input).unwrap();
            let action: AgentAction = serde_json::from_str(&input).unwrap();
            let result = match &action {
                AgentAction::Context => store::context(&connection, &live[0]),
                AgentAction::Receive => store::claim(&mut connection, &live[0], "idle", &live, 6_000_003).map(|dispatch| dispatch.map_or(Value::Null, |dispatch|
                    json!({"dispatch_id":dispatch.id,"message_id":dispatch.message_id,"text":teams::envelope_text(&dispatch.envelope),"envelope":dispatch.envelope}))),
                AgentAction::Receipt { dispatch_id, outcome } => store::finish_for(&mut connection, &live[0], dispatch_id, outcome, 6_000_004).map(|_| json!({"status":"recorded"})),
                _ => panic!("unexpected watcher action"),
            };
            received.push(serde_json::to_value(&action).unwrap());
            let reply = teams::TeamReply { request_id:"context".into(), result };
            let _ = stream.write_all(format!("{}\n", serde_json::to_string(&reply).unwrap()).as_bytes());
        }
        if child.try_wait().unwrap().is_some() { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    let status = child.try_wait().unwrap();
    if status.is_none() { child.kill().unwrap(); }
    let output = child.wait_with_output().unwrap();
    assert_eq!(status.and_then(|status| status.code()), Some(2), "historical Context must not prevent Receive: {}", stderr(&output));
    assert_eq!(received.iter().map(|request| request["action"].clone()).collect::<Vec<_>>(), [json!("context"),json!("receive"),json!("receipt")]);
    assert_eq!(stderr(&output).matches("Team wake still works").count(), 1);
    let (state, native_id): (String, String) = connection.query_row("SELECT m.state,a.native_id FROM messages m JOIN attempts a ON a.message_id=m.id WHERE m.id=?1",
        [accepted["message_id"].as_str().unwrap()], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!(state, "received");
    assert_eq!(native_id, format!("claude-wake:{}", received[2]["dispatch_id"].as_str().unwrap()));
    assert!(store::claim(&mut connection, &live[0], "idle", &live, 6_000_005).unwrap().is_none());
}
