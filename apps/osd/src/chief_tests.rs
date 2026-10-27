use super::*;
use cockpit::chief::{ChiefAgentAction, ChiefHostAction};
use cockpit::teams::{AgentAction, ErrorCode, HostRequest, VerifiedSession};

#[path = "chief_workflow_tests.rs"]
mod workflow;

fn start(bench: &mut Bench, request: &str) -> serde_json::Value {
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief {
        action: ChiefHostAction::Start { request_id: request.into(), previous_attempt: None },
    }).unwrap().result.unwrap()
}

#[test]
fn chief_start_dispatches_only_once_across_repeated_requests_and_ticks() {
    let mut bench = team_bench();
    let first = start(&mut bench, "start-one");
    start(&mut bench, "start-two");
    for _ in 0..3 { crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap(); }
    let world = bench.world.borrow();
    let launches: Vec<_> = world.started.iter().filter(|command|command.iter().any(|argument|argument=="chief-start")).collect();
    assert_eq!(launches.len(), 1);
    assert!(launches[0].contains(&first["session_id"].as_str().unwrap().to_string()));
    let startup = cockpit::chief::store::startup(bench.registry.teams.as_ref().unwrap()).unwrap().unwrap();
    assert_eq!(startup["state"], "checking");
    assert_eq!(startup["can_retry"], false);
}

#[test]
fn chief_snapshot_reads_never_dispatch_a_reserved_start() {
    let mut bench = team_bench();
    start(&mut bench, "start");
    for _ in 0..2 { crate::chief::snapshot(&mut bench.registry, 0, 50, None).unwrap(); }
    assert!(bench.world.borrow().started.iter().all(|command|!command.iter().any(|argument|argument=="chief-start")));
}

#[test]
fn chief_launch_that_cannot_start_returns_its_error_and_allows_retry() {
    let mut bench = team_bench();
    bench.world.borrow_mut().start_fails = true;
    start(&mut bench, "start");
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    let startup = cockpit::chief::store::startup(bench.registry.teams.as_ref().unwrap()).unwrap().unwrap();
    assert_eq!(startup["state"], "failed");
    assert_eq!(startup["can_retry"], true);
    assert!(startup["error"].as_str().is_some_and(|message|!message.is_empty()));
}

fn report_reserved_chief(bench: &mut Bench, startup: &serde_json::Value, directory: &str) {
    fs::create_dir_all(directory).unwrap();
    bench.world.borrow_mut().sender_targets.insert(40, target(&[("tmux_pane", "%40")]));
    bench.world.borrow_mut().sender_directories.insert(40, directory.into());
    bench.world.borrow_mut().sender_runtimes.insert(40, "runtime-chief".into());
    let report = parse_session(&json!({"session":startup["session_id"],"agent":"opencode","project":"cockpit-chief","cwd":directory,"state":"waiting"}).to_string()).unwrap();
    bench.registry.report_from(report, 40).unwrap();
}

#[test]
fn chief_activation_waits_for_admission_then_enables_the_same_reserved_session() {
    let mut bench = team_bench();
    let startup = start(&mut bench, "start");
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    report_reserved_chief(&mut bench, &startup, startup["directory"].as_str().unwrap());
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    let waiting = cockpit::chief::store::startup(bench.registry.teams.as_ref().unwrap()).unwrap().unwrap();
    assert_eq!(waiting["state"], "checking");
    assert!(waiting["error"].as_str().unwrap().contains("verification"));
    let session = verified(&mut bench, startup["session_id"].as_str().unwrap());
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::VerifyChiefAdmission {
        session:session.clone(),adapter:"opencode-queued-v1".into(),evidence:"Controlled host-verification fixture".into(),
    }).unwrap().result.unwrap();
    for _ in 0..2 { crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap(); }
    let connection = bench.registry.teams.as_ref().unwrap();
    let active = cockpit::chief::store::startup(connection).unwrap().unwrap();
    assert_eq!(active["state"], "active");
    assert_eq!(active["session"], json!(session));
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_grants", [], |row|row.get::<_,i64>(0)).unwrap(), 1);
    assert_eq!(bench.world.borrow().started.iter().filter(|command|command.contains(&"chief-start".into())).count(), 1);
}

fn send_check(bench: &mut Bench, request: &str, attempt: &str) -> Result<serde_json::Value, cockpit::teams::TeamError> {
    crate::chief::host(&mut bench.registry, &ChiefHostAction::SendCheck { request_id: request.into(), attempt: attempt.into() }).unwrap().result
}

fn shown_startup(bench: &mut Bench) -> serde_json::Value {
    crate::chief::snapshot(&mut bench.registry, 0, 50, None).unwrap()["startup"].clone()
}

fn checks(bench: &Bench) -> i64 {
    bench.registry.teams.as_ref().unwrap().query_row("SELECT count(*) FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_check'", [], |row|row.get(0)).unwrap()
}

fn grants(bench: &Bench) -> i64 {
    bench.registry.teams.as_ref().unwrap().query_row("SELECT count(*) FROM chief_grants", [], |row|row.get(0)).unwrap()
}

#[test]
fn no_delivery_check_is_offered_or_sent_before_the_reserved_chief_is_live() {
    let mut bench = team_bench();
    let startup = start(&mut bench, "start");
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    assert!(shown_startup(&mut bench)["check_session"].is_null());
    assert!(send_check(&mut bench, "check-1", startup["id"].as_str().unwrap()).is_err());
    assert_eq!(checks(&bench), 0);
}

#[test]
fn a_delivery_check_reaches_only_the_exact_chief_and_its_confirmation_turns_it_on() {
    let mut bench = team_bench();
    let startup = start(&mut bench, "start");
    let attempt = startup["id"].as_str().unwrap().to_string();
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    report_reserved_chief(&mut bench, &startup, startup["directory"].as_str().unwrap());
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    let session = verified(&mut bench, startup["session_id"].as_str().unwrap());
    assert_eq!(shown_startup(&mut bench)["check_session"], json!(session));
    let first = send_check(&mut bench, "check-1", &attempt).unwrap();
    assert_eq!(send_check(&mut bench, "check-1", &attempt).unwrap(), first, "a repeated request replays");
    assert_eq!(send_check(&mut bench, "check-2", &attempt).unwrap()["check"], first["check"], "a pending check is never sent twice");
    assert_eq!(checks(&bench), 1);
    assert!(crate::teams::request(&mut bench.registry, 20, AgentAction::Receive).unwrap().result.unwrap().is_null(), "another session never gets it");
    let delivery = crate::teams::request(&mut bench.registry, 40, AgentAction::Receive).unwrap().result.unwrap();
    assert_eq!(delivery["envelope"]["kind"], "chief_check");
    assert!(delivery["text"].as_str().unwrap().contains("delivery check"), "{delivery}");
    crate::teams::request(&mut bench.registry, 40, AgentAction::Receipt {
        dispatch_id: delivery["dispatch_id"].as_str().unwrap().into(),
        outcome: cockpit::teams::TransportOutcome::Received { native_id: format!("msg_cockpit{}", delivery["message_id"].as_str().unwrap()) },
    }).unwrap().result.unwrap();
    assert_eq!(shown_startup(&mut bench)["check"]["state"], "received");
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    assert_eq!(grants(&bench), 0, "a received check alone is not Theo's confirmation");
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::VerifyChiefAdmission {
        session:session.clone(),adapter:"opencode-queued-v1".into(),evidence:"Theo saw the delivery check arrive".into(),
    }).unwrap().result.unwrap();
    for _ in 0..2 { crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap(); }
    let active = shown_startup(&mut bench);
    assert_eq!(active["state"], "active");
    assert_eq!(active["session"], json!(session));
    assert_eq!(grants(&bench), 1);
}

#[test]
fn a_verification_of_another_session_or_runtime_never_turns_the_chief_on() {
    let mut bench = team_bench();
    let startup = start(&mut bench, "start");
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    report_reserved_chief(&mut bench, &startup, startup["directory"].as_str().unwrap());
    let other = verified(&mut bench, OPENCODE_SESSION);
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::VerifyChiefAdmission {
        session:other,adapter:"opencode-queued-v1".into(),evidence:"Another session's check".into(),
    }).unwrap().result.unwrap();
    let mut earlier = verified(&mut bench, startup["session_id"].as_str().unwrap());
    earlier.runtime = "runtime-chief-earlier".into();
    cockpit::chief::store::verify_admission(bench.registry.teams.as_mut().unwrap(), &earlier, "opencode-queued-v1", "An earlier runtime's check", 1).unwrap();
    for _ in 0..2 { crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap(); }
    assert_eq!(shown_startup(&mut bench)["state"], "checking");
    assert_eq!(grants(&bench), 0);
}

#[test]
fn chief_report_from_another_folder_cannot_activate_the_reservation() {
    let mut bench = team_bench();
    let startup = start(&mut bench, "start");
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    let other = bench.fixture.root.join("other-project");
    report_reserved_chief(&mut bench, &startup, other.to_str().unwrap());
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    let connection = bench.registry.teams.as_ref().unwrap();
    assert!(cockpit::chief::store::chief_session(connection).unwrap().is_none());
    assert_eq!(cockpit::chief::store::startup(connection).unwrap().unwrap()["can_retry"], false);
}

#[test]
fn a_claimed_chief_start_is_not_relaunched_after_store_reopen() {
    let mut bench = team_bench();
    start(&mut bench, "start");
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    bench.registry.teams = None;
    for _ in 0..2 { crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap(); }
    assert_eq!(bench.world.borrow().started.iter().filter(|command|command.contains(&"chief-start".into())).count(), 1);
    assert_eq!(cockpit::chief::store::startup(bench.registry.teams.as_ref().unwrap()).unwrap().unwrap()["can_retry"], false);
}

#[test]
fn only_confirmed_prelaunch_failure_allows_chief_retry() {
    for (exit, expected) in [(2, "failed"), (3, "checking"), (-1, "checking")] {
        let mut bench = team_bench();
        let startup = start(&mut bench, "start");
        crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
        bench.world.borrow_mut().chief_results.insert(startup["session_id"].as_str().unwrap().into(), Ok((exit,"launch cause".into())));
        crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
        let result = cockpit::chief::store::startup(bench.registry.teams.as_ref().unwrap()).unwrap().unwrap();
        assert_eq!(result["state"], expected);
        assert!(result["error"].as_str().unwrap().contains("launch cause"));
    }
}

#[test]
fn a_chief_disabled_during_activation_ends_the_attempt_so_start_is_offered_again() {
    let mut bench = team_bench();
    let startup = start(&mut bench, "start");
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    report_reserved_chief(&mut bench, &startup, startup["directory"].as_str().unwrap());
    let session = verified(&mut bench, startup["session_id"].as_str().unwrap());
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::VerifyChiefAdmission {
        session:session.clone(),adapter:"opencode-queued-v1".into(),evidence:"Controlled fixture".into(),
    }).unwrap().result.unwrap();
    let enabled = crate::chief::host(&mut bench.registry, &ChiefHostAction::Enable {
        request_id:format!("chief-start-{}", startup["id"].as_str().unwrap()),session,
    }).unwrap().result.unwrap();
    crate::chief::host(&mut bench.registry, &ChiefHostAction::Disable { request_id:"disable".into(),expected_epoch:enabled["epoch"].as_str().unwrap().into() }).unwrap().result.unwrap();
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    let result = cockpit::chief::store::startup(bench.registry.teams.as_ref().unwrap()).unwrap().unwrap();
    assert_eq!(result["state"], "ended");
    assert!(cockpit::chief::store::chief_session(bench.registry.teams.as_ref().unwrap()).unwrap().is_none());
    let again = start(&mut bench, "start-again");
    assert_eq!(again["state"], "starting");
    assert_ne!(again["session_id"], startup["session_id"]);
}

#[test]
fn start_use_disable_then_start_again_launches_a_new_chief() {
    let mut bench = team_bench();
    let first = start(&mut bench, "start");
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    report_reserved_chief(&mut bench, &first, first["directory"].as_str().unwrap());
    let session = verified(&mut bench, first["session_id"].as_str().unwrap());
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::VerifyChiefAdmission {
        session,adapter:"opencode-queued-v1".into(),evidence:"Controlled fixture".into(),
    }).unwrap().result.unwrap();
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    let epoch = bench.registry.teams.as_ref().unwrap().query_row("SELECT epoch FROM chief_grants WHERE state='active'", [], |row|row.get::<_,String>(0)).unwrap();
    crate::chief::host(&mut bench.registry, &ChiefHostAction::Disable { request_id:"disable".into(),expected_epoch:epoch }).unwrap().result.unwrap();
    assert_eq!(cockpit::chief::store::startup(bench.registry.teams.as_ref().unwrap()).unwrap().unwrap()["state"], "ended");
    start(&mut bench, "start-again");
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    assert_eq!(bench.world.borrow().started.iter().filter(|command|command.contains(&"chief-start".into())).count(), 2);
}

#[test]
fn chief_crash_after_dispatch_claim_cannot_launch_on_recovery() {
    let mut bench = team_bench();
    start(&mut bench, "start");
    bench.registry.teams.as_ref().unwrap().execute("UPDATE chief_startup SET state='checking',revision=revision+1", []).unwrap();
    bench.registry.teams = None;
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    let result = cockpit::chief::store::startup(bench.registry.teams.as_ref().unwrap()).unwrap().unwrap();
    assert_eq!(result["state"], "checking");
    assert_eq!(result["can_retry"], false);
    assert!(bench.world.borrow().started.iter().all(|command|!command.contains(&"chief-start".into())));
}

