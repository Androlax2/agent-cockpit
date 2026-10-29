mod common;
use cockpit::teams::client;
use common::sandbox::{ANSWERS_KEY, ReportSocket, Sandbox, stderr, stdout};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::process::Stdio;
use std::time::Duration;

#[test]
fn sleep_cli_uses_host_socket_and_stdin_tokens() {
    for (verb, request) in [
        ("sleep-prepare", "prepare_sleep"),
        ("sleep-cancel", "cancel_sleep"),
        ("sleep-begin-wake", "begin_wake"),
        ("sleep-status", "sleep_status"),
        ("sleep-recorded", "sleep_recorded"),
        ("restart-prepare", "prepare_restart"),
    ] {
        let sandbox = Sandbox::new();
        let folder = sandbox.runtime.join("claude-team-host");
        std::fs::create_dir_all(&folder).unwrap();
        let listener = UnixListener::bind(folder.join("host.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut child = sandbox
            .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
            .args([verb, "aaaaaaaa-0000-4000-8000-000000000000"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if !verb.ends_with("-prepare") {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(json!({"token":"a".repeat(32)}).to_string().as_bytes())
                .unwrap();
        }
        drop(child.stdin.take());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let sent = loop {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut sent = String::new();
                stream.read_to_string(&mut sent).unwrap();
                stream
                    .write_all(
                        b"{\"request_id\":\"sleep\",\"result\":{\"Ok\":{\"state\":\"prepared\"}}}",
                    )
                    .unwrap();
                break sent;
            }
            if child.try_wait().unwrap().is_some() || std::time::Instant::now() >= deadline {
                break String::new();
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{verb}: {}", stderr(&output));
        let sent: Value = serde_json::from_str(&sent).unwrap();
        assert_eq!(sent["request"], request);
        assert_eq!(sent["session_id"], "aaaaaaaa-0000-4000-8000-000000000000");
    }
}

#[test]
fn sleep_cli_rejects_bad_ids_argv_tokens_and_oversized_stdin() {
    for arguments in [
        vec!["sleep-status", "../escape"],
        vec![
            "sleep-status",
            "aaaaaaaa-0000-4000-8000-000000000000",
            "secret-token",
        ],
    ] {
        let sandbox = Sandbox::new();
        let output = sandbox
            .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(!stderr(&output).contains("secret-token"));
    }
    let sandbox = Sandbox::new();
    let mut child = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .args(["sleep-status", "aaaaaaaa-0000-4000-8000-000000000000"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&vec![b'x'; 32769])
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("32 KiB"));
}

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
    let mut child = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .arg("propose")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let request = json!({"action":"propose","proposal":{"submission_key":"link-1","task":{"project":"project-a","task_id":"1d3fd318"},
        "members":[{"session_id":"aaaaaaaa-0000-4000-8000-000000000000","role":"designer"}],"reason":"Review this task"}});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&output)).unwrap()["status"],
        "pending"
    );
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
        stream
            .write_all(b"{\"request_id\":\"snapshot\",\"result\":{\"Ok\":{\"proposals\":[]}}}")
            .unwrap();
        request
    });
    let reply = std::thread::scope(|_| {
        // SAFETY: this test binary's other tests do not read XDG_RUNTIME_DIR concurrently with this one.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", &sandbox.runtime);
        }
        client::host_request(&cockpit::teams::HostRequest::Snapshot)
    })
    .unwrap();
    assert_eq!(reply.result.unwrap()["proposals"], json!([]));
    assert_eq!(
        serde_json::from_str::<Value>(&receiver.join().unwrap()).unwrap(),
        json!({"request": "snapshot"})
    );
}

#[test]
fn help_shows_send_examples_that_parse_as_actions() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    let examples: Vec<&str> = help
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("{\"action\""))
        .collect();
    assert!(examples.len() >= 3, "{help}");
    for example in examples {
        serde_json::from_str::<cockpit::teams::AgentAction>(example)
            .unwrap_or_else(|error| panic!("{example}: {error}"));
    }
}

#[test]
fn agent_cli_rejects_host_controls_before_transport() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .arg("approve")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("not an agent action"));
}

#[test]
fn missing_team_socket_never_falls_back_to_the_shared_writable_report_path() {
    let sandbox = Sandbox::new();
    let report = ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), "ok");
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .arg("context")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("team socket"));
    assert!(report.received(Duration::from_millis(10)).is_empty());
}

