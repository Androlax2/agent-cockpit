use cockpit::chief::{store, ChiefAgentAction, ChiefHostAction, WorkBrief, WorkRef, WorkerState};
use cockpit::teams::{ErrorCode, Role, TaskKey, VerifiedSession};
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
        session, role:None, ready:false, blocked_by:vec!["host_role_unbound".into()], observed_at_ms:100,
    }).collect();
    (connection,live)
}

fn proposal(key: &str, project: &str, text: &str) -> ChiefAgentAction {
    serde_json::from_value(json!({"action":"propose_work","submission_key":key,"proposal":{
        "task":{"project":project,"task_id":"aaaaaaaa"},"brief":text,"role":"developer","prerequisites":[],"gates":[]
    }})).expect("typed proposal intake")
}

fn propose(connection: &mut Connection, live: &[WorkerState], key: &str, text: &str) -> Value {
    store::agent(connection,&live[1].session,&proposal(key,"path:/a",text),live,100).unwrap().result.unwrap()
}

#[test]
fn proposals_are_immutable_host_numbered_and_do_not_authorize_work() {
    let (mut connection,live) = setup();
    let first = propose(&mut connection,&live,"one","First exact scope");
    assert_eq!(first["brief"]["work"]["revision"],1);
    assert_eq!(first["sender"],json!(live[1].session));
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_work_revisions",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(propose(&mut connection,&live,"one","First exact scope"),first);
    let second = propose(&mut connection,&live,"two","Changed exact scope");
    assert_eq!(second["brief"]["work"]["revision"],2);
    assert_ne!(first["id"],second["id"]);
    assert!(store::agent(&mut connection,&live[1].session,&proposal("one","path:/a","Different"),&live,100).unwrap().result.is_err());
    let approve = |record: &Value| ChiefHostAction::ApproveWork {request_id:record["id"].as_str().unwrap().into(),
        brief:serde_json::from_value(record["brief"].clone()).unwrap(),approval_hash:record["approval_hash"].as_str().unwrap().into(),
        expected_revision:record["brief"]["work"]["revision"].as_i64().unwrap()-1};
    assert_eq!(store::host(&mut connection,&approve(&first),&live,100).unwrap().result.unwrap_err().code,ErrorCode::Conflict);
    store::host(&mut connection,&approve(&second),&live,100).unwrap().result.unwrap();
    let third = propose(&mut connection,&live,"three","Changed exact scope");
    assert_eq!(third["brief"]["work"]["revision"],3);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_work_revisions WHERE state='approved'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    assert!(connection.execute("UPDATE chief_work_proposals SET brief='{}'",[]).is_err());
}

#[test]
fn only_active_chief_can_propose_foreign_work_and_provenance_cannot_be_forged() {
    let (mut connection,live) = setup();
    assert!(store::agent(&mut connection,&live[1].session,&proposal("foreign","path:/b","Foreign"),&live,100).unwrap().result.is_err());
    let mut dependency = serde_json::to_value(proposal("dependency","path:/a","Dependency")).unwrap();
    dependency["proposal"]["prerequisites"] = json!([{"task":{"project":"path:/b","task_id":"bbbbbbbb"},"revision":1}]);
    assert!(store::agent(&mut connection,&live[1].session,&serde_json::from_value(dependency).unwrap(),&live,100).unwrap().result.is_err());
    store::host(&mut connection,&ChiefHostAction::Enable {request_id:"enable".into(),session:live[0].session.clone()},&live,100).unwrap().result.unwrap();
    admit_events(&mut connection,&live,"foreign-proposal",100);
    store::agent(&mut connection,&live[0].session,&proposal("foreign","path:/b","Foreign"),&live,100).unwrap().result.unwrap();
    let mut forged = live[1].session.clone(); forged.runtime = "forged".into();
    assert!(store::agent(&mut connection,&forged,&proposal("forged","path:/a","Forged"),&live,100).unwrap().result.is_err());
    for field in ["sender","approval_hash","revision"] {
        let mut value = serde_json::to_value(proposal("wire","path:/a","Wire")).unwrap();
        value["proposal"][field] = json!("forged");
        assert!(serde_json::from_value::<ChiefAgentAction>(value).is_err());
    }
}

