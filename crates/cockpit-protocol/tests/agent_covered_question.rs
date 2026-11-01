//! The agent's own question waiting under an open dialog, on the owner's wire (#250).
use cockpit_protocol::{AgentState, QuestionSource, SessionReport};
use serde_json::json;

#[test]
fn a_session_report_carries_the_question_a_dialog_covers() {
    let report = SessionReport {
        session: "s1".to_owned(),
        state: Some(AgentState::Waiting),
        label: None,
        doing: None,
        needs_input: Some("Allow Bash?".to_owned()),
        question_source: Some(QuestionSource::Dialog),
        covered_question: Some("Which database?".to_owned()),
        outcome: None,
        reported_at: 42,
    };

    let wire = serde_json::to_value(&report).unwrap();

    assert_eq!(wire["covered_question"], "Which database?");
    assert_eq!(
        serde_json::from_value::<SessionReport>(wire).unwrap(),
        report
    );
}

#[test]
fn a_session_report_from_an_older_daemon_reads_without_a_covered_question() {
    let wire = json!({
        "session": "s1", "state": "waiting", "label": null, "doing": null,
        "needs_input": "Allow Bash?", "question_source": "dialog", "reported_at": 42
    });

    let report: SessionReport = serde_json::from_value(wire).unwrap();

    assert_eq!(report.covered_question, None);
}
