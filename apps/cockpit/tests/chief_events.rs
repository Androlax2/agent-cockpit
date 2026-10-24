use cockpit::chief::{store, ChiefAgentAction, ChiefHostAction, WorkerState};
use cockpit::teams::{ErrorCode, VerifiedSession};
use rusqlite::Connection;
use serde_json::{json, Value};
mod common;

fn session(id: &str, project: &str) -> VerifiedSession {
    VerifiedSession { session_id:format!("ses_{id}000000000000"), agent:"opencode".into(), project:project.into(), runtime:format!("runtime-{id}") }
}

fn setup() -> (Connection, Vec<WorkerState>) {
    let mut connection = Connection::open_in_memory().unwrap();
    store::migrate(&mut connection).unwrap();
    let live = [session("chief","path:/a"),session("worker","path:/a")].into_iter().map(|session| WorkerState {
        session, role:None, ready:true, blocked_by:vec![], observed_at_ms:100,
    }).collect();
    (connection,live)
}

#[test]
fn summary_request_is_persisted_without_inventing_a_completed_summary() {
    let (mut connection,live) = setup();
    let request = ChiefHostAction::RequestSummary {request_id:"summary".into()};
    let first = store::host(&mut connection,&request,&live,100).unwrap().result.unwrap();
    assert_eq!(first["state"],"failed");
    assert_eq!(store::host(&mut connection,&request,&live,101).unwrap().result.unwrap(),first);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_summary_requests",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_attention",[],|row|row.get::<_,i64>(0)).unwrap(),1);
}

#[test]
fn operational_review_does_not_require_assembling_terminal_audit_history() {
    let (connection,live)=setup();
    connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at) VALUES ('current-review','scope','key','Current exact decision','Review only','open',1,100)",[]).unwrap();
    connection.execute_batch("WITH RECURSIVE history(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM history WHERE n<26000)
        INSERT INTO chief_events(id,source,submission_key,fingerprint,kind,body,created_at)
        SELECT 'history-'||n,'host','history-'||n,'fixture','status','{}',n FROM history;").unwrap();
    let page=store::snapshot_page(&connection,&live,0,50,None,100).unwrap();
    assert_eq!(page["has_more"],false,"current review must not traverse terminal audit history");
    assert!(page["records"].as_array().unwrap().iter().any(|record|record["id"]=="current-review"));
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_events",[],|row|row.get::<_,i64>(0)).unwrap(),26000);
}

fn enable_events(connection: &mut Connection, live: &[WorkerState]) -> Value {
    store::host(connection,&ChiefHostAction::Enable {request_id:"enable-events".into(),session:live[0].session.clone()},live,100).unwrap().result.unwrap()
}

fn admit_events(connection: &mut Connection, live: &[WorkerState], turn: &str, now: i64) -> cockpit::teams::Dispatch {
    store::verify_admission(connection,&live[0].session,"opencode-queued-v1","Controlled fixture, not live proof",now).unwrap();
    store::observe(connection,live,now).unwrap();
    let wake = store::claim(connection,&live[0].session,turn,now).unwrap().expect("durable chief wake");
    assert_eq!(wake.envelope["kind"],"chief_wake");
    store::finish_for(connection,&live[0].session,&wake.id,&cockpit::teams::TransportOutcome::Received {native_id:format!("native-{turn}")},now).unwrap();
    wake
}

#[test]
fn enable_seeds_one_native_bootstrap_and_only_its_admission_opens_the_mutation_budget() {
    let (mut connection,live) = setup();
    let grant = enable_events(&mut connection,&live);
    assert_eq!(enable_events(&mut connection,&live),grant);
    let status = ChiefAgentAction::Status {summary:"Coordination report".into()};
    assert!(store::agent(&mut connection,&live[0].session,&status,&live,100).unwrap().result.is_err());
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_wake'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    let wake = admit_events(&mut connection,&live,"bootstrap",100);
    for _ in 0..20 {store::agent(&mut connection,&live[0].session,&status,&live,100).unwrap().result.unwrap();}
    let read = ChiefAgentAction::Context {after:0,limit:1};
    store::agent(&mut connection,&live[0].session,&read,&live,100).unwrap().result.unwrap();
    store::finish_for(&mut connection,&live[0].session,&wake.id,&cockpit::teams::TransportOutcome::Received {native_id:"native-bootstrap".into()},100).unwrap();
    assert_eq!(store::agent(&mut connection,&live[0].session,&status,&live,100).unwrap().result.unwrap_err().code,ErrorCode::Limit);
}

#[test]
fn equivalent_observations_and_status_flood_do_not_schedule_another_wake() {
    let (mut connection,mut live) = setup();
    enable_events(&mut connection,&live);
    admit_events(&mut connection,&live,"bootstrap",100);
    for index in 0..100 {
        for worker in &mut live {worker.observed_at_ms=101+index;}
        store::observe(&mut connection,&live,101+index).unwrap();
        let status = ChiefAgentAction::Status {summary:"Unchanged progress".into()};
        let _ = store::agent(&mut connection,&live[0].session,&status,&live,101+index).unwrap();
    }
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_wake'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    assert!(store::claim(&mut connection,&live[0].session,"later",30_100).unwrap().is_none());
}