#[test]
fn version_seven_upgrade_preserves_approved_work_and_adds_immutable_proposal_intake() {
    let (mut connection,live) = setup();
    let pending = propose(&mut connection,&live,"before","Scope");
    let brief: WorkBrief = serde_json::from_value(pending["brief"].clone()).unwrap();
    store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"approve".into(),approval_hash:brief.approval_hash().unwrap(),brief,expected_revision:0},&live,100).unwrap().result.unwrap();
    connection.execute_batch("DROP TRIGGER chief_work_proposal_immutable; DROP TRIGGER chief_work_proposal_no_delete; DROP TABLE chief_work_proposals; PRAGMA user_version=7;").unwrap();
    store::migrate(&mut connection).unwrap();
    assert_eq!(connection.query_row("SELECT state FROM chief_work_revisions",[],|row|row.get::<_,String>(0)).unwrap(),"approved");
    let next = propose(&mut connection,&live,"after","New scope");
    assert_eq!(next["brief"]["work"]["revision"],2);
    assert!(connection.execute("DELETE FROM chief_work_proposals",[]).is_err());
    store::migrate(&mut connection).unwrap();
    assert_eq!(propose(&mut connection,&live,"after","New scope"),next);
}

#[test]
fn proposal_intake_is_bounded_and_replays_do_not_consume_capacity() {
    let (mut connection,live) = setup();
    for index in 0..256 {
        let mut action = proposal(&format!("key-{index}"),"path:/a","Bounded scope");
        if let ChiefAgentAction::ProposeWork {proposal,..} = &mut action {proposal.task.task_id=format!("{index:08x}");}
        store::agent(&mut connection,&live[1].session,&action,&live,index*60_000).unwrap().result.unwrap();
    }
    assert_eq!(store::agent(&mut connection,&live[1].session,&proposal("overflow","path:/a","Overflow"),&live,256*60_000).unwrap().result.unwrap_err().code,ErrorCode::Limit);
    let mut replay = proposal("key-0","path:/a","Bounded scope");
    if let ChiefAgentAction::ProposeWork {proposal,..} = &mut replay {proposal.task.task_id="00000000".into();}
    store::agent(&mut connection,&live[1].session,&replay,&live,256*60_000).unwrap().result.unwrap();
    let mut replacement = replay.clone();
    if let ChiefAgentAction::ProposeWork {submission_key,proposal} = &mut replacement { *submission_key="replacement".into(); proposal.brief="New exact scope".into(); }
    let replacement = store::agent(&mut connection,&live[1].session,&replacement,&live,256*60_000).unwrap().result.unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_work_proposals",[],|row|row.get::<_,i64>(0)).unwrap(),257);
    let brief: WorkBrief = serde_json::from_value(replacement["brief"].clone()).unwrap();
    store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"free-pending-slot".into(),expected_revision:brief.work.revision-1,
        approval_hash:replacement["approval_hash"].as_str().unwrap().into(),brief},&live,256*60_000).unwrap().result.unwrap();
    store::agent(&mut connection,&live[1].session,&proposal("overflow","path:/a","Overflow"),&live,256*60_000).unwrap().result.unwrap();
    assert!(store::agent(&mut connection,&live[1].session,&proposal("large","path:/a",&"a".repeat(4001)),&live,100).unwrap().result.is_err());
    assert!(store::agent(&mut connection,&live[1].session,&ChiefAgentAction::Context {after:0,limit:50},&live,100).unwrap().result.is_err());
}

#[test]
fn historical_approved_and_superseded_proposals_do_not_consume_pending_capacity() {
    let (mut connection,live) = setup();
    for index in 0..270 {
        let now = index*60_000;
        let record = store::agent(&mut connection,&live[1].session,&proposal(&format!("history-{index}"),"path:/a","Historical scope"),&live,now).unwrap().result.unwrap();
        if index%2==0 {
            let brief: WorkBrief = serde_json::from_value(record["brief"].clone()).unwrap();
            store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:format!("approve-{index}"),expected_revision:brief.work.revision-1,
                approval_hash:record["approval_hash"].as_str().unwrap().into(),brief},&live,now).unwrap().result.unwrap();
        }
    }
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_work_proposals",[],|row|row.get::<_,i64>(0)).unwrap(),270);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_work_revisions",[],|row|row.get::<_,i64>(0)).unwrap(),135);
    let record = store::agent(&mut connection,&live[1].session,&proposal("after-history","path:/a","Current scope"),&live,270*60_000).unwrap().result.unwrap();
    assert_eq!(record["brief"]["work"]["revision"],271);
    let stale = proposal("history-1","path:/a","Historical scope");
    let stale = store::agent(&mut connection,&live[1].session,&stale,&live,270*60_000).unwrap().result.unwrap();
    let brief: WorkBrief = serde_json::from_value(stale["brief"].clone()).unwrap();
    assert_eq!(store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"stale-history".into(),expected_revision:brief.work.revision-1,
        approval_hash:stale["approval_hash"].as_str().unwrap().into(),brief},&live,270*60_000).unwrap().result.unwrap_err().code,ErrorCode::Conflict);
}

