use cockpit::chief::{store, ChiefAgentAction, ChiefHostAction, WorkBrief, WorkRef, WorkerState};
use cockpit::teams::{Role, TaskKey, VerifiedSession, TransportOutcome};
use rusqlite::Connection;
mod common;
use common::sandbox::Sandbox;

const NOW: i64 = 100_000;

#[test]
fn unknown_legacy_launch_keeps_ownership_until_exact_host_safe_stop() {
    let mut connection=Connection::open_in_memory().unwrap();store::migrate(&mut connection).unwrap();
    let task=brief("aaaaaaaa").work.task;
    let token=store::reserve_legacy(&mut connection,&task,None,NOW).unwrap();
    store::observe(&mut connection,&[],NOW+1).unwrap();
    let page=store::snapshot(&connection,&[],0,50).unwrap();
    assert!(page["records"].as_array().unwrap().iter().any(|record|record["kind"]=="legacy_launch" && record["id"]==token),"unknown launch must remain visibly reconcilable with Chief never enabled");
    let action:ChiefHostAction=serde_json::from_value(serde_json::json!({"action":"reconcile_launch","request_id":"stop-unknown","token":token,"expected_revision":1,"evidence":{"kind":"safe_stop","note":"Verified exact launch cannot execute","confirmed_stopped":true}})).unwrap();
    store::host(&mut connection,&action,&[],NOW+2).unwrap().result.unwrap();
    store::reserve_legacy(&mut connection,&task,None,NOW+3).unwrap();
}

#[test]
fn uncertain_coordination_has_exact_host_recovery_without_changing_handled_assignment() {
    for stopped in [false,true] {
        let (mut connection,mut live)=setup();
        let assigned=dispatch(&mut connection,&live,"recovery-owner","aaaaaaaa",'b').unwrap();
        let assignment=assigned["assignment"].as_str().unwrap().to_string();
        store::observe(&mut connection,&live,NOW).unwrap();
        let first=store::claim(&mut connection,&session('b'),"recovery-worker",NOW).unwrap().unwrap();
        store::finish_for(&mut connection,&session('b'),&first.id,&TransportOutcome::Received {native_id:"owner-native".into()},NOW).unwrap();
        store::agent(&mut connection,&session('b'),&ChiefAgentAction::Accept {assignment:assignment.clone()},&live,NOW).unwrap().result.unwrap();
        let action=ChiefAgentAction::Coordinate {submission_key:"recover-message".into(),work:brief("aaaaaaaa").work,recipient:session('b').session_id,body:"Clarify exact scope".into()};
        store::agent(&mut connection,&session('a'),&action,&live,NOW).unwrap().result.unwrap();
        let delivery=store::claim(&mut connection,&session('b'),"coordination-turn",NOW).unwrap().unwrap();
        store::finish_for(&mut connection,&session('b'),&delivery.id,&TransportOutcome::Uncertain {reason:"Lost receipt".into()},NOW).unwrap();
        for worker in &mut live {worker.observed_at_ms=NOW+1;}
        store::observe(&mut connection,&live,NOW+1).unwrap();
        let page=store::snapshot(&connection,&live,0,50).unwrap();
        assert_eq!(page["records"].as_array().unwrap().iter().filter(|record|record["kind"]=="decision" && record["decision_key"]=="delivery").count(),1);
        let evidence=if stopped {serde_json::json!({"kind":"safe_stop","note":"Exact worker stopped","confirmed_stopped":true})} else {serde_json::json!({"kind":"handled","note":"Exact message handled"})};
        let recover:ChiefHostAction=serde_json::from_value(serde_json::json!({"action":"reconcile_delivery","request_id":"recover","message":delivery.envelope["message_id"],"expected_revision":1,"evidence":evidence})).unwrap();
        store::host(&mut connection,&recover,&live,NOW+1).unwrap().result.unwrap();
        store::finish_for(&mut connection,&session('b'),&delivery.id,&TransportOutcome::Received {native_id:"late-native".into()},NOW+2).unwrap();
        assert_eq!(connection.query_row("SELECT state FROM chief_assignments WHERE id=?1",[&assignment],|row|row.get::<_,String>(0)).unwrap(),if stopped {"cancelled"} else {"accepted"});
        if !stopped {
            let next=ChiefAgentAction::Coordinate {submission_key:"next-message".into(),work:brief("aaaaaaaa").work,recipient:session('b').session_id,body:"Next clarification".into()};
            store::agent(&mut connection,&session('a'),&next,&live,NOW+2).unwrap().result.unwrap();
            assert!(store::claim(&mut connection,&session('b'),"next-coordination",NOW+2).unwrap().is_some());
        }
    }
}

#[test]
fn working_blocker_reports_reach_only_the_chief_and_identical_reports_coalesce() {
    let (mut connection,mut live)=setup();
    let assigned=dispatch(&mut connection,&live,"report-owner","aaaaaaaa",'b').unwrap();
    let assignment=assigned["assignment"].as_str().unwrap().to_string();
    store::observe(&mut connection,&live,NOW).unwrap();
    let delivery=store::claim(&mut connection,&session('b'),"report-worker",NOW).unwrap().unwrap();
    store::finish_for(&mut connection,&session('b'),&delivery.id,&TransportOutcome::Received {native_id:"report-native".into()},NOW).unwrap();
    store::agent(&mut connection,&session('b'),&ChiefAgentAction::Accept {assignment:assignment.clone()},&live,NOW).unwrap().result.unwrap();
    let report=|summary:&str|ChiefAgentAction::Status {assignment:Some(assignment.clone()),summary:summary.into()};
    store::agent(&mut connection,&session('b'),&report("Working"),&live,NOW).unwrap().result.unwrap();
    let after=connection.query_row("SELECT max(seq) FROM chief_events",[],|row|row.get::<_,i64>(0)).unwrap();
    for _ in 0..3 {store::agent(&mut connection,&session('b'),&report("Blocked on exact API clarification"),&live,NOW+1).unwrap().result.unwrap();}
    for worker in &mut live {worker.observed_at_ms=NOW+30_000;}
    store::observe(&mut connection,&live,NOW+30_000).unwrap();
    let wake=store::claim(&mut connection,&session('a'),"report-chief",NOW+30_000).unwrap().unwrap();
    store::finish_for(&mut connection,&session('a'),&wake.id,&TransportOutcome::Received {native_id:"report-chief-native".into()},NOW+30_000).unwrap();
    let context=ChiefAgentAction::Context {after,limit:50};
    let reply=store::agent(&mut connection,&session('a'),&context,&live,NOW+30_000).unwrap().result.unwrap();
    let reports:Vec<_>=reply["records"].as_array().unwrap().iter().filter(|record|record["kind"]=="worker_status").collect();
    assert_eq!(reports.len(),1);
    assert_eq!(reports[0]["summary"],"Blocked on exact API clarification");
    assert_eq!(reports[0]["source"],"assigned_worker_report_not_evidence");
    assert!(store::agent(&mut connection,&session('b'),&context,&live,NOW+30_000).unwrap().result.is_err());
    let snapshot=store::agent(&mut connection,&session('a'),&ChiefAgentAction::ContextSnapshot {after:0,limit:50,generation:None},&live,NOW+30_000).unwrap().result.unwrap();
    assert!(snapshot["records"].as_array().unwrap().iter().any(|record|record["kind"]=="worker_status" && record["summary"]=="Blocked on exact API clarification"));
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_attention",[],|row|row.get::<_,i64>(0)).unwrap(),0);
}

#[test]
fn transitive_revocation_reaches_a_b_c_mixed_running_and_pending_claims_but_not_d() {
    use cockpit::chief::{Evidence,GateKind,GateRequirement};
    let (mut connection,mut live) = setup();
    for letter in ['d','e'] {
        live.push(WorkerState {session:session(letter),role:Some(Role::Developer),ready:true,blocked_by:vec![],observed_at_ms:NOW});
        store::host(&mut connection,&ChiefHostAction::BindWorker {request_id:format!("chain-bind-{letter}"),session:session(letter),role:Role::Developer,expected_epoch:None},&live,NOW).unwrap().result.unwrap();
    }
    let mut foundation = brief("eeeeeeee");
    foundation.gates.push(GateRequirement {kind:GateKind::Tests,source:"host_attestation".into(),expected:serde_json::json!({"commit":"1".repeat(40)})});
    let mut middle = brief("ffffffff"); middle.prerequisites.push(foundation.work.clone()); middle.gates=foundation.gates.clone();
    let mut leaf = brief("cccccccc"); leaf.prerequisites.push(middle.work.clone());
    let independent = brief("dddddddd");
    for approved in [&foundation,&middle,&leaf,&independent] {
        store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:format!("chain-approve-{}",approved.work.task.task_id),approval_hash:approved.approval_hash().unwrap(),brief:approved.clone(),expected_revision:0},&live,NOW).unwrap().result.unwrap();
    }
    for (approved,worker) in [(&foundation,'b'),(&middle,'c')] {
        let assigned = dispatch(&mut connection,&live,&format!("chain-{}",worker),&approved.work.task.task_id,worker).unwrap();
        store::observe(&mut connection,&live,NOW).unwrap();
        let delivery = store::claim(&mut connection,&session(worker),&format!("chain-idle-{worker}"),NOW).unwrap().unwrap();
        store::finish_for(&mut connection,&session(worker),&delivery.id,&TransportOutcome::Received {native_id:format!("chain-native-{worker}")},NOW).unwrap();
        store::agent(&mut connection,&session(worker),&ChiefAgentAction::Accept {assignment:assigned["assignment"].as_str().unwrap().into()},&live,NOW).unwrap().result.unwrap();
        let evidence = Evidence {id:format!("chain-proof-{worker}"),work:approved.work.clone(),kind:GateKind::Tests,commit:"1".repeat(40),source:"host_attestation".into(),result:serde_json::json!({"passed":true,"note":"Explicit host fixture attestation"})};
        store::host(&mut connection,&ChiefHostAction::AttestEvidence {request_id:format!("chain-attest-{worker}"),evidence},&live,NOW+1).unwrap().result.unwrap();
    }
    dispatch(&mut connection,&live,"chain-leaf","cccccccc",'d').unwrap();
    dispatch(&mut connection,&live,"chain-independent","dddddddd",'e').unwrap();
    store::host(&mut connection,&ChiefHostAction::RevokeWork {request_id:"chain-revoke".into(),work:foundation.work},&live,NOW+2).unwrap().result.unwrap();
    let states = connection.prepare("SELECT task_id,state FROM chief_assignments ORDER BY task_id").unwrap().query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    assert_eq!(states,vec![("cccccccc".into(),"cancelled".into()),("dddddddd".into(),"queued".into()),("eeeeeeee".into(),"uncertain".into()),("ffffffff".into(),"uncertain".into())]);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_inbox WHERE json_extract(envelope,'$.kind')='host_stop_notice'",[],|row|row.get::<_,i64>(0)).unwrap(),2);
    assert_eq!(connection.query_row("SELECT count(*) FROM scheduling_reservations WHERE owner='chief'",[],|row|row.get::<_,i64>(0)).unwrap(),3);
    assert_eq!(connection.query_row("SELECT state FROM chief_work_revisions WHERE task_id='dddddddd'",[],|row|row.get::<_,String>(0)).unwrap(),"approved");
}