#[test]
fn decisions_coalesce_new_evidence_updates_without_repeating_attention_and_only_host_resolves() {
    let (mut connection,live) = setup();
    enable_events(&mut connection,&live);
    admit_events(&mut connection,&live,"bootstrap",100);
    let decision = |question: &str| ChiefAgentAction::Decision {key:"missing-proof".into(),question:question.into(),recommendation:"Host must review evidence; no permission is implied".into()};
    let first = store::agent(&mut connection,&live[0].session,&decision("Missing approved test result"),&live,100).unwrap().result.unwrap();
    for _ in 0..5 {assert_eq!(store::agent(&mut connection,&live[0].session,&decision("Missing approved test result"),&live,100).unwrap().result.unwrap(),first);}
    let changed = store::agent(&mut connection,&live[0].session,&decision("Test source is now present but does not pass"),&live,100).unwrap().result.unwrap();
    assert_eq!(changed["decision"],first["decision"]);
    assert_eq!(changed["revision"],2);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_attention",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    assert!(store::agent(&mut connection,&live[1].session,&decision("Forged"),&live,100).unwrap().result.is_err());
    store::host(&mut connection,&ChiefHostAction::Resolve {request_id:"resolve".into(),decision:first["decision"].as_str().unwrap().into(),expected_revision:2,resolution:"Acknowledged; still no execution authorization".into()},&live,100).unwrap().result.unwrap();
    let action = ChiefAgentAction::ContextSnapshot {after:0,limit:50,generation:None};
    let snapshot = store::agent(&mut connection,&live[0].session,&action,&live,100).unwrap().result.unwrap();
    let decision = snapshot["records"].as_array().unwrap().iter().find(|record|record["kind"]=="decision").expect("current host control state survives cursor recovery");
    assert_eq!(decision["state"],"resolved");
    assert_eq!(decision["source"],"protected_host_control");
    let new_evidence = ChiefAgentAction::Decision {key:"missing-proof".into(),question:"New source contradicts the previously reviewed result".into(),recommendation:"Host must review this new revision".into()};
    store::agent(&mut connection,&live[0].session,&new_evidence,&live,100).unwrap().result.unwrap();
    assert_eq!(connection.query_row("SELECT state FROM chief_decisions WHERE decision_key='missing-proof'",[],|row|row.get::<_,String>(0)).unwrap(),"open");
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_attention",[],|row|row.get::<_,i64>(0)).unwrap(),1);
}

#[test]
fn reopening_at_capacity_preserves_the_answered_revision_but_open_material_updates_remain_allowed() {
    let (mut connection,live) = setup(); enable_events(&mut connection,&live); admit_events(&mut connection,&live,"bootstrap",100);
    let action = |key: &str, question: &str| ChiefAgentAction::Decision {key:key.into(),question:question.into(),recommendation:"Host review only, no grants".into()};
    let answered = store::agent(&mut connection,&live[0].session,&action("answered","Original blocker"),&live,100).unwrap().result.unwrap();
    store::host(&mut connection,&ChiefHostAction::Resolve {request_id:"answer-capacity".into(),decision:answered["decision"].as_str().unwrap().into(),expected_revision:1,resolution:"Keep this exact host answer".into()},&live,100).unwrap().result.unwrap();
    let epoch: String = connection.query_row("SELECT epoch FROM chief_grants WHERE state='active'",[],|row|row.get(0)).unwrap();
    for index in 0..100 {
        connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at) VALUES (?1,?2,?3,'Original open blocker','Host review only','open',1,100)",
            rusqlite::params![format!("open-{index}"),format!("chief-{epoch}"),format!("capacity-{index}")]).unwrap();
    }
    let result = store::agent(&mut connection,&live[0].session,&action("answered","Materially changed answered blocker"),&live,100).unwrap().result;
    let preserved: (String,i64,String) = connection.query_row("SELECT state,revision,resolution FROM chief_decisions WHERE id=?1",[answered["decision"].as_str().unwrap()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!(preserved,("resolved".into(),2,"Keep this exact host answer".into()));
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_decisions WHERE state='open'",[],|row|row.get::<_,i64>(0)).unwrap(),100);
    assert_eq!(result.unwrap_err().code,ErrorCode::Limit);
    let updated = store::agent(&mut connection,&live[0].session,&action("capacity-0","Materially changed already-open blocker"),&live,100).unwrap().result.unwrap();
    assert_eq!(updated["revision"],2);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_decisions WHERE state='open'",[],|row|row.get::<_,i64>(0)).unwrap(),100);
}

#[test]
fn decision_resolved_events_identify_the_exact_control_without_exporting_body_or_prose() {
    let (mut connection,mut live) = setup(); enable_events(&mut connection,&live);
    admit_events(&mut connection,&live,"bootstrap",100);
    let decision = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Decision {key:"exact-control".into(),question:"PRIVATE question".into(),recommendation:"PRIVATE recommendation".into()},&live,100).unwrap().result.unwrap();
    let after: i64 = connection.query_row("SELECT max(seq) FROM chief_events",[],|row|row.get(0)).unwrap();
    store::host(&mut connection,&ChiefHostAction::Resolve {request_id:"resolve-control".into(),decision:decision["decision"].as_str().unwrap().into(),expected_revision:1,resolution:"PRIVATE resolution, not a permission grant".into()},&live,101).unwrap().result.unwrap();
    connection.execute("UPDATE chief_events SET body=json_set(body,'$.question','PRIVATE body question','$.registry',json_object('request','PRIVATE registry request')) WHERE kind='decision_resolved'",[]).unwrap();
    for worker in &mut live {worker.observed_at_ms=30_100;}
    admit_events(&mut connection,&live,"resolved-control",30_100);
    let context = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Context {after,limit:50},&live,30_100).unwrap().result.unwrap();
    assert_eq!(context["snapshot_required"],false);
    let event = context["records"].as_array().unwrap().iter().find(|event|event["kind"]=="decision_resolved").unwrap();
    assert_eq!(event["decision_id"],decision["decision"]);
    assert_eq!(event["decision_revision"],2);
    assert!(event.get("body").is_none());
    assert!(!context.to_string().contains("PRIVATE"));
}