#[test]
fn oversized_agent_requests_are_refused_before_transport() {
    let sandbox = Sandbox::new();
    let mut child = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .arg("send")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&vec![b'x'; 32769])
        .unwrap();
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
    assert_eq!(
        std::fs::read_to_string(answers.join("teams/agent.sock")).unwrap(),
        "do not replace this file"
    );
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
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .arg("context")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&output)).unwrap()["session_id"],
        "original-host"
    );
    assert!(receiver.join().unwrap().contains("context"));
    assert_eq!(
        replacement.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn recruit_is_an_agent_action() {
    let sandbox = Sandbox::new();
    let source = sandbox.root.join("host.sock");
    let listener = UnixListener::bind(&source).unwrap();
    let answers = sandbox.answers("teams").parent().unwrap().to_path_buf();
    client::publish_socket(&source, &answers).unwrap();
    let receiver = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        stream.read_to_string(&mut request).unwrap();
        stream.write_all(b"{\"request_id\":\"recruit-1\",\"result\":{\"Ok\":{\"proposal_id\":\"p\",\"revision\":1}}}\n").unwrap();
        request
    });
    let mut child = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .arg("recruit")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let request = json!({"action":"recruit","recruitment":{"submission_key":"recruit-1","task":{"project":"project-a","task_id":"1d3fd318"},
        "role":"developer","recruits":[{"role":"designer","brief":"Design the jobs list"}],"reason":"The UI needs a design"}});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let sent: Value = serde_json::from_str(receiver.join().unwrap().trim()).unwrap();
    assert_eq!(sent["action"], "recruit");
    assert_eq!(sent["recruitment"]["recruits"][0]["role"], "designer");
}

#[test]
fn help_shows_how_to_recruit() {
    let sandbox = Sandbox::new();
    let help = stdout(
        &sandbox
            .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
            .arg("--help")
            .output()
            .unwrap(),
    );
    assert!(
        help.lines()
            .any(|line| line.trim().starts_with("{\"action\":\"recruit\"")),
        "{help}"
    );
    assert!(
        help.contains("automatically") && help.contains("every project"),
        "{help}"
    );
    assert!(!help.contains("Theo approves"), "{help}");
}

fn link_actions() -> Vec<(&'static str, Value)> {
    vec![
        (
            "link-propose",
            json!({"action":"link_propose","proposal":{"submission_key":"joint","task":{"project":"path:/project-a","task_id":"1d3fd318"},
        "peer_session_id":"aaaaaaaa-0000-4000-8000-000000000000","purpose":"Coordinate shared work"}}),
        ),
        (
            "link-send",
            json!({"action":"link_send","message":{"link_id":"link-1","submission_key":"note","kind":"notice","body":"Ready"}}),
        ),
        (
            "link-assess",
            json!({"action":"link_assess","link_id":"link-1","thread_id":"thread-1","resolved":true}),
        ),
    ]
}

fn cli_input(sandbox: &Sandbox, verb: &str, input: &[u8]) -> std::process::Output {
    let mut child = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .arg(verb)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(input);
    child.wait_with_output().unwrap()
}

#[test]
fn link_cli_actions_use_existing_authenticated_transport() {
    for (verb, action) in link_actions() {
        let sandbox = Sandbox::new();
        let requests = sandbox.team_socket(Value::Null);
        let output = cli_input(&sandbox, verb, action.to_string().as_bytes());
        assert!(output.status.success(), "{verb}: {}", stderr(&output));
        let sent = requests.lock().unwrap()[0].clone();
        let expected = serde_json::from_value::<cockpit::teams::AgentAction>(action).unwrap();
        assert_eq!(sent, serde_json::to_value(expected).unwrap());
    }
}

#[test]
fn link_cli_rejects_mismatches_unknown_fields_and_host_actions_before_transport() {
    let sandbox = Sandbox::new();
    let requests = sandbox.team_socket(Value::Null);
    for (verb, action) in link_actions() {
        let wrong = cli_input(&sandbox, verb, b"{\"action\":\"context\"}");
        assert!(
            stderr(&wrong).contains("command and JSON action do not match"),
            "{}",
            stderr(&wrong)
        );
        let mut forged = action;
        forged["sender"] = json!("forged");
        let output = cli_input(&sandbox, verb, forged.to_string().as_bytes());
        assert_eq!(output.status.code(), Some(2));
        assert!(
            stderr(&output).contains("unknown field"),
            "{}",
            stderr(&output)
        );
    }
    for action in ["link_approve", "link_edit", "link_reject", "link_revoke"] {
        let output = cli_input(
            &sandbox,
            "link-send",
            json!({"action":action}).to_string().as_bytes(),
        );
        assert_eq!(output.status.code(), Some(2));
        let output = cli_input(&sandbox, &action.replace('_', "-"), b"");
        assert!(stderr(&output).contains("not an agent action"));
    }
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn link_cli_keeps_shared_request_and_reply_bounds() {
    for (verb, _) in link_actions() {
        let sandbox = Sandbox::new();
        let output = cli_input(&sandbox, verb, &vec![b'x'; client::REQUEST_MAX + 1]);
        assert_eq!(output.status.code(), Some(2));
        assert!(stderr(&output).contains("32 KiB"), "{}", stderr(&output));
    }
    let sandbox = Sandbox::new();
    sandbox.team_socket(json!({"padding":"x".repeat(client::AGENT_REPLY_MAX)}));
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .arg("receive")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("team reply exceeds its limit"));
}

