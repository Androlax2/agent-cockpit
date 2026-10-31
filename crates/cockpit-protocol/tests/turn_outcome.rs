//! A turn's outcome on the agent channel's wire (#183): tagged by kind, a reason only on a
//! failure, and absent from what older daemons send.
use cockpit_protocol::{
    AgentReport, Command, ErrorCode, SessionReport, TurnOutcome, decode_request,
};
use serde_json::json;

fn reported_outcome(outcome: serde_json::Value) -> serde_json::Value {
    json!({
        "id": 1,
        "command": {"kind": "agent-report", "report": {"outcome": outcome}}
    })
}

#[test]
fn each_outcome_round_trips_through_its_wire_form() {
    let cases = [
        (json!({"kind": "completed"}), TurnOutcome::Completed),
        (json!({"kind": "quota-stopped"}), TurnOutcome::QuotaStopped),
        (
            json!({"kind": "failed"}),
            TurnOutcome::Failed { reason: None },
        ),
        (
            json!({"kind": "failed", "reason": "server_error"}),
            TurnOutcome::Failed {
                reason: Some("server_error".to_owned()),
            },
        ),
    ];
    for (outcome_wire, outcome) in cases {
        let wire = reported_outcome(outcome_wire);

        let request = decode_request(&wire.to_string()).unwrap();

        assert_eq!(
            request.command,
            Command::AgentReport {
                launch_token: None,
                report: AgentReport {
                    outcome: Some(outcome),
                    ..AgentReport::default()
                },
            }
        );
        assert_eq!(serde_json::to_value(request).unwrap(), wire);
    }
}

#[test]
fn an_unknown_outcome_is_a_malformed_request() {
    let wire = reported_outcome(json!({"kind": "abandoned"}));

    let rejection = decode_request(&wire.to_string()).unwrap_err();

    assert_eq!(rejection.error.code, ErrorCode::InvalidRequest);
}

#[test]
fn a_session_report_from_an_older_daemon_has_no_outcome() {
    let wire = json!({
        "session": "s1",
        "state": "waiting",
        "label": null,
        "doing": null,
        "needs_input": null,
        "reported_at": 42
    });

    let report: SessionReport = serde_json::from_value(wire).unwrap();

    assert_eq!(report.outcome, None);
}