#[test]
fn chief_activation_replay_never_transfers_the_grant_to_a_restarted_runtime() {
    let mut bench = team_bench();
    let startup = start(&mut bench, "start");
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    report_reserved_chief(&mut bench, &startup, startup["directory"].as_str().unwrap());
    let session = verified(&mut bench, startup["session_id"].as_str().unwrap());
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::VerifyChiefAdmission {
        session:session.clone(),adapter:"opencode-queued-v1".into(),evidence:"Controlled fixture".into(),
    }).unwrap().result.unwrap();
    crate::chief::host(&mut bench.registry, &ChiefHostAction::Enable {
        request_id:format!("chief-start-{}", startup["id"].as_str().unwrap()),session:session.clone(),
    }).unwrap().result.unwrap();
    bench.world.borrow_mut().sender_runtimes.insert(40, "runtime-chief-restarted".into());
    bench.registry.sessions.get_mut(&session.session_id).unwrap().runtime = "runtime-chief-restarted".into();
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();
    let connection = bench.registry.teams.as_ref().unwrap();
    assert_eq!(cockpit::chief::store::startup(connection).unwrap().unwrap()["state"], "checking");
    assert_eq!(connection.query_row("SELECT runtime FROM chief_grants WHERE session_id=?1", [&session.session_id], |row|row.get::<_,String>(0)).unwrap(), "runtime-chief");
}

fn verified(bench: &mut Bench, id: &str) -> VerifiedSession {
    crate::teams::live_sessions(&mut bench.registry).unwrap().into_iter().find(|session| session.session_id == id).unwrap()
}

fn enable(bench: &mut Bench, id: &str) -> serde_json::Value {
    let worker = verified(bench,OPENCODE_SESSION);
    crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::VerifyChiefAdmission {
        session:worker, adapter:"opencode-queued-v1".into(), evidence:"Controlled host-verification fixture, not a live support claim".into(),
    }).unwrap().result.unwrap();
    let session = verified(bench, id);
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: ChiefHostAction::Enable {
        request_id: format!("enable-{}", id.replace(':', "-")), session,
    }}).unwrap().result.unwrap()
}

fn context(bench: &mut Bench, peer: u32) -> Result<cockpit::teams::TeamReply, cockpit::teams::TeamError> {
    crate::teams::request(&mut bench.registry, peer, AgentAction::Chief { request: ChiefAgentAction::Context { after: 0, limit: 50 } })
}

fn prepare_chief_boundary(bench: &mut Bench) {
    let chief = verified(bench,OPENCODE_SESSION);
    let entry = bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap();
    entry.state="waiting".into(); entry.needs_input=false;
    entry.since=(crate::teams::now_ms()/1000-600) as f64;
    crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ChiefFocus {viewed:String::new(),workspace_active:false,gaming:false}).unwrap();
    crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ChiefNativeObservation {session:chief,idle:true,pending_input:false,observed_at_ms:crate::teams::now_ms()}).unwrap();
}

fn admit_chief(bench: &mut Bench) {
    prepare_chief_boundary(bench);
    let dispatch = crate::teams::request(&mut bench.registry,20,AgentAction::Receive).unwrap().result.unwrap();
    assert_eq!(dispatch["envelope"]["kind"],"chief_wake");
    crate::teams::request(&mut bench.registry,20,AgentAction::Receipt {dispatch_id:dispatch["dispatch_id"].as_str().unwrap().into(),outcome:cockpit::teams::TransportOutcome::Received {native_id:"chief-bootstrap".into()}}).unwrap().result.unwrap();
}

#[test]
fn chief_host_review_pagination_invalidates_on_material_change_not_heartbeat_time() {
    let mut bench = team_bench();
    enable(&mut bench,SESSION_A);
    let first = crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ChiefSnapshot {after:0,limit:1,generation:None}).unwrap().result.unwrap();
    let generation = first["generation"].as_str().expect("host review generation");
    let request = |generation: &str| serde_json::from_value::<HostRequest>(json!({"request":"chief_snapshot","after":1,"limit":1,"generation":generation})).unwrap();
    let second = crate::teams::host(&mut bench.registry, std::process::id(),request(generation)).unwrap().result.unwrap();
    assert_ne!(second["invalidated"],true);
    bench.registry.teams.as_ref().unwrap().execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at) VALUES ('pagination-change','scope','key','Material host change','Review only','open',1,100)",[]).unwrap();
    let stale = crate::teams::host(&mut bench.registry, std::process::id(),request(generation)).unwrap().result.unwrap();
    assert_eq!(stale["invalidated"],true);
    assert_eq!(stale["next"],0);
}

#[test]
fn the_chief_snapshot_names_sessions() {
    let mut bench = team_bench();
    enable(&mut bench,SESSION_A);
    let connection = bench.registry.teams.as_ref().unwrap();
    connection.execute("INSERT INTO chief_stops(id,session_id,runtime,boundary,project,task_id,bundle,state,created_at) VALUES ('stop-b',?1,'runtime-30','waiting:1','path:/project-a','1d3fd318','{}','answered',10)", [SESSION_B]).unwrap();
    connection.execute("INSERT INTO chief_answers(id,stop_id,session_id,question,answer,reason,source,delivery,created_at) VALUES ('answer-b','stop-b',?1,'Which?','A.','Because.','chief','received',20)", [SESSION_B]).unwrap();
    bench.registry.sessions.get_mut(SESSION_A).unwrap().label = "Fix login".into();
    bench.registry.sessions.get_mut(SESSION_B).unwrap().label = String::new();
    let snapshot = |bench: &mut Bench| crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ChiefSnapshot {after:0,limit:50,generation:None}).unwrap().result.unwrap();
    let names = snapshot(&mut bench)["session_names"].clone();
    assert_eq!(names[SESSION_A], "Fix login · project-a");
    assert!(names.get(SESSION_B).is_none(), "a session without a label has no name: {names}");
    assert!(names.get(OPENCODE_SESSION).is_none());
    bench.registry.sessions.get_mut(SESSION_B).unwrap().label = "Review PR".into();
    bench.registry.sessions.get_mut(SESSION_B).unwrap().runtime = String::new();
    assert!(crate::teams::live_sessions(&mut bench.registry).unwrap().iter().all(|session| session.session_id != SESSION_B), "the session is no longer live");
    assert_eq!(snapshot(&mut bench)["session_names"][SESSION_B], "Review PR · project-a", "a session an answer refers to is named while it is not live");
}

fn blocked_by_for(bench: &mut Bench, id: &str) -> Vec<String> {
    let session = verified(bench,id);
    crate::chief::workers(&mut bench.registry).unwrap().into_iter().find(|worker|worker.session==session).unwrap().blocked_by
}

#[test]
fn the_chief_is_woken_without_waiting_for_the_idle_guard() {
    let mut bench = team_bench(); enable(&mut bench,OPENCODE_SESSION);
    prepare_chief_boundary(&mut bench);
    bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap().since=(crate::teams::now_ms()/1000-40) as f64;
    assert!(!blocked_by_for(&mut bench,OPENCODE_SESSION).contains(&"idle_guard".to_string()));
    let dispatch = crate::teams::request(&mut bench.registry,20,AgentAction::Receive).unwrap().result.unwrap();
    assert_eq!(dispatch["envelope"]["kind"],"chief_wake");
}

#[test]
fn workers_still_wait_for_the_idle_guard() {
    let mut bench = team_bench();
    prepare_chief_boundary(&mut bench);
    bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap().since=(crate::teams::now_ms()/1000-40) as f64;
    assert!(blocked_by_for(&mut bench,OPENCODE_SESSION).contains(&"idle_guard".to_string()));
}

#[test]
fn chief_unavailable_is_one_host_notice_even_without_a_chief_report() {
    let mut bench = team_bench();
    enable(&mut bench,SESSION_A);
    bench.registry.tick_queues();
    let notices = bench.raised().into_iter().filter(|id|id.starts_with("chief-decision-")).collect::<Vec<_>>();
    assert_eq!(notices.len(),1);
    bench.registry.tick_queues();
    assert_eq!(bench.raised().into_iter().filter(|id|id.starts_with("chief-decision-")).count(),1);
}

#[test]
fn an_escalation_banner_names_its_category() {
    let mut bench = team_bench();
    enable(&mut bench,SESSION_A);
    crate::chief::workers(&mut bench.registry).unwrap();
    let connection = bench.registry.teams.as_ref().unwrap();
    for (id,category) in [("esc-security","security"),("esc-scope","scope"),("esc-unsure","not_sure")] {
        connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at,category) VALUES (?1,'scope',?1,'Q','R','open',1,100,?2)",[id,category]).unwrap();
        connection.execute("INSERT INTO chief_attention(decision,created_at) VALUES (?1,100)",[id]).unwrap();
    }
    bench.registry.tick_queues();
    assert_eq!(bench.notice_title("chief-decision-esc-security").as_deref(),Some("Chief · security question needs you"));
    assert_eq!(bench.notice_title("chief-decision-esc-scope").as_deref(),Some("Chief · scope question needs you"));
    assert_eq!(bench.notice_title("chief-decision-esc-unsure").as_deref(),Some("Chief · not sure question needs you"));
}

fn restart_chief_daemon(bench: &mut Bench) {
    bench.shown=Rc::new(RefCell::new(Shown::default()));
    bench.registry=Registry::new(Box::new(FakeSystem(bench.world.clone())),Box::new(FakeBanners(bench.shown.clone())));
}

#[test]
fn chief_attention_restores_after_a_crash_before_first_display_even_with_old_notified_marker() {
    let mut bench = team_bench(); enable(&mut bench,SESSION_A);
    crate::chief::workers(&mut bench.registry).unwrap();
    let connection = bench.registry.teams.as_mut().unwrap();
    let decision: String = connection.query_row("SELECT decision FROM chief_attention",[],|row|row.get(0)).unwrap();
    // Reproduce a v9 crash after its durable marker but before any in-memory presentation.
    connection.execute("UPDATE chief_attention SET notified_at=1 WHERE decision=?1",[&decision]).unwrap();
    assert!(bench.raised().is_empty());
    restart_chief_daemon(&mut bench);
    bench.registry.tick_queues();
    assert_eq!(bench.raised(),vec![format!("chief-decision-{decision}")]);
    bench.registry.tick_queues();
    assert_eq!(bench.raised().len(),1);
}

#[test]
fn chief_attention_restores_once_per_daemon_and_resolution_removes_it() {
    let mut bench = team_bench(); enable(&mut bench,SESSION_A);
    bench.registry.tick_queues();
    let initial = bench.raised(); assert_eq!(initial.len(),1);
    restart_chief_daemon(&mut bench);
    bench.registry.tick_queues();
    assert_eq!(bench.raised(),initial,"unresolved attention survives a normal daemon restart with the same presentation id");
    let connection = bench.registry.teams.as_mut().unwrap();
    let decision: String = connection.query_row("SELECT decision FROM chief_attention",[],|row|row.get(0)).unwrap();
    connection.execute("UPDATE chief_decisions SET question='Changed material fixture',revision=revision+1 WHERE id=?1",[&decision]).unwrap();
    for _ in 0..10 {bench.registry.tick_queues();}
    assert_eq!(bench.raised(),initial,"material revisions and unchanged ticks do not create new notifications");
    let revision: i64 = bench.registry.teams.as_ref().unwrap().query_row("SELECT revision FROM chief_decisions WHERE id=?1",[&decision],|row|row.get(0)).unwrap();
    crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::Chief {action:ChiefHostAction::Resolve {request_id:"resolve-restored".into(),decision,expected_revision:revision,resolution:"Host acknowledged; no authority granted".into()}}).unwrap().result.unwrap();
    bench.registry.tick_queues();
    assert!(bench.dropped().contains(&format!("notice:{}",initial[0])));
    bench.registry.tick_queues();
    assert_eq!(bench.raised(),initial);
    restart_chief_daemon(&mut bench);
    bench.registry.tick_queues();
    assert!(bench.raised().is_empty(),"answered attention is not restored");
}

#[test]
fn chief_attention_presents_all_pending_records_even_when_the_observer_hits_capacity() {
    let mut bench = team_bench(); enable(&mut bench,SESSION_A);
    let connection = bench.registry.teams.as_mut().unwrap();
    for index in 0..100 {
        let id = format!("pending-{index}");
        connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at) VALUES (?1,?1,'blocked','Existing host blocker','Review only','open',1,100)",[&id]).unwrap();
        connection.execute("INSERT INTO chief_attention(decision,created_at,notified_at) VALUES (?1,100,101)",[id]).unwrap();
    }
    restart_chief_daemon(&mut bench);
    for _ in 0..3 {bench.registry.tick_queues();}
    assert_eq!(bench.raised().iter().filter(|id|id.starts_with("chief-decision-")).count(),100);
    assert_eq!(bench.raised().iter().filter(|id|id.as_str()=="chief-host-fault").count(),1);
}