#[test]
fn agent_reply_limit_counts_the_daemon_newline() {
    for (json_bytes, accepted) in [(65_535, true), (65_536, false)] {
        let sandbox = Sandbox::new();
        let source = sandbox.root.join("framed.sock");
        let listener = UnixListener::bind(&source).unwrap();
        client::publish_socket(&source, sandbox.answers("teams").parent().unwrap()).unwrap();
        let mut reply = json!({"request_id":"context","result":{"Ok":{"padding":""}}});
        let padding_bytes = json_bytes - reply.to_string().len();
        reply["result"]["Ok"]["padding"] = json!("x".repeat(padding_bytes));
        let wire = format!("{reply}\n");
        assert_eq!(wire.len(), json_bytes + 1);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            stream.write_all(wire.as_bytes()).unwrap();
        });
        let output = sandbox
            .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
            .arg("context")
            .output()
            .unwrap();
        server.join().unwrap();
        assert_eq!(
            output.status.success(),
            accepted,
            "{json_bytes} JSON bytes: {}",
            stderr(&output)
        );
        if accepted {
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stdout).unwrap()["padding"]
                    .as_str()
                    .unwrap()
                    .len(),
                padding_bytes
            );
        } else {
            assert!(stderr(&output).contains("team reply exceeds its limit"));
        }
    }
}

#[test]
fn link_help_explains_explicit_addressing_closure_and_no_permission_relay() {
    let sandbox = Sandbox::new();
    let help = stdout(
        &sandbox
            .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
            .arg("--help")
            .output()
            .unwrap(),
    );
    for (verb, action) in link_actions() {
        assert!(help.contains(verb), "missing {verb}");
        let discriminator = action["action"].as_str().unwrap();
        assert!(
            help.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
                .any(|example| example["action"] == discriminator)
        );
    }
    assert!(help.contains("explicit link_id"));
    assert!(help.contains("TODO done") && help.contains("reopening"));
    assert!(help.contains("cannot relay Theo's permission"));
}

/// A session's link left on the socket of a daemon that has exited: connecting is refused.
fn stale_link(sandbox: &Sandbox) -> std::path::PathBuf {
    let old = sandbox.root.join("old-daemon.sock");
    drop(UnixListener::bind(&old).unwrap());
    let answers = sandbox.answers("teams").parent().unwrap().to_path_buf();
    client::publish_socket(&old, &answers).unwrap();
    answers
}

#[test]
fn a_team_request_sent_during_a_daemon_restart_reaches_the_restarted_daemon_once() {
    let sandbox = Sandbox::new();
    let answers = stale_link(&sandbox);
    let child = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .arg("context")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    let new = sandbox.root.join("new-daemon.sock");
    let listener = UnixListener::bind(&new).unwrap();
    client::publish_socket(&new, &answers).unwrap();
    let (mut stream, _) = listener.accept().unwrap();
    let mut request = String::new();
    stream.read_to_string(&mut request).unwrap();
    stream
        .write_all(b"{\"request_id\":\"context\",\"result\":{\"Ok\":{\"task\":null}}}")
        .unwrap();
    drop(stream);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        serde_json::from_str::<Value>(&request).unwrap()["action"],
        "context"
    );
    listener.set_nonblocking(true).unwrap();
    assert!(
        listener.accept().is_err(),
        "the request was delivered more than once"
    );
}

#[test]
fn a_team_request_that_never_connects_says_nothing_was_sent() {
    let sandbox = Sandbox::new();
    stale_link(&sandbox);
    let started = std::time::Instant::now();
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .arg("context")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        started.elapsed() >= Duration::from_secs(15),
        "{:?}",
        started.elapsed()
    );
    assert!(
        stderr(&output).contains("nothing was sent: retry with the same submission key"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_session_without_a_team_link_is_told_at_once_to_restart() {
    let sandbox = Sandbox::new();
    sandbox.answers("teams");
    let started = std::time::Instant::now();
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_cockpit-team"), &[])
        .arg("context")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(
        stderr(&output).contains("restart this session"),
        "{}",
        stderr(&output)
    );
}