#[test]
fn unrecognized_incremental_control_events_require_bounded_snapshot_recovery_without_cursor_skip() {
    let (mut connection,mut live) = setup(); enable_events(&mut connection,&live); admit_events(&mut connection,&live,"bootstrap",100);
    let after: i64 = connection.query_row("SELECT max(seq) FROM chief_events",[],|row|row.get(0)).unwrap();
    connection.execute("INSERT INTO chief_events(id,source,submission_key,fingerprint,kind,body,created_at) VALUES ('future-control','host','future-control','fixture','future_control','PRIVATE raw non-JSON prose',101)",[]).unwrap();
    for worker in &mut live {worker.observed_at_ms=30_100;}
    admit_events(&mut connection,&live,"future-control",30_100);
    let context = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Context {after,limit:2},&live,30_100).unwrap().result.unwrap();
    assert_eq!(context["snapshot_required"],true);
    assert_eq!(context["next"],after);
    assert!(context["snapshot_generation"].is_string());
    assert!(context["records"].as_array().unwrap().len()<=2);
    assert!(!context.to_string().contains("PRIVATE"));
}

#[test]
fn concurrent_reopenings_share_one_available_decision_slot() {
    use std::sync::{Arc,Barrier};
    let sandbox = common::sandbox::Sandbox::new(); let path = sandbox.root.join("reopen.sqlite");
    let (memory,live) = setup(); drop(memory);
    let mut connection = Connection::open(&path).unwrap(); store::migrate(&mut connection).unwrap(); enable_events(&mut connection,&live); admit_events(&mut connection,&live,"bootstrap",100);
    for index in 0..2 {
        let action = ChiefAgentAction::Decision {key:format!("reopen-{index}"),question:"Original blocker".into(),recommendation:"Review only".into()};
        let decision = store::agent(&mut connection,&live[0].session,&action,&live,100).unwrap().result.unwrap();
        store::host(&mut connection,&ChiefHostAction::Resolve {request_id:format!("resolve-{index}"),decision:decision["decision"].as_str().unwrap().into(),expected_revision:1,resolution:"Original host answer".into()},&live,100).unwrap().result.unwrap();
    }
    for index in 0..99 {connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at) VALUES (?1,?1,'blocked','Other blocker','Review only','open',1,100)",[format!("filled-{index}")]).unwrap();}
    let barrier = Arc::new(Barrier::new(2));
    let threads = (0..2).map(|index| {
        let path=path.clone(); let live=live.clone(); let barrier=barrier.clone();
        std::thread::spawn(move || {
            let mut connection = Connection::open(path).unwrap(); connection.busy_timeout(std::time::Duration::from_secs(3)).unwrap();
            barrier.wait();
            let action = ChiefAgentAction::Decision {key:format!("reopen-{index}"),question:"Materially changed blocker".into(),recommendation:"Review new evidence only".into()};
            match store::agent(&mut connection,&live[0].session,&action,&live,100).unwrap().result {
                Ok(_) => true,
                Err(error) => {assert_eq!(error.code,ErrorCode::Limit); false},
            }
        })
    }).collect::<Vec<_>>();
    assert_eq!(threads.into_iter().map(|thread|thread.join().unwrap()).filter(|accepted|*accepted).count(),1);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_decisions WHERE state='open'",[],|row|row.get::<_,i64>(0)).unwrap(),100);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_decisions WHERE decision_key LIKE 'reopen-%' AND state='resolved' AND revision=2 AND resolution='Original host answer'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
}

