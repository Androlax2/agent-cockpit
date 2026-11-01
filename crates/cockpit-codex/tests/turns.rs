//! How a failed turn's outcome is read from the app-server's `Turn`, and the report it
//! becomes; the shapes are 0.161.0's (measured with a refused API key).

use cockpit_codex::turns::{failure_outcome, outcome_arguments, outcome_report};
use cockpit_provider_api::{ConversationId, TurnOutcome};
use serde_json::{Value, json};

fn turn(status: &str, error_info: Value) -> Value {
    json!({
        "id": "01a1981d-0cd0-7fb0-82d7-6ebfd0b348d5",
        "status": status,
        "error": {"message": "unexpected status 401 Unauthorized", "codexErrorInfo": error_info},
    })
}

#[test]
fn a_failed_turn_fails_with_codexs_word_for_the_error() {
    assert_eq!(
        failure_outcome(&turn("failed", json!("unauthorized"))),
        Some(TurnOutcome::Failed {
            reason: Some("unauthorized".to_owned())
        })
    );
}

#[test]
fn an_error_kind_with_details_gives_its_name() {
    let info = json!({"responseStreamDisconnected": {"httpStatusCode": 401}});

    assert_eq!(
        failure_outcome(&turn("failed", info)),
        Some(TurnOutcome::Failed {
            reason: Some("responseStreamDisconnected".to_owned())
        })
    );
}

#[test]
fn a_failed_turn_without_an_error_kind_fails_without_a_reason() {
    assert_eq!(
        failure_outcome(&turn("failed", Value::Null)),
        Some(TurnOutcome::Failed { reason: None })
    );
}

#[test]
fn the_usage_limit_stops_the_turn_on_quota() {
    assert_eq!(
        failure_outcome(&turn("failed", json!("usageLimitExceeded"))),
        Some(TurnOutcome::QuotaStopped)
    );
}

#[test]
fn a_turn_that_did_not_fail_has_no_failure_outcome() {
    for status in ["completed", "interrupted", "inProgress"] {
        assert_eq!(
            failure_outcome(&turn(status, Value::Null)),
            None,
            "{status}"
        );
    }
}

#[test]
fn a_failed_turn_reports_waiting_with_its_outcome_and_thread() {
    let thread = ConversationId::new("01a1981e-525b-75f2-902e-1e2e6d337eea").unwrap();
    let outcome = TurnOutcome::Failed {
        reason: Some("unauthorized".to_owned()),
    };

    assert_eq!(
        outcome_report(&thread, &outcome),
        [
            "agent",
            "report",
            "--state",
            "waiting",
            "--outcome",
            "failed",
            "--outcome-reason",
            "unauthorized",
            "--conversation",
            "01a1981e-525b-75f2-902e-1e2e6d337eea"
        ]
    );
}

#[test]
fn a_reason_the_channel_would_refuse_is_left_out() {
    for reason in ["two words", "", &"x".repeat(81)] {
        let outcome = TurnOutcome::Failed {
            reason: Some(reason.to_owned()),
        };

        assert_eq!(
            outcome_arguments(&outcome),
            ["--outcome", "failed"],
            "{reason:?}"
        );
    }
}
