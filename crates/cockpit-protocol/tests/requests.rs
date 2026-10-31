use cockpit_protocol::{
    Command, CommitRevision, Cursor, DatabaseEpoch, ErrorCode, FeedPosition, Outcome,
    ProtocolError, Reply, Request, RequestId, Response, ServerFrame, decode_request,
};
use serde_json::json;

#[test]
fn a_request_round_trips_through_its_wire_form() {
    let wire = json!({
        "id": 4,
        "command": {"kind": "subscribe", "from": {"cursor": {"epoch": "e1", "revision": 9}}}
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request,
        Request {
            id: RequestId(4),
            command: Command::Subscribe {
                from: FeedPosition::Cursor(Cursor {
                    epoch: DatabaseEpoch("e1".into()),
                    revision: CommitRevision(9),
                }),
            },
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn unknown_request_fields_from_a_newer_client_are_ignored() {
    let wire = json!({"id": 1, "deadline_ms": 500, "command": {"kind": "ping", "loud": true}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(request.command, Command::Ping);
}

#[test]
fn an_unknown_command_is_refused_with_its_kind_and_request_id() {
    let rejection = decode_request(r#"{"id": 2, "command": {"kind": "teleport"}}"#).unwrap_err();

    assert_eq!(rejection.id, Some(RequestId(2)));
    assert_eq!(rejection.error.code, ErrorCode::UnknownCommand);
    assert!(
        rejection.error.message.contains("`teleport`"),
        "{}",
        rejection.error.message
    );
}

#[test]
fn a_malformed_frame_is_invalid_and_keeps_any_readable_id() {
    let cases = [
        (
            r#"{"id": 3, "command": {"kind": "subscribe", "after": "yesterday"}}"#,
            Some(RequestId(3)),
        ),
        (r#"{"id": 5, "command": {"kind": 5}}"#, Some(RequestId(5))),
        (r#"{"id": "six", "command": {"kind": "ping"}}"#, None),
        (r#"{"command": {"kind": "ping"}}"#, None),
        ("ping\n", None),
    ];

    for (frame, readable_id) in cases {
        let rejection = decode_request(frame).unwrap_err();

        assert_eq!(rejection.error.code, ErrorCode::InvalidRequest, "{frame}");
        assert_eq!(rejection.id, readable_id, "{frame}");
    }
}

#[test]
fn the_unknown_command_placeholder_cannot_be_sent() {
    let request = Request {
        id: RequestId(1),
        command: Command::Unknown,
    };

    assert!(serde_json::to_value(request).is_err());
}

#[test]
fn a_rejection_with_an_id_answers_that_request() {
    let rejection = decode_request(r#"{"id": 2, "command": {"kind": "teleport"}}"#).unwrap_err();

    let wire = serde_json::to_value(rejection.into_frame()).unwrap();

    assert_eq!(wire["type"], "response");
    assert_eq!(wire["id"], 2);
    assert_eq!(wire["outcome"]["error"]["code"], "unknown-command");
}

#[test]
fn a_rejection_without_an_id_is_a_malformed_frame() {
    let rejection = decode_request("[]").unwrap_err();

    let wire = serde_json::to_value(rejection.into_frame()).unwrap();

    assert_eq!(wire["type"], "malformed-frame");
    assert_eq!(wire["code"], "invalid-request");
}

#[test]
fn a_successful_reply_round_trips_through_its_wire_form() {
    let wire = json!({
        "type": "response",
        "id": 7,
        "outcome": {"ok": {"kind": "subscribed", "cursor": {"epoch": "e1", "revision": 12}}}
    });

    let frame: ServerFrame = serde_json::from_value(wire.clone()).unwrap();

    assert_eq!(
        frame,
        ServerFrame::Response(Response {
            id: RequestId(7),
            outcome: Outcome::Ok(Reply::Subscribed {
                cursor: Cursor {
                    epoch: DatabaseEpoch("e1".into()),
                    revision: CommitRevision(12),
                },
            }),
        })
    );
    assert_eq!(serde_json::to_value(frame).unwrap(), wire);
}

#[test]
fn an_error_reply_carries_its_reason_code() {
    let wire = json!({
        "type": "response",
        "id": 8,
        "outcome": {"error": {"code": "resnapshot-required", "message": "cursor e0 is gone"}}
    });

    let frame: ServerFrame = serde_json::from_value(wire).unwrap();

    let ServerFrame::Response(Response {
        outcome: Outcome::Error(error),
        ..
    }) = frame
    else {
        panic!("an error response");
    };
    assert_eq!(error.code, ErrorCode::ResnapshotRequired);
}

#[test]
fn an_unknown_error_code_from_a_newer_daemon_is_preserved() {
    let wire = json!({"code": "quota-exhausted", "message": "try later"});

    let error: ProtocolError = serde_json::from_value(wire.clone()).unwrap();

    assert_eq!(error.code, ErrorCode::Unknown("quota-exhausted".into()));
    assert_eq!(serde_json::to_value(error).unwrap(), wire);
}

#[test]
fn every_known_error_code_has_its_published_name() {
    let published = [
        (ErrorCode::InvalidRequest, "invalid-request"),
        (ErrorCode::UnknownCommand, "unknown-command"),
        (ErrorCode::NotAuthorized, "not-authorized"),
        (ErrorCode::UnidentifiedPeer, "unidentified-peer"),
        (ErrorCode::InvalidArgument, "invalid-argument"),
        (ErrorCode::NotNegotiated, "not-negotiated"),
        (ErrorCode::Busy, "busy"),
        (ErrorCode::StaleRevision, "stale-revision"),
        (ErrorCode::ResnapshotRequired, "resnapshot-required"),
        (ErrorCode::Internal, "internal"),
    ];

    for (code, name) in published {
        assert_eq!(serde_json::to_value(&code).unwrap(), json!(name));
        assert_eq!(
            serde_json::from_value::<ErrorCode>(json!(name)).unwrap(),
            code
        );
    }
}

#[test]
fn a_rejection_does_not_echo_a_huge_input_back() {
    let huge = "x".repeat(100_000);
    for frame in [
        format!(r#"{{"id": 1, "command": {{"kind": "{huge}"}}}}"#),
        format!(r#"{{"id": 1, "command": {{"kind": "subscribe", "from": "{huge}"}}}}"#),
    ] {
        let rejection = decode_request(&frame).unwrap_err();

        assert!(
            rejection.error.message.len() < 400,
            "{}",
            &rejection.error.message[..80]
        );
    }
}