#[test]
fn new_proposals_are_limited_to_ten_per_sender_per_minute_without_charging_replays() {
    let (mut connection,live) = setup();
    for index in 0..10 {propose(&mut connection,&live,&format!("rate-{index}"),"Rate scope");}
    let action = proposal("rate-eleven","path:/a","Eleventh");
    assert_eq!(store::agent(&mut connection,&live[1].session,&action,&live,100).unwrap().result.unwrap_err().code,ErrorCode::Limit);
    assert_eq!(store::agent(&mut connection,&live[1].session,&action,&live,60_099).unwrap().result.unwrap_err().code,ErrorCode::Limit);
    propose(&mut connection,&live,"rate-0","Rate scope");
    assert_eq!(store::agent(&mut connection,&live[1].session,&proposal("rate-0","path:/a","Changed"),&live,100).unwrap().result.unwrap_err().code,ErrorCode::Conflict);
    store::agent(&mut connection,&live[0].session,&proposal("other-sender","path:/a","Independent"),&live,100).unwrap().result.unwrap();
    let mut replacement_live = live.clone();
    replacement_live[1].session.runtime="replacement-runtime".into();
    assert_eq!(store::agent(&mut connection,&replacement_live[1].session,&action,&replacement_live,100).unwrap().result.unwrap_err().code,ErrorCode::Limit);
    store::agent(&mut connection,&live[1].session,&action,&live,60_100).unwrap().result.unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_work_proposals",[],|row|row.get::<_,i64>(0)).unwrap(),12);
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
    let (mut connection,live)=setup();
    let current=propose(&mut connection,&live,"current-review","Current exact proposal");
    connection.execute_batch("WITH RECURSIVE history(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM history WHERE n<26000)
        INSERT INTO chief_events(id,source,submission_key,fingerprint,kind,body,created_at)
        SELECT 'history-'||n,'host','history-'||n,'fixture','status','{}',n FROM history;").unwrap();
    let page=store::snapshot_page(&connection,&live,0,50,None,100).unwrap();
    assert_eq!(page["has_more"],false,"current review must not traverse terminal audit history");
    assert!(page["records"].as_array().unwrap().iter().any(|record|record["id"]==current["id"]));
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
    let status = ChiefAgentAction::Status {assignment:None,summary:"Coordination report".into()};
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
        let status = ChiefAgentAction::Status {assignment:None,summary:"Unchanged progress".into()};
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
    let decision = |question: &str| ChiefAgentAction::Decision {work:None,key:"missing-proof".into(),question:question.into(),recommendation:"Host must review evidence; no permission is implied".into()};
    let first = store::agent(&mut connection,&live[0].session,&decision("Missing approved test result"),&live,100).unwrap().result.unwrap();
    for _ in 0..5 {assert_eq!(store::agent(&mut connection,&live[0].session,&decision("Missing approved test result"),&live,100).unwrap().result.unwrap(),first);}
    let changed = store::agent(&mut connection,&live[0].session,&decision("Test source is now present but does not pass"),&live,100).unwrap().result.unwrap();
    assert_eq!(changed["decision"],first["decision"]);
    assert_eq!(changed["revision"],2);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_attention",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    assert!(store::agent(&mut connection,&live[1].session,&decision("Forged"),&live,100).unwrap().result.is_err());
    store::host(&mut connection,&ChiefHostAction::Resolve {request_id:"resolve".into(),decision:first["decision"].as_str().unwrap().into(),expected_revision:2,resolution:"Acknowledged; still no execution authorization".into()},&live,100).unwrap().result.unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_evidence",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    let action = ChiefAgentAction::ContextSnapshot {after:0,limit:50,generation:None};
    let snapshot = store::agent(&mut connection,&live[0].session,&action,&live,100).unwrap().result.unwrap();
    let decision = snapshot["records"].as_array().unwrap().iter().find(|record|record["kind"]=="decision").expect("current host control state survives cursor recovery");
    assert_eq!(decision["state"],"resolved");
    assert_eq!(decision["source"],"protected_host_control");
    let new_evidence = ChiefAgentAction::Decision {work:None,key:"missing-proof".into(),question:"New source contradicts the previously reviewed result".into(),recommendation:"Host must review this new revision".into()};
    store::agent(&mut connection,&live[0].session,&new_evidence,&live,100).unwrap().result.unwrap();
    assert_eq!(connection.query_row("SELECT state FROM chief_decisions WHERE decision_key='missing-proof'",[],|row|row.get::<_,String>(0)).unwrap(),"open");
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_attention",[],|row|row.get::<_,i64>(0)).unwrap(),1);
}

#[test]
fn reopening_at_capacity_preserves_the_answered_revision_but_open_material_updates_remain_allowed() {
    let (mut connection,live) = setup(); enable_events(&mut connection,&live); admit_events(&mut connection,&live,"bootstrap",100);
    let action = |key: &str, question: &str| ChiefAgentAction::Decision {work:None,key:key.into(),question:question.into(),recommendation:"Host review only, no grants".into()};
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
    let approved = WorkBrief {work:WorkRef {task:TaskKey {project:"path:/a".into(),task_id:"bbbbbbbb".into()},revision:1},brief:"Exact approved scope".into(),role:Role::Developer,prerequisites:vec![],gates:vec![]};
    store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"control-work".into(),approval_hash:approved.approval_hash().unwrap(),brief:approved.clone(),expected_revision:0},&live,100).unwrap().result.unwrap();
    admit_events(&mut connection,&live,"bootstrap",100);
    let decision = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Decision {work:Some(approved.work.clone()),key:"exact-control".into(),question:"PRIVATE question".into(),recommendation:"PRIVATE recommendation".into()},&live,100).unwrap().result.unwrap();
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
    assert_eq!(event["work"],json!(approved.work));
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
fn evidence_events_expose_only_protected_work_and_evidence_identities() {
    use cockpit::chief::{Evidence,GateKind,GateRequirement};
    let (mut connection,mut live) = setup(); enable_events(&mut connection,&live);
    let approved = WorkBrief {work:WorkRef {task:TaskKey {project:"path:/a".into(),task_id:"bbbbbbbb".into()},revision:1},brief:"Approved gated scope".into(),role:Role::Developer,prerequisites:vec![],
        gates:vec![GateRequirement {kind:GateKind::Tests,source:"host_attestation".into(),expected:json!({"commit":"1".repeat(40)})}]};
    store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"metadata-work".into(),approval_hash:approved.approval_hash().unwrap(),brief:approved.clone(),expected_revision:0},&live,100).unwrap().result.unwrap();
    admit_events(&mut connection,&live,"bootstrap",100);
    let evidence = Evidence {id:"metadata-proof".into(),work:approved.work.clone(),kind:GateKind::Tests,commit:"1".repeat(40),source:"host_attestation".into(),result:json!({"passed":true,"note":"PRIVATE result prose"})};
    store::host(&mut connection,&ChiefHostAction::AttestEvidence {request_id:"metadata-attest".into(),evidence},&live,101).unwrap().result.unwrap();
    let sequence: i64 = connection.query_row("SELECT seq FROM chief_events WHERE kind='evidence_recorded'",[],|row|row.get(0)).unwrap();
    connection.execute("UPDATE chief_events SET body=json_set(body,'$.full_request','PRIVATE registry request','$.result',json_object('note','PRIVATE body prose')) WHERE seq=?1",[sequence]).unwrap();
    for worker in &mut live {worker.observed_at_ms=30_100;}
    admit_events(&mut connection,&live,"proof-control",30_100);
    let context = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Context {after:sequence-1,limit:1},&live,30_100).unwrap().result.unwrap();
    assert_eq!(context["snapshot_required"],false);
    assert_eq!(context["records"][0]["evidence_id"],"metadata-proof");
    assert_eq!(context["records"][0]["work"],json!(approved.work));
    assert!(!context.to_string().contains("PRIVATE"));
}