#[test]
fn failed_native_bootstrap_never_opens_budget_or_blindly_retries() {
    for outcome in [cockpit::teams::TransportOutcome::NotSubmitted {reason:"Offline".into()},cockpit::teams::TransportOutcome::Uncertain {reason:"Lost native reply".into()}] {
        let (mut connection,live) = setup();
        enable_events(&mut connection,&live);
        store::verify_admission(&mut connection,&live[0].session,"opencode-queued-v1","Controlled fixture",100).unwrap();
        store::observe(&mut connection,&live,100).unwrap();
        let wake = store::claim(&mut connection,&live[0].session,"bootstrap",100).unwrap().unwrap();
        store::finish_for(&mut connection,&live[0].session,&wake.id,&outcome,100).unwrap();
        assert!(store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {summary:"No authority".into()},&live,100).unwrap().result.is_err());
        store::observe(&mut connection,&live,100).unwrap();
        assert!(store::claim(&mut connection,&live[0].session,"later",60_100).unwrap().is_none());
        assert_eq!(connection.query_row("SELECT count(*) FROM chief_decisions WHERE state='open'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    }
}

#[test]
fn fifty_records_are_a_cumulative_turn_budget_and_new_events_wait_for_next_admitted_boundary() {
    let (mut connection,mut live) = setup();
    enable_events(&mut connection,&live);
    for index in 0..60 {
        connection.execute("INSERT INTO chief_events(id,source,submission_key,fingerprint,kind,body,created_at) VALUES (?1,'host',?1,'test','status','{}',100)",[format!("control-{index}")]).unwrap();
    }
    admit_events(&mut connection,&live,"bootstrap",100);
    let first = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Context {after:1,limit:30},&live,100).unwrap().result.unwrap();
    assert_eq!(first["records"].as_array().unwrap().len(),30);
    let next = first["next"].as_i64().unwrap();
    let second = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Context {after:next,limit:30},&live,100).unwrap().result.unwrap();
    assert_eq!(second["records"].as_array().unwrap().len(),20);
    assert_eq!(store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Context {after:0,limit:1},&live,100).unwrap().result.unwrap_err().code,ErrorCode::Limit);
    for worker in &mut live {worker.observed_at_ms=30_100;}
    store::observe(&mut connection,&live,30_100).unwrap();
    let wake = store::claim(&mut connection,&live[0].session,"backlog-boundary",30_100).unwrap().expect("unread control backlog has a durable deadline wake");
    assert_eq!(wake.envelope["context_continuation"]["resume_after"],second["next"]);
    store::finish_for(&mut connection,&live[0].session,&wake.id,&cockpit::teams::TransportOutcome::Received {native_id:"backlog-admitted".into()},30_100).unwrap();
    let epoch: String = connection.query_row("SELECT epoch FROM chief_grants WHERE state='active'",[],|row|row.get(0)).unwrap();
    store::host(&mut connection,&ChiefHostAction::Disable {request_id:"replace-backlog-disable".into(),expected_epoch:epoch},&live,30_100).unwrap().result.unwrap();
    store::host(&mut connection,&ChiefHostAction::Enable {request_id:"replace-backlog-enable".into(),session:live[0].session.clone()},&live,30_100).unwrap().result.unwrap();
    let replacement = admit_events(&mut connection,&live,"replacement-bootstrap",30_100);
    assert!(replacement.envelope["context_continuation"].is_null(),"new grants must not inherit an old chief's cursor");
}

#[test]
fn cooldown_survives_reopen_and_receipts_cannot_reuse_an_idle_boundary() {
    let sandbox = common::sandbox::Sandbox::new();
    let path = sandbox.root.join("events.sqlite");
    let (memory,mut live) = setup(); drop(memory);
    let mut connection = Connection::open(&path).unwrap(); store::migrate(&mut connection).unwrap();
    enable_events(&mut connection,&live);
    admit_events(&mut connection,&live,"bootstrap",100);
    let request = ChiefHostAction::RequestSummary {request_id:"material".into()};
    store::host(&mut connection,&request,&live,101).unwrap().result.unwrap();
    for _ in 0..20 {store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {summary:"Durable turn budget".into()},&live,101).unwrap().result.unwrap();}
    drop(connection);
    let mut connection = Connection::open(&path).unwrap(); store::migrate(&mut connection).unwrap();
    assert_eq!(store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {summary:"Reopen must not reset allowance".into()},&live,101).unwrap().result.unwrap_err().code,ErrorCode::Limit);
    for worker in &mut live {worker.observed_at_ms=30_099;}
    store::observe(&mut connection,&live,30_099).unwrap();
    assert!(store::claim(&mut connection,&live[0].session,"next",30_099).unwrap().is_none());
    for worker in &mut live {worker.observed_at_ms=30_100;}
    store::observe(&mut connection,&live,30_100).unwrap();
    assert!(store::claim(&mut connection,&live[0].session,"bootstrap",30_100).unwrap().is_none());
    let next = store::claim(&mut connection,&live[0].session,"next",30_100).unwrap().unwrap();
    assert_eq!(next.envelope["summary_requests"],json!(["material"]));
    store::finish_for(&mut connection,&live[0].session,&next.id,&cockpit::teams::TransportOutcome::Received {native_id:"native-next".into()},30_100).unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_turns",[],|row|row.get::<_,i64>(0)).unwrap(),2);
    store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {summary:"New admitted boundary has its own bounded allowance".into()},&live,30_100).unwrap().result.unwrap();
    for worker in &mut live {worker.observed_at_ms=60_100;}
    let epoch: String = connection.query_row("SELECT epoch FROM chief_grants WHERE state='active'",[],|row|row.get(0)).unwrap();
    store::host(&mut connection,&ChiefHostAction::Pause {request_id:"boundary-pause".into(),expected_epoch:epoch.clone()},&live,60_100).unwrap().result.unwrap();
    store::host(&mut connection,&ChiefHostAction::Resume {request_id:"boundary-resume".into(),expected_epoch:epoch},&live,60_100).unwrap().result.unwrap();
    store::observe(&mut connection,&live,60_100).unwrap();
    assert!(store::claim(&mut connection,&live[0].session,"bootstrap",60_100).unwrap().is_none(),"an older consumed boundary cannot be recycled after another turn");
    assert!(store::claim(&mut connection,&live[0].session,"third",60_100).unwrap().is_some());
}