#[test]
fn coordination_backlog_is_bounded_across_many_admitted_turns() {
    let (mut connection,mut live) = setup();
    dispatch(&mut connection,&live,"queue-owner","aaaaaaaa",'b').unwrap();
    store::observe(&mut connection,&live,NOW).unwrap();
    let assignment = store::claim(&mut connection,&session('b'),"queue-worker",NOW).unwrap().unwrap();
    store::finish_for(&mut connection,&session('b'),&assignment.id,&TransportOutcome::Received {native_id:"queue-owner".into()},NOW).unwrap();
    let mut accepted = 0;
    for turn in 0..14 {
        let now = NOW+turn*30_000;
        if turn>0 {
            for worker in &mut live {worker.observed_at_ms=now;}
            let approved = brief(&format!("{turn:08x}"));
            store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:format!("queue-material-{turn}"),approval_hash:approved.approval_hash().unwrap(),brief:approved,expected_revision:0},&live,now).unwrap().result.unwrap();
            store::observe(&mut connection,&live,now).unwrap();
            let wake = store::claim(&mut connection,&session('a'),&format!("queue-chief-{turn}"),now).unwrap().unwrap();
            store::finish_for(&mut connection,&session('a'),&wake.id,&TransportOutcome::Received {native_id:format!("queue-native-{turn}")},now).unwrap();
        }
        for index in 0..if turn==0 {19} else {20} {
            let action = ChiefAgentAction::Coordinate {submission_key:format!("queue-{turn}-{index}"),work:brief("aaaaaaaa").work,recipient:session('b').session_id,body:"Bounded coordination only".into()};
            if let Err(error) = store::agent(&mut connection,&session('a'),&action,&live,now).unwrap().result {
                assert_eq!(accepted,256);
                assert_eq!(error.code,cockpit::teams::ErrorCode::Limit);
                assert!(error.message.contains("inbox"));
                return;
            }
            accepted+=1;
        }
    }
    panic!("unbounded pending coordination messages: {accepted}");
}

#[test]
fn revocation_cancels_unsent_but_retains_running_claims_and_delivers_host_stop_without_authority() {
    for stage in ["queued","not_submitted","submitting","uncertain","accepted"] {
        let (mut connection,live) = setup();
        let assigned = dispatch(&mut connection,&live,"revoke-dispatch","aaaaaaaa",'b').unwrap();
        let assignment = assigned["assignment"].as_str().unwrap();
        store::observe(&mut connection,&live,NOW).unwrap();
        let mut attempt = None;
        if stage != "queued" {
            let delivery = store::claim(&mut connection,&session('b'),"revoke-turn",NOW).unwrap().unwrap();
            if stage != "submitting" {
                let outcome = match stage {
                    "not_submitted" => TransportOutcome::NotSubmitted {reason:"Never sent".into()},
                    "uncertain" => TransportOutcome::Uncertain {reason:"Unknown native admission".into()},
                    _ => TransportOutcome::Received {native_id:"running-native".into()},
                };
                store::finish_for(&mut connection,&session('b'),&delivery.id,&outcome,NOW).unwrap();
                if stage == "accepted" {
                    store::agent(&mut connection,&session('b'),&ChiefAgentAction::Accept {assignment:assignment.into()},&live,NOW).unwrap().result.unwrap();
                }
            }
            attempt = Some(delivery);
        }
        let revoke = ChiefHostAction::RevokeWork {request_id:"revoke".into(),work:brief("aaaaaaaa").work};
        store::host(&mut connection,&revoke,&live,NOW+1).unwrap().result.unwrap();
        store::host(&mut connection,&revoke,&live,NOW+1).unwrap().result.unwrap();
        let unsent = matches!(stage,"queued"|"not_submitted");
        assert_eq!(connection.query_row("SELECT state FROM chief_work_revisions WHERE task_id='aaaaaaaa'",[],|row|row.get::<_,String>(0)).unwrap(),"revoked","{stage}");
        assert_eq!(connection.query_row("SELECT count(*) FROM scheduling_reservations",[],|row|row.get::<_,i64>(0)).unwrap(),i64::from(!unsent),"{stage}");
        assert_eq!(connection.query_row("SELECT count(*) FROM chief_inbox WHERE json_extract(envelope,'$.kind')='host_stop_notice'",[],|row|row.get::<_,i64>(0)).unwrap(),i64::from(!unsent),"{stage}");
        let review = store::snapshot(&connection,&live,0,50).unwrap();
        let record = review["records"].as_array().unwrap().iter().find(|record|record["kind"]=="assignment").unwrap();
        assert_eq!(record["id"],assignment);
        assert_eq!(record["worker_identity"],serde_json::json!(session('b')));
        assert!(record["worker_epoch"].is_string());
        assert!(record["chief_epoch"].is_string());
        assert!(record["revision"].as_i64().unwrap()>1);
        let epoch = connection.query_row("SELECT epoch FROM chief_grants",[],|row|row.get(0)).unwrap();
        store::host(&mut connection,&ChiefHostAction::Disable {request_id:"disable-old".into(),expected_epoch:epoch},&live,NOW+2).unwrap().result.unwrap();
        store::host(&mut connection,&ChiefHostAction::Enable {request_id:"replace".into(),session:session('a')},&live,NOW+2).unwrap().result.unwrap();
        if let Some(delivery) = attempt {
            let native_id = if stage == "accepted" {"running-native"} else {"late-native"};
            store::finish_for(&mut connection,&session('b'),&delivery.id,&TransportOutcome::Received {native_id:native_id.into()},NOW+3).unwrap();
        }
        let state = connection.query_row("SELECT state FROM chief_assignments WHERE id=?1",[assignment],|row|row.get::<_,String>(0)).unwrap();
        assert_eq!(state,if unsent {"cancelled"} else {"uncertain"},"{stage}");
        if !unsent {
            let stop = store::claim(&mut connection,&session('b'),"stop-turn",NOW+4).unwrap().expect(stage);
            assert_eq!(stop.envelope["kind"],"host_stop_notice");
            assert_eq!(stop.envelope["sender"]["role"],"host");
            assert_eq!(stop.envelope["recipient"],serde_json::json!(session('b')));
            assert_eq!(stop.envelope["work"],serde_json::json!(brief("aaaaaaaa").work));
            assert!(stop.envelope.get("approval").is_none());
            let text = cockpit::teams::envelope_text(&stop.envelope);
            assert!(text.contains("Host stop-at-safe-boundary notice"));
            assert!(text.contains(&assignment));
            assert!(!text.contains("cockpit-team send"));
            store::finish_for(&mut connection,&session('b'),&stop.id,&TransportOutcome::Received {native_id:"stop-native".into()},NOW+5).unwrap();
            assert_eq!(connection.query_row("SELECT state FROM chief_assignments WHERE id=?1",[assignment],|row|row.get::<_,String>(0)).unwrap(),"uncertain");
            assert!(store::agent(&mut connection,&session('b'),&ChiefAgentAction::Accept {assignment:assignment.into()},&live,NOW+6).unwrap().result.is_err());
        }
        assert!(dispatch(&mut connection,&live,"cannot-revive","aaaaaaaa",'c').is_err());
    }
}

#[test]
fn a_conclusive_late_negative_receipt_cancels_revoked_ownership_and_its_stop_notice() {
    let (mut connection,live) = setup();
    dispatch(&mut connection,&live,"revoke-late-negative","aaaaaaaa",'b').unwrap();
    store::observe(&mut connection,&live,NOW).unwrap();
    let delivery = store::claim(&mut connection,&session('b'),"revoke-turn",NOW).unwrap().unwrap();
    store::host(&mut connection,&ChiefHostAction::RevokeWork {request_id:"revoke".into(),work:brief("aaaaaaaa").work},&live,NOW+1).unwrap().result.unwrap();
    store::finish_for(&mut connection,&session('b'),&delivery.id,&TransportOutcome::NotSubmitted {reason:"Native request never left".into()},NOW+2).unwrap();
    assert_eq!(connection.query_row("SELECT state FROM chief_assignments",[],|row|row.get::<_,String>(0)).unwrap(),"cancelled");
    assert_eq!(connection.query_row("SELECT count(*) FROM scheduling_reservations",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    assert!(store::claim(&mut connection,&session('b'),"no-stop-needed",NOW+3).unwrap().is_none());
    store::finish_for(&mut connection,&session('b'),&delivery.id,&TransportOutcome::Received {native_id:"impossible-late-positive".into()},NOW+4).unwrap();
    assert_eq!(connection.query_row("SELECT state FROM chief_assignments",[],|row|row.get::<_,String>(0)).unwrap(),"cancelled");
}

#[test]
fn safe_stop_requires_explicit_host_confirmation_and_late_receipts_cannot_revive_it() {
    use cockpit::chief::ReconciliationEvidence;
    let (mut connection, live) = setup();
    let assigned = dispatch(&mut connection, &live, "old-key", "aaaaaaaa", 'b').unwrap();
    let assignment = assigned["assignment"].as_str().unwrap().to_string();
    store::observe(&mut connection, &live, NOW).unwrap();
    let delivery = store::claim(&mut connection, &session('b'), "old-turn", NOW).unwrap().unwrap();
    store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Uncertain { reason:"lost reply".into() }, NOW).unwrap();
    let revision: i64 = connection.query_row("SELECT revision FROM chief_assignments WHERE id=?1", [&assignment], |row|row.get(0)).unwrap();
    let action = |confirmed| ChiefHostAction::Reconcile { request_id:format!("stop-{confirmed}"), assignment:assignment.clone(), expected_revision:revision,
        evidence:ReconciliationEvidence::SafeStop { note:"Old native inbox removed and runtime stopped".into(), confirmed_stopped:confirmed } };
    assert!(store::host(&mut connection, &action(false), &live, NOW).unwrap().result.is_err());
    store::host(&mut connection, &action(true), &live, NOW).unwrap().result.unwrap();
    store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Received { native_id:"late".into() }, NOW).unwrap();
    assert_eq!(connection.query_row("SELECT state FROM chief_assignments WHERE id=?1", [&assignment], |row|row.get::<_,String>(0)).unwrap(), "cancelled");
    dispatch(&mut connection, &live, "old-key", "aaaaaaaa", 'b').unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_assignments", [], |row|row.get::<_,i64>(0)).unwrap(), 1);
    dispatch(&mut connection, &live, "fresh-key", "aaaaaaaa", 'b').unwrap();
}

