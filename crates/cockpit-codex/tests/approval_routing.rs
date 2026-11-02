//! #436: transport observations are separate from signed-in acceptance.

#[path = "common/approval_probe.rs"]
mod approval_probe;
mod common;
#[path = "common/recording_proxy.rs"]
mod recording_proxy;

use std::time::Duration;

use cockpit_codex::app_server::AppServer;
use common::app_server::FakeAppServer;
use recording_proxy::{Direction, RecordingProxy};
use serde_json::json;

#[test]
#[ignore = "needs Codex 0.166.0 and tmux; isolated home and local fake model only"]
fn a_delivered_start_reaches_the_real_tui_after_delivery_disconnects() {
    approval_probe::run(false);
}

#[test]
#[ignore = "needs Codex 0.166.0 and tmux; isolated home and local fake model only"]
fn a_delivered_steer_keeps_approval_on_the_real_tui() {
    approval_probe::run(true);
}

#[test]
#[ignore = "needs explicit host approval, signed-in Codex 0.166.0 and tmux; consumes account usage"]
fn a_signed_in_delivered_turn_reaches_the_real_tui() {
    approval_probe::run_signed_in();
}

#[test]
#[ignore = "needs Codex 0.166.0; isolated home, no credentials or model usage"]
fn an_enabled_mcp_server_is_refused_before_a_model_turn_or_tool_launch() {
    approval_probe::enabled_mcp_is_refused();
}

#[test]
#[ignore = "needs Codex 0.166.0 and tmux; isolated home and local fake model only"]
fn an_inherited_notification_command_is_disabled() {
    approval_probe::notification_is_disabled();
}

#[test]
#[ignore = "needs Codex 0.166.0; isolated home, no credentials or model usage"]
fn inherited_execution_rules_are_refused_before_a_model_turn() {
    approval_probe::execution_rules_are_refused();
}

#[test]
#[ignore = "needs Codex 0.166.0; isolated home, no credentials or model usage"]
fn a_custom_provider_is_not_signed_in_conformance() {
    approval_probe::custom_provider_is_refused();
}

#[test]
#[ignore = "needs Codex 0.166.0; isolated home, no credentials or model usage"]
fn an_empty_home_is_not_signed_in_conformance() {
    approval_probe::signed_out_is_refused();
}

#[test]
#[ignore = "needs Codex 0.166.0; isolated home, no credentials or model usage"]
fn project_integrations_and_execution_rules_are_refused_before_a_turn() {
    approval_probe::project_settings_are_refused();
}

#[test]
#[ignore = "needs Codex 0.166.0; isolated home, no credentials or model usage"]
fn a_keymap_that_can_change_escape_is_refused_before_the_tui_starts() {
    approval_probe::custom_keymap_is_refused();
}

#[test]
fn the_proxy_records_both_directions_and_delivery_disconnects() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("upstream.sock");
    let _server = FakeAppServer::start(&upstream, |_, params| Ok(params.clone()));
    let proxy = RecordingProxy::start(&dir.path().join("record.sock"), &upstream);
    let mut client = AppServer::connect(proxy.path(), "delivery", Duration::from_secs(2)).unwrap();
    let params = json!({"text": "x".repeat(200)});
    assert_eq!(client.call("echo", params.clone()).unwrap(), params);
    drop(client);
    proxy.wait_for(|entries| entries.iter().any(|entry| entry.disconnected));
    let entries = proxy.entries();
    let request = entries
        .iter()
        .find(|entry| entry.body["method"] == "echo")
        .unwrap();
    assert_eq!(request.direction, Direction::Client);
    assert_eq!(request.body["params"], params);
    assert!(
        entries
            .iter()
            .any(|entry| entry.direction == Direction::Server
                && entry.connection == request.connection
                && entry.body["id"] == request.body["id"]
                && entry.body["result"] == params)
    );
}

#[test]
fn the_proxy_keeps_owner_and_delivery_connections_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("upstream.sock");
    let server = FakeAppServer::start(&upstream, |_, _| Ok(json!({})));
    let proxy = RecordingProxy::start(&dir.path().join("record.sock"), &upstream);
    let mut owner = AppServer::connect(proxy.path(), "owner", Duration::from_secs(2)).unwrap();
    let delivery = AppServer::connect(proxy.path(), "delivery", Duration::from_secs(2)).unwrap();
    server.wait_for_connections(2);
    drop(delivery);
    proxy.wait_for(|entries| entries.iter().any(|entry| entry.disconnected));
    server.notify("turn/completed", json!({"threadId": "fixture"}));
    assert_eq!(owner.next_notification().unwrap().method, "turn/completed");
    let entries = proxy.entries();
    let owner_id = entries
        .iter()
        .find(|entry| entry.body.pointer("/params/clientInfo/name") == Some(&json!("owner")))
        .unwrap()
        .connection;
    let delivery_id = entries
        .iter()
        .find(|entry| entry.body.pointer("/params/clientInfo/name") == Some(&json!("delivery")))
        .unwrap()
        .connection;
    assert_ne!(owner_id, delivery_id);
    assert!(
        entries
            .iter()
            .any(|entry| entry.connection == delivery_id && entry.disconnected)
    );
    assert!(
        !entries
            .iter()
            .any(|entry| entry.connection == owner_id && entry.disconnected)
    );
}

#[test]
fn a_start_disconnect_does_not_count_as_the_steer_disconnect() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("upstream.sock");
    let _server = FakeAppServer::start(&upstream, |_, _| Ok(json!({})));
    let proxy = RecordingProxy::start(&dir.path().join("record.sock"), &upstream);
    let mut start = AppServer::connect(proxy.path(), "start", Duration::from_secs(2)).unwrap();
    start
        .call("turn/start", json!({"threadId":"fixture"}))
        .unwrap();
    drop(start);
    proxy.wait_for(|entries| entries.iter().any(|entry| entry.disconnected));
    let mut steer = AppServer::connect(proxy.path(), "steer", Duration::from_secs(2)).unwrap();
    steer
        .call("turn/steer", json!({"threadId":"fixture"}))
        .unwrap();
    let entries = proxy.entries();
    let connection = entries
        .iter()
        .find(|entry| entry.body["method"] == "turn/steer")
        .unwrap()
        .connection;
    assert!(
        !entries
            .iter()
            .any(|entry| entry.connection == connection && entry.disconnected)
    );
    drop(steer);
    proxy.wait_for(|entries| {
        entries
            .iter()
            .any(|entry| entry.connection == connection && entry.disconnected)
    });
}