#[test]
fn summary_result_is_an_exact_bounded_request_reply_not_an_evidence_or_permission_grant() {
    let (mut connection,live) = setup();
    enable_events(&mut connection,&live);
    store::host(&mut connection,&ChiefHostAction::RequestSummary {request_id:"summary-result".into()},&live,100).unwrap().result.unwrap();
    let wake = admit_events(&mut connection,&live,"bootstrap",100);
    assert_eq!(wake.envelope["summary_requests"],json!(["summary-result"]));
    let response = |id: &str| serde_json::from_value::<ChiefAgentAction>(json!({"action":"summary_response","request_id":id,
        "sections":{"completed":"No verified completions","active":"No active work","decisions":"None","idle":"No approved work"}})).unwrap();
    assert!(serde_json::from_value::<ChiefAgentAction>(json!({"action":"summary_response","request_id":"summary-result",
        "sections":{"completed":"None","active":"None","decisions":"None","idle":"None"},"evidence_ids":["claim"]})).is_err(),"evidence references are no longer part of a summary");
    assert!(store::agent(&mut connection,&live[0].session,&response("not-requested"),&live,100).unwrap().result.is_err());
    assert!(store::agent(&mut connection,&live[1].session,&response("summary-result"),&live,100).unwrap().result.is_err());
    let result = store::agent(&mut connection,&live[0].session,&response("summary-result"),&live,100).unwrap().result.unwrap();
    assert_eq!(result["state"],"completed");
    let record: String = connection.query_row("SELECT response FROM chief_summary_requests WHERE id='summary-result'",[],|row|row.get(0)).unwrap();
    let record: Value = serde_json::from_str(&record).unwrap();
    assert_eq!(record["provenance"],"chief_narrative_unverified");
    assert!(record.get("references").is_none());
}

#[test]
fn handling_a_crashed_second_wake_closes_the_old_allowance_before_observation() {
    let (mut connection,mut live)=setup();
    let grant=enable_events(&mut connection,&live);
    admit_events(&mut connection,&live,"bootstrap",100);
    store::host(&mut connection,&ChiefHostAction::RequestSummary {request_id:"second-wake".into()},&live,101).unwrap().result.unwrap();
    for worker in &mut live {worker.observed_at_ms=30_100;}
    store::observe(&mut connection,&live,30_100).unwrap();
    let wake=store::claim(&mut connection,&live[0].session,"second",30_100).unwrap().unwrap();
    store::recover(&mut connection,30_101).unwrap();
    let status=ChiefAgentAction::Status {summary:"Cannot spend bootstrap allowance".into()};
    assert!(store::agent(&mut connection,&live[0].session,&status,&live,30_101).unwrap().result.is_err());
    store::host(&mut connection,&ChiefHostAction::ReconcileDelivery {request_id:"handle-crash".into(),message:wake.envelope["message_id"].as_str().unwrap().into(),expected_revision:1,evidence:cockpit::chief::ReconciliationEvidence::Handled {note:"Exact crashed wake handled".into()}},&live,30_102).unwrap().result.unwrap();
    assert!(store::agent(&mut connection,&live[0].session,&status,&live,30_102).unwrap().result.is_err());
    store::observe(&mut connection,&live,30_103).unwrap();
    assert!(store::claim(&mut connection,&live[0].session,"third",30_103).unwrap().is_none());
    store::host(&mut connection,&ChiefHostAction::Disable {request_id:"disable-old".into(),expected_epoch:grant["epoch"].as_str().unwrap().into()},&live,30_104).unwrap().result.unwrap();
    store::host(&mut connection,&ChiefHostAction::Enable {request_id:"replacement".into(),session:live[0].session.clone()},&live,30_105).unwrap().result.unwrap();
    admit_events(&mut connection,&live,"replacement",30_105);
    store::agent(&mut connection,&live[0].session,&status,&live,30_105).unwrap().result.unwrap();
}

#[test]
fn handling_an_old_crashed_wake_does_not_close_a_replacement_epoch() {
    let (mut connection,live)=setup();let grant=enable_events(&mut connection,&live);
    store::verify_admission(&mut connection,&live[0].session,"opencode-queued-v1","Controlled fixture, not live proof",100).unwrap();
    store::observe(&mut connection,&live,100).unwrap();
    let old=store::claim(&mut connection,&live[0].session,"old",100).unwrap().unwrap();
    store::recover(&mut connection,101).unwrap();
    store::host(&mut connection,&ChiefHostAction::Disable {request_id:"disable-old".into(),expected_epoch:grant["epoch"].as_str().unwrap().into()},&live,102).unwrap().result.unwrap();
    store::host(&mut connection,&ChiefHostAction::Enable {request_id:"replacement".into(),session:live[0].session.clone()},&live,103).unwrap().result.unwrap();
    store::host(&mut connection,&ChiefHostAction::ReconcileDelivery {request_id:"handle-old".into(),message:old.envelope["message_id"].as_str().unwrap().into(),expected_revision:1,evidence:cockpit::chief::ReconciliationEvidence::Handled {note:"Old exact wake handled".into()}},&live,104).unwrap().result.unwrap();
    admit_events(&mut connection,&live,"replacement",104);
    store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {summary:"Replacement native allowance remains usable".into()},&live,104).unwrap().result.unwrap();
}