#[test]
fn chief_decision_overflow_is_visible_without_blocking_host_review_or_resolve() {
    let mut bench = team_bench(); enable(&mut bench,SESSION_A);
    let connection = bench.registry.teams.as_mut().unwrap();
    for index in 0..100 {
        let id = format!("overflow-{index}");
        connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at) VALUES (?1,?1,'blocked','Host review needed','Review exact source','open',1,100)",[id]).unwrap();
    }
    bench.registry.tick_queues();
    assert!(bench.raised().iter().any(|id|id=="chief-host-fault"));
    let review = crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ChiefSnapshot {after:0,limit:50,generation:None}).unwrap().result.unwrap();
    assert_eq!(review["projection"],"host_review");
    crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::Chief {action:ChiefHostAction::Resolve {request_id:"resolve-overflow".into(),decision:"overflow-0".into(),expected_revision:1,resolution:"Acknowledged, without granting evidence or permissions".into()}}).unwrap().result.unwrap();
    assert_eq!(bench.registry.teams.as_ref().unwrap().query_row("SELECT count(*) FROM chief_decisions WHERE state='open'",[],|row|row.get::<_,i64>(0)).unwrap(),99);
}

#[test]
fn chief_summary_production_route_delivers_one_request_and_accepts_only_its_native_linked_reply() {
    let mut bench = team_bench(); enable(&mut bench,OPENCODE_SESSION);
    prepare_chief_boundary(&mut bench);
    bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap().state="working".into();
    let request = HostRequest::Chief {action:ChiefHostAction::RequestSummary {request_id:"host-summary".into()}};
    let first = crate::teams::host(&mut bench.registry, std::process::id(),request.clone()).unwrap().result.unwrap();
    assert_eq!(first["state"],"queued");
    assert_eq!(crate::teams::host(&mut bench.registry, std::process::id(),request).unwrap().result.unwrap(),first);
    assert!(crate::teams::request(&mut bench.registry,20,AgentAction::Receive).unwrap().result.unwrap().is_null());
    let entry=bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap();entry.state="waiting".into();entry.since=(crate::teams::now_ms()/1000) as f64;
    assert!(blocked_by_for(&mut bench,OPENCODE_SESSION).iter().all(|reason|reason!="idle_guard"),"the Chief's own turns skip the idle guard");
    let reply = serde_json::from_value::<AgentAction>(json!({"action":"chief","request":{"action":"summary_response","request_id":"host-summary",
        "sections":{"completed":"None verified","active":"No assignments","decisions":"No host decisions","idle":"No approved work"}}})).unwrap();
    assert!(crate::teams::request(&mut bench.registry,20,reply.clone()).unwrap().result.is_err());
    admit_chief(&mut bench);
    assert!(crate::teams::request(&mut bench.registry,10,reply.clone()).unwrap().result.is_err());
    crate::teams::request(&mut bench.registry,20,reply).unwrap().result.unwrap();
    let review = crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ChiefSnapshot {after:0,limit:50,generation:None}).unwrap().result.unwrap();
    let summary = review["records"].as_array().unwrap().iter().find(|record|record["kind"]=="summary").unwrap();
    assert_eq!(summary["state"],"completed");
    assert_eq!(summary["last_result"]["provenance"],"chief_narrative_unverified");
    bench.registry.tick_queues();
    assert_eq!(bench.registry.teams.as_ref().unwrap().query_row("SELECT count(*) FROM chief_attention",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(bench.registry.teams.as_ref().unwrap().query_row("SELECT count(*) FROM chief_summary_requests",[],|row|row.get::<_,i64>(0)).unwrap(),1);
}

#[test]
fn chief_only_selected_live_runtime_gets_redacted_global_context() {
    let mut bench = team_bench();
    let project = bench.fixture.root.join("home/IdeaProjects/project-b");
    fs::create_dir_all(project.join(".git")).unwrap();
    bench.registry.sessions.get_mut(SESSION_B).unwrap().cwd = project.to_string_lossy().into();
    bench.registry.sessions.get_mut(SESSION_B).unwrap().state = "working".into();
    for entry in bench.registry.sessions.values_mut() {
        entry.full_request = "SECRET-APPROVAL".into();
        entry.detail = "SECRET-DETAIL".into();
        entry.outcome = "SECRET-OUTPUT".into();
    }
    enable(&mut bench, SESSION_A);
    let global = context(&mut bench, 10).unwrap().result.unwrap();
    let text = global.to_string();
    assert!(text.contains("project-a"));
    assert!(text.contains("project-b"));
    assert!(text.contains("busy"));
    for forbidden in ["SECRET", "full_request", "detail", "output", "credential"] { assert!(!text.contains(forbidden), "{text}"); }
    assert_eq!(context(&mut bench, 20).unwrap().result.unwrap_err().code, ErrorCode::Unauthorized);
    assert_eq!(context(&mut bench, 99).unwrap_err().code, ErrorCode::Unauthorized);
    bench.world.borrow_mut().sender_runtimes.insert(10, "replaced-runtime".into());
    assert_eq!(context(&mut bench, 10).unwrap_err().code, ErrorCode::Unauthorized);
}

#[test]
fn chief_lifecycle_conflict_pause_resume_disable_and_restart_without_team_membership() {
    let mut bench = team_bench();
    let grant = enable(&mut bench, SESSION_A);
    let epoch = grant["epoch"].as_str().unwrap().to_string();
    let session = verified(&mut bench, SESSION_B);
    let reply = crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: ChiefHostAction::Enable { request_id: "second".into(), session } }).unwrap();
    assert_eq!(reply.result.unwrap_err().code, ErrorCode::Conflict);
    bench.registry.teams = None;
    bench.registry.sessions.get_mut(SESSION_A).unwrap().runtime.clear();
    assert!(context(&mut bench, 10).unwrap().result.is_ok());
    for (request_id, action, allowed) in [
        ("pause", "pause", false), ("resume", "resume", true), ("disable", "disable", false),
    ] {
        let action = match action {
            "pause" => ChiefHostAction::Pause { request_id: request_id.into(), expected_epoch: epoch.clone() },
            "resume" => ChiefHostAction::Resume { request_id: request_id.into(), expected_epoch: epoch.clone() },
            _ => ChiefHostAction::Disable { request_id: request_id.into(), expected_epoch: epoch.clone() },
        };
        crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action }).unwrap().result.unwrap();
        assert_eq!(context(&mut bench, 10).unwrap().result.is_ok(), allowed);
    }
}

#[test]
fn chief_enable_rejects_team_members_and_forged_runtime() {
    let mut bench = linked_team_bench();
    let mut session = verified(&mut bench, SESSION_A);
    for request_id in ["team-member", "forged-runtime"] {
        if request_id == "forged-runtime" { session.runtime = "forged".into(); }
        let reply = crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: ChiefHostAction::Enable {
            request_id: request_id.into(), session: session.clone(),
        }}).unwrap();
        assert_eq!(reply.result.unwrap_err().code, ErrorCode::Unauthorized);
    }
}

#[test]
fn chief_resume_requires_the_original_runtime_and_replacement_requires_a_new_host_grant() {
    let mut bench = team_bench();
    let epoch = enable(&mut bench, SESSION_A)["epoch"].as_str().unwrap().to_string();
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: ChiefHostAction::Pause { request_id: "pause".into(), expected_epoch: epoch.clone() } }).unwrap().result.unwrap();
    bench.world.borrow_mut().sender_runtimes.insert(10, "replacement".into());
    bench.registry.sessions.get_mut(SESSION_A).unwrap().runtime = "replacement".into();
    let reply = crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: ChiefHostAction::Resume { request_id: "resume".into(), expected_epoch: epoch.clone() } }).unwrap();
    assert_eq!(reply.result.unwrap_err().code, ErrorCode::Unauthorized);
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: ChiefHostAction::Disable { request_id: "disable".into(), expected_epoch: epoch } }).unwrap().result.unwrap();
    assert_eq!(context(&mut bench, 10).unwrap().result.unwrap_err().code, ErrorCode::Unauthorized);
    let session = verified(&mut bench, SESSION_A);
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: ChiefHostAction::Enable { request_id: "replacement".into(), session } }).unwrap().result.unwrap();
    assert!(context(&mut bench, 10).unwrap().result.is_ok());
}

/// Files 12 adds in project-b through the report socket as `peer`; returns how many were saved
/// and the reason the first refused one gave in its receipt.
fn file_handoffs_in_project_b(bench: &mut Bench, peer: u32, forged: serde_json::Value) -> (usize, serde_json::Value) {
    fs::create_dir_all(bench.fixture.root.join("home/IdeaProjects/project-b/.git")).unwrap();
    let started_in = bench.world.borrow().sender_directories[&peer].clone();
    let key = crate::registry::answers_key_of(Path::new(&started_in));
    let mut todos = cockpit::todo::TodoList::load();
    let mut saved = 0;
    let mut refusal = serde_json::Value::Null;
    for number in 0..12 {
        let request_id = format!("{number:012x}");
        let mut raw = json!({"action": "add", "project": "project-b", "text": format!("Chief work {number}"), "request_id": request_id});
        raw.as_object_mut().unwrap().extend(forged.as_object().unwrap().clone());
        bench.registry.request_from("todo-update", &raw.to_string(), peer).unwrap();
        saved += todos.apply_updates();
        let receipt = cockpit::util::read_json(&cockpit::approval::answers_dir(&key, "todos").join(format!("{request_id}.json"))).unwrap();
        if receipt["outcome"] == "rejected" && refusal.is_null() { refusal = receipt; }
    }
    (saved, refusal)
}

#[test]
fn the_active_chief_may_keep_ten_filings_open_in_another_project() {
    let mut bench = team_bench();
    enable(&mut bench, SESSION_A);
    let (saved, refusal) = file_handoffs_in_project_b(&mut bench, 10, json!({}));
    assert_eq!(saved, 10);
    assert!(refusal["reason"].as_str().unwrap().ends_with("10 already open"), "{refusal}");
    assert_eq!(refusal["can_approve"], true);
}

#[test]
fn another_session_of_the_chiefs_project_keeps_three_even_naming_the_epoch() {
    let mut bench = team_bench();
    let epoch = enable(&mut bench, SESSION_A)["epoch"].clone();
    let (saved, refusal) = file_handoffs_in_project_b(&mut bench, 30, json!({"chief_epoch": epoch}));
    assert_eq!(saved, 3);
    assert!(refusal["reason"].as_str().unwrap().ends_with("3 already open"), "{refusal}");
    assert_eq!(refusal["can_approve"], true);
}

#[test]
fn a_paused_chief_keeps_three_filings_open_in_another_project() {
    let mut bench = team_bench();
    let epoch = enable(&mut bench, SESSION_A)["epoch"].as_str().unwrap().to_string();
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: ChiefHostAction::Pause { request_id: "pause".into(), expected_epoch: epoch } }).unwrap().result.unwrap();
    assert_eq!(file_handoffs_in_project_b(&mut bench, 10, json!({})).0, 3);
}

#[test]
fn a_session_without_a_chief_store_keeps_three_filings_open_in_another_project() {
    let mut bench = team_bench();
    assert_eq!(file_handoffs_in_project_b(&mut bench, 10, json!({"chief_epoch": "forged"})).0, 3);
}

#[test]
fn chief_agent_wire_rejects_forged_identity_role_and_approval_fields() {
    for extra in ["sender", "runtime", "role", "approval_hash"] {
        let mut action = json!({"action":"chief", "request":{"action":"context", "after":0, "limit":50}});
        action["request"][extra] = json!("forged");
        assert!(serde_json::from_value::<AgentAction>(action).is_err());
    }
}

