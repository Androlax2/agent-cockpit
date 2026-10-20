mod common;
use common::sandbox::{ANSWERS_KEY, Sandbox, ReportSocket, stdout, stderr};
use cockpit::teams::client;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::process::Stdio;
use std::time::Duration;

#[test]
fn prefilled_requests_use_the_read_only_answers_socket_and_never_self_approve() {
    let sandbox = Sandbox::new();
    let source = sandbox.root.join("host.sock");
    let listener = UnixListener::bind(&source).unwrap();
    let answers = sandbox.answers("teams").parent().unwrap().to_path_buf();
    client::publish_socket(&source, &answers).unwrap();
    let receiver = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        stream.read_to_string(&mut request).unwrap();
        stream.write_all(b"{\"request_id\":\"link-1\",\"result\":{\"Ok\":{\"proposal_id\":\"pending-request\",\"status\":\"pending\"}}}\n").unwrap();
        request
    });
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("propose")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let request = json!({"action":"propose","proposal":{"submission_key":"link-1","task":{"project":"project-a","task_id":"1d3fd318"},
        "members":[{"session_id":"aaaaaaaa-0000-4000-8000-000000000000","role":"designer"}],"reason":"Review this task"}});
    child.stdin.take().unwrap().write_all(request.to_string().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(serde_json::from_str::<Value>(&stdout(&output)).unwrap()["status"], "pending");
    let sent: Value = serde_json::from_str(receiver.join().unwrap().trim()).unwrap();
    assert_eq!(sent["proposal"]["task"]["task_id"], "1d3fd318");
    assert!(sent.get("sender").is_none());
    assert!(sent["proposal"].get("approved").is_none());
}

#[test]
fn host_requests_use_the_host_only_socket() {
    let sandbox = Sandbox::new();
    let folder = sandbox.runtime.join("claude-team-host");
    std::fs::create_dir_all(&folder).unwrap();
    let listener = UnixListener::bind(folder.join("host.sock")).unwrap();
    let receiver = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        stream.read_to_string(&mut request).unwrap();
        stream.write_all(b"{\"request_id\":\"snapshot\",\"result\":{\"Ok\":{\"proposals\":[]}}}").unwrap();
        request
    });
    let reply = std::thread::scope(|_| {
        // SAFETY: this test binary's other tests do not read XDG_RUNTIME_DIR concurrently with this one.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &sandbox.runtime); }
        client::host_request(&cockpit::teams::HostRequest::Snapshot)
    }).unwrap();
    assert_eq!(reply.result.unwrap()["proposals"], json!([]));
    assert_eq!(serde_json::from_str::<Value>(&receiver.join().unwrap()).unwrap(), json!({"request": "snapshot"}));
}

#[test]
fn agent_cli_rejects_host_controls_before_transport() {
    let sandbox = Sandbox::new();
    let output = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("approve").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("not an agent action"));
}

#[test]
fn missing_team_socket_never_falls_back_to_the_shared_writable_report_path() {
    let sandbox = Sandbox::new();
    let report = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let output = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("context").output().unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("team socket"));
    assert!(report.received(Duration::from_millis(10)).is_empty());
}

#[test]
fn oversized_agent_requests_are_refused_before_transport() {
    let sandbox = Sandbox::new();
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("send")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(&vec![b'x'; 32769]).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("32 KiB"));
}

#[test]
fn publishing_a_team_socket_preserves_unfamiliar_files() {
    let sandbox = Sandbox::new();
    let private = sandbox.root.join("host.sock");
    let _listener = UnixListener::bind(&private).unwrap();
    let answers = sandbox.runtime.join("claude-answers").join(ANSWERS_KEY);
    std::fs::create_dir_all(answers.join("teams")).unwrap();
    std::fs::write(answers.join("teams/agent.sock"), "do not replace this file").unwrap();
    assert!(client::publish_socket(&private, &answers).is_err());
    assert_eq!(std::fs::read_to_string(answers.join("teams/agent.sock")).unwrap(), "do not replace this file");
}

#[test]
fn a_published_hard_link_keeps_the_original_listener_identity() {
    let sandbox = Sandbox::new();
    let private = sandbox.root.join("host.sock");
    let original = UnixListener::bind(&private).unwrap();
    let answers = sandbox.runtime.join("claude-answers").join(ANSWERS_KEY);
    std::fs::create_dir_all(&answers).unwrap();
    client::publish_socket(&private, &answers).unwrap();
    std::fs::remove_file(&private).unwrap();
    let replacement = UnixListener::bind(&private).unwrap();
    replacement.set_nonblocking(true).unwrap();
    let receiver = std::thread::spawn(move || {
        let (mut stream, _) = original.accept().unwrap();
        let mut request = String::new();
        stream.read_to_string(&mut request).unwrap();
        stream.write_all(b"{\"request_id\":\"context\",\"result\":{\"Ok\":{\"session_id\":\"original-host\"}}}\n").unwrap();
        request
    });
    let output = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("context").output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(serde_json::from_str::<Value>(&stdout(&output)).unwrap()["session_id"], "original-host");
    assert!(receiver.join().unwrap().contains("context"));
    assert_eq!(replacement.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}