#[test]
fn a_failed_second_wake_cannot_reuse_the_previous_allowance() {
    for outcome in [cockpit::teams::TransportOutcome::Uncertain {reason:"Native inbox result lost".into()},cockpit::teams::TransportOutcome::Received {native_id:"native-bootstrap".into()}] {
        let (mut connection,mut live) = setup(); enable_events(&mut connection,&live); admit_events(&mut connection,&live,"bootstrap",100);
        store::host(&mut connection,&ChiefHostAction::RequestSummary {request_id:"next-request".into()},&live,101).unwrap().result.unwrap();
        for worker in &mut live {worker.observed_at_ms=30_100;}
        store::observe(&mut connection,&live,30_100).unwrap();
        let wake = store::claim(&mut connection,&live[0].session,"next",30_100).unwrap().unwrap();
        store::finish_for(&mut connection,&live[0].session,&wake.id,&outcome,30_100).unwrap();
        assert!(store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {summary:"Old allowance must stay closed".into()},&live,30_100).unwrap().result.is_err());
    }
}

#[test]
fn chief_running_then_idle_is_not_its_own_material_wake_source() {
    let (mut connection,mut live) = setup(); enable_events(&mut connection,&live); admit_events(&mut connection,&live,"bootstrap",100);
    live[0].blocked_by=vec!["native_busy".into()];
    store::observe(&mut connection,&live,101).unwrap();
    live[0].blocked_by=vec![];
    for worker in &mut live {worker.observed_at_ms=30_100;}
    store::observe(&mut connection,&live,30_100).unwrap();
    assert!(store::claim(&mut connection,&live[0].session,"must-not-loop",30_100).unwrap().is_none());
}

#[test]
fn an_expired_summary_preserves_the_previous_result_timestamp_and_does_not_regenerate() {
    let (mut connection,mut live) = setup(); enable_events(&mut connection,&live);
    store::host(&mut connection,&ChiefHostAction::RequestSummary {request_id:"first-summary".into()},&live,100).unwrap().result.unwrap();
    admit_events(&mut connection,&live,"bootstrap",100);
    let reply = serde_json::from_value::<ChiefAgentAction>(json!({"action":"summary_response","request_id":"first-summary","sections":{"completed":"None","active":"None","decisions":"None","idle":"No approved work"}})).unwrap();
    store::agent(&mut connection,&live[0].session,&reply,&live,101).unwrap().result.unwrap();
    store::host(&mut connection,&ChiefHostAction::RequestSummary {request_id:"failed-summary".into()},&live,102).unwrap().result.unwrap();
    for worker in &mut live {worker.observed_at_ms=30_100;}
    admit_events(&mut connection,&live,"timeout-summary",30_100);
    for worker in &mut live {worker.observed_at_ms=150_100;}
    store::observe(&mut connection,&live,150_100).unwrap();
    let snapshot = store::snapshot(&connection,&live,0,50).unwrap();
    let summary = snapshot["records"].as_array().unwrap().iter().find(|record|record["kind"]=="summary").unwrap();
    assert_eq!(summary["state"],"failed");
    assert_eq!(summary["last_completed_at_ms"],101);
    assert_eq!(summary["last_result"]["request_id"],"first-summary");
    store::observe(&mut connection,&live,120_102).unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_summary_requests",[],|row|row.get::<_,i64>(0)).unwrap(),2);
}

#[test]
fn summary_waits_for_idle_admission_before_starting_its_response_deadline() {
    let (mut connection,mut live) = setup(); enable_events(&mut connection,&live);
    live[0].blocked_by=vec!["native_busy".into()];
    store::host(&mut connection,&ChiefHostAction::RequestSummary {request_id:"busy-summary".into()},&live,100).unwrap().result.unwrap();
    for now in [120_100,300_099] {
        for worker in &mut live {worker.observed_at_ms=now;}
        store::observe(&mut connection,&live,now).unwrap();
        assert_eq!(connection.query_row("SELECT state FROM chief_summary_requests",[],|row|row.get::<_,String>(0)).unwrap(),"queued");
        assert!(store::claim(&mut connection,&live[0].session,"still-busy",now).unwrap().is_none());
    }
    live[0].blocked_by=vec![];
    for worker in &mut live {worker.observed_at_ms=300_100;}
    let wake=admit_events(&mut connection,&live,"after-idle",300_100);
    assert_eq!(wake.envelope["summary_requests"],json!(["busy-summary"]));
    let reply=serde_json::from_value::<ChiefAgentAction>(json!({"action":"summary_response","request_id":"busy-summary","sections":{"completed":"None","active":"None","decisions":"None","idle":"No approved work"}})).unwrap();
    store::agent(&mut connection,&live[0].session,&reply,&live,400_100).unwrap().result.unwrap();
    assert_eq!(connection.query_row("SELECT completed_at FROM chief_summary_requests",[],|row|row.get::<_,i64>(0)).unwrap(),400_100);
}

