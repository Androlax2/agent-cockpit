//! The open question's source on the wire (#182, decided in #200): a report sends a dialog
//! apart from the agent's own question and says when it follows a new prompt; the owner's
//! view says where the open question came from.
use cockpit_protocol::{
    AgentReport, AgentState, Command, QuestionSource, SessionReport, decode_request,
};
use serde_json::json;

#[test]
fn a_dialog_report_round_trips_through_its_wire_form() {
    let wire = json!({
        "id": 1,
        "command": {
            "kind": "agent-report",
            "report": {"state": "waiting", "dialog": "Allow Bash?"}
        }
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::AgentReport {
            launch_token: None,
            report: AgentReport {
                state: Some(AgentState::Waiting),
                dialog: Some("Allow Bash?".to_owned()),
                ..AgentReport::default()
            },
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn a_new_prompt_report_round_trips_through_its_wire_form() {
    let wire = json!({
        "id": 1,
        "command": {
            "kind": "agent-report",
            "report": {"state": "working", "is_new_prompt": true}
        }
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::AgentReport {
            launch_token: None,
            report: AgentReport {
                state: Some(AgentState::Working),
                is_new_prompt: true,
                ..AgentReport::default()
            },
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn a_session_report_names_its_questions_source() {
    let report = SessionReport {
        session: "s1".to_owned(),
        state: Some(AgentState::Waiting),
        label: None,
        doing: None,
        needs_input: Some("Allow Bash?".to_owned()),
        question_source: Some(QuestionSource::Dialog),
        outcome: None,
        reported_at: 42,
    };

    let wire = serde_json::to_value(&report).unwrap();

    assert_eq!(wire["question_source"], "dialog");
    assert_eq!(
        serde_json::from_value::<SessionReport>(wire).unwrap(),
        report
    );
}

#[test]
fn a_session_report_from_an_older_daemon_reads_without_a_source() {
    let wire = json!({
        "session": "s1", "state": "waiting", "label": null, "doing": null,
        "needs_input": "Ship it?", "reported_at": 42
    });

    let report: SessionReport = serde_json::from_value(wire).unwrap();

    assert_eq!(report.question_source, None);
}