#[test]
fn handled_reconciliation_preserves_the_claim_and_reservation() {
    let (mut connection, live) = setup();
    let assigned = dispatch(&mut connection, &live, "handled", "aaaaaaaa", 'b').unwrap();
    let assignment = assigned["assignment"].as_str().unwrap().to_string();
    store::observe(&mut connection, &live, NOW).unwrap();
    let delivery = store::claim(&mut connection, &session('b'), "old-turn", NOW).unwrap().unwrap();
    store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Uncertain { reason:"lost reply".into() }, NOW).unwrap();
    let revision = connection.query_row("SELECT revision FROM chief_assignments", [], |row|row.get(0)).unwrap();
    store::host(&mut connection, &ChiefHostAction::Reconcile { request_id:"handled-host".into(), assignment, expected_revision:revision,
        evidence:cockpit::chief::ReconciliationEvidence::Handled { note:"Confirmed received".into() } }, &live, NOW).unwrap().result.unwrap();
    assert!(dispatch(&mut connection, &live, "other-worker", "aaaaaaaa", 'c').is_err());
    assert_eq!(connection.query_row("SELECT count(*) FROM scheduling_reservations", [], |row|row.get::<_,i64>(0)).unwrap(), 1);
}

#[test]
fn gate_completion_keeps_worker_capacity_until_a_fresh_exact_safe_idle_observation() {
    use cockpit::chief::{Evidence,GateKind,GateRequirement};
    let (mut connection,live) = setup();
    let mut approved = brief("cccccccc");
    approved.gates.push(GateRequirement { kind:GateKind::Tests,source:"host_attestation".into(),expected:serde_json::json!({"commit":"1".repeat(40)}) });
    store::host(&mut connection,&ChiefHostAction::ApproveWork { request_id:"gated".into(),approval_hash:approved.approval_hash().unwrap(),brief:approved.clone(),expected_revision:0 },&live,NOW).unwrap().result.unwrap();
    let action = ChiefAgentAction::Dispatch { submission_key:"gated-dispatch".into(),work:approved.work.clone(),worker:session('b').session_id };
    store::agent(&mut connection,&session('a'),&action,&live,NOW).unwrap().result.unwrap();
    store::observe(&mut connection,&live,NOW).unwrap();
    let delivery = store::claim(&mut connection,&session('b'),"gated-turn",NOW).unwrap().unwrap();
    store::finish_for(&mut connection,&session('b'),&delivery.id,&TransportOutcome::Received {native_id:"gated-native".into()},NOW).unwrap();
    let evidence = Evidence {id:"gated-tests".into(),work:approved.work,kind:GateKind::Tests,commit:"1".repeat(40),source:"host_attestation".into(),result:serde_json::json!({"passed":true,"note":"Verified"})};
    store::host(&mut connection,&ChiefHostAction::AttestEvidence {request_id:"gated-attest".into(),evidence},&live,NOW+1).unwrap().result.unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM scheduling_reservations",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    store::observe(&mut connection,&live,NOW+2).unwrap();
    assert!(dispatch(&mut connection,&live,"still-reserved","bbbbbbbb",'b').is_err());
    let fresh = live.into_iter().map(|mut worker|{worker.observed_at_ms=NOW+3;worker}).collect::<Vec<_>>();
    store::observe(&mut connection,&fresh,NOW+3).unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM scheduling_reservations",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    let projection = store::owned_work(&connection).unwrap();
    assert!(projection.iter().any(|work|work["task_id"]=="cccccccc" && work["status"]=="done"));
}

#[test]
fn invalidating_evidence_cancels_unsent_dependents_but_retains_running_claims_and_events() {
    use cockpit::chief::{Evidence,GateKind,GateRequirement};
    for stage in ["queued","received","inflight"] {
        let started = stage != "queued";
        let (mut connection,live) = setup();
        let mut prerequisite = brief("cccccccc");
        prerequisite.gates.push(GateRequirement {kind:GateKind::Tests,source:"host_attestation".into(),expected:serde_json::json!({"commit":"1".repeat(40)})});
        store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"prerequisite".into(),approval_hash:prerequisite.approval_hash().unwrap(),brief:prerequisite.clone(),expected_revision:0},&live,NOW).unwrap().result.unwrap();
        let mut result = Evidence {id:"passed".into(),work:prerequisite.work.clone(),kind:GateKind::Tests,commit:"1".repeat(40),source:"host_attestation".into(),result:serde_json::json!({"passed":true,"note":"Verified"})};
        store::host(&mut connection,&ChiefHostAction::AttestEvidence {request_id:"pass".into(),evidence:result.clone()},&live,NOW).unwrap().result.unwrap();
        let mut dependent = brief("dddddddd");
        dependent.prerequisites.push(prerequisite.work);
        store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"dependent".into(),approval_hash:dependent.approval_hash().unwrap(),brief:dependent.clone(),expected_revision:0},&live,NOW).unwrap().result.unwrap();
        store::agent(&mut connection,&session('a'),&ChiefAgentAction::Dispatch {submission_key:"dependent-dispatch".into(),work:dependent.work,worker:session('b').session_id},&live,NOW).unwrap().result.unwrap();
        let mut inflight = None;
        if started {
            store::observe(&mut connection,&live,NOW).unwrap();
            let delivery = store::claim(&mut connection,&session('b'),"dependent-turn",NOW).unwrap().unwrap();
            if stage == "inflight" { inflight = Some(delivery); } else {
                store::finish_for(&mut connection,&session('b'),&delivery.id,&TransportOutcome::Received {native_id:"dependent-native".into()},NOW).unwrap();
            }
        }
        result.id = "failed".into(); result.result["passed"] = serde_json::json!(false);
        store::host(&mut connection,&ChiefHostAction::AttestEvidence {request_id:"invalidate".into(),evidence:result},&live,NOW+1).unwrap().result.unwrap();
        if let Some(delivery) = &inflight {
            store::finish_for(&mut connection,&session('b'),&delivery.id,&TransportOutcome::Received {native_id:"late-dependent-native".into()},NOW+2).unwrap();
            assert_eq!(connection.query_row("SELECT native_id FROM chief_delivery_attempts WHERE id=?1",[&delivery.id],|row|row.get::<_,String>(0)).unwrap(),"late-dependent-native");
        }
        assert_eq!(connection.query_row("SELECT state FROM chief_assignments WHERE task_id='dddddddd'",[],|row|row.get::<_,String>(0)).unwrap(),if started {"uncertain"} else {"cancelled"});
        assert_eq!(connection.query_row("SELECT count(*) FROM scheduling_reservations",[],|row|row.get::<_,i64>(0)).unwrap(),i64::from(started));
        assert_eq!(connection.query_row("SELECT count(*) FROM chief_events WHERE kind='prerequisite_invalidated'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
        if let Some(delivery) = inflight {
            let (assignment,revision) = connection.query_row("SELECT id,revision FROM chief_assignments WHERE task_id='dddddddd'",[],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?))).unwrap();
            store::host(&mut connection,&ChiefHostAction::Reconcile {request_id:"stop-invalidated".into(),assignment,expected_revision:revision,
                evidence:cockpit::chief::ReconciliationEvidence::SafeStop {note:"Host confirmed the old runtime and pending inbox stopped".into(),confirmed_stopped:true}},&live,NOW+3).unwrap().result.unwrap();
            // A terminal assignment must also guard legacy/inconsistent attempt state.
            connection.execute("UPDATE chief_delivery_attempts SET state='submitting' WHERE id=?1",[&delivery.id]).unwrap();
            store::finish_for(&mut connection,&session('b'),&delivery.id,&TransportOutcome::Received {native_id:"late-dependent-native".into()},NOW+4).unwrap();
            assert_eq!(connection.query_row("SELECT state FROM chief_assignments WHERE task_id='dddddddd'",[],|row|row.get::<_,String>(0)).unwrap(),"cancelled");
            assert_eq!(connection.query_row("SELECT state FROM chief_delivery_attempts WHERE id=?1",[&delivery.id],|row|row.get::<_,String>(0)).unwrap(),"cancelled");
            assert_eq!(connection.query_row("SELECT count(*) FROM scheduling_reservations",[],|row|row.get::<_,i64>(0)).unwrap(),0);
        }
    }
}