#[test]
fn paused_summary_queue_has_a_bounded_unavailable_deadline() {
    let (mut connection,mut live)=setup(); let grant=enable_events(&mut connection,&live);
    store::host(&mut connection,&ChiefHostAction::Pause {request_id:"pause".into(),expected_epoch:grant["epoch"].as_str().unwrap().into()},&live,100).unwrap().result.unwrap();
    store::host(&mut connection,&ChiefHostAction::RequestSummary {request_id:"paused-summary".into()},&live,100).unwrap().result.unwrap();
    for worker in &mut live {worker.observed_at_ms=900_100;}
    store::observe(&mut connection,&live,900_100).unwrap();
    assert_eq!(connection.query_row("SELECT state FROM chief_summary_requests",[],|row|row.get::<_,String>(0)).unwrap(),"failed");
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_summary_requests",[],|row|row.get::<_,i64>(0)).unwrap(),1);
}

#[test]
fn missed_event_cursors_request_a_bounded_snapshot_without_advancing_over_control_records() {
    let (mut connection,live) = setup(); enable_events(&mut connection,&live);
    for index in 0..1100 {connection.execute("INSERT INTO chief_events(id,source,submission_key,fingerprint,kind,body,created_at) VALUES (?1,'host',?1,'fixture','control','{}',100)",[format!("retention-{index}")]).unwrap();}
    admit_events(&mut connection,&live,"bootstrap",100);
    let context = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Context {after:1,limit:2},&live,100).unwrap().result.unwrap();
    assert_eq!(context["snapshot_required"],true);
    assert_eq!(context["next"],1);
    assert!(context["records"].as_array().unwrap().len()<=2);
    assert!(context["snapshot_generation"].is_string());
}

#[test]
fn failed_actions_consume_a_durable_budget_without_partial_mutations() {
    let (mut connection,live) = setup(); enable_events(&mut connection,&live); admit_events(&mut connection,&live,"bootstrap",100);
    for index in 0..20 {
        let action = serde_json::from_value::<ChiefAgentAction>(json!({"action":"summary_response","request_id":format!("unrequested-{index}"),
            "sections":{"completed":"None","active":"None","decisions":"None","idle":"None"}})).unwrap();
        assert_eq!(store::agent(&mut connection,&live[0].session,&action,&live,100).unwrap().result.unwrap_err().code,ErrorCode::Conflict);
    }
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_summary_requests",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {summary:"No new reasoning turn".into()},&live,100).unwrap().result.unwrap_err().code,ErrorCode::Limit);
}

#[test]
fn large_redacted_snapshots_continue_at_a_new_native_boundary_without_invalidating_their_own_generation() {
    let (mut connection,mut live) = setup(); enable_events(&mut connection,&live);
    for index in 0..60 {
        connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at) VALUES (?1,?1,'key','Bounded host decision','Review only','open',1,100)",[format!("snapshot-{index}")]).unwrap();
    }
    admit_events(&mut connection,&live,"bootstrap",100);
    let first = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::ContextSnapshot {after:0,limit:50,generation:None},&live,100).unwrap().result.unwrap();
    assert_eq!(first["records"].as_array().unwrap().len(),50);
    for worker in &mut live {worker.observed_at_ms=30_100;}
    let wake = admit_events(&mut connection,&live,"snapshot-boundary",30_100);
    assert_eq!(wake.envelope["context_continuation"]["snapshot_after"],first["next"]);
    let second = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::ContextSnapshot {after:first["next"].as_i64().unwrap(),limit:50,generation:Some(first["generation"].as_str().unwrap().into())},&live,30_100).unwrap().result.unwrap();
    assert_ne!(second["invalidated"],true);
    assert_eq!(second["has_more"],false);
    assert_eq!(first["records"].as_array().unwrap().len()+second["records"].as_array().unwrap().len(),63);
}

#[test]
fn routine_capacity_idle_and_no_approved_work_do_not_create_host_attention() {
    let (mut connection,live) = setup(); enable_events(&mut connection,&live); admit_events(&mut connection,&live,"bootstrap",100);
    for key in ["capacity","idle","no_approved_work","no-approved-work","progress","handoff"] {
        let action = ChiefAgentAction::Decision {key:key.into(),question:"No scheduling action is currently available".into(),recommendation:"Wait silently for material work".into()};
        let result = store::agent(&mut connection,&live[0].session,&action,&live,100).unwrap().result.unwrap();
        assert_eq!(result["status"],"silent");
    }
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_attention",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_decisions",[],|row|row.get::<_,i64>(0)).unwrap(),0);
}

#[test]
fn context_reads_before_bootstrap_are_bounded_and_cannot_mint_a_mutation_turn() {
    let (mut connection,live) = setup(); enable_events(&mut connection,&live);
    for _ in 0..50 {store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Context {after:0,limit:1},&live,100).unwrap().result.unwrap();}
    assert_eq!(store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Context {after:0,limit:1},&live,100).unwrap().result.unwrap_err().code,ErrorCode::Limit);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_turns",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    admit_events(&mut connection,&live,"bootstrap",100);
    store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Context {after:0,limit:1},&live,100).unwrap().result.unwrap();
}

