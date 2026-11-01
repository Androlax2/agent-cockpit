//! The wire form of the workflow commands and views (#39).
use cockpit_protocol::{
    Command, WorkflowBlockedActionView, WorkflowHandoffView, WorkflowNextView, WorkflowStatusView,
    WorkflowStepStatusView, decode_request,
};
use serde_json::json;

#[test]
fn a_workflow_complete_round_trips_with_only_the_fields_it_was_given() {
    let wire = json!({
        "id": 1,
        "command": {
            "kind": "workflow-complete",
            "launch_token": "token",
            "workflow": 4,
            "handoff": {
                "changed": "Rewrote the form validation",
                "checked": "commit abc123",
                "outcome": "12 passed"
            }
        }
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::WorkflowComplete {
            launch_token: Some("token".to_owned()),
            workflow: 4,
            handoff: WorkflowHandoffView {
                changed: "Rewrote the form validation".to_owned(),
                checked: "commit abc123".to_owned(),
                outcome: "12 passed".to_owned(),
                needs_human: String::new(),
                evidence: Vec::new(),
            },
            waive: Vec::new(),
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn a_requirement_a_completion_claims_passed_never_reaches_the_daemon() {
    let wire = json!({
        "id": 2,
        "command": {
            "kind": "workflow-complete",
            "workflow": 4,
            "handoff": {"changed": "a", "checked": "b", "outcome": "c"},
            "satisfied": ["checks"]
        }
    });

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        serde_json::to_value(request.command)
            .unwrap()
            .get("satisfied"),
        None
    );
}

#[test]
fn a_workflow_start_names_its_preset_only_when_the_owner_chose_one() {
    let wire = json!({"id": 3, "command": {"kind": "workflow-start", "item": 7}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(
        request.command,
        Command::WorkflowStart {
            item: 7,
            preset: None
        }
    );
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn the_next_action_travels_tagged_by_kind() {
    let next = WorkflowNextView::InProgress {
        position: 2,
        session: Some("s0123456789ab".to_owned()),
    };

    let wire = serde_json::to_value(&next).unwrap();

    assert_eq!(
        wire,
        json!({"kind": "in-progress", "position": 2, "session": "s0123456789ab"})
    );
    assert_eq!(
        serde_json::from_value::<WorkflowNextView>(wire).unwrap(),
        next
    );
}

#[test]
fn a_status_action_or_next_this_version_does_not_know_reads_as_unknown() {
    let status: WorkflowStatusView = serde_json::from_value(json!("archived")).unwrap();
    let step: WorkflowStepStatusView = serde_json::from_value(json!("paused")).unwrap();
    let action: WorkflowBlockedActionView = serde_json::from_value(json!("pay-invoice")).unwrap();
    let next: WorkflowNextView = serde_json::from_value(json!({"kind": "await-review"})).unwrap();

    assert_eq!(
        (status, step, action, next),
        (
            WorkflowStatusView::Unknown,
            WorkflowStepStatusView::Unknown,
            WorkflowBlockedActionView::Unknown,
            WorkflowNextView::Unknown
        )
    );
}

#[test]
fn a_blocked_action_parses_only_the_known_names() {
    assert_eq!(
        WorkflowBlockedActionView::parse("approve-operation"),
        Some(WorkflowBlockedActionView::ApproveOperation)
    );
    assert_eq!(WorkflowBlockedActionView::parse("unknown"), None);
}