#[test]
fn concurrent_reopenings_share_one_available_decision_slot() {
    use std::sync::{Arc,Barrier};
    let sandbox = common::sandbox::Sandbox::new(); let path = sandbox.root.join("reopen.sqlite");
    let (memory,live) = setup(); drop(memory);
    let mut connection = Connection::open(&path).unwrap(); store::migrate(&mut connection).unwrap(); enable_events(&mut connection,&live); admit_events(&mut connection,&live,"bootstrap",100);
    for index in 0..2 {
        let action = ChiefAgentAction::Decision {work:None,key:format!("reopen-{index}"),question:"Original blocker".into(),recommendation:"Review only".into()};
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
            let action = ChiefAgentAction::Decision {work:None,key:format!("reopen-{index}"),question:"Materially changed blocker".into(),recommendation:"Review new evidence only".into()};
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
        assert!(store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {assignment:None,summary:"No authority".into()},&live,100).unwrap().result.is_err());
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
        connection.execute("INSERT INTO chief_events(id,source,submission_key,fingerprint,assignment,kind,body,created_at) VALUES (?1,'host',?1,'test','bounded-assignment','assignment_accepted','{}',100)",[format!("control-{index}")]).unwrap();
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
    for _ in 0..20 {store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {assignment:None,summary:"Durable turn budget".into()},&live,101).unwrap().result.unwrap();}
    drop(connection);
    let mut connection = Connection::open(&path).unwrap(); store::migrate(&mut connection).unwrap();
    assert_eq!(store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {assignment:None,summary:"Reopen must not reset allowance".into()},&live,101).unwrap().result.unwrap_err().code,ErrorCode::Limit);
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
    store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {assignment:None,summary:"New admitted boundary has its own bounded allowance".into()},&live,30_100).unwrap().result.unwrap();
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
        "sections":{"completed":"No verified completions","active":"No active work","decisions":"None","idle":"No approved work"},"evidence_ids":["unverified-agent-claim"]})).unwrap();
    assert!(store::agent(&mut connection,&live[0].session,&response("not-requested"),&live,100).unwrap().result.is_err());
    assert!(store::agent(&mut connection,&live[1].session,&response("summary-result"),&live,100).unwrap().result.is_err());
    let result = store::agent(&mut connection,&live[0].session,&response("summary-result"),&live,100).unwrap().result.unwrap();
    assert_eq!(result["state"],"completed");
    let record: String = connection.query_row("SELECT response FROM chief_summary_requests WHERE id='summary-result'",[],|row|row.get(0)).unwrap();
    let record: Value = serde_json::from_str(&record).unwrap();
    assert_eq!(record["provenance"],"chief_narrative_unverified");
    assert_eq!(record["references"][0]["verified_current"],false);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_evidence",[],|row|row.get::<_,i64>(0)).unwrap(),0);
}