#[test]
fn schema_eight_active_grants_gain_one_bootstrap_without_an_initial_mutation_allowance() {
    let (mut connection,live) = setup(); let grant = enable_events(&mut connection,&live);
    common::chief::remove_link_schema(&connection);
    let triggers = connection.prepare("SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'review_%'").unwrap().query_map([],|row|row.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    for trigger in triggers {connection.execute_batch(&format!("DROP TRIGGER {trigger}")).unwrap();}
    connection.execute_batch("DELETE FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_wake'; DELETE FROM chief_events WHERE kind='bootstrap';
        DROP INDEX chief_one_queued_wake; DROP TABLE chief_coordination; DROP TABLE chief_turns; DROP TABLE chief_attention; DROP TABLE chief_summary_requests; DROP TABLE chief_review_generation; PRAGMA user_version=8;").unwrap();
    store::migrate(&mut connection).unwrap();
    assert_eq!(enable_events(&mut connection,&live),grant);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_wake'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    assert!(store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {summary:"Migration is not native admission".into()},&live,100).unwrap().result.is_err());
    store::migrate(&mut connection).unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_events WHERE kind='bootstrap'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
}

#[test]
fn a_native_boundary_change_between_reservation_and_mutation_fails_closed() {
    for transition in ["received","uncertain"] {
        let (mut connection,mut live) = setup(); enable_events(&mut connection,&live);
        let first = admit_events(&mut connection,&live,"bootstrap",100);
        store::host(&mut connection,&ChiefHostAction::RequestSummary {request_id:"race-material".into()},&live,101).unwrap().result.unwrap();
        for worker in &mut live {worker.observed_at_ms=30_100;}
        store::observe(&mut connection,&live,30_100).unwrap();
        let next = store::claim(&mut connection,&live[0].session,"next",30_100).unwrap().unwrap();
        let epoch = next.envelope["epoch"].as_str().unwrap();
        // Controlled SQLite interleaving: native admission/failure wins exactly after reservation.
        let change = if transition=="received" {format!("INSERT INTO chief_turns(dispatch_id,epoch,idle_turn,native_id,through_seq,admitted_at) VALUES ('{}','{}','next','native-next',{},30100);",next.id,epoch,next.envelope["through_seq"])}
            else {format!("UPDATE chief_coordination SET failed=1 WHERE epoch='{epoch}';")};
        connection.execute_batch(&format!("CREATE TRIGGER simulated_native_boundary AFTER UPDATE OF mutations ON chief_turns WHEN NEW.dispatch_id='{}' BEGIN {change} END;",first.id)).unwrap();
        let result = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {summary:"Must not execute using a different or failed native turn".into()},&live,30_100).unwrap().result;
        assert!(result.is_err(),"{transition}");
        assert_eq!(connection.query_row("SELECT count(*) FROM chief_events WHERE kind='status'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    }
}

#[test]
fn host_review_exposes_full_records_while_agent_context_stays_redacted() {
    let (mut connection,live) = setup();
    store::host(&mut connection,&ChiefHostAction::Enable {request_id:"enable".into(),session:live[0].session.clone()},&live,100).unwrap().result.unwrap();
    connection.execute("INSERT INTO chief_decisions (id,scope_key,decision_key,question,recommendation,state,revision,created_at) VALUES ('decision','scope','key','Review exact evidence','Private recommendation','open',3,100)",[]).unwrap();
    let review = store::snapshot(&connection,&live,0,50).unwrap();
    let records = review["records"].as_array().unwrap();
    let record = |kind: &str| records.iter().find(|record|record["kind"]==kind).unwrap();
    assert_eq!(record("chief")["session"],json!(live[0].session));
    assert_eq!(record("worker")["session"]["runtime"],live[0].session.runtime);
    assert_eq!(record("decision")["revision"],3);
    assert_eq!(record("decision")["recommendation"],"Private recommendation");
    assert_eq!(record("summary")["state"],"unavailable");
    let context = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Context {after:0,limit:50},&live,101).unwrap().result.unwrap().to_string();
    for secret in ["Private recommendation",live[1].session.runtime.as_str()] {assert!(!context.contains(secret),"{context}");}
    let mut paged = Vec::new();
    let mut after = 0;
    loop {
        let page = store::snapshot(&connection,&live,after,1).unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len()<65_000);
        paged.extend(page["records"].as_array().unwrap().clone());
        if page["has_more"]==false {break;}
        after = page["next"].as_i64().unwrap();
    }
    assert_eq!(paged,*records);
}

#[test]
fn host_audit_pages_keep_large_historical_decisions_whole_under_the_transport_bound() {
    let (connection,live) = setup();
    let text = "界".repeat(1000);
    for index in 0..20 {
        connection.execute("INSERT INTO chief_decisions (id,scope_key,decision_key,question,recommendation,state,revision,created_at) VALUES (?1,?1,'key',?2,?2,'resolved',1,?3)",
            rusqlite::params![format!("large-{index}"),text,index]).unwrap();
    }
    let mut after = 0;
    let mut decision_count = 0;
    let mut generation=None;
    loop {
        let page = store::audit_page(&connection,&live,after,50,generation.as_deref(),100).unwrap();
        generation=Some(page["generation"].as_str().unwrap().to_string());
        assert!(serde_json::to_vec(&page).unwrap().len()<65_000);
        for record in page["records"].as_array().unwrap() {
            if record["kind"]=="decision" {assert_eq!(record["question"],text);decision_count+=1;}
        }
        if page["has_more"]==false {break;}
        assert!(page["next"].as_i64().unwrap()>after);
        after = page["next"].as_i64().unwrap();
    }
    assert_eq!(decision_count,20);
}
