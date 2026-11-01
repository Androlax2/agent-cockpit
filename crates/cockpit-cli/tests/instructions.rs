//! The instruction bundle through the `cockpit` program (#49): the one-read manual, the topic
//! catalog, one topic, the launch brief's socket rule and the MCP channel, all served by the
//! running daemon so they match its release.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::Stdio;

use common::TestHome;
use serde_json::{Value, json};

#[test]
fn skill_prints_the_whole_manual_in_one_read() {
    let home = TestHome::new();
    let _daemon = home.start();

    let manual = home.cockpit_ok(&["--skill"]);

    assert!(manual.starts_with("<!-- Cockpit "), "{manual}");
    for heading in [
        "# Working in a Cockpit session",
        "# Workflow: your task's steps",
        "# Checks and evidence",
        "# Asking the owner: approvals and guides",
        "# Coordination: status, questions, follow-ups",
        "# Sandbox, network and Docker",
    ] {
        assert!(manual.contains(heading), "missing {heading}");
    }
}

#[test]
fn docs_lists_each_topic_with_when_to_read_it() {
    let home = TestHome::new();
    let _daemon = home.start();

    let catalog = home.cockpit_ok(&["docs"]);

    let topics: Vec<&str> = catalog
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .take(6)
        .collect();
    assert_eq!(
        topics,
        [
            "overview",
            "workflow",
            "checks",
            "approvals",
            "coordination",
            "sandbox"
        ]
    );
    assert!(
        catalog.contains("prove a result with receipts"),
        "{catalog}"
    );
    assert!(catalog.contains("`cockpit docs TOPIC`"), "{catalog}");
}

#[test]
fn docs_with_a_topic_prints_that_topic_alone() {
    let home = TestHome::new();
    let _daemon = home.start();

    let checks = home.cockpit_ok(&["docs", "checks"]);

    assert!(checks.starts_with("# Checks and evidence"), "{checks}");
    assert!(!checks.contains("# Sandbox"), "{checks}");
}

#[test]
fn an_unknown_topic_fails_naming_the_known_ones() {
    let home = TestHome::new();
    let _daemon = home.start();

    let stderr = home.cockpit_err(&["docs", "teleport"]);

    assert!(stderr.contains("no instruction topic teleport"), "{stderr}");
    assert!(stderr.contains("workflow, checks, approvals"), "{stderr}");
}

#[test]
fn the_manual_fails_loud_without_a_daemon() {
    let home = TestHome::new();

    let stderr = home.cockpit_err(&["--skill"]);

    assert!(stderr.contains("cannot reach the daemon"), "{stderr}");
}

#[test]
fn the_owner_has_no_launch_brief() {
    let home = TestHome::new();
    let _daemon = home.start();

    let stderr = home.cockpit_err(&["brief"]);

    assert!(stderr.contains("a launch brief is a session's"), "{stderr}");
}

/// The manual tells agents to run commands; each must be one this `cockpit` accepts.
#[test]
fn every_command_the_manual_names_is_a_verb_of_this_cli() {
    let home = TestHome::new();
    let _daemon = home.start();
    let manual = home.cockpit_ok(&["--skill"]);
    let usage = home.cockpit_err(&[]);
    let usage_lines: Vec<&str> = usage
        .lines()
        .map(|line| line.trim().trim_start_matches("usage: "))
        .collect();

    let named: Vec<&str> = manual
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| span.starts_with("cockpit "))
        .collect();

    assert!(named.len() > 20, "{named:?}");
    for command in named {
        let words: Vec<&str> = command.split_whitespace().collect();
        let accepted = usage_lines.iter().any(|line| {
            let line_words: Vec<&str> = line.split_whitespace().collect();
            line_words.get(1) == words.get(1)
                && words.get(2).is_none_or(|second| {
                    !second
                        .chars()
                        .all(|character| character.is_ascii_lowercase())
                        || line_words.contains(second)
                })
        });
        assert!(accepted, "`{command}` is not in the usage:\n{usage}");
    }
}

#[test]
fn the_mcp_server_offers_the_topics_as_resources_and_tools() {
    let home = TestHome::new();
    let _daemon = home.start();
    let mut server = McpServer::start(&home);

    let initialized = server.call(
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}),
    );
    server.notify("notifications/initialized");
    let resources = server.call(2, "resources/list", json!({}));
    let checks = server.call(
        3,
        "resources/read",
        json!({"uri": "cockpit://instructions/checks"}),
    );
    let tools = server.call(4, "tools/list", json!({}));
    let approvals = server.call(
        5,
        "tools/call",
        json!({"name": "cockpit_docs", "arguments": {"topic": "approvals"}}),
    );

    assert_eq!(initialized["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(initialized["result"]["serverInfo"]["name"], "cockpit");
    let uris: Vec<&str> = resources["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|resource| resource["uri"].as_str().unwrap())
        .collect();
    assert!(
        uris.contains(&"cockpit://instructions/workflow"),
        "{uris:?}"
    );
    let text = checks["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("# Checks and evidence"), "{text}");
    let tool_names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(tool_names, ["cockpit_docs", "cockpit_brief"]);
    let text = approvals["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("cockpit ask run"), "{text}");
    assert_eq!(approvals["result"]["isError"], false);
}

#[test]
fn an_mcp_tool_call_the_daemon_refuses_is_a_tool_error() {
    let home = TestHome::new();
    let _daemon = home.start();
    let mut server = McpServer::start(&home);

    let missing = server.call(
        1,
        "tools/call",
        json!({"name": "cockpit_docs", "arguments": {"topic": "teleport"}}),
    );
    let unknown = server.call(2, "teleport/now", json!({}));

    assert_eq!(missing["result"]["isError"], true);
    let text = missing["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("no instruction topic teleport"), "{text}");
    assert_eq!(unknown["error"]["code"], -32601);
}

#[test]
fn an_unreadable_mcp_line_is_a_parse_error_and_the_server_keeps_serving() {
    let home = TestHome::new();
    let _daemon = home.start();
    let mut server = McpServer::start(&home);

    server.send("{not json");
    let parse_error = server.receive();
    let pong = server.call(1, "ping", json!({}));

    assert_eq!(parse_error["error"]["code"], -32700);
    assert_eq!(pong["result"], json!({}));
}

/// `cockpit mcp` on pipes, one JSON-RPC message per line.
struct McpServer {
    child: std::process::Child,
    replies: BufReader<std::process::ChildStdout>,
}

impl McpServer {
    fn start(home: &TestHome) -> McpServer {
        let mut child = home
            .base_command(Path::new(env!("CARGO_BIN_EXE_cockpit")))
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("cockpit mcp runs");
        let replies = BufReader::new(child.stdout.take().unwrap());
        McpServer { child, replies }
    }

    fn send(&mut self, line: &str) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    fn receive(&mut self) -> Value {
        let mut line = String::new();
        self.replies.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}: {line:?}"))
    }

    fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string(),
        );
        let reply = self.receive();
        assert_eq!(reply["id"], id, "{reply}");
        reply
    }

    fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}).to_string());
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}