#[test]
fn safe_stop_tombstones_same_work_and_dependency_coordination_for_only_the_stopped_recipient() {
    for kind in ["same","dependency"] {
        for stage in ["queued","submitting","uncertain"] {
            let (mut connection,live) = setup();
            let assigned = dispatch(&mut connection,&live,"stopped-assignment","aaaaaaaa",'b').unwrap();
            dispatch(&mut connection,&live,"unrelated-assignment","bbbbbbbb",'c').unwrap();
            store::observe(&mut connection,&live,NOW).unwrap();
            for worker in ['b','c'] {
                let delivery = store::claim(&mut connection,&session(worker),"assignment-turn",NOW).unwrap().unwrap();
                store::finish_for(&mut connection,&session(worker),&delivery.id,&TransportOutcome::Received {native_id:format!("assignment-{worker}")},NOW).unwrap();
            }
            let mut dependent = brief("cccccccc");
            dependent.prerequisites = vec![brief("aaaaaaaa").work,brief("bbbbbbbb").work];
            store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"coordination-work".into(),approval_hash:dependent.approval_hash().unwrap(),brief:dependent.clone(),expected_revision:0},&live,NOW).unwrap().result.unwrap();
            let mut old_messages = Vec::new();
            for message_kind in [kind,if kind=="same" {"dependency"} else {"same"}] {
                let work = if message_kind=="same" {brief("aaaaaaaa").work} else {dependent.work.clone()};
                let message = store::agent(&mut connection,&session('a'),&ChiefAgentAction::Coordinate {submission_key:format!("old-{message_kind}"),work,recipient:session('b').session_id,body:"Old coordination".into()},&live,NOW).unwrap().result.unwrap();
                old_messages.push(message["message_id"].as_str().unwrap().to_string());
            }
            let unrelated = store::agent(&mut connection,&session('a'),&ChiefAgentAction::Coordinate {submission_key:"unrelated-coordination".into(),work:dependent.work,recipient:session('c').session_id,body:"Keep this coordination".into()},&live,NOW).unwrap().result.unwrap();
            let attempt = if stage!="queued" {
                let delivery = store::claim(&mut connection,&session('b'),"coordination-turn",NOW).unwrap().unwrap();
                assert_eq!(delivery.message_id,old_messages[0]);
                if stage=="uncertain" {
                    store::finish_for(&mut connection,&session('b'),&delivery.id,&TransportOutcome::Uncertain {reason:"Native admission unknown".into()},NOW).unwrap();
                }
                Some(delivery)
            } else {None};
            // Host observes the existing assignment as unresolved after coordination was queued.
            connection.execute("UPDATE chief_assignments SET state='uncertain',revision=revision+1 WHERE id=?1",[assigned["assignment"].as_str().unwrap()]).unwrap();
            let revision = connection.query_row("SELECT revision FROM chief_assignments WHERE id=?1",[assigned["assignment"].as_str().unwrap()],|row|row.get(0)).unwrap();
            store::host(&mut connection,&ChiefHostAction::Reconcile {request_id:"stop-coordination".into(),assignment:assigned["assignment"].as_str().unwrap().into(),expected_revision:revision,
                evidence:cockpit::chief::ReconciliationEvidence::SafeStop {note:"Host confirmed old worker and native inbox stopped".into(),confirmed_stopped:true}},&live,NOW+1).unwrap().result.unwrap();
            for message in &old_messages {
                assert_eq!(connection.query_row("SELECT state FROM chief_inbox WHERE id=?1",[message],|row|row.get::<_,String>(0)).unwrap(),"cancelled","{kind}/{stage}");
            }
            if let Some(delivery) = attempt {
                for outcome in [TransportOutcome::NotSubmitted {reason:"Late negative receipt".into()},TransportOutcome::Received {native_id:"late-coordination".into()}] {
                    store::finish_for(&mut connection,&session('b'),&delivery.id,&outcome,NOW+2).unwrap();
                }
                assert_eq!(connection.query_row("SELECT state FROM chief_delivery_attempts WHERE id=?1",[&delivery.id],|row|row.get::<_,String>(0)).unwrap(),"cancelled");
            }
            assert_eq!(connection.query_row("SELECT state FROM chief_inbox WHERE id=?1",[unrelated["message_id"].as_str().unwrap()],|row|row.get::<_,String>(0)).unwrap(),"queued");
            dispatch(&mut connection,&live,"fresh-after-stop","aaaaaaaa",'b').unwrap();
            let fresh = store::claim(&mut connection,&session('b'),"fresh-turn",NOW+3).unwrap().unwrap();
            assert_eq!(fresh.envelope["kind"],"chief_assignment");
            store::finish_for(&mut connection,&session('b'),&fresh.id,&TransportOutcome::Received {native_id:"fresh-native".into()},NOW+3).unwrap();
            assert!(store::claim(&mut connection,&session('b'),"after-fresh-turn",NOW+4).unwrap().is_none());
        }
    }
}

#[test]
fn disabling_a_chief_does_not_release_its_unresolved_scheduling_ownership() {
    let (mut connection, live) = setup();
    dispatch(&mut connection, &live, "reserved", "aaaaaaaa", 'b').unwrap();
    let epoch: String = connection.query_row("SELECT epoch FROM chief_grants", [], |row| row.get(0)).unwrap();
    store::host(&mut connection, &ChiefHostAction::Disable { request_id: "disable".into(), expected_epoch: epoch }, &live, NOW).unwrap().result.unwrap();
    assert!(store::scheduling_owned(&connection).unwrap());
}

#[test]
fn legacy_reservations_are_shared_by_sidebars_and_chief_dispatch() {
    let (mut connection, live) = setup();
    let task = brief("aaaaaaaa").work.task;
    assert!(store::reserve_legacy(&mut connection, &task, Some(&session('b').session_id), NOW).is_err());
    connection.execute("UPDATE chief_grants SET state='disabled'", []).unwrap();
    // Approved chief work never silently returns to the legacy queue, even without a claim.
    assert!(store::reserve_legacy(&mut connection, &task, None, NOW).is_err());
    let legacy = TaskKey { project: task.project, task_id: "cccccccc".into() };
    store::reserve_legacy(&mut connection, &legacy, Some(&session('b').session_id), NOW).unwrap();
    assert!(store::reserve_legacy(&mut connection, &legacy, Some(&session('c').session_id), NOW).is_err());
    store::host(&mut connection, &ChiefHostAction::Enable { request_id: "enable-again".into(), session: session('a') }, &live, NOW).unwrap().result.unwrap();
    assert!(dispatch(&mut connection, &live, "blocked-worker", "bbbbbbbb", 'b').is_err());
}

#[test]
fn existing_doing_todos_are_not_free_chief_tasks_or_workers() {
    let (mut connection, live) = setup();
    let mut todos = cockpit::todo::TodoList::default();
    todos.items.push(cockpit::todo::Item(serde_json::json!({"id":"aaaaaaaa", "text":"already started", "cwd":"/project-a",
        "status":"doing", "session":session('b').session_id}).as_object().unwrap().clone()));
    store::reconcile_todos(&mut connection, &todos, NOW).unwrap();
    assert!(dispatch(&mut connection, &live, "already-started", "aaaaaaaa", 'c').is_err());
    assert!(dispatch(&mut connection, &live, "busy-worker", "bbbbbbbb", 'b').is_err());
}

#[test]
fn projection_replays_a_claim_without_treating_todo_done_as_completion() {
    let (mut connection, live) = setup();
    let assigned = dispatch(&mut connection, &live, "claim-before-save", "aaaaaaaa", 'b').unwrap();
    let mut todos = cockpit::todo::TodoList::default();
    todos.items.push(cockpit::todo::Item(serde_json::json!({"id":"aaaaaaaa", "text":"already started", "cwd":"/project-a",
        "status":"done"}).as_object().unwrap().clone()));
    store::reconcile_todos(&mut connection, &todos, NOW).unwrap();
    let projection = store::owned_work(&connection).unwrap();
    assert_eq!(projection[0]["assignment"], assigned["assignment"]);
    assert_eq!(projection[0]["worker"], session('b').session_id);
    assert_eq!(projection[0]["status"], "doing");
    store::recover(&mut connection, NOW).unwrap();
    assert_eq!(store::owned_work(&connection).unwrap(), projection);
    assert!(dispatch(&mut connection, &live, "duplicate-after-repair", "aaaaaaaa", 'c').is_err());
}

#[test]
fn two_legacy_clients_cannot_reserve_the_same_task() {
    use std::sync::{Arc, Barrier};
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("legacy.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    store::migrate(&mut connection).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let threads = (0..2).map(|_| {
        let path = path.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            let mut connection = Connection::open(path).unwrap();
            connection.busy_timeout(std::time::Duration::from_secs(3)).unwrap();
            barrier.wait();
            store::reserve_legacy(&mut connection, &brief("aaaaaaaa").work.task, None, NOW).is_ok()
        })
    }).collect::<Vec<_>>();
    assert_eq!(threads.into_iter().map(|thread| thread.join().unwrap()).filter(|reserved| *reserved).count(), 1);
}

#[test]
fn an_unknown_legacy_launch_reserves_its_project_until_the_worker_is_known() {
    let (mut connection, live) = setup();
    connection.execute("UPDATE chief_grants SET state='disabled'", []).unwrap();
    let pending = TaskKey { project:"path:/project-a".into(), task_id:"cccccccc".into() };
    store::reserve_legacy(&mut connection, &pending, None, NOW).unwrap();
    let second = TaskKey { project:pending.project.clone(), task_id:"dddddddd".into() };
    assert!(store::reserve_legacy(&mut connection, &second, None, NOW).is_err());
    store::host(&mut connection, &ChiefHostAction::Enable { request_id:"chief-after-launch".into(),session:session('a') }, &live,NOW).unwrap().result.unwrap();
    assert!(dispatch(&mut connection,&live,"unknown-launch-worker","aaaaaaaa",'b').is_err());
}

#[test]
fn upgrade_seeds_existing_chief_claims_before_any_legacy_reservation() {
    let (mut connection, live) = setup();
    dispatch(&mut connection, &live, "existing-claim", "aaaaaaaa", 'b').unwrap();
    connection.execute_batch("DROP TABLE scheduling_reservations; DROP TABLE chief_admission_verifications; PRAGMA user_version=5;").unwrap();
    store::migrate(&mut connection).unwrap();
    assert_eq!(connection.query_row("SELECT owner FROM scheduling_reservations WHERE task_id='aaaaaaaa'", [], |row| row.get::<_,String>(0)).unwrap(), "chief");
    assert!(store::reserve_legacy(&mut connection, &brief("aaaaaaaa").work.task, None, NOW).is_err());
}