#[test]
fn recording_evidence_updates_a_blocker_without_answering_the_host_decision() {
    use cockpit::chief::{Evidence,GateKind,GateRequirement};
    let (mut connection,mut live) = setup(); enable_events(&mut connection,&live);
    store::host(&mut connection,&ChiefHostAction::RequestSummary {request_id:"evidence-summary".into()},&live,100).unwrap().result.unwrap();
    admit_events(&mut connection,&live,"bootstrap",100);
    let approved = WorkBrief {work:WorkRef {task:TaskKey {project:"path:/a".into(),task_id:"bbbbbbbb".into()},revision:1},brief:"Exact gated scope".into(),role:Role::Developer,prerequisites:vec![],
        gates:vec![GateRequirement {kind:GateKind::Tests,source:"host_attestation".into(),expected:json!({"commit":"1".repeat(40)})}]};
    store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"approve-blocked".into(),approval_hash:approved.approval_hash().unwrap(),brief:approved.clone(),expected_revision:0},&live,100).unwrap().result.unwrap();
    store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Decision {work:Some(approved.work.clone()),key:"missing_evidence".into(),question:"Missing gate proof".into(),recommendation:"Host must review".into()},&live,100).unwrap().result.unwrap();
    let evidence = Evidence {id:"exact-proof".into(),work:approved.work.clone(),kind:GateKind::Tests,commit:"1".repeat(40),source:"host_attestation".into(),result:json!({"passed":true,"note":"Explicit host attestation"})};
    store::host(&mut connection,&ChiefHostAction::AttestEvidence {request_id:"attest".into(),evidence},&live,101).unwrap().result.unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_events WHERE kind='evidence_recorded'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_events WHERE kind='work_completed'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(connection.query_row("SELECT state FROM chief_decisions WHERE decision_key='missing_evidence'",[],|row|row.get::<_,String>(0)).unwrap(),"open");
    assert_eq!(connection.query_row("SELECT revision FROM chief_decisions WHERE decision_key='missing_evidence'",[],|row|row.get::<_,i64>(0)).unwrap(),2);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_attention",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    let context = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::ContextSnapshot {after:0,limit:50,generation:None},&live,101).unwrap().result.unwrap();
    assert!(context["records"].as_array().unwrap().iter().any(|record|record["kind"]=="work" && record["state"]=="completed"));
    assert!(context["records"].as_array().unwrap().iter().any(|record|record["kind"]=="evidence" && record["id"]=="exact-proof" && record["source"]=="host_attestation"));
    let response = serde_json::from_value::<ChiefAgentAction>(json!({"action":"summary_response","request_id":"evidence-summary","sections":{"completed":"One protected test gate passed","active":"None","decisions":"Host review pending","idle":"No capacity claim"},"evidence_ids":["exact-proof"]})).unwrap();
    store::agent(&mut connection,&live[0].session,&response,&live,101).unwrap().result.unwrap();
    let response: String = connection.query_row("SELECT response FROM chief_summary_requests WHERE id='evidence-summary'",[],|row|row.get(0)).unwrap();
    let response: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["references"][0]["verified_current"],true);
    for (id,passed) in [("later-failure",false),("later-pass",true)] {
        let evidence = Evidence {id:id.into(),work:approved.work.clone(),kind:GateKind::Tests,commit:"1".repeat(40),source:"host_attestation".into(),result:json!({"passed":passed,"note":"Later host finding"})};
        store::host(&mut connection,&ChiefHostAction::AttestEvidence {request_id:id.into(),evidence},&live,102).unwrap().result.unwrap();
        let snapshot = store::snapshot(&connection,&live,0,50).unwrap();
        let summary = snapshot["records"].as_array().unwrap().iter().find(|record|record["kind"]=="summary").unwrap();
        assert_eq!(summary["last_result"]["references"][0]["verified_current"],false,"superseded proof is historical even after another pass");
        assert_eq!(connection.query_row("SELECT state FROM chief_work_revisions WHERE task_id='bbbbbbbb'",[],|row|row.get::<_,String>(0)).unwrap(),if passed {"completed"} else {"approved"});
        let now=if passed {60_100} else {30_100};
        store::host(&mut connection,&ChiefHostAction::RequestSummary {request_id:format!("summary-{id}")},&live,now).unwrap().result.unwrap();
        for worker in &mut live {worker.observed_at_ms=now;}
        admit_events(&mut connection,&live,id,now);
        let response=serde_json::from_value::<ChiefAgentAction>(json!({"action":"summary_response","request_id":format!("summary-{id}"),"sections":{"completed":"Current references only","active":"None","decisions":"Unchanged","idle":"None"},"evidence_ids":["exact-proof",id]})).unwrap();
        store::agent(&mut connection,&live[0].session,&response,&live,now).unwrap().result.unwrap();
        let snapshot=store::snapshot(&connection,&live,0,50).unwrap();let summary=snapshot["records"].as_array().unwrap().iter().find(|record|record["kind"]=="summary").unwrap();
        assert_eq!(summary["last_result"]["references"][0]["verified_current"],false);
        assert_eq!(summary["last_result"]["references"][1]["verified_current"],passed);
    }
    store::host(&mut connection,&ChiefHostAction::RevokeWork {request_id:"stale-summary-source".into(),work:approved.work},&live,102).unwrap().result.unwrap();
    let snapshot = store::snapshot(&connection,&live,0,50).unwrap();
    let summary = snapshot["records"].as_array().unwrap().iter().find(|record|record["kind"]=="summary").unwrap();
    assert_eq!(summary["last_result"]["references"][0]["verified_current"],false);
    assert_eq!(summary["last_completed_at_ms"],60_100);
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
    let status=ChiefAgentAction::Status {assignment:None,summary:"Cannot spend bootstrap allowance".into()};
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
    store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {assignment:None,summary:"Replacement native allowance remains usable".into()},&live,104).unwrap().result.unwrap();
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
        assert!(store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {assignment:None,summary:"Old allowance must stay closed".into()},&live,30_100).unwrap().result.is_err());
    }
}