#[test]
fn fetched_issue_manual_status_uses_the_real_app_and_host_then_releases() {
    use cockpit::app::{App,Key,Screen};
    use std::io::{Read,Write};
    struct QuietScreen;
    impl Screen for QuietScreen {
        fn draw(&mut self,_: &mut App) {}
        fn next_key(&mut self,_: Option<std::time::Duration>) -> Option<Key> {None}
        fn outside(&mut self,_: &mut dyn FnMut()->Option<String>) -> Option<String> {None}
    }
    let mut bench=team_bench();
    let directory=bench.registry.sessions[OPENCODE_SESSION].cwd.clone();
    let shims=bench.fixture.root.join("issue-bin");fs::create_dir_all(&shims).unwrap();
    let gh=shims.join("gh");
    fs::write(&gh,"#!/bin/sh\nif [ \"$1\" = issue ]; then printf '%s' '[{\"number\":42,\"title\":\"First fetched issue\",\"url\":\"https://github.com/owner/project-a/issues/42\",\"labels\":[]}]'; else echo '[]'; fi\n").unwrap();
    fs::set_permissions(&gh,std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let old_path=std::env::var("PATH").unwrap();
    // SAFETY: team_bench holds the process-wide test environment lock.
    unsafe {std::env::set_var("PATH",format!("{}:{old_path}",shims.display()));}
    let socket=cockpit::paths::team_host_socket();
    fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let listener=std::os::unix::net::UnixListener::bind(socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let app_directory=directory.clone();
    let app=std::thread::spawn(move || {
        let mut app=App::new();
        app.reviews.checkouts.insert("owner/project-a".into(),app_directory);
        app.reviews.repositories=vec!["owner/project-a".into()];
        app.reviews.refresh();
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
        while app.reviews.current().2 {assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(1));}
        assert_eq!(app.reviews.issues().len(),1);
        assert!(!app.todos.issues.contains_key("owner/project-a#42"));
        app.build();app.focus_panel(3);
        let index=app.todo_entries.iter().position(|entry|entry.key()=="owner/project-a#42").unwrap();
        for _ in 0..index {app.handle(Key::Down,&mut QuietScreen);}
        app.handle(Key::Char('i'),&mut QuietScreen);
        app.handle(Key::Char('i'),&mut QuietScreen);
        let todos=cockpit::todo::TodoList::load();
        assert_eq!(todos.status_of("owner/project-a#42"),"doing");
        let token=todos.manual_reservation("owner/project-a#42").unwrap();
        app.handle(Key::Char('i'),&mut QuietScreen);
        app.handle(Key::Char('i'),&mut QuietScreen);
        assert_eq!(cockpit::todo::TodoList::load().status_of("owner/project-a#42"),"todo");
        token
    });
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(15);
    while !app.is_finished() {
        if let Ok((mut stream,_))=listener.accept() {
            stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
            let mut wire=String::new();stream.read_to_string(&mut wire).unwrap();
            let result=crate::teams::host(&mut bench.registry, std::process::id(),serde_json::from_str(&wire).unwrap());
            let reply=result.unwrap_or_else(|error|cockpit::teams::TeamReply {request_id:"host-error".into(),result:Err(error)});
            stream.write_all(serde_json::to_string(&reply).unwrap().as_bytes()).unwrap();
        }
        assert!(std::time::Instant::now()<deadline,"App host exchange timed out");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let result=app.join();
    // SAFETY: the App and fetch threads finished while the environment lock is held.
    unsafe {std::env::set_var("PATH",old_path);}
    let token=result.unwrap();
    assert_eq!(bench.registry.teams.as_ref().unwrap().query_row("SELECT count(*) FROM scheduling_reservations WHERE token=?1",[token],|row|row.get::<_,i64>(0)).unwrap(),0);
    for task_id in ["deadbeef","owner/repo#0","owner/repo#1/extra","not-an-issue"] {
        assert!(crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ReserveManual {task_id:task_id.into(),directory:directory.clone()}).is_err(),"{task_id}");
    }
    assert!(crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ReserveManual {task_id:"owner/repo#1".into(),directory:"/unregistered-project".into()}).is_err());
    let other=bench.fixture.root.join("home/IdeaProjects/project-b");fs::create_dir_all(other.join(".git")).unwrap();
    let id=cockpit::todo::TodoList::load().add("project-a",&directory,"Exact typed project");
    assert!(crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ReserveManual {task_id:id,directory:other.to_string_lossy().into_owned()}).is_err());
}

#[test]
fn legacy_host_manual_undo_and_delete_release_only_status_reservations() {
    for remove in [false,true] {
        let mut bench=team_bench();
        for session in [SESSION_A,SESSION_B,OPENCODE_SESSION] {bench.world.borrow_mut().live_ids.insert(session.into());}
        let directory=bench.registry.sessions[OPENCODE_SESSION].cwd.clone();
        let mut todos=cockpit::todo::TodoList::load();
        let id=todos.add("project-a",&directory,"Manual status only");
        let manual=crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ReserveManual {task_id:id.clone(),directory:directory.clone()}).unwrap().result.unwrap();
        assert!(todos.set_status(&id,"doing",""));
        if remove {todos.remove(&id).unwrap();} else {assert!(todos.set_status(&id,"todo",""));}
        crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ReleaseManual {task_id:id,directory:directory.clone(),token:manual["reservation"].as_str().unwrap().into()}).unwrap().result.unwrap();
        let next=todos.add("project-a",&directory,"Next task");
        let reserved=crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ReserveTask {task_id:next,directory,worker:None,automatic:false}).unwrap().result.unwrap();
        let token=reserved["reservation"].as_str().unwrap().to_string();
        crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::SettleReservation {token:token.clone(),expected_revision:1,outcome:"begin".into()}).unwrap().result.unwrap();
        restart_chief_daemon(&mut bench);
        let snapshot=crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ChiefSnapshot {after:0,limit:50,generation:None}).unwrap().result.unwrap();
        let launch=snapshot["records"].as_array().unwrap().iter().find(|record|record["kind"]=="legacy_launch" && record["id"]==token).unwrap();
        assert_eq!(launch["state"],"uncertain");assert!(launch["worker"].is_null());
        let action=serde_json::from_value(json!({"action":"reconcile_launch","request_id":"stop-launch","token":token,"expected_revision":launch["revision"],"evidence":{"kind":"safe_stop","note":"Exact launch confirmed unable to execute","confirmed_stopped":true}})).unwrap();
        crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::Chief {action}).unwrap().result.unwrap();
        assert_eq!(bench.registry.teams.as_ref().unwrap().query_row("SELECT count(*) FROM scheduling_reservations WHERE token=?1",[&token],|row|row.get::<_,i64>(0)).unwrap(),0);
    }
}

#[test]
fn chief_host_routes_do_not_allow_team_approval_to_add_an_active_or_paused_chief() {
    use cockpit::teams::{HostAction, Member, Proposal, Role, TaskKey};
    for paused in [false, true] {
        let mut bench = team_bench();
        let session = verified(&mut bench, SESSION_A);
        let pending = crate::teams::request(&mut bench.registry, 20, AgentAction::Propose { proposal: Proposal {
            submission_key: "link-chief".into(), task: TaskKey { project: session.project, task_id: "1d3fd318".into() },
            members: vec![Member { session_id: SESSION_A.into(), role: Role::Designer }, Member { session_id: OPENCODE_SESSION.into(), role: Role::Developer }],
            reason: "Request a product team".into(),
        }}).unwrap().result.unwrap();
        let epoch = enable(&mut bench, SESSION_A)["epoch"].as_str().unwrap().to_string();
        if paused {
            crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: ChiefHostAction::Pause { request_id: "pause".into(), expected_epoch: epoch.clone() } }).unwrap().result.unwrap();
        }
        let approval = HostAction::Approve { request_id: "approve".into(), proposal_id: pending["proposal_id"].as_str().unwrap().into(),
            expected_revision: 1, mapping_hash: pending["mapping_hash"].as_str().unwrap().into() };
        assert_eq!(crate::teams::host_control(&mut bench.registry, &approval).unwrap_err().code, ErrorCode::Unauthorized);
        let own = crate::teams::request(&mut bench.registry, 10, AgentAction::Context).unwrap().result.unwrap();
        assert!(own["members"].as_array().unwrap().is_empty());
        assert_eq!(context(&mut bench, 10).unwrap().result.is_ok(), !paused);
        crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: ChiefHostAction::Disable { request_id: "disable".into(), expected_epoch: epoch } }).unwrap().result.unwrap();
        crate::teams::host_control(&mut bench.registry, &approval).unwrap().result.unwrap();
        assert_eq!(context(&mut bench, 10).unwrap().result.unwrap_err().code, ErrorCode::Unauthorized);
    }
}

#[test]
fn chief_opencode_readiness_requires_fresh_host_native_and_focus_facts_and_live_proof() {
    let mut bench = team_bench();
    let session = verified(&mut bench, OPENCODE_SESSION);
    let entry = bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap();
    entry.state = "waiting".into();
    entry.needs_input = false;
    entry.since = (crate::teams::now_ms() / 1000 - 600) as f64;
    let ready = |bench: &mut Bench| crate::chief::workers(&mut bench.registry).unwrap().into_iter()
        .find(|worker| worker.session == session).unwrap().ready;
    assert!(!ready(&mut bench));
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ChiefFocus { viewed:String::new(), workspace_active:false, gaming:false }).unwrap();
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ChiefNativeObservation { session:session.clone(), idle:true, pending_input:false, observed_at_ms:crate::teams::now_ms() }).unwrap();
    assert!(!ready(&mut bench), "host/native idle is not live admission verification");
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::VerifyChiefAdmission { session:session.clone(), adapter:"opencode-queued-v1".into(),
        evidence:"Host live test: existing draft preserved; queued user input precedes assignment; busy race queues".into() }).unwrap();
    assert!(ready(&mut bench));
    for case in ["native-input", "native-busy", "native-stale", "focus", "gaming", "focus-stale", "question", "fresh", "sleep"] {
        let original_focus = bench.registry.chief_focus.clone();
        let original_native = bench.registry.chief_native.clone();
        let original_entry = bench.registry.sessions[OPENCODE_SESSION].clone();
        match case {
            "native-input" => bench.registry.chief_native.get_mut(OPENCODE_SESSION).unwrap().2 = true,
            "native-busy" => bench.registry.chief_native.get_mut(OPENCODE_SESSION).unwrap().1 = false,
            "native-stale" => bench.registry.chief_native.get_mut(OPENCODE_SESSION).unwrap().3 -= 31_000,
            "focus" => bench.registry.chief_focus = Some((OPENCODE_SESSION.into(), true, false, crate::teams::now_ms())),
            "gaming" => bench.registry.chief_focus.as_mut().unwrap().2 = true,
            "focus-stale" => bench.registry.chief_focus.as_mut().unwrap().3 -= 31_000,
            "question" => bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap().needs_input = true,
            "fresh" => bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap().since = (crate::teams::now_ms()/1000) as f64,
            "sleep" => bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap().state = "gone".into(),
            _ => unreachable!(),
        }
        assert!(!ready(&mut bench), "{case}");
        bench.registry.chief_focus = original_focus;
        bench.registry.chief_native = original_native;
        bench.registry.sessions.insert(OPENCODE_SESSION.into(), original_entry);
    }
    let sleeping = sleep_host_for(&mut bench, OPENCODE_SESSION, "prepare_sleep", None).unwrap();
    assert!(sleeping["token"].is_null(), "standalone identity does not acquire team membership");
    assert!(!ready(&mut bench), "a standalone Chief worker cannot dispatch during the host sleep operation");
    bench.sleep_guards.clear();
    assert!(ready(&mut bench), "an abandoned standalone sleep restores availability");
    bench.registry.chief_native.clear();
    assert!(!ready(&mut bench), "restart discards native observations");
}

#[test]
fn a_standalone_session_is_ready_without_a_bound_role() {
    let mut bench = team_bench();
    let session = verified(&mut bench, OPENCODE_SESSION);
    let entry = bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap();
    entry.state = "waiting".into();
    entry.needs_input = false;
    entry.since = (crate::teams::now_ms() / 1000 - 600) as f64;
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ChiefFocus { viewed:String::new(), workspace_active:false, gaming:false }).unwrap();
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ChiefNativeObservation { session:session.clone(), idle:true, pending_input:false, observed_at_ms:crate::teams::now_ms() }).unwrap();
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::VerifyChiefAdmission { session:session.clone(), adapter:"opencode-queued-v1".into(),
        evidence:"Controlled host proof fixture, not a live support claim".into() }).unwrap();
    let worker = crate::chief::workers(&mut bench.registry).unwrap().into_iter().find(|worker| worker.session == session).unwrap();
    assert!(worker.role.is_none());
    assert!(worker.blocked_by.is_empty(), "{:?}", worker.blocked_by);
    assert!(worker.ready);
}

#[test]
fn an_enabled_chief_leaves_reserve_task_and_reserve_manual_available() {
    for manual in [false, true] {
        let mut bench = team_bench();
        enable(&mut bench, SESSION_A);
        let directory = bench.registry.sessions[SESSION_B].cwd.clone();
        let task_id = cockpit::todo::TodoList::load().add("project-a", &directory, "legacy work");
        let request = if manual { HostRequest::ReserveManual { task_id, directory } }
            else { HostRequest::ReserveTask { task_id, directory, worker:None, automatic:false } };
        crate::teams::host(&mut bench.registry, std::process::id(), request).unwrap().result.unwrap();
    }
}

#[test]
fn chief_legacy_reservations_validate_the_real_todo_project_and_status() {
    let mut bench = team_bench();
    let worker = verified(&mut bench, SESSION_B);
    let directory = bench.registry.sessions[SESSION_B].cwd.clone();
    let mut todos = cockpit::todo::TodoList::load();
    let id = todos.add("project-a", &directory, "legacy work");
    let wrong = bench.fixture.root.join("home/IdeaProjects/wrong");
    fs::create_dir_all(wrong.join(".git")).unwrap();
    assert!(crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ReserveTask {
        task_id:id.clone(), directory:wrong.to_string_lossy().into(), worker:None, automatic:false,
    }).is_err(), "a caller's directory cannot move the real task out of chief ownership");
    todos.set_status(&id, "doing", &worker.session_id);
    assert!(crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ReserveTask {
        task_id:id, directory, worker:Some(worker.session_id), automatic:false,
    }).is_err());
}

#[test]
fn chief_resume_reservation_survives_uncertainty_until_the_real_runtime_returns() {
    let mut bench = team_bench();
    let directory = bench.registry.sessions[SESSION_B].cwd.clone();
    assert!(crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ReserveResume { session:SESSION_B.into(), directory:directory.clone() }).is_err());
    let entry = bench.registry.sessions.remove(SESSION_B).unwrap();
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ReserveResume { session:SESSION_B.into(), directory:directory.clone() }).unwrap().result.unwrap();
    assert!(crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ReserveResume { session:SESSION_B.into(), directory }).is_err());
    bench.registry.sessions.insert(SESSION_B.into(),entry);
    crate::teams::live_sessions(&mut bench.registry).unwrap();
    assert!(!cockpit::chief::store::legacy_worker_busy(bench.registry.teams.as_ref().unwrap(),SESSION_B).unwrap());
}

