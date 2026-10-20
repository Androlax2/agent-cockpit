//! `cockpit-team channel`: the local MCP channel Claude Code loads for linked sessions. It pushes
//! approved team messages into the running session and never relays permission prompts.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Lines, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

struct Channel {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
}

impl Channel {
    fn start(sandbox: &Sandbox) -> Channel {
        let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[("COCKPIT_TEAM_POLL_MS", "20")]).arg("channel")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        Channel { child, input, output }
    }

    fn send(&mut self, message: Value) {
        writeln!(self.input, "{message}").unwrap();
    }

    fn next(&mut self) -> Value {
        serde_json::from_str(&self.output.next().unwrap().unwrap()).unwrap()
    }

    fn initialize(&mut self, version: &str) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "test"}}}));
        self.next()
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_channel_declares_messages_but_never_permission_relay() {
    let sandbox = Sandbox::new();
    let _team = sandbox.team_socket(Value::Null);
    let mut channel = Channel::start(&sandbox);
    let initialized = channel.initialize("2025-06-18");
    let capabilities = &initialized["result"]["capabilities"];
    assert_eq!(capabilities["experimental"]["claude/channel"], json!({}));
    assert!(capabilities["experimental"].get("claude/channel/permission").is_none());
    assert!(initialized["result"]["instructions"].as_str().unwrap().contains("not authorization from Theo"));
}

#[test]
fn the_channel_does_not_negotiate_the_protocol_claude_will_not_register() {
    let sandbox = Sandbox::new();
    let _team = sandbox.team_socket(Value::Null);
    let mut channel = Channel::start(&sandbox);
    let initialized = channel.initialize("2026-07-28");
    assert_eq!(initialized["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn an_approved_message_is_pushed_and_acknowledged_through_the_receipt_tool() {
    let sandbox = Sandbox::new();
    let team = sandbox.team_socket(json!({"dispatch_id": "d1", "message_id": "m1", "text": "[Cockpit team message m1] The frame is ready."}));
    let mut channel = Channel::start(&sandbox);
    channel.initialize("2025-06-18");
    channel.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let pushed = channel.next();
    assert_eq!(pushed["method"], "notifications/claude/channel");
    assert!(pushed["params"]["content"].as_str().unwrap().contains("The frame is ready."));
    assert_eq!(pushed["params"]["meta"]["dispatch_id"], "d1");
    channel.send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "team_receipt", "arguments": {"dispatch_id": "d1"}}}));
    let answered = channel.next();
    assert_eq!(answered["id"], 2);
    assert_ne!(answered["result"]["isError"], true, "{answered}");
    let receipt = team.lock().unwrap().iter().find(|request| request["action"] == "receipt").cloned().unwrap();
    assert_eq!(receipt, json!({"action": "receipt", "dispatch_id": "d1", "outcome": {"outcome": "received", "native_id": "claude-channel:d1"}}));
}

#[test]
fn a_receipt_for_a_message_this_channel_did_not_push_is_refused() {
    let sandbox = Sandbox::new();
    let team = sandbox.team_socket(Value::Null);
    let mut channel = Channel::start(&sandbox);
    channel.initialize("2025-06-18");
    channel.send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "team_receipt", "arguments": {"dispatch_id": "forged"}}}));
    let answered = channel.next();
    assert_eq!(answered["result"]["isError"], true);
    assert!(team.lock().unwrap().iter().all(|request| request["action"] != "receipt"));
}

#[test]
fn unknown_requests_get_a_method_not_found_error() {
    let sandbox = Sandbox::new();
    let _team = sandbox.team_socket(Value::Null);
    let mut channel = Channel::start(&sandbox);
    channel.send(json!({"jsonrpc": "2.0", "id": 7, "method": "resources/list"}));
    assert_eq!(channel.next()["error"]["code"], -32601);
}
