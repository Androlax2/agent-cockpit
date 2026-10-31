//! The wire form of the project and checkout commands (#28).
use cockpit_protocol::{
    CheckoutKindView, CheckoutView, Command, ErrorCode, KeptReason, Outcome, ProtocolError, Reply,
    TrustView, decode_request,
};
use serde_json::json;

#[test]
fn a_claim_round_trips_through_its_wire_form() {
    let wire = json!({
        "id": 7,
        "command": {"kind": "checkout-claim", "checkout": 3, "holder": "session-a"}
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::CheckoutClaim {
            checkout: 3,
            holder: "session-a".to_owned(),
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn a_checkout_view_names_its_kind_trust_and_occupant_on_the_wire() {
    let view = CheckoutView {
        id: 4,
        project: "app".to_owned(),
        kind: CheckoutKindView::Parallel {
            branch: "feature/x".to_owned(),
        },
        path: "/p/.parallel/app/feature-x".to_owned(),
        incarnation: 2,
        removing: false,
        occupant: Some("session-a".to_owned()),
        trust: TrustView::Inherited {
            from: "/p/app".to_owned(),
        },
    };

    let wire = serde_json::to_value(Reply::Checkout {
        checkout: Box::new(view),
    })
    .unwrap();

    assert_eq!(
        wire,
        json!({
            "kind": "checkout",
            "checkout": {
                "id": 4,
                "project": "app",
                "kind": {"kind": "parallel", "branch": "feature/x"},
                "path": "/p/.parallel/app/feature-x",
                "incarnation": 2,
                "removing": false,
                "occupant": "session-a",
                "trust": {"kind": "inherited", "from": "/p/app"}
            }
        })
    );
}

#[test]
fn a_kept_reason_is_tagged_by_kind() {
    let wire = serde_json::to_value(KeptReason::Occupied {
        holder: "session-a".to_owned(),
    })
    .unwrap();

    assert_eq!(wire, json!({"kind": "occupied", "holder": "session-a"}));
}

#[test]
fn the_project_error_codes_have_kebab_case_names() {
    for (code, name) in [
        (ErrorCode::NotFound, "not-found"),
        (ErrorCode::Conflict, "conflict"),
        (ErrorCode::Refused, "refused"),
    ] {
        let outcome = Outcome::Error(ProtocolError::new(code, ""));
        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            json!({"error": {"code": name, "message": ""}})
        );
    }
}

#[test]
fn an_error_codes_text_is_its_name_on_the_wire() {
    for code in [
        ErrorCode::InvalidRequest,
        ErrorCode::UnknownCommand,
        ErrorCode::NotAuthorized,
        ErrorCode::UnidentifiedPeer,
        ErrorCode::InvalidArgument,
        ErrorCode::NotNegotiated,
        ErrorCode::Busy,
        ErrorCode::NotFound,
        ErrorCode::Conflict,
        ErrorCode::Refused,
        ErrorCode::StaleRevision,
        ErrorCode::ResnapshotRequired,
        ErrorCode::Internal,
        ErrorCode::Unknown("from-a-newer-daemon".to_owned()),
    ] {
        assert_eq!(
            serde_json::to_value(&code).unwrap(),
            serde_json::Value::from(code.as_str()),
            "{code:?}"
        );
    }
}