#[test]
fn chief_running_then_idle_is_not_its_own_material_wake_source() {
    let (mut connection,mut live) = setup(); enable_events(&mut connection,&live); admit_events(&mut connection,&live,"bootstrap",100);
    live[0].blocked_by=vec!["native_busy".into()];
    store::observe(&mut connection,&live,101).unwrap();
    live[0].blocked_by=vec!["host_role_unbound".into()];
    for worker in &mut live {worker.observed_at_ms=30_100;}
    store::observe(&mut connection,&live,30_100).unwrap();
    assert!(store::claim(&mut connection,&live[0].session,"must-not-loop",30_100).unwrap().is_none());
}

#[test]
fn an_expired_summary_preserves_the_previous_result_timestamp_and_does_not_regenerate() {
    let (mut connection,mut live) = setup(); enable_events(&mut connection,&live);
    store::host(&mut connection,&ChiefHostAction::RequestSummary {request_id:"first-summary".into()},&live,100).unwrap().result.unwrap();
    admit_events(&mut connection,&live,"bootstrap",100);
    let reply = serde_json::from_value::<ChiefAgentAction>(json!({"action":"summary_response","request_id":"first-summary","sections":{"completed":"None","active":"None","decisions":"None","idle":"No approved work"},"evidence_ids":[]})).unwrap();
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
    live[0].blocked_by=vec!["host_role_unbound".into()];
    for worker in &mut live {worker.observed_at_ms=300_100;}
    let wake=admit_events(&mut connection,&live,"after-idle",300_100);
    assert_eq!(wake.envelope["summary_requests"],json!(["busy-summary"]));
    let reply=serde_json::from_value::<ChiefAgentAction>(json!({"action":"summary_response","request_id":"busy-summary","sections":{"completed":"None","active":"None","decisions":"None","idle":"No approved work"},"evidence_ids":[]})).unwrap();
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
        let action = ChiefAgentAction::Dispatch {submission_key:format!("unapproved-{index}"),work:WorkRef {task:TaskKey {project:"path:/a".into(),task_id:"bbbbbbbb".into()},revision:1},worker:live[1].session.session_id.clone()};
        assert_eq!(store::agent(&mut connection,&live[0].session,&action,&live,100).unwrap().result.unwrap_err().code,ErrorCode::Conflict);
    }
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_assignments",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {assignment:None,summary:"No new reasoning turn".into()},&live,100).unwrap().result.unwrap_err().code,ErrorCode::Limit);
}