#[test]
fn a_chief_issue_projection_is_not_adopted_as_a_second_legacy_assignment() {
    let (mut connection, live) = setup();
    let issue = brief("org/repo#1");
    store::host(&mut connection,&ChiefHostAction::ApproveWork { request_id:"approve-issue".into(),approval_hash:issue.approval_hash().unwrap(),
        brief:issue,expected_revision:0 },&live,NOW).unwrap().result.unwrap();
    dispatch(&mut connection,&live,"issue-assignment","org/repo#1",'b').unwrap();
    let mut todos = cockpit::todo::TodoList::default();
    todos.issues.insert("org/repo#1".into(),serde_json::json!({"status":"doing","session":session('b').session_id}));
    store::reconcile_todos(&mut connection,&todos,NOW).unwrap();
    assert!(!store::legacy_worker_busy(&connection,&session('b').session_id).unwrap());
    assert_eq!(connection.query_row("SELECT count(*) FROM scheduling_reservations WHERE task_id='org/repo#1'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
}

fn session(letter: char) -> VerifiedSession {
    if matches!(letter,'a'|'d') {
        return VerifiedSession {session_id:format!("ses_{letter}00000000000000000000000000"),agent:"opencode".into(),project:"path:/project-a".into(),runtime:format!("runtime-{letter}")};
    }
    VerifiedSession { session_id: format!("{0}{0}{0}{0}{0}{0}{0}{0}-{0}{0}{0}{0}-{0}{0}{0}{0}-{0}{0}{0}{0}-{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}{0}", letter),
        agent: "claude".into(), project: "path:/project-a".into(), runtime: format!("runtime-{letter}") }
}

fn brief(task: &str) -> WorkBrief {
    WorkBrief { work: WorkRef { task: TaskKey { project: "path:/project-a".into(), task_id: task.into() }, revision: 1 },
        brief: "Only approved work".into(), role: Role::Developer, prerequisites: vec![], gates: vec![] }
}

fn setup() -> (Connection, Vec<WorkerState>) {
    populate(Connection::open_in_memory().unwrap())
}

fn populate(mut connection: Connection) -> (Connection, Vec<WorkerState>) {
    store::migrate(&mut connection).unwrap();
    let live = ['a', 'b', 'c'].into_iter().map(|letter| WorkerState { session: session(letter), role: Some(Role::Developer), ready: true,
        blocked_by: vec![], observed_at_ms: NOW }).collect::<Vec<_>>();
    store::host(&mut connection, &ChiefHostAction::Enable { request_id: "enable".into(), session: session('a') }, &live, NOW).unwrap().result.unwrap();
    for letter in ['b', 'c'] {
        store::host(&mut connection, &ChiefHostAction::BindWorker { request_id: format!("bind-{letter}"), session: session(letter), role: Role::Developer,
            expected_epoch: None }, &live, NOW).unwrap().result.unwrap();
    }
    for task in ["aaaaaaaa", "bbbbbbbb"] {
        let brief = brief(task);
        store::host(&mut connection, &ChiefHostAction::ApproveWork { request_id: task.into(), approval_hash: brief.approval_hash().unwrap(),
            brief, expected_revision: 0 }, &live, NOW).unwrap().result.unwrap();
    }
    common::chief::admit(&mut connection,&live,&session('a'),NOW);
    (connection, live)
}

#[test]
fn simultaneous_claims_cannot_own_one_work_or_one_worker_twice() {
    use std::sync::{Arc, Barrier};
    for same_work in [true, false] {
        let sandbox = Sandbox::new();
        let path = sandbox.root.join("chief.sqlite");
        let (_, live) = populate(Connection::open(&path).unwrap());
        let barrier = Arc::new(Barrier::new(2));
        let threads = (0..2).map(|index| {
            let path = path.clone(); let live = live.clone(); let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut connection = Connection::open(path).unwrap();
                connection.busy_timeout(std::time::Duration::from_secs(3)).unwrap();
                barrier.wait();
                dispatch(&mut connection, &live, &format!("key-{index}"), if same_work || index == 0 { "aaaaaaaa" } else { "bbbbbbbb" },
                    if !same_work || index == 0 { 'b' } else { 'c' }).is_ok()
            })
        }).collect::<Vec<_>>();
        let successful = threads.into_iter().map(|thread| thread.join().unwrap()).filter(|success| *success).count();
        assert_eq!(successful, 1);
        let connection = Connection::open(path).unwrap();
        assert_eq!(connection.query_row("SELECT count(*) FROM chief_assignments", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    }
}

#[test]
fn reopening_before_claim_after_claim_and_after_receipt_never_resends_uncertain_work() {
    for stage in ["queued", "submitting", "received"] {
        let sandbox = Sandbox::new();
        let path = sandbox.root.join("chief.sqlite");
        let (mut connection, live) = populate(Connection::open(&path).unwrap());
        let assigned = dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap();
        store::observe(&mut connection, &live, NOW).unwrap();
        if stage != "queued" {
            let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
            if stage == "received" {
                store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Received { native_id: "n1".into() }, NOW).unwrap();
            }
        }
        drop(connection);
        let mut connection = Connection::open(path).unwrap();
        store::recover(&mut connection, NOW).unwrap();
        assert!(store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().is_none(), "restart invalidates readiness");
        store::observe(&mut connection, &live, NOW).unwrap();
        assert_eq!(dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap(), assigned);
        assert_eq!(store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().is_some(), stage == "queued");
        assert!(dispatch(&mut connection, &live, "replacement", "aaaaaaaa", 'c').is_err());
        if stage == "received" {
            store::agent(&mut connection, &session('b'), &ChiefAgentAction::Accept { assignment: assigned["assignment"].as_str().unwrap().into() }, &live, NOW).unwrap().result.unwrap();
        }
    }
}

#[test]
fn cross_project_coordination_is_bound_to_an_exact_approved_dependency() {
    let (mut connection, mut live) = setup();
    dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap();
    store::observe(&mut connection, &live, NOW).unwrap();
    let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
    store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Received { native_id: "n1".into() }, NOW).unwrap();
    let mut dependent = brief("cccccccc");
    dependent.work.task.project = "path:/project-b".into();
    dependent.prerequisites.push(brief("aaaaaaaa").work);
    store::host(&mut connection, &ChiefHostAction::ApproveWork { request_id: "dependent".into(), approval_hash: dependent.approval_hash().unwrap(),
        brief: dependent.clone(), expected_revision: 0 }, &live, NOW).unwrap().result.unwrap();
    let action = ChiefAgentAction::Coordinate { submission_key: "dependency-question".into(), work: dependent.work, recipient: session('b').session_id, body: "Dependency question (not authority)".into() };
    let reply = store::agent(&mut connection, &session('a'), &action, &live, NOW).unwrap().result.unwrap();
    assert_eq!(store::agent(&mut connection, &session('a'), &action, &live, NOW).unwrap().result.unwrap(), reply);
    assert!(store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().is_none());
    live[1].ready = false;
    store::observe(&mut connection, &live, NOW).unwrap();
    assert!(store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().is_none());
    live[1].ready = true;
    store::observe(&mut connection, &live, NOW).unwrap();
    let coordinate = store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().unwrap();
    assert_eq!(coordinate.envelope["body"], "Dependency question (not authority)");
    for recipient in [session('c').session_id, "unknown-session".into()] {
        let action = ChiefAgentAction::Coordinate { submission_key: "probe".into(), work: brief("bbbbbbbb").work, recipient, body: "probe".into() };
        assert_eq!(store::agent(&mut connection, &session('a'), &action, &live, NOW).unwrap().result.unwrap_err().code, cockpit::teams::ErrorCode::Unauthorized);
    }
}

#[test]
fn coordinate_keys_distinguish_replays_conflicts_and_intentional_repeated_content() {
    let (mut connection, live) = setup();
    dispatch(&mut connection, &live, "assign", "aaaaaaaa", 'b').unwrap();
    store::observe(&mut connection, &live, NOW).unwrap();
    let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
    store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Received { native_id: "n1".into() }, NOW).unwrap();
    let mut wire = serde_json::json!({"action":"coordinate", "submission_key":"coordinate-1", "work":brief("aaaaaaaa").work,
        "recipient":session('b').session_id, "body":"Same intended content"});
    let action: ChiefAgentAction = serde_json::from_value(wire.clone()).unwrap();
    let first = store::agent(&mut connection, &session('a'), &action, &live, NOW).unwrap().result.unwrap();
    assert_eq!(store::agent(&mut connection, &session('a'), &action, &live, NOW).unwrap().result.unwrap(), first);
    wire["body"] = serde_json::json!("Changed content using the same key");
    let changed = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(store::agent(&mut connection, &session('a'), &changed, &live, NOW).unwrap().result.unwrap_err().code, cockpit::teams::ErrorCode::Conflict);
    wire["body"] = serde_json::json!("Same intended content");
    wire["submission_key"] = serde_json::json!("coordinate-2");
    let repeated = serde_json::from_value(wire).unwrap();
    let second = store::agent(&mut connection, &session('a'), &repeated, &live, NOW).unwrap().result.unwrap();
    assert_ne!(first["message_id"], second["message_id"]);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_coordinate'", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
}

#[test]
fn coordinate_keys_survive_reopen_and_are_scoped_to_the_verified_sender_runtime() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("chief.sqlite");
    let (mut connection, mut live) = populate(Connection::open(&path).unwrap());
    dispatch(&mut connection, &live, "assign", "aaaaaaaa", 'b').unwrap();
    store::observe(&mut connection, &live, NOW).unwrap();
    let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
    store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Received { native_id: "n1".into() }, NOW).unwrap();
    let action = ChiefAgentAction::Coordinate { submission_key: "coordinate-1".into(), work: brief("aaaaaaaa").work,
        recipient: session('b').session_id, body: "Same intended content".into() };
    let first = store::agent(&mut connection, &session('a'), &action, &live, NOW).unwrap().result.unwrap();
    drop(connection);
    let mut connection = Connection::open(path).unwrap();
    assert_eq!(store::agent(&mut connection, &session('a'), &action, &live, NOW).unwrap().result.unwrap(), first);
    let epoch: String = connection.query_row("SELECT epoch FROM chief_grants", [], |row| row.get(0)).unwrap();
    store::host(&mut connection, &ChiefHostAction::Disable { request_id: "disable".into(), expected_epoch: epoch }, &live, NOW).unwrap().result.unwrap();
    let mut replacement = session('a');
    replacement.runtime = "replacement-chief-runtime".into();
    live[0].session = replacement.clone();
    store::host(&mut connection, &ChiefHostAction::Enable { request_id: "replacement".into(), session: replacement.clone() }, &live, NOW).unwrap().result.unwrap();
    common::chief::admit(&mut connection,&live,&replacement,NOW);
    assert!(store::agent(&mut connection, &session('a'), &action, &live, NOW).unwrap().result.is_err());
    let second = store::agent(&mut connection, &replacement, &action, &live, NOW).unwrap().result.unwrap();
    assert_ne!(second["message_id"], first["message_id"]);
}

