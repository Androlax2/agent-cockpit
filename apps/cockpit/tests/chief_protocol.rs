use cockpit::chief::{self, ChiefAgentAction, ChiefHostAction};
use cockpit::teams::{AgentAction, Role};
use serde_json::json;

#[test]
fn agents_cannot_approve_work_or_claim_a_chief_team_role() {
    assert!(serde_json::from_value::<AgentAction>(json!({"action":"chief","request":{"action":"approve_work"}})).is_err());
    assert!(serde_json::from_value::<Role>(json!("chief")).is_err());
}

#[test]
fn removed_dispatch_actions_no_longer_parse() {
    let work = json!({"task":{"project":"path:/project-a","task_id":"aaaaaaaa"},"revision":1});
    for agent_action in [
        json!({"action":"propose_work","submission_key":"propose-1","proposal":{"task":{"project":"path:/project-a","task_id":"aaaaaaaa"},
            "brief":"Scope","role":"developer","prerequisites":[],"gates":[]}}),
        json!({"action":"dispatch","submission_key":"dispatch-1","work":work,"worker":"worker"}),
        json!({"action":"accept","assignment":"assignment-1"}),
        json!({"action":"coordinate","submission_key":"coordinate-1","work":work,"recipient":"worker","body":"Question"}),
        json!({"action":"reprioritize","submission_key":"order-1","ordered":[work]}),
    ] {
        assert!(serde_json::from_value::<ChiefAgentAction>(agent_action.clone()).is_err(), "{agent_action}");
    }
    let brief = json!({"work":work,"brief":"Scope","role":"developer","prerequisites":[],"gates":[]});
    let evidence = json!({"id":"proof","work":work,"kind":"tests","commit":"1".repeat(40),"source":"host_attestation","result":{}});
    for host_action in [
        json!({"action":"approve_work","request_id":"approve-1","brief":brief,"approval_hash":"hash","expected_revision":0}),
        json!({"action":"revoke_work","request_id":"revoke-1","work":work}),
        json!({"action":"attest_evidence","request_id":"attest-1","evidence":evidence}),
        json!({"action":"import_evidence","request_id":"import-1","work":work,"gate":"tests","source_id":"run-1"}),
        json!({"action":"bind_worker","request_id":"bind-1","session":{"session_id":"s","agent":"claude","project":"p","runtime":"r"},"role":"developer","expected_epoch":null}),
        json!({"action":"reconcile","request_id":"reconcile-1","assignment":"assignment-1","expected_revision":1,"evidence":{"kind":"handled","note":"done"}}),
    ] {
        assert!(serde_json::from_value::<ChiefHostAction>(host_action.clone()).is_err(), "{host_action}");
    }
}

#[test]
fn agent_payload_validation_enforces_body_blocker_and_cursor_limits() {
    for limit in [0, 51] {
        assert!(ChiefAgentAction::Context { after: 0, limit }.validate().is_err());
    }
    assert!(ChiefAgentAction::Context { after: -1, limit: 1 }.validate().is_err());
    assert!(ChiefAgentAction::Context { after: 0, limit: 50 }.validate().is_ok());
    assert!(ChiefAgentAction::Status { summary: "x".repeat(4001) }.validate().is_err());
    for length in [1000, 1001] {
        let action = ChiefAgentAction::Decision { key: "blocked-1".into(),
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