#[test]
fn large_redacted_snapshots_continue_at_a_new_native_boundary_without_invalidating_their_own_generation() {
    let (mut connection,mut live) = setup(); enable_events(&mut connection,&live);
    for index in 0..60 {
        let approved = WorkBrief {work:WorkRef {task:TaskKey {project:"path:/a".into(),task_id:format!("{index:08x}")},revision:1},brief:"Approved bounded scope".into(),role:Role::Developer,prerequisites:vec![],gates:vec![]};
        store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:format!("snapshot-{index}"),approval_hash:approved.approval_hash().unwrap(),brief:approved,expected_revision:0},&live,100).unwrap().result.unwrap();
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
        let action = ChiefAgentAction::Decision {work:None,key:key.into(),question:"No scheduling action is currently available".into(),recommendation:"Wait silently for material work".into()};
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
    let triggers = connection.prepare("SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'review_%'").unwrap().query_map([],|row|row.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    for trigger in triggers {connection.execute_batch(&format!("DROP TRIGGER {trigger}")).unwrap();}
    connection.execute_batch("DELETE FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_wake'; DELETE FROM chief_events WHERE kind='bootstrap';
        DROP INDEX chief_one_queued_wake; DROP TABLE chief_coordination; DROP TABLE chief_turns; DROP TABLE chief_attention; DROP TABLE chief_summary_requests; DROP TABLE chief_review_generation; PRAGMA user_version=8;").unwrap();
    store::migrate(&mut connection).unwrap();
    assert_eq!(enable_events(&mut connection,&live),grant);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_wake'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    assert!(store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {assignment:None,summary:"Migration is not native admission".into()},&live,100).unwrap().result.is_err());
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
        let result = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Status {assignment:None,summary:"Must not execute using a different or failed native turn".into()},&live,30_100).unwrap().result;
        assert!(result.is_err(),"{transition}");
        assert_eq!(connection.query_row("SELECT count(*) FROM chief_events WHERE kind='status'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    }
}

#[test]
fn host_review_exposes_full_records_while_agent_context_stays_redacted() {
    let (mut connection,mut live) = setup();
    store::host(&mut connection,&ChiefHostAction::Enable {request_id:"enable".into(),session:live[0].session.clone()},&live,100).unwrap().result.unwrap();
    live[1].role = Some(Role::Developer);
    let binding = store::host(&mut connection,&ChiefHostAction::BindWorker {request_id:"bind".into(),session:live[1].session.clone(),role:Role::Developer,expected_epoch:None},&live,100).unwrap().result.unwrap();
    let pending = propose(&mut connection,&live,"pending","Private full proposed scope");
    let approved = WorkBrief {work:WorkRef {task:TaskKey {project:"path:/a".into(),task_id:"bbbbbbbb".into()},revision:1},brief:"Approved scope".into(),role:Role::Developer,prerequisites:vec![],
        gates:vec![cockpit::chief::GateRequirement {kind:cockpit::chief::GateKind::Tests,source:"host_attestation".into(),expected:json!({"commit":"1".repeat(40)})}]};
    store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"approve".into(),brief:approved.clone(),approval_hash:approved.approval_hash().unwrap(),expected_revision:0},&live,100).unwrap().result.unwrap();
    let source = cockpit::chief::Evidence {id:"source".into(),work:approved.work.clone(),kind:cockpit::chief::GateKind::Tests,commit:"1".repeat(40),source:"host_attestation".into(),result:json!({"passed":false,"note":"Private source detail"})};
    store::publish_source(&mut connection,&source,101).unwrap();
    connection.execute("INSERT INTO chief_decisions (id,scope_key,decision_key,question,recommendation,state,revision,created_at) VALUES ('decision','scope','key','Review exact evidence','Private recommendation','open',3,100)",[]).unwrap();
    let review = store::snapshot(&connection,&live,0,50).unwrap();
    let records = review["records"].as_array().unwrap();
    let record = |kind: &str| records.iter().find(|record|record["kind"]==kind).unwrap();
    assert_eq!(record("chief")["session"],json!(live[0].session));
    assert_eq!(record("worker")["session"]["runtime"],live[0].session.runtime);
    assert_eq!(record("binding")["epoch"],binding["epoch"]);
    assert_eq!(record("proposal")["id"],pending["id"]);
    assert_eq!(record("proposal")["freshness"],"pending");
    assert_eq!(record("proposal")["brief"],pending["brief"]);
    assert_eq!(record("work")["brief"],json!(approved));
    assert_eq!(record("work")["approval_hash"],approved.approval_hash().unwrap());
    assert_eq!(record("source")["evidence"],json!(source));
    assert_eq!(record("source")["can_import"],false);
    assert_eq!(record("decision")["revision"],3);
    assert_eq!(record("summary")["state"],"unavailable");
    let context = store::agent(&mut connection,&live[0].session,&ChiefAgentAction::Context {after:0,limit:50},&live,101).unwrap().result.unwrap().to_string();
    for secret in ["Private full proposed scope","Private source detail","Private recommendation",live[1].session.runtime.as_str(),binding["epoch"].as_str().unwrap()] {assert!(!context.contains(secret),"{context}");}
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
fn host_audit_pages_keep_large_historical_proposals_whole_under_the_transport_bound() {
    let (mut connection,live) = setup();
    let text = "界".repeat(4000);
    for index in 0..20 {store::agent(&mut connection,&live[1].session,&proposal(&format!("large-{index}"),"path:/a",&text),&live,index*60_000).unwrap().result.unwrap();}
    let mut after = 0;
    let mut proposal_count = 0;
    let mut generation=None;
    loop {
        let page = store::audit_page(&connection,&live,after,50,generation.as_deref(),100).unwrap();
        generation=Some(page["generation"].as_str().unwrap().to_string());
        assert!(serde_json::to_vec(&page).unwrap().len()<65_000);
        for record in page["records"].as_array().unwrap() {
            if record["kind"]=="proposal" {assert_eq!(record["brief"]["brief"],text);proposal_count+=1;}
        }
        if page["has_more"]==false {break;}
        assert!(page["next"].as_i64().unwrap()>after);
        after = page["next"].as_i64().unwrap();
    }
    assert_eq!(proposal_count,20);
}

#[test]
fn evidence_payloads_reserve_room_for_complete_host_review_metadata() {
    let evidence = cockpit::chief::Evidence {id:"large".into(),work:WorkRef {task:TaskKey {project:"path:/a".into(),task_id:"aaaaaaaa".into()},revision:1},
        kind:cockpit::chief::GateKind::Tests,commit:"1".repeat(40),source:"host_attestation".into(),result:json!({"passed":true,"note":"n".repeat(58_000)})};
    assert_eq!(evidence.validate().unwrap_err().code,ErrorCode::Limit);
}

#[test]
fn brief_payloads_reserve_room_for_complete_host_review_metadata() {
    let prerequisites = (0..180).map(|index|WorkRef {task:TaskKey {project:format!("path:/{}","p".repeat(260)),task_id:format!("{index:08x}")},revision:1}).collect();
    let brief = WorkBrief {work:WorkRef {task:TaskKey {project:"path:/a".into(),task_id:"aaaaaaaa".into()},revision:1},brief:"Scope".into(),
        role:Role::Developer,prerequisites,gates:vec![]};
    let bytes = serde_json::to_vec(&brief).unwrap().len();
    assert!((56_001..60_000).contains(&bytes),"{bytes}");
    assert_eq!(brief.validate().unwrap_err().code,ErrorCode::Limit);
}