#[test]
fn replacing_a_disabled_chief_cancels_unsent_coordination_without_starving_new_messages() {
    for reopen in [false, true] {
        let sandbox = Sandbox::new();
        let path = sandbox.root.join("chief.sqlite");
        let (mut connection, mut live) = populate(Connection::open(&path).unwrap());
        dispatch(&mut connection, &live, "assign", "aaaaaaaa", 'b').unwrap();
        store::observe(&mut connection, &live, NOW).unwrap();
        let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
        store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Received { native_id: "n1".into() }, NOW).unwrap();
        let old = ChiefAgentAction::Coordinate { submission_key: "old-note".into(), work: brief("aaaaaaaa").work,
            recipient: session('b').session_id, body: "Old chief's unsent note".into() };
        let old = store::agent(&mut connection, &session('a'), &old, &live, NOW).unwrap().result.unwrap();
        let epoch: String = connection.query_row("SELECT epoch FROM chief_grants", [], |row| row.get(0)).unwrap();
        store::host(&mut connection, &ChiefHostAction::Disable { request_id: "disable".into(), expected_epoch: epoch }, &live, NOW).unwrap().result.unwrap();
        assert_eq!(connection.query_row("SELECT state FROM chief_inbox WHERE id=?1", [old["message_id"].as_str().unwrap()], |row| row.get::<_, String>(0)).unwrap(), "cancelled");
        live.push(WorkerState { session: session('d'), role: None, ready: true, blocked_by: vec![], observed_at_ms: NOW });
        store::host(&mut connection, &ChiefHostAction::Enable { request_id: "replacement".into(), session: session('d') }, &live, NOW).unwrap().result.unwrap();
        common::chief::admit(&mut connection,&live,&session('d'),NOW);
        let fresh = ChiefAgentAction::Coordinate { submission_key: "fresh-note".into(), work: brief("aaaaaaaa").work,
            recipient: session('b').session_id, body: "Replacement chief's relevant note".into() };
        let fresh = store::agent(&mut connection, &session('d'), &fresh, &live, NOW).unwrap().result.unwrap();
        if reopen {
            drop(connection);
            connection = Connection::open(path).unwrap();
            store::recover(&mut connection, NOW).unwrap();
        }
        store::observe(&mut connection, &live, NOW).unwrap();
        let next = store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().expect("fresh coordination must not be starved");
        assert_eq!(next.message_id, fresh["message_id"].as_str().unwrap());
        assert_eq!(connection.query_row("SELECT state FROM chief_assignments", [], |row| row.get::<_, String>(0)).unwrap(), "received");
        assert!(dispatch(&mut connection, &live, "old-chief-retry", "aaaaaaaa", 'c').is_err());
    }
}

#[test]
fn disabling_a_chief_cancels_only_conclusively_unsent_inbox_entries_and_never_releases_ownership() {
    for stage in ["queued", "safe_retry", "submitting", "uncertain"] {
        let (mut connection, live) = setup();
        dispatch(&mut connection, &live, "assign", "aaaaaaaa", 'b').unwrap();
        store::observe(&mut connection, &live, NOW).unwrap();
        if stage != "queued" {
            let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
            match stage {
                "safe_retry" => store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::NotSubmitted { reason: "not admitted".into() }, NOW).unwrap(),
                "uncertain" => store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Uncertain { reason: "lost response".into() }, NOW).unwrap(),
                _ => {},
            }
        }
        let attempts: Vec<(String, String)> = connection.prepare("SELECT id,state FROM chief_delivery_attempts").unwrap().query_map([], |row| Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
        let epoch: String = connection.query_row("SELECT epoch FROM chief_grants", [], |row| row.get(0)).unwrap();
        store::host(&mut connection, &ChiefHostAction::Disable { request_id: "disable".into(), expected_epoch: epoch }, &live, NOW).unwrap().result.unwrap();
        let expected = if matches!(stage, "queued" | "safe_retry") { "cancelled" } else { stage };
        assert_eq!(connection.query_row("SELECT state FROM chief_inbox WHERE assignment IS NOT NULL", [], |row| row.get::<_, String>(0)).unwrap(), expected, "{stage}");
        let after: Vec<(String, String)> = connection.prepare("SELECT id,state FROM chief_delivery_attempts").unwrap().query_map([], |row| Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(after, attempts, "disable must not rewrite attempts");
        assert_eq!(connection.query_row("SELECT count(*) FROM chief_assignments WHERE state NOT IN ('completed','cancelled')", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
        store::host(&mut connection, &ChiefHostAction::Enable { request_id: "replacement".into(), session: session('a') }, &live, NOW).unwrap().result.unwrap();
        assert!(dispatch(&mut connection, &live, "reassign-task", "aaaaaaaa", 'c').is_err());
        assert!(dispatch(&mut connection, &live, "reassign-worker", "bbbbbbbb", 'b').is_err());
        assert!(store::claim(&mut connection, &session('b'), "idle-2", NOW + 2_000).unwrap().is_none());
    }
}

#[test]
fn revoked_coordination_work_does_not_block_later_relevant_messages() {
    for reopen in [false, true] {
        let sandbox = Sandbox::new();
        let path = sandbox.root.join("chief.sqlite");
        let (mut connection, live) = populate(Connection::open(&path).unwrap());
        dispatch(&mut connection, &live, "assign", "aaaaaaaa", 'b').unwrap();
        store::observe(&mut connection, &live, NOW).unwrap();
        let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
        store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Received { native_id: "n1".into() }, NOW).unwrap();
        let mut dependent = brief("cccccccc");
        dependent.prerequisites.push(brief("aaaaaaaa").work);
        store::host(&mut connection, &ChiefHostAction::ApproveWork { request_id: "dependent".into(), approval_hash: dependent.approval_hash().unwrap(), brief: dependent.clone(), expected_revision: 0 }, &live, NOW).unwrap().result.unwrap();
        let old = ChiefAgentAction::Coordinate { submission_key: "dependent-note".into(), work: dependent.work.clone(), recipient: session('b').session_id, body: "Unsent dependency question".into() };
        let old = store::agent(&mut connection, &session('a'), &old, &live, NOW).unwrap().result.unwrap();
        store::host(&mut connection, &ChiefHostAction::RevokeWork { request_id: "revoke".into(), work: dependent.work }, &live, NOW).unwrap().result.unwrap();
        let fresh = ChiefAgentAction::Coordinate { submission_key: "current-work-note".into(), work: brief("aaaaaaaa").work, recipient: session('b').session_id, body: "Still relevant".into() };
        let fresh = store::agent(&mut connection, &session('a'), &fresh, &live, NOW).unwrap().result.unwrap();
        if reopen { drop(connection); connection = Connection::open(path).unwrap(); store::recover(&mut connection, NOW).unwrap(); }
        store::observe(&mut connection, &live, NOW).unwrap();
        let next = store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().expect("revoked work must not starve valid coordination");
        assert_eq!(next.message_id, fresh["message_id"].as_str().unwrap());
        assert_eq!(connection.query_row("SELECT state FROM chief_inbox WHERE id=?1", [old["message_id"].as_str().unwrap()], |row| row.get::<_, String>(0)).unwrap(), "cancelled");
    }
}

#[test]
fn a_late_safe_negative_receipt_after_disable_cannot_starve_replacement_coordination() {
    for reopen in [false, true] {
        let sandbox = Sandbox::new();
        let path = sandbox.root.join("chief.sqlite");
        let (mut connection, live) = populate(Connection::open(&path).unwrap());
        dispatch(&mut connection, &live, "assign", "aaaaaaaa", 'b').unwrap();
        store::observe(&mut connection, &live, NOW).unwrap();
        let assignment = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
        store::finish_for(&mut connection, &session('b'), &assignment.id, &TransportOutcome::Received { native_id: "n1".into() }, NOW).unwrap();
        let old = ChiefAgentAction::Coordinate { submission_key: "old-note".into(), work: brief("aaaaaaaa").work, recipient: session('b').session_id, body: "Old note".into() };
        store::agent(&mut connection, &session('a'), &old, &live, NOW).unwrap().result.unwrap();
        let attempt = store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().unwrap();
        let epoch: String = connection.query_row("SELECT epoch FROM chief_grants", [], |row| row.get(0)).unwrap();
        store::host(&mut connection, &ChiefHostAction::Disable { request_id: "disable".into(), expected_epoch: epoch }, &live, NOW).unwrap().result.unwrap();
        assert_eq!(connection.query_row("SELECT state FROM chief_delivery_attempts WHERE id=?1", [&attempt.id], |row| row.get::<_, String>(0)).unwrap(), "submitting");
        store::finish_for(&mut connection, &session('b'), &attempt.id, &TransportOutcome::NotSubmitted { reason: "native admission did not occur".into() }, NOW).unwrap();
        store::host(&mut connection, &ChiefHostAction::Enable { request_id: "replacement".into(), session: session('a') }, &live, NOW).unwrap().result.unwrap();
        let fresh = ChiefAgentAction::Coordinate { submission_key: "fresh-note".into(), work: brief("aaaaaaaa").work, recipient: session('b').session_id, body: "Fresh note".into() };
        common::chief::admit(&mut connection,&live,&session('a'),NOW);
        let fresh = store::agent(&mut connection, &session('a'), &fresh, &live, NOW).unwrap().result.unwrap();
        if reopen { drop(connection); connection = Connection::open(path).unwrap(); store::recover(&mut connection, NOW).unwrap(); }
        store::observe(&mut connection, &live, NOW).unwrap();
        let next = store::claim(&mut connection, &session('b'), "idle-3", NOW + 2_000).unwrap().expect("a disabled grant's safe retry must not block a replacement");
        assert_eq!(next.message_id, fresh["message_id"].as_str().unwrap());
        assert_eq!(connection.query_row("SELECT state FROM chief_inbox WHERE id=?1", [&attempt.message_id], |row| row.get::<_, String>(0)).unwrap(), "cancelled");
        assert_eq!(connection.query_row("SELECT state FROM chief_delivery_attempts WHERE id=?1", [&attempt.id], |row| row.get::<_, String>(0)).unwrap(), "not_submitted");
    }
}

#[test]
fn temporary_pause_and_readiness_defer_unsent_coordination_without_cancelling_it() {
    let (mut connection, mut live) = setup();
    dispatch(&mut connection, &live, "assign", "aaaaaaaa", 'b').unwrap();
    store::observe(&mut connection, &live, NOW).unwrap();
    let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
    store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Received { native_id: "n1".into() }, NOW).unwrap();
    let action = ChiefAgentAction::Coordinate { submission_key: "note".into(), work: brief("aaaaaaaa").work, recipient: session('b').session_id, body: "Wait for readiness".into() };
    store::agent(&mut connection, &session('a'), &action, &live, NOW).unwrap().result.unwrap();
    let epoch: String = connection.query_row("SELECT epoch FROM chief_grants", [], |row| row.get(0)).unwrap();
    store::host(&mut connection, &ChiefHostAction::Pause { request_id: "pause".into(), expected_epoch: epoch.clone() }, &live, NOW).unwrap().result.unwrap();
    assert!(store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().is_none());
    store::host(&mut connection, &ChiefHostAction::Resume { request_id: "resume".into(), expected_epoch: epoch }, &live, NOW).unwrap().result.unwrap();
    live[1].ready = false;
    store::observe(&mut connection, &live, NOW).unwrap();
    assert!(store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().is_none());
    assert_eq!(connection.query_row("SELECT state FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_coordinate'", [], |row| row.get::<_, String>(0)).unwrap(), "queued");
    live[1].ready = true;
    store::observe(&mut connection, &live, NOW).unwrap();
    assert!(store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().is_some());
}

