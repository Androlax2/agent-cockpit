//! The wire form of the verification commands and views (#313).
use cockpit_protocol::{
    CheckOutcomeView, CheckStatusView, Command, GateCheckView, Reply, StaleReasonView,
    decode_request,
};
use serde_json::json;

#[test]
fn a_check_claim_carries_no_outcome_and_no_session() {
    let wire = json!({
        "id": 1,
        "command": {
            "kind": "check-claim",
            "launch_token": "token",
            "check": "test",
            "statement": "cargo test passed locally"
        }
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::CheckClaim {
            launch_token: Some("token".to_owned()),
            check: "test".to_owned(),
            statement: "cargo test passed locally".to_owned(),
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn a_payload_naming_an_outcome_is_not_a_receipt() {
    let wire = json!({
        "id": 2,
        "command": {
            "kind": "check-claim",
            "check": "test",
            "statement": "passed",
            "outcome": "passed"
        }
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert!(matches!(request.command, Command::CheckClaim { .. }));
    assert!(
        !serde_json::to_value(request)
            .unwrap()
            .to_string()
            .contains("outcome")
    );
}

#[test]
fn a_stale_check_names_its_reason_and_path() {
    let check = GateCheckView {
        name: "test".to_owned(),
        status: CheckStatusView::Stale {
            reason: StaleReasonView::ArtifactChanged {
                path: "src/lib.rs".to_owned(),
            },
        },
        receipt: Some(3),
        next_action: Some("run `test` again on the current result".to_owned()),
    };

    let wire = serde_json::to_value(&check).unwrap();

    assert_eq!(
        wire["status"],
        json!({"status": "stale", "reason": {"kind": "artifact-changed", "path": "src/lib.rs"}})
    );
    assert_eq!(
        serde_json::from_value::<GateCheckView>(wire).unwrap(),
        check
    );
}

#[test]
fn a_newer_status_or_outcome_reads_as_unknown() {
    let status: CheckStatusView =
        serde_json::from_value(json!({"status": "waived-by-plugin"})).unwrap();
    let outcome: CheckOutcomeView = serde_json::from_value(json!("flaky")).unwrap();

    assert_eq!(status, CheckStatusView::Unknown);
    assert_eq!(outcome, CheckOutcomeView::Unknown);
}

#[test]
fn a_run_not_ended_yet_has_no_receipt() {
    let reply: Reply =
        serde_json::from_value(json!({"kind": "check-receipt", "receipt": null})).unwrap();

    assert_eq!(reply, Reply::CheckReceipt { receipt: None });
}