#[test]
fn resume_is_reserved_while_the_session_is_doing_its_own_todo() {
    let mut bench = team_bench();
    let directory = bench.registry.sessions[SESSION_B].cwd.clone();
    let mut todos = cockpit::todo::TodoList::load();
    let id = todos.add("project-a", &directory, "work the session left unfinished");
    todos.set_status(&id, "doing", SESSION_B);
    bench.registry.sessions.remove(SESSION_B).unwrap();
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ReserveResume { session:SESSION_B.into(), directory }).unwrap().result.unwrap();
}

#[test]
fn resume_is_reserved_while_unassigned_work_is_doing_in_the_project() {
    let mut bench = team_bench();
    let directory = bench.registry.sessions[SESSION_B].cwd.clone();
    let mut todos = cockpit::todo::TodoList::load();
    let id = todos.add("project-a", &directory, "work nobody claimed");
    todos.set_status(&id, "doing", "");
    bench.registry.sessions.remove(SESSION_B).unwrap();
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ReserveResume { session:SESSION_B.into(), directory }).unwrap().result.unwrap();
}

#[test]
fn chief_normal_enable_requires_recorded_host_live_queued_admission_proof() {
    let mut bench = team_bench();
    let session = verified(&mut bench,SESSION_A);
    let reply = crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::Chief { action:ChiefHostAction::Enable {
        request_id:"no-live-proof".into(), session,
    }}).unwrap();
    assert_eq!(reply.result.unwrap_err().code,ErrorCode::Unavailable);
    assert_eq!(bench.registry.teams.as_ref().unwrap().query_row("SELECT count(*) FROM chief_grants", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn chief_legacy_automatic_claim_rechecks_host_capacity_instead_of_sidebar_snapshot() {
    let mut bench = team_bench();
    let directory = bench.registry.sessions[SESSION_B].cwd.clone();
    let entry = bench.registry.sessions.get_mut(SESSION_B).unwrap();
    entry.state = "waiting".into();
    entry.needs_input = false;
    entry.since = (crate::teams::now_ms()/1000-600) as f64;
    let id = cockpit::todo::TodoList::load().add("project-a",&directory,"legacy automatic work");
    let request = HostRequest::ReserveTask { task_id:id,directory,worker:Some(SESSION_B.into()),automatic:true };
    assert!(crate::teams::host(&mut bench.registry, std::process::id(),request.clone()).is_err(), "missing host focus fails closed");
    for case in ["busy","input","focus","gaming","fresh","stale-focus"] {
        bench.registry.chief_focus = Some((String::new(),false,false,crate::teams::now_ms()));
        let original = bench.registry.sessions[SESSION_B].clone();
        match case {
            "busy" => bench.registry.sessions.get_mut(SESSION_B).unwrap().state = "working".into(),
            "input" => bench.registry.sessions.get_mut(SESSION_B).unwrap().needs_input = true,
            "focus" => bench.registry.chief_focus = Some((SESSION_B.into(),true,false,crate::teams::now_ms())),
            "gaming" => bench.registry.chief_focus.as_mut().unwrap().2 = true,
            "fresh" => bench.registry.sessions.get_mut(SESSION_B).unwrap().since = (crate::teams::now_ms()/1000) as f64,
            "stale-focus" => bench.registry.chief_focus.as_mut().unwrap().3 -= 31_000,
            _ => unreachable!(),
        }
        assert!(crate::teams::host(&mut bench.registry, std::process::id(),request.clone()).is_err(),"{case}");
        bench.registry.sessions.insert(SESSION_B.into(),original);
    }
    bench.registry.chief_focus = Some((String::new(),false,false,crate::teams::now_ms()));
    assert!(crate::teams::host(&mut bench.registry, std::process::id(),request).unwrap().result.is_ok());
}

#[test]
fn chief_legacy_queue_can_reserve_a_host_sleep_note_but_chief_workers_remain_unavailable() {
    let mut bench = team_bench();
    let entry = bench.registry.sessions.remove(SESSION_B).unwrap();
    let id = cockpit::todo::TodoList::load().add("project-a",&entry.cwd,"legacy wake");
    let notes = cockpit::paths::asleep_dir();
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join(SESSION_B),serde_json::json!({"agent":"claude","project":"project-a","cwd":entry.cwd,
        "idle_since":crate::teams::now_ms()/1000-600,"slept_at":crate::teams::now_ms()/1000}).to_string()).unwrap();
    bench.registry.chief_focus = Some((String::new(),false,false,crate::teams::now_ms()));
    let result = crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ReserveTask {
        task_id:id,directory:entry.cwd,worker:Some(SESSION_B.into()),automatic:true,
    });
    assert!(result.unwrap().result.is_ok());
    assert!(!crate::chief::workers(&mut bench.registry).unwrap().iter().any(|worker|worker.session.session_id==SESSION_B));
}

#[test]
fn a_sleeping_chief_session_cannot_be_reserved_for_a_todo() {
    let mut bench = team_bench();
    enable(&mut bench, SESSION_A);
    let entry = bench.registry.sessions.remove(SESSION_A).unwrap();
    let id = cockpit::todo::TodoList::load().add("project-a",&entry.cwd,"work for a sleeping chief");
    let notes = cockpit::paths::asleep_dir();
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join(SESSION_A),serde_json::json!({"agent":"claude","project":"project-a","cwd":entry.cwd,
        "idle_since":crate::teams::now_ms()/1000-600,"slept_at":crate::teams::now_ms()/1000}).to_string()).unwrap();
    bench.registry.chief_focus = Some((String::new(),false,false,crate::teams::now_ms()));
    let result = crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ReserveTask {
        task_id:id,directory:entry.cwd,worker:Some(SESSION_A.into()),automatic:true,
    });
    assert!(result.is_err());
}

// The CLI plugin receives the real connected native client; only its API results are controlled here.
const NATIVE_TELEMETRY_HARNESS: &str = r#"
const [plugin, sessionID, mode] = process.argv.slice(2)
const {default: definition} = await import(plugin)
const calls = []
let route = mode==='other-route'?{type:'session',sessionID:'ses_3f2a9c0b2000AbCdEfGhIjKlMn'}
  :mode==='home-route'?{type:'home'}:{type:'session',sessionID}
const pending = () => [{id:'private-id',sessionID,text:'PRIVATE-NATIVE-CONTENT'}]
const context = {ui:{router:{current:()=>{
  if(mode==='route-error') throw new Error('PRIVATE-NATIVE-CONTENT')
  return route
}}},client:{
  session:{
    active:async (options) => {
      calls.push('active')
      if(mode==='route-changes') route={type:'session',sessionID:'ses_3f2a9c0b2000AbCdEfGhIjKlMn'}
      if(mode==='error') throw new Error('PRIVATE-NATIVE-CONTENT')
      if(mode==='timeout') return await new Promise((resolve,reject)=>options.signal.addEventListener('abort',()=>reject(new Error('timeout')),{once:true}))
      if(mode==='bad-active') return []
      if(mode==='child-busy') return {ses_3f2a9c0b2000AbCdEfGhIjKlMn:{type:'running'}}
      return mode==='busy'?{[sessionID]:{type:'running'}}:{}
    },
    inbox:{list:async (input) => { calls.push(['inbox',input]); return mode==='malformed'?{}:mode==='inbox'?pending():mode==='wrong-session'?[{id:'private-id',sessionID:'another-session'}]:[] }},
    form:{list:async (input) => { calls.push(['forms',input]); return mode==='forms'?pending():[] }},
  },
  permission:{list:async (input) => { calls.push(['permissions',input]); return mode==='permission'?pending():[] }},
}}
if(mode==='missing-router') delete context.ui.router
const keepAlive = setInterval(()=>{},1000)
const cleanup = await definition.setup(context)
await new Promise(resolve=>setTimeout(resolve,50))
await cleanup?.()
clearInterval(keepAlive)
console.log(JSON.stringify(calls))
"#;