#[test]
fn unlinking_the_approved_team_binding_cancels_unsent_coordination_but_keeps_assignment_ownership() {
    use cockpit::teams::{self, HostAction, Member, Proposal};
    let (mut connection, live) = setup();
    let proposal = Proposal { submission_key: "team".into(), task: brief("aaaaaaaa").work.task,
        members: vec![Member { session_id: session('b').session_id, role: Role::Developer }], reason: "Approved task binding".into() };
    let sessions = live.iter().map(|worker| worker.session.clone()).collect::<Vec<_>>();
    let pending = teams::store::propose(&mut connection, &session('b'), &proposal, &sessions, NOW).unwrap().result.unwrap();
    teams::store::control(&mut connection, &HostAction::Approve { request_id: "approve-team".into(), proposal_id: pending["proposal_id"].as_str().unwrap().into(),
        expected_revision: 1, mapping_hash: pending["mapping_hash"].as_str().unwrap().into() }, &sessions, NOW).unwrap().result.unwrap();
    dispatch(&mut connection, &live, "assign", "aaaaaaaa", 'b').unwrap();
    store::observe(&mut connection, &live, NOW).unwrap();
    let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
    store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Received { native_id: "n1".into() }, NOW).unwrap();
    let action = ChiefAgentAction::Coordinate { submission_key: "team-note".into(), work: brief("aaaaaaaa").work, recipient: session('b').session_id, body: "Old team binding".into() };
    store::agent(&mut connection, &session('a'), &action, &live, NOW).unwrap().result.unwrap();
    teams::store::control(&mut connection, &HostAction::Unlink { request_id: "unlink".into(), task: proposal.task, session_id: session('b').session_id }, &sessions, NOW).unwrap().result.unwrap();
    assert!(store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().is_none());
    assert_eq!(connection.query_row("SELECT state FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_coordinate'", [], |row| row.get::<_, String>(0)).unwrap(), "cancelled");
    assert_eq!(connection.query_row("SELECT state FROM chief_assignments", [], |row| row.get::<_, String>(0)).unwrap(), "received");
}

#[test]
fn team_and_chief_inboxes_exclude_each_other_in_both_directions() {
    use cockpit::teams::{self, AgentAction, HostAction, Member, Proposal, Submission, MessageKind};
    for chief_first in [true, false] {
        let (mut connection, live) = setup();
        let proposal = Proposal { submission_key: "team".into(), task: brief("aaaaaaaa").work.task,
            members: vec![Member { session_id: session('b').session_id, role: Role::Developer }, Member { session_id: session('c').session_id, role: Role::Designer }], reason: "Approved team".into() };
        let sessions = live.iter().map(|worker| worker.session.clone()).collect::<Vec<_>>();
        let proposed = teams::store::propose(&mut connection, &session('b'), &proposal, &sessions, NOW).unwrap().result.unwrap();
        teams::store::control(&mut connection, &HostAction::Approve { request_id: "approve-team".into(), proposal_id: proposed["proposal_id"].as_str().unwrap().into(), expected_revision: 1,
            mapping_hash: proposed["mapping_hash"].as_str().unwrap().into() }, &sessions, NOW).unwrap().result.unwrap();
        dispatch(&mut connection, &live, "chief", "aaaaaaaa", 'b').unwrap();
        teams::store::accept(&mut connection, &session('c'), &AgentAction::Send { message: Submission { task: brief("aaaaaaaa").work.task, submission_key: "note".into(), recipient: session('b').session_id,
            kind: MessageKind::Notice, body: "collaborator notice".into(), thread_id: None, handoff_id: None } }, NOW).unwrap().result.unwrap();
        store::observe(&mut connection, &live, NOW).unwrap();
        if chief_first {
            connection.execute("UPDATE members SET last_idle_turn='legacy-turn' WHERE session_id=?1", [&session('b').session_id]).unwrap();
            assert!(store::claim(&mut connection, &session('b'), "legacy-turn", NOW).unwrap().is_none(), "migration preserves the team's consumed turn");
        }
        let first = if chief_first { store::claim(&mut connection, &session('b'), "idle-1", NOW) } else { teams::store::claim(&mut connection, &session('b'), "idle-1", NOW) }.unwrap().unwrap();
        let second = if chief_first { teams::store::claim(&mut connection, &session('b'), "idle-2", NOW) } else { store::claim(&mut connection, &session('b'), "idle-2", NOW) }.unwrap();
        assert!(second.is_none());
        if chief_first { store::finish_for(&mut connection, &session('b'), &first.id, &TransportOutcome::Received { native_id: "n1".into() }, NOW).unwrap(); }
        else { teams::store::finish_for(&mut connection, &session('b'), &first.id, &TransportOutcome::Received { native_id: "n1".into() }, NOW).unwrap(); }
        let second = if chief_first { teams::store::claim(&mut connection, &session('b'), "idle-1", NOW) } else { store::claim(&mut connection, &session('b'), "idle-1", NOW) }.unwrap();
        assert!(second.is_none());
        let second = if chief_first { teams::store::claim(&mut connection, &session('b'), "idle-2", NOW) } else { store::claim(&mut connection, &session('b'), "idle-2", NOW) }.unwrap();
        assert!(second.is_some());
    }
}

#[test]
fn prepared_team_sleep_blocks_chief_assignment_even_with_stale_ready_observations() {
    use cockpit::teams::{self, HostAction, Member, Proposal};
    let (mut connection, live) = setup();
    let proposal = Proposal { submission_key: "sleep-team".into(), task: brief("aaaaaaaa").work.task,
        members: vec![Member { session_id: session('b').session_id, role: Role::Developer }], reason: "Approved team".into() };
    let sessions = live.iter().map(|worker| worker.session.clone()).collect::<Vec<_>>();
    let pending = teams::store::propose(&mut connection, &session('b'), &proposal, &sessions, NOW).unwrap().result.unwrap();
    teams::store::control(&mut connection, &HostAction::Approve { request_id: "sleep-team-approval".into(),
        proposal_id: pending["proposal_id"].as_str().unwrap().into(), expected_revision: 1,
        mapping_hash: pending["mapping_hash"].as_str().unwrap().into() }, &sessions, NOW).unwrap().result.unwrap();
    let token = teams::store::sleep::prepare(&mut connection, &session('b'), "/project-a", NOW).unwrap().unwrap();
    assert!(dispatch(&mut connection, &live, "sleeping-assignment", "aaaaaaaa", 'b').is_err());
    teams::store::sleep::cancel(&mut connection, &session('b').session_id, &token).unwrap();
    dispatch(&mut connection, &live, "awake-assignment", "aaaaaaaa", 'b').unwrap();
    assert!(teams::store::sleep::prepare(&mut connection, &session('b'), "/project-a", NOW).is_err(),
        "pending Chief delivery must protect the same sleep boundary as team delivery");
}

#[test]
fn missing_receipt_expires_to_uncertain_without_releasing_ownership() {
    let (mut connection, mut live) = setup();
    dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap();
    store::observe(&mut connection, &live, NOW).unwrap();
    let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
    let now = NOW + cockpit::teams::store::RECEIPT_TIMEOUT_MS;
    for worker in &mut live { worker.observed_at_ms = now; }
    store::observe(&mut connection, &live, now).unwrap();
    assert_eq!(connection.query_row("SELECT state FROM chief_assignments", [], |row| row.get::<_, String>(0)).unwrap(), "uncertain");
    assert!(store::claim(&mut connection, &session('b'), "idle-2", now).unwrap().is_none());
    assert!(store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::NotSubmitted { reason: "too late".into() }, now).is_err());
    store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Received { native_id: "late-native-receipt".into() }, now).unwrap();
    assert_eq!(connection.query_row("SELECT state FROM chief_assignments", [], |row| row.get::<_, String>(0)).unwrap(), "received");
}

