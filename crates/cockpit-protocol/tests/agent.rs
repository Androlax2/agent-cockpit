//! The agent channel's wire form (report 17): reports name no session, and refusals say what
//! would lift them.
use cockpit_protocol::{
    AgentReport, AgentState, Command, ErrorCode, Outcome, ProtocolError, Unblock, UnblockedBy,
    decode_request,
};
use serde_json::json;

#[test]
fn an_agent_report_round_trips_through_its_wire_form() {
    let wire = json!({
        "id": 1,
        "command": {
            "kind": "agent-report",
            "launch_token": "abc",
            "report": {"state": "waiting", "needs_input": "Start #32 now?"}
        }
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::AgentReport {
            launch_token: Some("abc".to_owned()),
            report: AgentReport {
                state: Some(AgentState::Waiting),
                needs_input: Some("Start #32 now?".to_owned()),
                ..AgentReport::default()
            },
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

/// A sender cannot name a session: a payload field claiming one is not part of the command.
#[test]
fn a_session_claimed_in_a_report_payload_is_not_read() {
    let wire = json!({
        "id": 1,
        "command": {"kind": "agent-report", "session": "someone-else", "report": {"session": "x"}}
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::AgentReport {
            launch_token: None,
            report: AgentReport::default(),
        }
    );
}

#[test]
fn an_unknown_agent_state_is_a_malformed_request() {
    let wire = json!({
        "id": 7,
        "command": {"kind": "agent-report", "report": {"state": "asleep"}}
    });

    let rejection = decode_request(&wire.to_string()).unwrap_err();

    assert_eq!(rejection.error.code, ErrorCode::InvalidRequest);
}

#[test]
fn a_refusal_carries_what_unblocks_it_and_who_can() {
    let refusal = Outcome::Error(ProtocolError {
        code: ErrorCode::NotAuthorized,
        message: "the session holds no change-state grant for this".to_owned(),
        unblock: Some(Unblock {
            action: "the owner grants change-state over session s1".to_owned(),
            by: UnblockedBy::Owner,
        }),
    });

    let wire = serde_json::to_value(&refusal).unwrap();

    assert_eq!(
        wire,
        json!({"error": {
            "code": "not-authorized",
            "message": "the session holds no change-state grant for this",
            "unblock": {"action": "the owner grants change-state over session s1", "by": "owner"}
        }})
    );
    assert_eq!(serde_json::from_value::<Outcome>(wire).unwrap(), refusal);
}

#[test]
fn an_error_without_an_unblock_leaves_the_field_out() {
    let error = ProtocolError::new(ErrorCode::Internal, "the daemon failed");

    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        json!({"code": "internal", "message": "the daemon failed"})
    );
}

#[test]
fn an_unblocking_party_from_a_newer_daemon_is_kept_as_sent() {
    let unblock: Unblock =
        serde_json::from_value(json!({"action": "wait", "by": "chief"})).unwrap();

    assert_eq!(unblock.by, UnblockedBy::Unknown("chief".to_owned()));
}

#[test]
fn each_agent_states_name_is_its_wire_form() {
    for state in AgentState::ALL {
        assert_eq!(
            serde_json::to_value(state).unwrap(),
            serde_json::Value::String(state.as_str().to_owned())
        );
    }
}
