#[path = "common/chief.rs"]
mod fixtures;

use cockpit::chief::{self, ChiefAgentAction, ChiefHostAction, Evidence, WorkBrief};
use cockpit::teams::{AgentAction, HostRequest, Role};
use serde_json::json;

#[test]
fn agents_cannot_approve_work_or_claim_a_chief_team_role() {
    assert!(serde_json::from_value::<AgentAction>(json!({"action":"chief","request":{"action":"approve_work"}})).is_err());
    assert!(serde_json::from_value::<Role>(json!("chief")).is_err());
}

#[test]
fn chief_dispatch_round_trips_without_accepting_authority_fields() {
    let action = AgentAction::Chief { request: ChiefAgentAction::Dispatch { submission_key: "dispatch-1".into(),
        work: fixtures::work(), worker: fixtures::session().session_id } };
    let valid = serde_json::to_value(action).unwrap();
    assert_eq!(valid["request"]["action"], "dispatch");
    assert!(serde_json::from_value::<AgentAction>(valid.clone()).is_ok());
    for field in ["approved", "runtime", "epoch", "role", "evidence"] {
        let mut forged = valid.clone();
        forged["request"][field] = json!(true);
        assert!(serde_json::from_value::<AgentAction>(forged).is_err(), "accepted {field}");
    }
}

#[test]
fn coordination_requires_an_explicit_valid_submission_key_without_authority_fields() {
    let valid = json!({"action":"coordinate", "submission_key":"coordinate-1", "work":fixtures::work(),
        "recipient":fixtures::session().session_id, "body":"An approved dependency question"});
    let action = serde_json::from_value::<ChiefAgentAction>(valid.clone()).expect("explicit coordinate keys are part of the protocol");
    action.validate().unwrap();
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("submission_key");
    assert!(serde_json::from_value::<ChiefAgentAction>(missing).is_err());
    for key in ["", "has space", "unsafe\u{1b}", &"x".repeat(129)] {
        let mut invalid = valid.clone();
        invalid["submission_key"] = json!(key);
        assert!(serde_json::from_value::<ChiefAgentAction>(invalid).unwrap().validate().is_err(), "{key:?}");
    }
    for field in ["runtime", "role", "approval_hash"] {
        let mut forged = valid.clone();
        forged[field] = json!("forged");
        assert!(serde_json::from_value::<ChiefAgentAction>(forged).is_err());
    }
}

#[test]
fn host_approval_binds_the_complete_brief_and_hash() {
    let action = HostRequest::Chief { action: ChiefHostAction::ApproveWork { request_id: "approve-1".into(),
        brief: fixtures::brief(), approval_hash: fixtures::brief().approval_hash().unwrap(), expected_revision: 0 } };
    let valid = serde_json::to_value(action).unwrap();
    assert!(serde_json::from_value::<HostRequest>(valid.clone()).is_ok());
    for pointer in ["/action/brief/approved", "/action/brief/work/runtime", "/action/brief/gates/0/approved"] {
        let mut forged = valid.clone();
        // The parent exists; insert an unknown field without changing the valid siblings.
        let (parent, field) = pointer.rsplit_once('/').unwrap();
        forged.pointer_mut(parent).unwrap().as_object_mut().unwrap().insert(field.into(), json!(true));
        assert!(serde_json::from_value::<HostRequest>(forged).is_err());
    }
    let mut changed = fixtures::brief();
    changed.brief.push_str(" Also do something else.");
    assert_ne!(changed.approval_hash().unwrap(), fixtures::brief().approval_hash().unwrap());
}

#[test]
fn host_approval_validation_rejects_stale_hashes_and_wrong_expected_revisions() {
    let approved_brief = fixtures::brief();
    let approval_hash = approved_brief.approval_hash().unwrap();
    let mut changed_brief = approved_brief.clone();
    changed_brief.brief.push_str(" Also change the dispatcher behavior.");
    for (name, brief, expected_revision, is_valid) in [
        ("exact approval", approved_brief.clone(), 0, true),
        ("stale approval hash", changed_brief, 0, false),
        ("wrong expected revision", approved_brief.clone(), 1, false),
        ("negative expected revision", approved_brief, -1, false),
    ] {
        let action = ChiefHostAction::ApproveWork { request_id: "approve-1".into(), brief,
            approval_hash: approval_hash.clone(), expected_revision };
        assert_eq!(action.validate().is_ok(), is_valid, "{name}");
    }
}