#[test]
fn chief_cli_uses_nested_agent_json_without_exposing_host_actions_on_agent_socket() {
    use std::{io::Write, process::Stdio};
    let sandbox = Sandbox::new();
    let requests = sandbox.team_socket(serde_json::Value::Null);
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("chief")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(br#"{"action":"chief","request":{"action":"context","after":0,"limit":10}}"#).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(requests.lock().unwrap()[0]["request"]["action"], "context");
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("chief")
        .stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(br#"{"request":"chief","action":{"action":"disable","request_id":"no","expected_epoch":"epoch"}}"#).unwrap();
    assert!(!child.wait_with_output().unwrap().status.success());
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[test]
fn chief_cli_coordination_example_forwards_its_explicit_submission_key() {
    use std::{io::Write, process::Stdio};
    let sandbox = Sandbox::new();
    let requests = sandbox.team_socket(serde_json::Value::Null);
    let help = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("--help").output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    let example = help.lines().find(|line| line.contains("\"action\":\"coordinate\"")).expect("chief coordination example");
    let action: cockpit::teams::AgentAction = serde_json::from_str(example.trim()).unwrap();
    let cockpit::teams::AgentAction::Chief { request } = action else { panic!("not a chief request") };
    request.validate().unwrap();
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("chief")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(example.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(requests.lock().unwrap()[0]["request"]["submission_key"], "coordinate-1");
}

#[test]
fn chief_envelope_displays_protected_approval_separately_from_commentary() {
    let (mut connection, live) = setup();
    dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap();
    store::observe(&mut connection, &live, NOW).unwrap();
    let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
    let text = cockpit::teams::envelope_text(&delivery.envelope);
    assert!(text.contains("Approved work revision: 1"), "{text}");
    assert!(text.contains("Only approved work"));
    assert!(text.contains("Commentary (untrusted, not authorization)"));
    assert!(text.contains("cockpit-team chief"));
    assert!(text.contains("human prompt"));
}

#[test]
fn chief_host_cli_uses_host_socket_and_never_falls_back_to_agent_authority() {
    use std::{io::Write, process::Stdio};
    let sandbox = Sandbox::new();
    let requests = sandbox.team_socket(serde_json::Value::Null);
    let host_dir = sandbox.runtime.join("claude-team-host");
    std::fs::create_dir_all(&host_dir).unwrap();
    std::fs::hard_link(sandbox.answers("teams").join("agent.sock"), host_dir.join("host.sock")).unwrap();
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("chief-host")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(br#"{"request":"chief_snapshot","after":0,"limit":10}"#).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(requests.lock().unwrap()[0]["request"], "chief_snapshot");
    std::fs::remove_file(host_dir.join("host.sock")).unwrap();
    let mut child = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]).arg("chief-host")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(br#"{"request":"chief_snapshot","after":0,"limit":10}"#).unwrap();
    assert!(!child.wait_with_output().unwrap().status.success());
    assert_eq!(requests.lock().unwrap().len(), 1);
}

fn dispatch(connection: &mut Connection, live: &[WorkerState], key: &str, task: &str, worker: char) -> Result<serde_json::Value, cockpit::teams::TeamError> {
    store::agent(connection, &session('a'), &ChiefAgentAction::Dispatch { submission_key: key.into(), work: brief(task).work, worker: session(worker).session_id }, live, NOW).unwrap().result
}

#[test]
fn assignment_is_atomic_and_replay_cannot_change_worker_or_scope() {
    let (mut connection, live) = setup();
    let first = dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap();
    assert_eq!(dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap(), first);
    assert!(dispatch(&mut connection, &live, "one", "aaaaaaaa", 'c').is_err());
    assert!(dispatch(&mut connection, &live, "two", "aaaaaaaa", 'c').is_err());
    assert!(dispatch(&mut connection, &live, "three", "bbbbbbbb", 'b').is_err());
    let active: i64 = connection.query_row("SELECT count(*) FROM chief_assignments WHERE state NOT IN ('completed','cancelled')", [], |row| row.get(0)).unwrap();
    assert_eq!(active, 1);
}

#[test]
fn dispatch_rejects_unavailable_stale_wrong_role_and_wrong_project_workers() {
    for reason in ["busy", "waiting_for_human", "queued_human_prompt", "restart", "asleep", "stale", "future", "role", "project", "unavailable"] {
        let (mut connection, mut live) = setup();
        match reason {
            "stale" => live[1].observed_at_ms = NOW - 30_001,
            "future" => live[1].observed_at_ms = NOW + 1,
            "role" => live[1].role = Some(Role::Qa),
            "project" => live[1].session.project = "path:/another".into(),
            "unavailable" => live[1].ready = false,
            _ => live[1].blocked_by.push(reason.into()),
        }
        assert!(dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').is_err(), "{reason}");
        assert_eq!(connection.query_row("SELECT count(*) FROM chief_assignments", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    }
}

#[test]
fn prerequisites_require_exact_completed_revision_not_a_todo_or_receipt() {
    let (mut connection, live) = setup();
    let mut dependent = brief("cccccccc");
    dependent.prerequisites.push(brief("aaaaaaaa").work);
    store::host(&mut connection, &ChiefHostAction::ApproveWork { request_id: "dependent".into(), approval_hash: dependent.approval_hash().unwrap(),
        brief: dependent, expected_revision: 0 }, &live, NOW).unwrap().result.unwrap();
    assert!(dispatch(&mut connection, &live, "dependent", "cccccccc", 'b').is_err());
}

#[test]
fn receipt_is_not_acceptance_and_worker_authority_is_not_a_chief_grant() {
    let (mut connection, live) = setup();
    let assignment = dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap()["assignment"].as_str().unwrap().to_string();
    let action = ChiefAgentAction::Accept { assignment: assignment.clone() };
    assert!(store::agent(&mut connection, &session('b'), &action, &live, NOW).unwrap().result.is_err());
    store::observe(&mut connection, &live, NOW).unwrap();
    let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
    assert_eq!(delivery.envelope["approval"]["work"]["revision"], 1);
    assert!(store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().is_none());
    store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Received { native_id: "native-1".into() }, NOW).unwrap();
    let state: String = connection.query_row("SELECT state FROM chief_assignments", [], |row| row.get(0)).unwrap();
    assert_eq!(state, "received");
    assert!(store::agent(&mut connection, &session('c'), &action, &live, NOW).unwrap().result.is_err());
    store::agent(&mut connection, &session('b'), &action, &live, NOW).unwrap().result.unwrap();
    for _ in 0..100 {store::agent(&mut connection, &session('b'), &ChiefAgentAction::Status { assignment: Some(assignment.clone()), summary: "done (not evidence)".into() }, &live, NOW).unwrap().result.unwrap();}
    assert_eq!(connection.query_row("SELECT state FROM chief_assignments", [], |row| row.get::<_, String>(0)).unwrap(), "working");
    for kind in ["assignment_accepted","assignment_working","native_delivery","worker_status"] {
        assert_eq!(connection.query_row("SELECT count(*) FROM chief_events WHERE assignment=?1 AND kind=?2",(&assignment,kind),|row|row.get::<_,i64>(0)).unwrap(),1,"{kind}");
    }
    assert_eq!(connection.query_row("SELECT revision FROM chief_assignments",[],|row|row.get::<_,i64>(0)).unwrap(),5);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_attention",[],|row|row.get::<_,i64>(0)).unwrap(),0);
}

#[test]
fn final_claim_revalidates_readiness_runtime_role_membership_and_grant() {
    for change in ["readiness", "stale", "runtime", "role", "binding", "grant", "chief_runtime", "revision", "team"] {
        let (mut connection, mut live) = setup();
        dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap();
        match change {
            "readiness" => live[1].blocked_by.push("queued_human_prompt".into()),
            "stale" => live[1].observed_at_ms = NOW - 30_001,
            "runtime" => live[1].session.runtime = "replacement".into(),
            "role" => live[1].role = Some(Role::Qa),
            "chief_runtime" => live[0].session.runtime = "replacement-chief".into(),
            "binding" => { connection.execute("UPDATE chief_worker_bindings SET state='revoked' WHERE session_id=?1", [&session('b').session_id]).unwrap(); },
            "grant" => { connection.execute("UPDATE chief_grants SET state='paused'", []).unwrap(); },
            "revision" => { connection.execute("UPDATE chief_work_revisions SET state='superseded' WHERE task_id='aaaaaaaa'", []).unwrap(); },
            "team" => {
                let task = serde_json::to_string(&brief("bbbbbbbb").work.task).unwrap();
                connection.execute("INSERT INTO teams VALUES (?1,'path:/project-a','bbbbbbbb',1)", [&task]).unwrap();
                connection.execute("INSERT INTO members VALUES (?1,?2,'developer','claude',?3,'team-epoch',NULL)", rusqlite::params![task,session('b').session_id,session('b').runtime]).unwrap();
            },
            _ => unreachable!(),
        }
        store::observe(&mut connection, &live, NOW).unwrap();
        assert!(store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().is_none(), "{change}");
        assert_eq!(connection.query_row("SELECT state FROM chief_assignments", [], |row| row.get::<_, String>(0)).unwrap(), "queued");
    }
}

#[test]
fn uncertain_delivery_retains_ownership_and_cannot_be_retried_by_a_negative_receipt() {
    let (mut connection, live) = setup();
    dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap();
    store::observe(&mut connection, &live, NOW).unwrap();
    let delivery = store::claim(&mut connection, &session('b'), "idle-1", NOW).unwrap().unwrap();
    store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::Uncertain { reason: "lost acknowledgement".into() }, NOW).unwrap();
    assert!(store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::NotSubmitted { reason: "try again".into() }, NOW).is_err());
    assert!(store::claim(&mut connection, &session('b'), "idle-2", NOW).unwrap().is_none());
    assert!(dispatch(&mut connection, &live, "two", "aaaaaaaa", 'c').is_err());
    assert!(dispatch(&mut connection, &live, "three", "bbbbbbbb", 'b').is_err());
}

#[test]
fn known_safe_retries_use_two_ten_thirty_seconds_then_stop() {
    let (mut connection, mut live) = setup();
    dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap();
    let mut now = NOW;
    for delay in [2_000_i64, 10_000, 30_000, 0] {
        for worker in &mut live { worker.observed_at_ms = now; }
        store::observe(&mut connection, &live, now).unwrap();
        let delivery = store::claim(&mut connection, &session('b'), "idle-1", now).unwrap().unwrap();
        store::finish_for(&mut connection, &session('b'), &delivery.id, &TransportOutcome::NotSubmitted { reason: "native busy before admission".into() }, now).unwrap();
        assert!(store::claim(&mut connection, &session('b'), "idle-1", now + delay.saturating_sub(1)).unwrap().is_none());
        now += delay;
    }
    assert_eq!(connection.query_row("SELECT state FROM chief_inbox WHERE assignment IS NOT NULL", [], |row| row.get::<_, String>(0)).unwrap(), "failed");
    assert!(dispatch(&mut connection, &live, "another", "aaaaaaaa", 'c').is_err());
    let assignment: String = connection.query_row("SELECT id FROM chief_assignments", [], |row| row.get(0)).unwrap();
    for action in [ChiefAgentAction::Accept { assignment: assignment.clone() }, ChiefAgentAction::Status { assignment: Some(assignment), summary: "start without delivery".into() }] {
        assert!(store::agent(&mut connection, &session('b'), &action, &live, now).unwrap().result.is_err());
    }
    let coordinate = ChiefAgentAction::Coordinate { submission_key: "failed-assignment".into(), work: brief("aaaaaaaa").work, recipient: session('b').session_id, body: "must not bypass failed assignment delivery".into() };
    assert!(store::agent(&mut connection, &session('a'), &coordinate, &live, now).unwrap().result.is_err());
}

#[test]
fn sender_submission_replay_survives_grant_replacement_without_new_ownership() {
    let (mut connection, live) = setup();
    let first = dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap();
    let epoch: String = connection.query_row("SELECT epoch FROM chief_grants", [], |row| row.get(0)).unwrap();
    store::host(&mut connection, &ChiefHostAction::Disable { request_id: "disable".into(), expected_epoch: epoch }, &live, NOW).unwrap().result.unwrap();
    store::host(&mut connection, &ChiefHostAction::Enable { request_id: "replace".into(), session: session('a') }, &live, NOW).unwrap().result.unwrap();
    assert_eq!(dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap(), first);
    assert!(dispatch(&mut connection, &live, "one", "bbbbbbbb", 'c').is_err());
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_assignments", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
}

#[test]
fn revoked_worker_authority_does_not_let_an_open_assignment_owner_become_chief() {
    let (mut connection, live) = setup();
    dispatch(&mut connection, &live, "one", "aaaaaaaa", 'b').unwrap();
    // Model a team unlink: the assignment still owns the worker after its role is revoked.
    connection.execute("UPDATE chief_worker_bindings SET state='revoked' WHERE session_id=?1", [&session('b').session_id]).unwrap();
    let epoch: String = connection.query_row("SELECT epoch FROM chief_grants", [], |row| row.get(0)).unwrap();
    store::host(&mut connection, &ChiefHostAction::Disable { request_id: "disable".into(), expected_epoch: epoch }, &live, NOW).unwrap().result.unwrap();
    let reply = store::host(&mut connection, &ChiefHostAction::Enable { request_id: "worker-as-chief".into(), session: session('b') }, &live, NOW).unwrap();
    assert_eq!(reply.result.unwrap_err().code, cockpit::teams::ErrorCode::Unauthorized);
}
