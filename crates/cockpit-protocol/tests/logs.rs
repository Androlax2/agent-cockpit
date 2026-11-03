use cockpit_protocol::{Capability, ErrorCode, Reply, decode_request};
use serde_json::json;

#[test]
fn log_level_commands_round_trip_and_bad_field_types_are_refused() {
    for command in [
        json!({"kind":"logs-level-status"}),
        json!({"kind":"logs-level-set", "directives":"tmux=trace"}),
        json!({"kind":"logs-level-set", "directives":"debug", "duration":"1s"}),
        json!({"kind":"logs-level-revert"}),
    ] {
        let wire = json!({"id":1,"command":command});
        let request = decode_request(&wire.to_string()).expect("log control is a known command");
        assert_eq!(serde_json::to_value(request).unwrap(), wire);
    }
    for command in [
        json!({"kind":"logs-level-set", "directives":true}),
        json!({"kind":"logs-level-set", "directives":"debug", "duration":15}),
    ] {
        let rejected = decode_request(&json!({"id":1,"command":command}).to_string()).unwrap_err();
        assert_eq!(rejected.error.code, ErrorCode::InvalidRequest);
    }
}

#[test]
fn log_level_status_and_capability_have_stable_wire_forms() {
    for wire in [
        json!({"kind":"logs-level", "directives":"warn", "expires_at_unix_ms":null, "reverts_to":null}),
        json!({"kind":"logs-level", "directives":"debug", "expires_at_unix_ms":12345, "reverts_to":"warn"}),
    ] {
        let reply: Reply = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(reply).unwrap(), wire);
    }
    let capability: Capability = serde_json::from_value(json!("log-level-control")).unwrap();
    assert!(!matches!(capability, Capability::Unknown(_)));
}

#[test]
fn a_log_level_reply_from_a_daemon_without_reverts_to_still_decodes() {
    let reply: Reply = serde_json::from_value(
        json!({"kind":"logs-level", "directives":"debug", "expires_at_unix_ms":12345}),
    )
    .unwrap();
    assert!(matches!(
        reply,
        Reply::LogsLevel {
            reverts_to: None,
            ..
        }
    ));
}