#[test]
fn evidence_import_cannot_supply_a_result_or_path() {
    let valid = json!({"request":"chief","action":{"action":"import_evidence","request_id":"import-1",
        "work":fixtures::work(),"gate":"tests","source_id":"run-1"}});
    assert!(serde_json::from_value::<HostRequest>(valid.clone()).is_ok());
    for field in ["success", "result", "path", "commit"] {
        let mut forged = valid.clone();
        forged["action"][field] = json!("forged");
        assert!(serde_json::from_value::<HostRequest>(forged).is_err());
    }
}

#[test]
fn brief_validation_rejects_oversize_controls_and_invalid_revisions() {
    let mut brief = fixtures::brief();
    brief.brief = "é".repeat(4000);
    assert!(brief.validate().is_ok());
    brief.brief.push('x');
    assert!(brief.validate().is_err());
    for text in ["", "   ", "unsafe\u{1b}[2J"] {
        brief.brief = text.into();
        assert!(brief.validate().is_err());
    }
    brief = fixtures::brief();
    brief.work.revision = 0;
    assert!(brief.validate().is_err());
    let mut wire = serde_json::to_value(fixtures::brief()).unwrap();
    wire["work"]["chief"] = json!(true);
    assert!(serde_json::from_value::<WorkBrief>(wire).is_err());
}

#[test]
fn agent_payload_validation_enforces_body_blocker_and_cursor_limits() {
    for limit in [0, 51] {
        assert!(ChiefAgentAction::Context { after: 0, limit }.validate().is_err());
    }
    assert!(ChiefAgentAction::Context { after: -1, limit: 1 }.validate().is_err());
    assert!(ChiefAgentAction::Context { after: 0, limit: 50 }.validate().is_ok());
    assert!(ChiefAgentAction::Coordinate { submission_key: "coordinate-1".into(), work: fixtures::work(), recipient: fixtures::session().session_id,
        body: "x".repeat(4001) }.validate().is_err());
    for length in [1000, 1001] {
        let action = ChiefAgentAction::Decision { work: Some(fixtures::work()), key: "blocked-1".into(),
            question: "x".repeat(length), recommendation: "Wait for approval.".into() };
        assert_eq!(action.validate().is_ok(), length == 1000);
    }
}

#[test]
fn context_pages_reject_oversize_records_instead_of_truncating() {
    assert!(chief::context_page(&[json!({"brief":"x".repeat(65536)})], 50).is_err());
    let records = vec![json!({"brief":"é".repeat(4000)}); 50];
    let page = chief::context_page(&records, 50).unwrap();
    assert!(serde_json::to_vec(&page).unwrap().len() < 65536);
    assert!(page.as_array().unwrap().len() < 50);
    assert_eq!(page[0]["brief"].as_str().unwrap().chars().count(), 4000);
}

#[test]
fn structured_qa_results_are_revision_bound_not_free_text_findings() {
    let valid = json!({"action":"qa_result","result":{"submission_key":"qa-1","task":fixtures::work().task,
        "handoff_id":"handoff-1","verdict":"pass","tests":[],"findings":"No findings."}});
    assert!(serde_json::from_value::<AgentAction>(valid.clone()).is_ok());
    let mut forged = valid;
    forged["source"] = json!("host_run");
    assert!(serde_json::from_value::<AgentAction>(forged).is_err());
    assert!(serde_json::from_value::<Evidence>(json!({"id":"e1","work":fixtures::work(),"kind":"tests",
        "commit":"a".repeat(40),"source":"agent_report","result":{}})).unwrap().validate().is_err());
}
