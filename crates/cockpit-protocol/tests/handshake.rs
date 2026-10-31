use cockpit_protocol::{
    Capability, HandshakeReply, Hello, ProtocolVersion, RefusalReason, SUPPORTED_VERSIONS,
    negotiate,
};
use serde_json::json;

fn hello_from_wire(wire: serde_json::Value) -> Hello {
    serde_json::from_value(wire).expect("a valid hello")
}

#[test]
fn a_client_speaking_the_daemon_version_is_accepted_at_that_version() {
    let hello = hello_from_wire(json!({"protocol_versions": [1], "capabilities": []}));

    let reply = negotiate(&hello, &[]);

    assert_eq!(
        serde_json::to_value(reply).unwrap(),
        json!({"outcome": "accepted", "protocol_version": 1, "capabilities": []})
    );
}

#[test]
fn the_highest_version_both_sides_speak_is_chosen() {
    let hello = hello_from_wire(json!({"protocol_versions": [7, 1, 0], "capabilities": []}));

    let HandshakeReply::Accepted(welcome) = negotiate(&hello, &[]) else {
        panic!("version 1 is shared");
    };

    assert_eq!(welcome.protocol_version, ProtocolVersion(1));
}

#[test]
fn a_version_mismatch_is_a_typed_refusal_listing_the_daemon_versions() {
    let hello = hello_from_wire(json!({"protocol_versions": [2, 3], "capabilities": []}));

    let wire = serde_json::to_value(negotiate(&hello, &[])).unwrap();

    assert_eq!(wire["outcome"], "refused");
    assert_eq!(wire["reason"], "version-mismatch");
    assert_eq!(wire["supported_versions"], json!(SUPPORTED_VERSIONS));
    assert!(wire["message"].as_str().unwrap().contains("2, 3"), "{wire}");
}

#[test]
fn a_hello_without_versions_is_refused_as_a_mismatch() {
    let hello = hello_from_wire(json!({"protocol_versions": [], "capabilities": []}));

    let HandshakeReply::Refused(refusal) = negotiate(&hello, &[]) else {
        panic!("no version is shared");
    };

    assert_eq!(refusal.reason, RefusalReason::VersionMismatch);
}

#[test]
fn negotiated_capabilities_are_those_both_sides_offer() {
    let hello = hello_from_wire(
        json!({"protocol_versions": [1], "capabilities": ["change-feed", "teleport"]}),
    );

    let HandshakeReply::Accepted(welcome) = negotiate(&hello, &[Capability::ChangeFeed]) else {
        panic!("version 1 is shared");
    };

    assert_eq!(welcome.capabilities, vec![Capability::ChangeFeed]);
}

#[test]
fn a_capability_the_daemon_does_not_offer_is_not_granted() {
    let hello = hello_from_wire(json!({"protocol_versions": [1], "capabilities": ["change-feed"]}));

    let HandshakeReply::Accepted(welcome) = negotiate(&hello, &[]) else {
        panic!("version 1 is shared");
    };

    assert_eq!(welcome.capabilities, Vec::<Capability>::new());
}

#[test]
fn a_capability_listed_twice_is_granted_once() {
    let hello = hello_from_wire(
        json!({"protocol_versions": [1], "capabilities": ["change-feed", "change-feed"]}),
    );

    let HandshakeReply::Accepted(welcome) = negotiate(&hello, &[Capability::ChangeFeed]) else {
        panic!("version 1 is shared");
    };

    assert_eq!(welcome.capabilities, vec![Capability::ChangeFeed]);
}

#[test]
fn an_unknown_capability_is_preserved_through_a_round_trip() {
    let wire = json!({"protocol_versions": [1], "capabilities": ["teleport"]});

    let hello = hello_from_wire(wire.clone());

    assert_eq!(
        hello.capabilities,
        vec![Capability::Unknown("teleport".into())]
    );
    assert_eq!(serde_json::to_value(hello).unwrap(), wire);
}

#[test]
fn unknown_hello_fields_from_a_newer_client_are_ignored() {
    let hello = hello_from_wire(json!({
        "protocol_versions": [1],
        "capabilities": [],
        "client": {"name": "cockpit-tui", "version": "9.0.0"}
    }));

    assert!(matches!(
        negotiate(&hello, &[]),
        HandshakeReply::Accepted(_)
    ));
}

#[test]
fn a_hello_with_a_non_numeric_version_is_malformed() {
    let parsed =
        serde_json::from_value::<Hello>(json!({"protocol_versions": ["one"], "capabilities": []}));

    assert!(parsed.is_err());
}

#[test]
fn an_unknown_refusal_reason_from_a_newer_daemon_is_preserved() {
    let wire = json!({
        "outcome": "refused",
        "reason": "owner-only",
        "message": "this daemon serves another user",
        "supported_versions": [4]
    });

    let reply: HandshakeReply = serde_json::from_value(wire.clone()).unwrap();

    let HandshakeReply::Refused(refusal) = &reply else {
        panic!("a refusal");
    };
    assert_eq!(refusal.reason, RefusalReason::Unknown("owner-only".into()));
    assert_eq!(serde_json::to_value(reply).unwrap(), wire);
}
