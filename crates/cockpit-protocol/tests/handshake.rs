use cockpit_protocol::{
    Capability, HandshakeReply, Hello, ProtocolVersion, RefusalReason, SUPPORTED_VERSIONS,
    decode_hello, negotiate,
};
use serde_json::json;

fn hello_from_wire(wire: serde_json::Value) -> Hello {
    serde_json::from_value(wire).expect("a valid hello")
}

#[test]
fn a_client_speaking_the_daemon_version_is_accepted_at_that_version() {
    let hello = hello_from_wire(json!({"protocol_versions": [1], "capabilities": []}));

    let reply = negotiate(&hello, &SUPPORTED_VERSIONS, &[]);

    assert_eq!(
        serde_json::to_value(reply).unwrap(),
        json!({"outcome": "accepted", "protocol_version": 1, "capabilities": []})
    );
}

#[test]
fn the_highest_version_both_sides_speak_is_chosen() {
    let hello = hello_from_wire(json!({"protocol_versions": [1, 2, 7], "capabilities": []}));

    for daemon_versions in [
        [ProtocolVersion(1), ProtocolVersion(2), ProtocolVersion(3)],
        [ProtocolVersion(3), ProtocolVersion(2), ProtocolVersion(1)],
    ] {
        let HandshakeReply::Accepted(welcome) = negotiate(&hello, &daemon_versions, &[]) else {
            panic!("versions 1 and 2 are shared");
        };
        assert_eq!(
            welcome.protocol_version,
            ProtocolVersion(2),
            "{daemon_versions:?}"
        );
    }
}

#[test]
fn a_version_mismatch_is_a_typed_refusal_listing_the_daemon_versions() {
    let hello = hello_from_wire(json!({"protocol_versions": [2, 3], "capabilities": []}));

    let wire = serde_json::to_value(negotiate(&hello, &SUPPORTED_VERSIONS, &[])).unwrap();

    assert_eq!(wire["outcome"], "refused");
    assert_eq!(wire["reason"], "version-mismatch");
    assert_eq!(wire["supported_versions"], json!(SUPPORTED_VERSIONS));
    assert!(
        wire["message"].as_str().unwrap().contains("up to 3"),
        "{wire}"
    );
}

#[test]
fn a_hello_without_versions_is_refused_as_a_mismatch() {
    let hello = hello_from_wire(json!({"protocol_versions": [], "capabilities": []}));

    let HandshakeReply::Refused(refusal) = negotiate(&hello, &SUPPORTED_VERSIONS, &[]) else {
        panic!("no version is shared");
    };

    assert_eq!(refusal.reason, RefusalReason::VersionMismatch);
}

#[test]
fn only_capabilities_both_sides_offer_are_granted_once() {
    let change_feed = || Capability::ChangeFeed;
    let cases = [
        (
            json!(["change-feed", "teleport"]),
            vec![change_feed()],
            vec![change_feed()],
        ),
        (json!(["change-feed"]), vec![], vec![]),
        (
            json!(["change-feed", "change-feed"]),
            vec![change_feed()],
            vec![change_feed()],
        ),
        (json!([]), vec![change_feed()], vec![]),
    ];

    for (client_capabilities, daemon_capabilities, granted) in cases {
        let hello =
            hello_from_wire(json!({"protocol_versions": [1], "capabilities": client_capabilities}));

        let HandshakeReply::Accepted(welcome) =
            negotiate(&hello, &SUPPORTED_VERSIONS, &daemon_capabilities)
        else {
            panic!("version 1 is shared");
        };
        assert_eq!(
            welcome.capabilities, granted,
            "client {client_capabilities}, daemon {daemon_capabilities:?}"
        );
    }
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
        negotiate(&hello, &SUPPORTED_VERSIONS, &[]),
        HandshakeReply::Accepted(_)
    ));
}

#[test]
fn a_mismatch_message_does_not_grow_with_the_client_list() {
    let versions: Vec<u32> = (2..10_000).collect();
    let hello = hello_from_wire(json!({"protocol_versions": versions, "capabilities": []}));

    let HandshakeReply::Refused(refusal) = negotiate(&hello, &SUPPORTED_VERSIONS, &[]) else {
        panic!("no version is shared");
    };

    assert!(refusal.message.len() < 200, "{}", refusal.message);
}

#[test]
fn an_unreadable_first_frame_gets_a_malformed_hello_refusal() {
    for frame in [
        r#"{"protocol_versions": ["one"], "capabilities": []}"#,
        r#"{"protocol_versions": [4294967296]}"#,
        r#"{"capabilities": []}"#,
        "HELLO 1\n",
    ] {
        let refusal = decode_hello(frame, &SUPPORTED_VERSIONS).unwrap_err();

        let wire = serde_json::to_value(HandshakeReply::Refused(refusal)).unwrap();
        assert_eq!(wire["reason"], "malformed-hello", "{frame}");
        assert_eq!(
            wire["supported_versions"],
            json!(SUPPORTED_VERSIONS),
            "{frame}"
        );
    }
}

#[test]
fn a_hello_without_capabilities_asks_for_none() {
    let hello = decode_hello(r#"{"protocol_versions": [1]}"#, &SUPPORTED_VERSIONS).unwrap();

    assert_eq!(hello.capabilities, Vec::<Capability>::new());
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
