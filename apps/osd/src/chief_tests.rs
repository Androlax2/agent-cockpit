use super::*;
use cockpit::chief::{ChiefAgentAction, ChiefHostAction};
use cockpit::teams::{AgentAction, ErrorCode, HostRequest, VerifiedSession};

#[path = "chief_workflow_tests.rs"]
mod workflow;

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
fn chief_unavailable_is_one_host_notice_even_without_a_chief_report() {
    let mut bench = team_bench();
    enable(&mut bench,SESSION_A);
    bench.registry.tick_queues();
    let notices = bench.raised().into_iter().filter(|id|id.starts_with("chief-decision-")).collect::<Vec<_>>();
    assert_eq!(notices.len(),1);
    bench.registry.tick_queues();
    assert_eq!(bench.raised().into_iter().filter(|id|id.starts_with("chief-decision-")).count(),1);
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
    assert!(crate::teams::request(&mut bench.registry,20,AgentAction::Receive).unwrap().result.unwrap().is_null(),"the complete five-minute idle guard still applies");
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
        let todos=cockpit::todo::TodoList::load();
        assert_eq!(todos.status_of("owner/project-a#42"),"doing");
        let token=todos.manual_reservation("owner/project-a#42").unwrap();
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