fn native_telemetry_pipeline(bench: &mut Bench, mode: &str) -> Vec<serde_json::Value> {
    use std::io::{Read,Write};
    use std::process::{Command,Stdio};
    let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let binary = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("cockpit-team");
    assert!(binary.is_file(),"build the real cockpit-team binary before this focused test");
    let shims = bench.fixture.root.join("bin");
    fs::create_dir_all(&shims).unwrap();
    std::os::unix::fs::symlink(binary,shims.join("cockpit-team")).unwrap();
    let socket = bench.fixture.root.join("native-agent.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let answers = bench.fixture.answers("teams");
    cockpit::teams::client::publish_socket(&socket,answers.parent().unwrap()).unwrap();
    let harness = bench.fixture.root.join("native-harness.mjs");
    fs::write(&harness,NATIVE_TELEMETRY_HARNESS).unwrap();
    let mut child = Command::new("node").arg(&harness)
        .arg(checkout.join("config/opencode/.config/opencode/plugins/cockpit-telemetry/tui.ts"))
        .args([OPENCODE_SESSION,mode]).env("CLAUDE_CODE_SESSION_ID",OPENCODE_SESSION)
        .env("CLAUDE_ANSWERS_KEY",ANSWERS_KEY).env("COCKPIT_TEAM_POLL_MS","5000")
        .env("PATH",format!("{}:{}",shims.display(),std::env::var("PATH").unwrap()))
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut requests = Vec::new();
    let deadline = std::time::Instant::now()+std::time::Duration::from_secs(10);
    loop {
        if let Ok((mut stream,_)) = listener.accept() {
            stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
            let peer = crate::fifo::peer_pid(&stream).unwrap();
            // Controlled kernel-peer → launcher mapping, as for the rest of the daemon boundary tests.
            bench.world.borrow_mut().sender_runtimes.insert(peer,"runtime-20".into());
            let mut text = String::new();
            stream.read_to_string(&mut text).unwrap();
            requests.push(serde_json::from_str(&text).unwrap());
            let action: AgentAction = serde_json::from_str(&text).unwrap();
            let reply = crate::teams::request(&mut bench.registry,peer,action).unwrap();
            assert!(reply.result.is_ok(),"{reply:?}");
            stream.write_all(serde_json::to_string(&reply).unwrap().as_bytes()).unwrap();
        }
        if child.try_wait().unwrap().is_some() { break; }
        if std::time::Instant::now() > deadline { child.kill().unwrap(); panic!("native telemetry producer did not finish"); }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let calls:Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    if !matches!(mode,"other-route"|"home-route"|"route-error"|"missing-router") {
        assert_eq!(calls.len(),4,"producer must read all native pending-input sources");
    }
    for call in calls.iter().filter(|call|call.is_array()) { assert_eq!(call[1]["sessionID"],OPENCODE_SESSION); }
    assert!(!requests.is_empty(),"production observer never called real cockpit-team IPC");
    assert!(!serde_json::to_string(&requests).unwrap().contains("PRIVATE-NATIVE-CONTENT"));
    requests
}

#[test]
fn chief_production_native_plugin_to_ipc_derives_availability_without_host_observation() {
    for mode in ["idle","busy","child-busy","inbox","permission","forms","malformed","bad-active","wrong-session","error","timeout",
        "other-route","home-route","route-error","missing-router","route-changes"] {
        let mut bench = team_bench();
        let worker = verified(&mut bench,OPENCODE_SESSION);
        let entry = bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap();
        entry.state = "waiting".into(); entry.needs_input = false;
        entry.since = (crate::teams::now_ms()/1000-600) as f64;
        bench.registry.chief_focus = Some((String::new(),false,false,crate::teams::now_ms()));
        let requests = native_telemetry_pipeline(&mut bench,mode);
        if matches!(mode,"other-route"|"home-route"|"route-error"|"missing-router"|"route-changes") {
            assert!(requests[0]["counts"].is_null(),"{mode}: another or unknown visible conversation must invalidate native availability");
        }
        assert_eq!(requests[0]["action"],"native_telemetry");
        assert!(bench.registry.chief_native.is_empty(),"plugin cannot publish host-native observations");
        let ready = |bench:&mut Bench| crate::chief::workers(&mut bench.registry).unwrap().into_iter().find(|state|state.session==worker).unwrap().ready;
        assert!(!ready(&mut bench),"telemetry is not host live-verification evidence");
        crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::VerifyChiefAdmission {
            session:worker.clone(),adapter:"opencode-queued-v1".into(),evidence:"Controlled host proof fixture, not live activation".into(),
        }).unwrap().result.unwrap();
        assert_eq!(ready(&mut bench),mode=="idle","{mode}");
    }
}

fn telemetry_action(sequence: i64, running: u32, pending: u32) -> AgentAction {
    serde_json::from_value(serde_json::json!({"action":"native_telemetry","sequence":sequence,"counts":{
        "running_sessions":running,"pending_inbox":pending,"pending_permissions":0,"pending_forms":0,
    }})).unwrap()
}

#[test]
fn chief_runtime_telemetry_cannot_impersonate_another_source_or_host_authority() {
    let mut bench = team_bench();
    for peer in [10,30,99] {
        assert_eq!(crate::teams::request(&mut bench.registry,peer,telemetry_action(1,0,0)).unwrap_err().code,ErrorCode::Unauthorized);
    }
    let wire = serde_json::to_value(telemetry_action(1,0,0)).unwrap();
    for field in ["session","session_id","runtime","role","project","ready","observed_at_ms","evidence","request"] {
        let mut forged = wire.clone(); forged[field] = serde_json::json!("forged");
        assert!(serde_json::from_value::<AgentAction>(forged).is_err(),"{field}");
    }
    assert!(serde_json::from_value::<AgentAction>(serde_json::json!({"action":"verify_chief_admission","session":"forged"})).is_err());
    assert!(bench.registry.chief_telemetry.is_empty());
    crate::teams::request(&mut bench.registry,20,telemetry_action(1,0,0)).unwrap().result.unwrap();
    assert_eq!(bench.registry.chief_telemetry.keys().collect::<Vec<_>>(),[OPENCODE_SESSION]);
    assert!(bench.registry.chief_native.is_empty());
    let worker = verified(&mut bench,OPENCODE_SESSION);
    assert!(!cockpit::chief::store::admission_verified(bench.registry.teams.as_ref().unwrap(),&worker).unwrap());
    assert_eq!(bench.registry.teams.as_ref().unwrap().query_row("SELECT count(*) FROM chief_grants", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn chief_malformed_native_counts_invalidate_availability_without_erasing_replay_order() {
    let mut bench = team_bench();
    let produced = native_telemetry_pipeline(&mut bench,"idle");
    let sequence = produced[0]["sequence"].as_i64().unwrap();
    crate::teams::request(&mut bench.registry,20,telemetry_action(sequence+1,1,0)).unwrap().result.unwrap();
    assert_eq!(crate::teams::request(&mut bench.registry,20,telemetry_action(sequence+2,0,10_001)).unwrap_err().code,ErrorCode::Invalid);
    let replay: AgentAction = serde_json::from_value(produced[0].clone()).unwrap();
    assert_eq!(crate::teams::request(&mut bench.registry,20,replay).unwrap_err().code,ErrorCode::Conflict);
    assert!(bench.registry.chief_telemetry[OPENCODE_SESSION].counts.is_none());
}

#[test]
fn chief_native_telemetry_expires_by_host_receipt_time_and_cannot_refresh_by_replay() {
    let mut bench = team_bench();
    let worker = verified(&mut bench,OPENCODE_SESSION);
    let entry = bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap();
    entry.state = "waiting".into(); entry.needs_input = false;
    entry.since = (crate::teams::now_ms()/1000-600) as f64;
    crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::VerifyChiefAdmission {
        session:worker.clone(),adapter:"opencode-queued-v1".into(),evidence:"Controlled host proof fixture".into(),
    }).unwrap().result.unwrap();
    bench.registry.chief_focus = Some((String::new(),false,false,crate::teams::now_ms()));
    let before = crate::teams::now_ms();
    let produced = native_telemetry_pipeline(&mut bench,"idle");
    assert!(bench.registry.chief_telemetry[OPENCODE_SESSION].received_at_ms >= before);
    let ready = |bench:&mut Bench| crate::chief::workers(&mut bench.registry).unwrap().into_iter().find(|state|state.session==worker).unwrap().ready;
    assert!(ready(&mut bench));
    bench.registry.chief_telemetry.get_mut(OPENCODE_SESSION).unwrap().received_at_ms -= 31_000;
    assert!(!ready(&mut bench));
    let expired = bench.registry.chief_telemetry[OPENCODE_SESSION].received_at_ms;
    let replay = serde_json::from_value(produced[0].clone()).unwrap();
    assert_eq!(crate::teams::request(&mut bench.registry,20,replay).unwrap_err().code,ErrorCode::Conflict);
    assert_eq!(bench.registry.chief_telemetry[OPENCODE_SESSION].received_at_ms,expired);
    assert!(!ready(&mut bench));
    let next = produced[0]["sequence"].as_i64().unwrap()+1;
    crate::teams::request(&mut bench.registry,20,telemetry_action(next,0,0)).unwrap().result.unwrap();
    assert!(ready(&mut bench));
    bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap().state = "working".into();
    assert!(!ready(&mut bench),"zero counts cannot bypass host registry busy state");
    bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap().state = "waiting".into();
    bench.registry.chief_telemetry.get_mut(OPENCODE_SESSION).unwrap().session.runtime = "another-runtime".into();
    assert!(!ready(&mut bench));
    bench.registry.chief_telemetry.clear();
    assert!(!ready(&mut bench),"restart clears telemetry");
}

#[test]
fn chief_and_legacy_readiness_scope_host_requests_to_the_protected_owner() {
    for kind in ["running-command","queued-guide"] {
        for owner in ["other","other-same-project","related","unresolved-other","unresolved-related"] {
            let mut bench = team_bench();
            let project = bench.fixture.root.join("home/IdeaProjects/project-b");
            fs::create_dir_all(project.join(".git")).unwrap();
            let directory = project.to_string_lossy().into_owned();
            let entry = bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap();
            entry.cwd = directory.clone(); entry.state = "waiting".into(); entry.needs_input = false;
            entry.since = (crate::teams::now_ms()/1000-600) as f64;
            bench.world.borrow_mut().sender_directories.insert(20,directory.clone());
            if owner=="other-same-project" {
                bench.registry.sessions.get_mut(SESSION_A).unwrap().cwd = directory.clone();
                bench.world.borrow_mut().sender_directories.insert(10,directory.clone());
            }
            let worker = verified(&mut bench,OPENCODE_SESSION);
            crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::VerifyChiefAdmission {
                session:worker.clone(),adapter:"opencode-queued-v1".into(),evidence:"Controlled host proof fixture".into(),
            }).unwrap().result.unwrap();
            crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ChiefNativeObservation {
                session:worker.clone(),idle:true,pending_input:false,observed_at_ms:crate::teams::now_ms(),
            }).unwrap().result.unwrap();
            bench.registry.chief_focus = Some((String::new(),false,false,crate::teams::now_ms()));
            let id = cockpit::todo::TodoList::load().add("project-b",&directory,"eligible work");
            let related = matches!(owner,"related"|"unresolved-related");
            let peer = if related {20} else {10};
            let requester = if related {OPENCODE_SESSION} else {SESSION_A};
            let claimed = if owner.starts_with("unresolved") { if related {SESSION_A} else {OPENCODE_SESSION} } else {requester};
            let started_in = bench.registry.sessions[requester].cwd.clone();
            let key = answers_key_of(std::path::Path::new(&started_in));
            let request_id = "cccccccccccc";
            let request_kind = if kind=="running-command" {"run-request"} else {"guide-request"};
            bench.registry.request_from(request_kind,&serde_json::json!({"id":request_id,"answers":key,"session":claimed,
                "verified_session":OPENCODE_SESSION,"command":"private command","why":"test ownership","title":"guide"}).to_string(),peer).unwrap();
            let extension = if kind=="running-command" {"request"} else {"guide"};
            let held = cockpit::util::read_json(&cockpit::approval::held_request(request_id,extension)).unwrap();
            if owner.starts_with("unresolved") { assert!(held.get("verified_session").is_none()); }
            else { assert_eq!(held["verified_session"],requester); }
            if kind=="running-command" {
                bench.registry.run_phase(request_id,"running");
                assert!(bench.registry.runs.running.iter().any(|id|id==request_id));
            } else {
                bench.registry.guides.current.clear();
                bench.registry.guides.queue.push(request_id.into());
            }
            let ready = crate::chief::workers(&mut bench.registry).unwrap().into_iter().find(|state|state.session==worker).unwrap().ready;
            assert_eq!(ready,!related,"chief {kind}: {owner}");
            let reserved = crate::teams::host(&mut bench.registry, std::process::id(),HostRequest::ReserveTask {
                task_id:id,directory,worker:Some(OPENCODE_SESSION.into()),automatic:true,
            });
            assert_eq!(reserved.is_ok_and(|reply|reply.result.is_ok()),!related,"legacy {kind}: {owner}");
        }
    }
}

const STOPPED_WORKER: &str = "ses_3f2a9c0b2000AbCdEfGhIjKlMn";
const STOPPED_WORKER_PEER: u32 = 40;

/// A second OpenCode session next to the Chief (`OPENCODE_SESSION`), waiting, with a conversation on disk.
fn stop_bench() -> Bench {
    let mut bench = team_bench();
    let project = bench.fixture.root.join("home/IdeaProjects/project-a");
    {
        let mut world = bench.world.borrow_mut();
        world.sender_targets.insert(STOPPED_WORKER_PEER, target(&[("tmux_pane", "%40")]));
        world.sender_directories.insert(STOPPED_WORKER_PEER, project.to_string_lossy().into_owned());
        world.sender_runtimes.insert(STOPPED_WORKER_PEER, "runtime-40".into());
    }
    let report = parse_session(&json!({"session":STOPPED_WORKER,"agent":"opencode","project":"project-a","cwd":project,"state":"waiting"}).to_string()).unwrap();
    bench.registry.report_from(report, STOPPED_WORKER_PEER).unwrap();
    bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap().since = (crate::teams::now_ms()/1000-10) as f64;
    enable(&mut bench, OPENCODE_SESSION);
    admit_chief(&mut bench);
    bench.registry.teams.as_ref().unwrap().execute("UPDATE chief_coordination SET last_admitted_at=last_admitted_at-31000", []).unwrap();
    write_conversation(&bench, STOPPED_WORKER, &[("user", "Fix login"), ("assistant", "Which option do you want?")]);
    bench
}

fn write_conversation(bench: &Bench, session: &str, messages: &[(&str, &str)]) {
    let path = bench.fixture.root.join("opencode.db");
    let connection = LinkConnection::open(&path).unwrap();
    connection.execute_batch("create table if not exists session_message (id text primary key, session_id text, type text, seq integer, data text);").unwrap();
    for (index, (role, text)) in messages.iter().enumerate() {
        let data = if *role == "assistant" { json!({"content":[{"type":"text","text":text}]}) } else { json!({"text":text}) };
        connection.execute("insert into session_message values (?1, ?2, ?3, ?4, ?5)",
            (format!("{session}-{index}"), session, role, index as i64 + 1, data.to_string())).unwrap();
    }
    // SAFETY: the fixture lock held by `bench` keeps every test that touches the environment one at a time.
    unsafe { std::env::set_var("COCKPIT_OPENCODE_DB", &path); }
}

fn assign_doing_todo(bench: &Bench, session: &str) -> String {
    let directory = bench.registry.sessions[STOPPED_WORKER].cwd.clone();
    let mut todos = cockpit::todo::TodoList::load();
    let id = todos.add("project-a", &directory, "Fix login");
    assert!(todos.set_status(&id, "doing", session));
    id
}

fn report_worker_permissions(bench: &mut Bench, sequence: i64, permissions: u32) {
    let action = serde_json::from_value(json!({"action":"native_telemetry","sequence":sequence,"counts":{
        "running_sessions":0,"pending_inbox":0,"pending_permissions":permissions,"pending_forms":0}})).unwrap();
    crate::teams::request(&mut bench.registry, STOPPED_WORKER_PEER, action).unwrap().result.unwrap();
}

fn stop_states(bench: &Bench) -> Vec<(String, String)> {
    let connection = bench.registry.teams.as_ref().unwrap();
    let mut statement = connection.prepare("SELECT boundary,state FROM chief_stops ORDER BY created_at,rowid").unwrap();
    statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
}

fn stop_event_count(bench: &Bench) -> i64 {
    bench.registry.teams.as_ref().unwrap().query_row("SELECT count(*) FROM chief_events WHERE kind='stop_detected'", [], |row| row.get(0)).unwrap()
}

fn warnings_about_stops() -> usize {
    fs::read_to_string(cockpit::paths::log()).unwrap_or_default().lines().filter(|line| line.contains("WARN") && line.contains("stop bundle")).count()
}

#[test]
fn a_stopped_worker_with_an_open_todo_wakes_the_chief_with_its_bundle() {
    let mut bench = stop_bench();
    let task = assign_doing_todo(&bench, STOPPED_WORKER);
    bench.registry.tick_queues();
    let boundary = format!("waiting:{}", bench.registry.sessions[STOPPED_WORKER].since as i64);
    assert_eq!(stop_states(&bench), [(boundary, "open".to_string())]);
    assert_eq!(stop_event_count(&bench), 1);
    prepare_chief_boundary(&mut bench);
    // A receipt spends its idle boundary, so the Chief needs a new one for this wake.
    bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap().since -= 7.0;
    let dispatch = crate::teams::request(&mut bench.registry, 20, AgentAction::Receive).unwrap().result.unwrap();
    assert_eq!(dispatch["envelope"]["kind"], "chief_wake");
    crate::teams::request(&mut bench.registry, 20, AgentAction::Receipt { dispatch_id:dispatch["dispatch_id"].as_str().unwrap().into(),
        outcome:cockpit::teams::TransportOutcome::Received {native_id:"chief-stop-wake".into()} }).unwrap().result.unwrap();
    let page = context(&mut bench, 20).unwrap().result.unwrap();
    let stops: Vec<_> = page["records"].as_array().unwrap().iter().filter(|record| record["kind"] == "stopped_session").collect();
    assert_eq!(stops.len(), 1, "{page}");
    assert_eq!(stops[0]["session"], STOPPED_WORKER);
    assert_eq!(stops[0]["task_id"], task);
    assert_eq!(stops[0]["bundle"]["last_assistant"], "Which option do you want?");
    assert_eq!(stops[0]["bundle"]["task_text"], "Fix login");
}

#[test]
fn a_stop_wakes_the_chief_with_habits_and_instructions() {
    let mut bench = stop_bench();
    let project = bench.registry.sessions[STOPPED_WORKER].cwd.clone();
    fs::write(std::path::Path::new(&project).join("AGENTS.md"), "Run make test before every commit.").unwrap();
    let project_key = cockpit::teams::canonical_project(&project).unwrap();
    bench.registry.teams.as_ref().unwrap().execute("INSERT INTO chief_habits(id,text,project,origin_project,source,source_kind,source_id,state,created_at,changed_at)
        VALUES ('habit-a','Prefer small commits.',?1,?1,'overrule','overrule','overrule-a','active',100,100)", [&project_key]).unwrap();
    assign_doing_todo(&bench, STOPPED_WORKER);
    bench.registry.tick_queues();
    prepare_chief_boundary(&mut bench);
    bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap().since -= 7.0;
    let dispatch = crate::teams::request(&mut bench.registry, 20, AgentAction::Receive).unwrap().result.unwrap();
    assert_eq!(dispatch["envelope"]["kind"], "chief_wake");
    crate::teams::request(&mut bench.registry, 20, AgentAction::Receipt { dispatch_id:dispatch["dispatch_id"].as_str().unwrap().into(),
        outcome:cockpit::teams::TransportOutcome::Received {native_id:"chief-habit-wake".into()} }).unwrap().result.unwrap();
    let page = context(&mut bench, 20).unwrap().result.unwrap();
    let records = page["records"].as_array().unwrap();
    let habits = records.iter().find(|record| record["kind"] == "habits").unwrap_or_else(|| panic!("{page}"));
    assert_eq!(habits["habits"], json!([{"id":"habit-a","text":"Prefer small commits.","project":project_key,"source":"overrule"}]));
    let stop = records.iter().find(|record| record["kind"] == "stopped_session").unwrap();
    assert_eq!(stop["bundle"]["instructions"], json!([{"path":"AGENTS.md","text":"Run make test before every commit."}]));
}

#[test]
fn a_project_key_without_a_path_reads_no_instruction_files() {
    let (instructions, warnings) = crate::chief::project_instructions("/etc");
    assert!(instructions.is_empty());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("/etc"), "{warnings:?}");
}

#[test]
fn a_stop_without_an_open_todo_is_ignored() {
    for case in ["no todo", "todo not started", "doing for another session"] {
        let mut bench = stop_bench();
        match case {
            "todo not started" => { let directory = bench.registry.sessions[STOPPED_WORKER].cwd.clone();
                cockpit::todo::TodoList::load().add("project-a", &directory, "Fix login"); },
            "doing for another session" => { assign_doing_todo(&bench, SESSION_B); },
            _ => {},
        }
        bench.registry.tick_queues();
        assert!(stop_states(&bench).is_empty(), "{case}");
        assert_eq!(stop_event_count(&bench), 0, "{case}");
    }
}

fn stop_task_ids(bench: &Bench) -> Vec<String> {
    let connection = bench.registry.teams.as_ref().unwrap();
    let mut statement = connection.prepare("SELECT task_id FROM chief_stops ORDER BY created_at,rowid").unwrap();
    statement.query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

#[test]
fn a_session_asking_for_an_answer_with_no_todo_in_progress_is_a_stop_without_a_task() {
    let mut bench = stop_bench();
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "needs-you".into(); entry.needs_input = true;
    report_worker_permissions(&mut bench, 1, 0);
    bench.registry.tick_queues();
    let boundary = format!("needs-you:{}", bench.registry.sessions[STOPPED_WORKER].since as i64);
    assert_eq!(stop_states(&bench), [(boundary, "open".to_string())]);
    assert_eq!(stop_task_ids(&bench), [String::new()]);
    assert_eq!(stop_event_count(&bench), 1, "the Chief is woken for it");
}

#[test]
fn a_session_that_only_finished_its_turn_with_no_todo_is_no_stop() {
    let mut bench = stop_bench();
    report_worker_permissions(&mut bench, 1, 0);
    bench.registry.tick_queues();
    assert!(stop_states(&bench).is_empty());
}

fn chief_answers(bench: &mut Bench, stop: &str) {
    let action = AgentAction::Chief { request: ChiefAgentAction::Answer { stop: stop.into(), text: "Use option A.".into(), reason: "The task says so.".into(), habits: vec![] } };
    crate::teams::request(&mut bench.registry, 20, action).unwrap().result.unwrap();
}

fn answer_inbox_states(bench: &Bench) -> Vec<String> {
    let connection = bench.registry.teams.as_ref().unwrap();
    let mut statement = connection.prepare("SELECT state FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_answer' ORDER BY rowid").unwrap();
    statement.query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

/// The worker resumes on its own, then stops again on the same todo.
fn resume_and_stop_again(bench: &mut Bench) {
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "working".into(); entry.since += 5.0;
    bench.registry.tick_queues();
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "waiting".into(); entry.since += 5.0;
    bench.registry.tick_queues();
}

#[test]
fn a_queued_answer_survives_a_brief_resume_and_answers_the_next_stop_on_the_same_todo() {
    let mut bench = stop_bench();
    assign_doing_todo(&bench, STOPPED_WORKER);
    bench.registry.tick_queues();
    let stop: String = bench.registry.teams.as_ref().unwrap().query_row("SELECT id FROM chief_stops", [], |row| row.get(0)).unwrap();
    chief_answers(&mut bench, &stop);
    resume_and_stop_again(&mut bench);
    let states: Vec<String> = stop_states(&bench).into_iter().map(|(_, state)| state).collect();
    assert_eq!(states, ["superseded", "answered"]);
    assert_eq!(answer_inbox_states(&bench), ["queued"]);
}

#[test]
fn a_queued_answer_is_cancelled_when_its_session_is_gone() {
    let mut bench = stop_bench();
    assign_doing_todo(&bench, STOPPED_WORKER);
    bench.registry.tick_queues();
    let stop: String = bench.registry.teams.as_ref().unwrap().query_row("SELECT id FROM chief_stops", [], |row| row.get(0)).unwrap();
    chief_answers(&mut bench, &stop);
    bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap().runtime.clear();
    bench.world.borrow_mut().sender_runtimes.remove(&STOPPED_WORKER_PEER);
    bench.registry.tick_queues();
    assert_eq!(answer_inbox_states(&bench), ["cancelled"]);
}

#[test]
fn only_other_opencode_sessions_than_the_chief_can_stop() {
    let mut bench = stop_bench();
    let directory = bench.registry.sessions[SESSION_B].cwd.clone();
    let mut todos = cockpit::todo::TodoList::load();
    for (session, text) in [(OPENCODE_SESSION, "chief work"), (SESSION_B, "claude work")] {
        let id = todos.add("project-a", &directory, text);
        assert!(todos.set_status(&id, "doing", session));
    }
    for session in [OPENCODE_SESSION, SESSION_B] { write_conversation(&bench, session, &[("user", "hello"), ("assistant", "done?")]); }
    bench.registry.sessions.get_mut(SESSION_B).unwrap().since = (crate::teams::now_ms()/1000-10) as f64;
    bench.registry.tick_queues();
    assert!(stop_states(&bench).is_empty());
}

#[test]
fn a_permission_prompt_is_never_a_chief_stop() {
    let mut bench = stop_bench();
    assign_doing_todo(&bench, STOPPED_WORKER);
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "needs-you".into(); entry.needs_input = true;
    bench.registry.tick_queues();
    assert!(stop_states(&bench).is_empty(), "without native telemetry the prompt cannot be told apart from a question");
    report_worker_permissions(&mut bench, 1, 1);
    bench.registry.tick_queues();
    assert!(stop_states(&bench).is_empty(), "an open permission prompt");
    report_worker_permissions(&mut bench, 2, 0);
    bench.registry.chief_telemetry.get_mut(STOPPED_WORKER).unwrap().received_at_ms -= 31_000;
    bench.registry.tick_queues();
    assert!(stop_states(&bench).is_empty(), "stale telemetry");
    report_worker_permissions(&mut bench, 3, 0);
    bench.registry.tick_queues();
    let boundary = format!("needs-you:{}", bench.registry.sessions[STOPPED_WORKER].since as i64);
    assert_eq!(stop_states(&bench), [(boundary, "open".to_string())], "a question with no permission open is a stop");
}

#[test]
fn a_question_stop_needs_telemetry_received_after_the_stop() {
    let mut bench = stop_bench();
    assign_doing_todo(&bench, STOPPED_WORKER);
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "needs-you".into(); entry.needs_input = true;
    let since = entry.since as i64;
    report_worker_permissions(&mut bench, 1, 0);
    bench.registry.chief_telemetry.get_mut(STOPPED_WORKER).unwrap().received_at_ms = since * 1000 + 999;
    bench.registry.tick_queues();
    assert!(stop_states(&bench).is_empty(), "a sample from the stop's own second may predate the question");
    report_worker_permissions(&mut bench, 2, 0);
    bench.registry.tick_queues();
    assert_eq!(stop_states(&bench), [(format!("needs-you:{since}"), "open".to_string())]);
}

#[test]
fn a_new_state_boundary_supersedes_the_open_stop() {
    let mut bench = stop_bench();
    assign_doing_todo(&bench, STOPPED_WORKER);
    bench.registry.tick_queues();
    let waiting = format!("waiting:{}", bench.registry.sessions[STOPPED_WORKER].since as i64);
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "working".into(); entry.since += 5.0;
    bench.registry.tick_queues();
    assert_eq!(stop_states(&bench), [(waiting.clone(), "superseded".to_string())], "an agent that resumed leaves no open stop");
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "waiting".into(); entry.since += 5.0;
    bench.registry.tick_queues();
    let second = format!("waiting:{}", bench.registry.sessions[STOPPED_WORKER].since as i64);
    assert_eq!(stop_states(&bench), [(waiting, "superseded".to_string()), (second, "open".to_string())]);
}

#[test]
fn the_same_boundary_is_recorded_once() {
    let mut bench = stop_bench();
    assign_doing_todo(&bench, STOPPED_WORKER);
    bench.registry.tick_queues();
    fs::remove_file(bench.fixture.root.join("opencode.db")).unwrap();
    for _ in 0..3 { bench.registry.tick_queues(); }
    assert_eq!(stop_states(&bench).len(), 1);
    assert_eq!(stop_event_count(&bench), 1);
    assert_eq!(warnings_about_stops(), 0, "a recorded boundary needs no new bundle");
}

#[test]
fn a_failing_bundle_is_retried_no_more_often_than_the_backoff_and_warns_once_per_boundary() {
    let mut bench = stop_bench();
    assign_doing_todo(&bench, STOPPED_WORKER);
    fs::remove_file(bench.fixture.root.join("opencode.db")).unwrap();
    for _ in 0..3 { bench.registry.tick_queues(); }
    assert!(stop_states(&bench).is_empty());
    assert_eq!(warnings_about_stops(), 1);
    write_conversation(&bench, STOPPED_WORKER, &[("assistant", "Back again")]);
    bench.registry.tick_queues();
    assert!(stop_states(&bench).is_empty(), "the history is readable again, but the retry is still backing off");
    bench.registry.chief_stop_failures.get_mut(STOPPED_WORKER).unwrap().1 -= 61_000;
    bench.registry.tick_queues();
    assert_eq!(stop_states(&bench).len(), 1, "the same boundary is retried once the backoff has passed");
    assert_eq!(warnings_about_stops(), 1, "a retry of the same boundary does not warn again");
}

#[test]
fn a_new_boundary_is_tried_at_once_even_while_an_older_one_backs_off() {
    let mut bench = stop_bench();
    assign_doing_todo(&bench, STOPPED_WORKER);
    fs::remove_file(bench.fixture.root.join("opencode.db")).unwrap();
    bench.registry.tick_queues();
    assert_eq!(warnings_about_stops(), 1);
    bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap().since += 5.0;
    write_conversation(&bench, STOPPED_WORKER, &[("assistant", "Back again")]);
    bench.registry.tick_queues();
    assert_eq!(stop_states(&bench).len(), 1);
}

#[test]
fn the_same_stop_before_and_after_a_daemon_restart_is_one_stop_and_one_wake() {
    let mut bench = stop_bench();
    assign_doing_todo(&bench, STOPPED_WORKER);
    // The sub-second part is what the snapshot drops, so a restarted daemon reads a different `since`.
    bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap().since += 0.5;
    bench.registry.tick_queues();
    assert_eq!(stop_states(&bench).len(), 1);
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.since = entry.since as i64 as f64;
    bench.registry.tick_queues();
    assert_eq!(stop_states(&bench), [(format!("waiting:{}", bench.registry.sessions[STOPPED_WORKER].since as i64), "open".to_string())]);
    assert_eq!(stop_event_count(&bench), 1);
    let wakes: i64 = bench.registry.teams.as_ref().unwrap().query_row(
        "SELECT count(*) FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_wake' AND state='queued'", [], |row| row.get(0)).unwrap();
    assert_eq!(wakes, 1);
}

#[test]
fn the_todo_list_is_only_read_when_a_session_is_at_a_stop_boundary() {
    let mut bench = stop_bench();
    assign_doing_todo(&bench, STOPPED_WORKER);
    let live = crate::chief::workers(&mut bench.registry).unwrap();
    let mut loads = 0;
    bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap().state = "working".into();
    crate::chief::observe_stops_with(&mut bench.registry, &live, crate::teams::now_ms(), || { loads += 1; cockpit::todo::TodoList::load() }).unwrap();
    assert_eq!(loads, 0, "a working session is no stop candidate");
    bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap().state = "waiting".into();
    crate::chief::observe_stops_with(&mut bench.registry, &live, crate::teams::now_ms(), || { loads += 1; cockpit::todo::TodoList::load() }).unwrap();
    assert_eq!(loads, 1);
    assert_eq!(stop_states(&bench).len(), 1);
}

/// A todo another project handed to project-a and queued, as `cockpit-todo add --project` files it.
fn handed_todo(bench: &Bench, id: &str, queued_by: &str) -> String {
    project_todo(bench, id, json!({"queued_by":queued_by}), true)
}

/// A todo of project-a (the Chief's own project in this bench) still to do, with `fields` on top.
fn project_todo(bench: &Bench, id: &str, fields: serde_json::Value, is_queued: bool) -> String {
    let directory = bench.registry.sessions[SESSION_B].cwd.clone();
    let mut todos: serde_json::Value = serde_json::from_str(&fs::read_to_string(cockpit::paths::todo()).unwrap()).unwrap();
    let mut item = json!({"id":id,"project":"project-a","cwd":directory,"text":"Fix the Figma MCP","status":"todo"});
    item.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
    todos["items"].as_array_mut().unwrap().push(item);
    if is_queued {
        let mut queued = todos["queued"].as_array().cloned().unwrap_or_default();
        queued.push(json!(id));
        todos["queued"] = json!(queued);
    }
    fs::write(cockpit::paths::todo(), todos.to_string()).unwrap();
    id.into()
}

fn admitted_chief_bench() -> Bench {
    let mut bench = team_bench();
    enable(&mut bench, OPENCODE_SESSION);
    admit_chief(&mut bench);
    bench
}

fn chief_starts_session(bench: &mut Bench, peer: u32, todo: &str, project: &str) -> Result<serde_json::Value, cockpit::teams::TeamError> {
    crate::teams::request(&mut bench.registry, peer, AgentAction::Chief { request: ChiefAgentAction::StartSession {
        todo: todo.into(), project: project.into(), reason: "Theo needs the Figma MCP fix today.".into(),
    }}).and_then(|reply| reply.result)
}

/// What the queue asks before it starts a fresh session for a handed todo.
fn reserve_fresh_session(bench: &mut Bench, todo: &str) -> Result<serde_json::Value, cockpit::teams::TeamError> {
    let directory = bench.registry.sessions[SESSION_B].cwd.clone();
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ReserveTask {
        task_id: todo.into(), directory, worker: None, automatic: true,
    }).and_then(|reply| reply.result)
}

fn session_start_states(bench: &Bench) -> Vec<String> {
    bench.registry.teams.as_ref().unwrap().prepare("SELECT state FROM chief_session_starts ORDER BY created_at,id").unwrap()
        .query_map([], |row| row.get(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap()
}

#[test]
fn a_queued_handed_todo_gets_no_fresh_session_while_its_project_runs_and_the_refusal_says_why() {
    let mut bench = admitted_chief_bench();
    let todo = handed_todo(&bench, "aa919cfe", "a cockpit-chief session (ses_e82b)");
    let refusal = reserve_fresh_session(&mut bench, &todo).unwrap_err();
    assert_eq!(refusal.code, ErrorCode::Conflict);
    assert!(refusal.message.ends_with(": a session of this project is running"), "{}", refusal.message);
}

#[test]
fn the_chief_can_start_a_fresh_session_for_a_queued_handed_todo_while_its_project_runs() {
    let mut bench = admitted_chief_bench();
    let todo = handed_todo(&bench, "aa919cfe", "a cockpit-chief session (ses_e82b)");
    let requested = chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap();
    assert_eq!(requested["state"], "requested");
    let reserved = reserve_fresh_session(&mut bench, &todo).unwrap();
    assert!(reserved["reservation"].as_str().is_some_and(|token| !token.is_empty()));
    assert_eq!(session_start_states(&bench), vec!["started"]);
    let notice = format!("chief-session-{}", requested["request"].as_str().unwrap());
    assert_eq!(bench.notice_title(&notice).as_deref(), Some("Chief · started a session in project-a"));
}

#[test]
fn a_started_chief_request_does_not_start_a_second_session() {
    let mut bench = admitted_chief_bench();
    let todo = handed_todo(&bench, "aa919cfe", "a cockpit-chief session (ses_e82b)");
    chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap();
    reserve_fresh_session(&mut bench, &todo).unwrap();
    assert_eq!(reserve_fresh_session(&mut bench, &todo).unwrap_err().code, ErrorCode::Conflict);
}

#[test]
fn the_chief_can_start_a_session_for_todos_theo_or_another_project_filed_queued_or_not() {
    for (fields, is_queued) in [
        (json!({}), true),
        (json!({}), false),
        (json!({"source":"session ses_other","reporter":"project-b","link":"project-b→project-a"}), false),
    ] {
        let mut bench = admitted_chief_bench();
        let todo = project_todo(&bench, "bb919cfe", fields.clone(), is_queued);
        chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap_or_else(|error| panic!("{fields} queued {is_queued}: {}", error.message));
        reserve_fresh_session(&mut bench, &todo).unwrap();
        assert_eq!(session_start_states(&bench), vec!["started"], "{fields} queued {is_queued}");
    }
}

#[test]
fn the_chief_cannot_start_a_session_for_a_todo_that_waits_or_is_for_the_designer_or_is_not_to_do() {
    for fields in [json!({"after":["ee919cfe"]}), json!({"kind":"design"}), json!({"status":"doing"}), json!({"status":"proposed"})] {
        let mut bench = admitted_chief_bench();
        let todo = project_todo(&bench, "bb919cfe", fields.clone(), true);
        assert_eq!(chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap_err().code, ErrorCode::Conflict, "{fields}");
        assert!(session_start_states(&bench).is_empty());
    }
}

#[test]
fn the_chief_cannot_start_a_session_for_a_todo_its_own_project_filed_until_theo_queues_it() {
    for (fields, is_queued) in [
        (json!({"source":"session ses_chief"}), false),
        (json!({"source":"session ses_chief","queued_by":"a project-a session (ses_chie) (project-a→project-b)"}), true),
        (json!({"source":"session ses_chief","reporter":"project-a","link":"project-a→project-a"}), true),
        (json!({"source":"session ses_chief","queued_by_theo":true}), false),
    ] {
        let mut bench = admitted_chief_bench();
        let todo = project_todo(&bench, "bb919cfe", fields.clone(), is_queued);
        let refusal = chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap_err();
        assert_eq!(refusal.code, ErrorCode::Conflict, "{fields} queued {is_queued}");
        assert!(refusal.message.contains("until Theo queues it"), "{}", refusal.message);
        assert!(session_start_states(&bench).is_empty());
    }
}

#[test]
fn the_chief_can_start_a_session_for_a_todo_its_own_project_filed_once_theo_queued_it() {
    let mut bench = admitted_chief_bench();
    let todo = project_todo(&bench, "bb919cfe", json!({"source":"session ses_chief","queued_by_theo":true}), true);
    chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap();
    reserve_fresh_session(&mut bench, &todo).unwrap();
    assert_eq!(session_start_states(&bench), vec!["started"]);
}

#[test]
fn the_chief_cannot_start_a_session_for_an_unknown_todo() {
    let mut bench = admitted_chief_bench();
    assert_eq!(chief_starts_session(&mut bench, 20, "cc919cfe", "project-a").unwrap_err().code, ErrorCode::NotFound);
}

#[test]
fn the_chief_names_the_project_the_todo_belongs_to() {
    let mut bench = admitted_chief_bench();
    let todo = handed_todo(&bench, "aa919cfe", "a cockpit-chief session (ses_e82b)");
    assert_eq!(chief_starts_session(&mut bench, 20, &todo, "project-b").unwrap_err().code, ErrorCode::Conflict);
}

#[test]
fn the_chief_can_ask_for_sessions_for_several_todos_of_one_project_at_once() {
    let mut bench = admitted_chief_bench();
    for id in ["aa919cfe", "dd919cfe", "ee919cfe"] {
        let todo = handed_todo(&bench, id, "a cockpit-chief session (ses_e82b)");
        chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap();
    }
    assert_eq!(session_start_states(&bench), vec!["requested"; 3]);
}

#[test]
fn only_one_chief_session_start_waits_per_todo() {
    let mut bench = admitted_chief_bench();
    let todo = handed_todo(&bench, "aa919cfe", "a cockpit-chief session (ses_e82b)");
    chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap();
    assert_eq!(chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap_err().code, ErrorCode::Conflict);
}

/// Session starts the Chief asked for 50 minutes ago, inside the rolling hour.
fn earlier_session_starts(bench: &Bench, count: usize, is_theo_queued: bool) {
    let recently = crate::teams::now_ms() - 50 * 60 * 1000;
    for index in 0..count {
        bench.registry.teams.as_ref().unwrap().execute("INSERT INTO chief_session_starts(id,epoch,project,task_id,reason,state,created_at,theo_queued)
            VALUES (?1,'epoch','path:/elsewhere','00000000','Earlier.','started',?2,?3)", (format!("earlier-{index}"), recently, is_theo_queued)).unwrap();
    }
}

fn set_chief_starts_per_hour(value: &str) {
    fs::write(cockpit::paths::settings(), json!({"chief-starts-per-hour": value}).to_string()).unwrap();
}

#[test]
fn the_chief_requests_at_most_six_session_starts_an_hour_by_default() {
    for (earlier, is_refused) in [(5, false), (6, true)] {
        let mut bench = admitted_chief_bench();
        let todo = handed_todo(&bench, "aa919cfe", "a cockpit-chief session (ses_e82b)");
        earlier_session_starts(&bench, earlier, false);
        let reply = chief_starts_session(&mut bench, 20, &todo, "project-a");
        assert_eq!(reply.err().map(|error| error.code), is_refused.then_some(ErrorCode::Limit), "{earlier} earlier starts");
    }
}

#[test]
fn the_hourly_cap_follows_theos_chief_starts_per_hour_setting() {
    let mut bench = admitted_chief_bench();
    set_chief_starts_per_hour("2");
    let todo = handed_todo(&bench, "aa919cfe", "a cockpit-chief session (ses_e82b)");
    earlier_session_starts(&bench, 2, false);
    let refusal = chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap_err();
    assert_eq!(refusal.code, ErrorCode::Limit);
    assert!(refusal.message.contains("at most 2 session starts an hour"), "{}", refusal.message);
}

#[test]
fn an_invalid_chief_starts_per_hour_setting_refuses_the_start_and_names_the_setting() {
    for value in ["0", "61", "six", "-1"] {
        let mut bench = admitted_chief_bench();
        set_chief_starts_per_hour(value);
        let todo = handed_todo(&bench, "aa919cfe", "a cockpit-chief session (ses_e82b)");
        let refusal = chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap_err();
        assert!(refusal.message.contains("chief-starts-per-hour"), "{value:?}: {}", refusal.message);
        assert!(session_start_states(&bench).is_empty());
    }
}

#[test]
fn a_todo_theo_queued_himself_starts_even_when_the_hourly_cap_is_reached() {
    let mut bench = admitted_chief_bench();
    earlier_session_starts(&bench, 6, false);
    let todo = project_todo(&bench, "bb919cfe", json!({"queued_by_theo":true}), true);
    chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap();
}

#[test]
fn starts_for_todos_theo_queued_do_not_count_against_the_hourly_cap() {
    let mut bench = admitted_chief_bench();
    earlier_session_starts(&bench, 6, true);
    let todo = handed_todo(&bench, "aa919cfe", "a cockpit-chief session (ses_e82b)");
    chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap();
}

#[test]
fn a_theo_queued_mark_on_a_todo_out_of_the_queue_still_counts_against_the_cap() {
    let mut bench = admitted_chief_bench();
    earlier_session_starts(&bench, 6, false);
    let todo = project_todo(&bench, "bb919cfe", json!({"queued_by_theo":true}), false);
    assert_eq!(chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap_err().code, ErrorCode::Limit);
}

#[test]
fn an_expired_chief_request_no_longer_starts_a_session() {
    let mut bench = admitted_chief_bench();
    let todo = handed_todo(&bench, "aa919cfe", "a cockpit-chief session (ses_e82b)");
    chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap();
    bench.registry.teams.as_ref().unwrap().execute("UPDATE chief_session_starts SET created_at=created_at-?1", [cockpit::chief::SESSION_START_OPEN_MS]).unwrap();
    assert_eq!(reserve_fresh_session(&mut bench, &todo).unwrap_err().code, ErrorCode::Conflict);
    assert_eq!(session_start_states(&bench), vec!["requested"]);
}

#[test]
fn a_chief_request_still_waits_for_the_other_queue_checks() {
    let mut bench = admitted_chief_bench();
    let todo = handed_todo(&bench, "aa919cfe", "a cockpit-chief session (ses_e82b)");
    chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap();
    bench.registry.chief_focus.as_mut().unwrap().3 -= 31_000;
    let refusal = reserve_fresh_session(&mut bench, &todo).unwrap_err();
    assert!(refusal.message.ends_with(": the cockpit focus report is stale"), "{}", refusal.message);
    assert_eq!(session_start_states(&bench), vec!["requested"]);
}

#[test]
fn a_session_without_the_chief_grant_cannot_start_sessions() {
    let mut bench = admitted_chief_bench();
    let todo = handed_todo(&bench, "aa919cfe", "a cockpit-chief session (ses_e82b)");
    assert_eq!(chief_starts_session(&mut bench, 10, &todo, "project-a").unwrap_err().code, ErrorCode::Unauthorized);
    assert!(session_start_states(&bench).is_empty());
}
