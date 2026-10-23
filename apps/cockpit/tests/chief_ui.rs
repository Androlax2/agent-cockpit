//! Approved Chief controls through real App, rendering and host IPC.
use cockpit::app::{App, Key, Screen};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::{Value, json};
use std::{collections::VecDeque, fs, io::{Read, Write}, os::unix::net::UnixListener,
    path::PathBuf, sync::{Arc, Mutex}, time::Duration};

static ENVIRONMENT: Mutex<()> = Mutex::new(());
const PROJECT: &str = "path:/home/theo/IdeaProjects/a-long-canonical-project";
fn proposal(id: &str, revision: i64) -> Value {
    let brief = json!({"work":{"task":{"project":PROJECT,"task_id":"1234abcd"},"revision":revision},
        "brief":"Bounded scope", "role":"developer", "prerequisites":[{"task":{"project":PROJECT,"task_id":"abcd1234"},"revision":3}],
        "gates":[{"kind":"tests","source":"host_attestation","expected":{"commit":"a".repeat(40)}}]});
    let typed: cockpit::chief::WorkBrief = serde_json::from_value(brief.clone()).unwrap();
    json!({"kind":"proposal","id":id,"sender":{"session_id":"ses_proposer","agent":"opencode","project":PROJECT,"runtime":"runtime-proposer"},
        "brief":brief,"approval_hash":typed.approval_hash().unwrap(),"created_at_ms":1,"provenance":"agent_proposal_not_authority","freshness":"pending"})
}
fn snapshot(records: Vec<Value>) -> Value {
    json!({"records":records,"generation":"generation-1","next":0,"has_more":false,"projection":"host_review","cursor_kind":"snapshot_offset","scheduling_owned":false,"coordination_error":null})
}
#[derive(Default)]
struct TestScreen { picks: VecDeque<String> }
impl Screen for TestScreen {
    fn draw(&mut self, _: &mut App) {}
    fn next_key(&mut self, _: Option<Duration>) -> Option<Key> { None }
    fn outside(&mut self, _: &mut dyn FnMut() -> Option<String>) -> Option<String> { self.picks.pop_front() }
}
struct Host {
    root: PathBuf,
    snapshot: Arc<Mutex<Value>>,
    requests: Arc<Mutex<Vec<Value>>>,
    results: Arc<Mutex<VecDeque<Value>>>,
    pages: Arc<Mutex<VecDeque<Value>>>,
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl Host {
    fn new(records: Vec<Value>) -> Self {
        let lock = ENVIRONMENT.lock().unwrap_or_else(|error|error.into_inner());
        let root = std::env::temp_dir().join(format!("chief-ui-{}",cockpit::util::token_hex(6)));
        for directory in ["home", "run", "state"] {fs::create_dir_all(root.join(directory)).unwrap();}
        // SAFETY: every test holds ENVIRONMENT for its entire lifetime.
        unsafe {
            for (name,directory) in [("HOME","home"),("XDG_RUNTIME_DIR","run"),("XDG_STATE_HOME","state"),("COCKPIT_STATE_HOME","state")] {
                std::env::set_var(name,root.join(directory));
            }
            std::env::set_var("CLAUDE_STATUS_URL",""); std::env::set_var("CODEX_STATUS_URL","");
            std::env::set_var("CLAUDE_AUDIT_JOURNAL","off");
            std::env::set_var("COCKPIT_OPENCODE_DB",root.join("absent.db"));
        }
        let path = cockpit::paths::team_host_socket(); fs::create_dir_all(path.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(path).unwrap();
        let snapshot = Arc::new(Mutex::new(snapshot(records)));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let results = Arc::new(Mutex::new(VecDeque::<Value>::new()));
        let pages = Arc::new(Mutex::new(VecDeque::<Value>::new()));
        let (state, calls, replies) = (snapshot.clone(),requests.clone(),results.clone());
        let read_pages=pages.clone();
        std::thread::spawn(move || for mut stream in listener.incoming().flatten() {
            let mut wire = String::new(); stream.read_to_string(&mut wire).unwrap();
            let request: Value = serde_json::from_str(&wire).unwrap(); calls.lock().unwrap().push(request.clone());
            let result = if request["request"] == "chief_snapshot" { read_pages.lock().unwrap().pop_front().unwrap_or_else(||state.lock().unwrap().clone()) }
                else { replies.lock().unwrap().pop_front().unwrap_or(json!({"recorded":true})) };
            let result = if let Some(message)=result["error"].as_str() {Err(cockpit::teams::TeamError::new(cockpit::teams::ErrorCode::Storage,message))} else {Ok(result)};
            let reply = serde_json::to_value(cockpit::teams::TeamReply {request_id:"fixture".into(),result}).unwrap();
            stream.write_all(reply.to_string().as_bytes()).unwrap();
        });
        Self {root,snapshot,requests,results,pages,_lock:lock}
    }
    fn open(&self) -> (App, TestScreen) {
        let mut app = App::new(); let mut screen = TestScreen {picks:VecDeque::from(["-\tchief".into()])};
        app.handle(Key::Char(':'),&mut screen); (app,screen)
    }
    fn mutations(&self) -> Vec<Value> { self.requests.lock().unwrap().iter().filter(|request|request["request"]=="chief").cloned().collect() }
}
impl Drop for Host { fn drop(&mut self) { fs::remove_dir_all(&self.root).unwrap(); } }
fn keys(app: &mut App, screen: &mut TestScreen, keys: &[Key]) {for key in keys {app.handle(key.clone(),screen);}}
fn rendered(app: &mut App, width: u16, height: u16) -> String {
    let mut buffer = Buffer::empty(Rect::new(0,0,width,height)); app.render(&mut buffer);
    buffer.content.iter().map(|cell|cell.symbol()).collect::<String>()
}
fn awaiting(app: &mut App, screen: &mut TestScreen) { keys(app,screen,&[Key::Down,Key::Enter,Key::Enter]); }

#[test]
fn chief_controls_show_exact_brief_revision_and_scope_before_approval() {
    let host = Host::new(vec![proposal("p1",1)]); let (mut app,mut screen) = host.open();
    assert!(rendered(&mut app,120,45).contains("Chief")); awaiting(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Char('a')]); let review = rendered(&mut app,180,65);
    for expected in [PROJECT,"revision","approval_hash","developer","prerequisites","abcd1234","host_attestation"] {
        assert!(review.contains(expected),"missing {expected}: {review}");
    }
    app.handle(Key::Char('y'),&mut screen); assert!(host.mutations().is_empty());
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]); let requests = host.mutations(); assert_eq!(requests.len(),1);
    assert_eq!(requests[0]["action"]["action"],"approve_work");
    assert_eq!(requests[0]["action"]["brief"],proposal("p1",1)["brief"]); assert_eq!(requests[0]["action"]["expected_revision"],0);
}

#[test]
fn narrow_review_and_confirmation_keep_cancel_and_action_instructions_visible() {
    let host=Host::new(vec![proposal("p1",1)]); let (mut app,mut screen)=host.open(); awaiting(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Char('a')]);
    let review=rendered(&mut app,24,8);
    assert!(review.contains("Esc cancel"),"{review}");
    assert!(review.contains("Enter"),"{review}");
    app.handle(Key::Enter,&mut screen);
    let confirmation=rendered(&mut app,24,8);
    assert!(confirmation.contains("Esc"),"{confirmation}");
    assert!(confirmation.contains("y submit"),"{confirmation}");
}

#[test]
fn stale_worker_and_pending_lists_keep_read_failure_after_movement() {
    for pending in [false,true] {
        let host=Host::new(vec![proposal("p1",1),worker()]); let (mut app,mut screen)=host.open();
        if pending {awaiting(&mut app,&mut screen);} else {keys(&mut app,&mut screen,&[Key::Down,Key::Down,Key::Enter]);}
        app.handle(Key::Tab,&mut screen);
        host.pages.lock().unwrap().push_back(json!({"error":"Host storage failed"}));
        screen.picks.push_back("-\tchief".into());app.handle(Key::Char(':'),&mut screen);
        app.handle(Key::Down,&mut screen);
        let text=rendered(&mut app,120,45);
        assert!(text.contains("Host storage failed"),"{text}");
    }
}

#[test]
fn live_but_unsupported_or_unverified_chiefs_are_unavailable() {
    for (agent,blocker) in [("claude","native_admission_unverified_for_adapter"),("opencode","live_queued_admission_verification_required")] {
        let mut selected=chief("active",true); selected["session"]["agent"]=json!(agent);
        let mut observed=worker(); observed["session"]=selected["session"].clone(); observed["blocked_by"]=json!([blocker]);
        let host=Host::new(vec![selected,observed]); let (mut app,_)=host.open();
        let text=rendered(&mut app,120,45);
        assert!(text.contains("Unavailable"),"{text}");
        assert!(text.contains("retained active grant"),"{text}");
    }
}
#[test]
fn old_confirmation_cannot_approve_new_revision() {
    let host = Host::new(vec![proposal("p1",1)]); let (mut app,mut screen)=host.open(); awaiting(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Char('a'),Key::Enter]);
    host.snapshot.lock().unwrap()["records"] = json!([proposal("p2",2)]); app.handle(Key::Char('y'),&mut screen);
    assert!(host.mutations().is_empty()); assert!(rendered(&mut app,120,45).to_lowercase().contains("changed"));
}
#[test]
fn empty_selection_and_detail_y_never_submit() {
    let host=Host::new(vec![proposal("p1",1)]); let (mut app,mut screen)=host.open(); awaiting(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Char('a'),Key::Char('y'),Key::Enter,Key::Char('y')]); assert!(host.mutations().is_empty());
}
#[test]
fn root_cancel_restores_focus_and_tab_does_not_submit() {
    let host=Host::new(vec![proposal("p1",1)]); let (mut app,mut screen)=host.open(); awaiting(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Char('a'),Key::Enter]);
    let before=app.focus; app.handle(Key::Tab,&mut screen); assert_ne!(app.focus,before);
    app.handle(Key::BackTab,&mut screen); assert_eq!(app.focus,before);
    app.handle(Key::Char('n'),&mut screen); assert!(host.mutations().is_empty());
}
#[test]
fn summary_request_does_not_enable_progress_notifications() {
    let host=Host::new(vec![json!({"kind":"summary","state":"completed","last_completed_at_ms":12345,
        "last_result":{"provenance":"chief_narrative_unverified","sections":{"completed":"Worker claim only"},"references":[]}})]);
    let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Down,Key::Down,Key::Down,Key::Down,Key::Enter,Key::Enter,Key::Enter]);
    let requests=host.mutations(); assert_eq!(requests.len(),1); assert_eq!(requests[0]["action"]["action"],"request_summary");
    let text=rendered(&mut app,120,45); assert!(text.contains("12345")&&text.contains("chief_narrative_unverified"),"{text}");
}

fn worker() -> Value {
    json!({"kind":"worker","session":{"session_id":"ses_3f2a9c0b1ffeQkTz8mWn4rJd2L","agent":"opencode","project":PROJECT,"runtime":"verified-native-runtime"},
        "role":null,"ready":false,"blocked_by":["host_native_observation_missing","admission_not_verified","pending_human_input"],
        "observed_at_ms":1,"observation_provenance":"daemon_derived_availability","native_observation":{"source":"runtime_report","runtime_report":{"provenance":"runtime_report_not_independently_proven"}}})
}
fn chief(state: &str, live: bool) -> Value {
    json!({"kind":"chief","session":worker()["session"],"epoch":"grant-epoch-1","revision":2,"state":state,"live_verified":live,"approval_hash":"grant-hash","created_at_ms":1})
}
fn approved() -> Value {
    let proposal=proposal("p1",1);
    json!({"kind":"work","brief":proposal["brief"],"approval_hash":proposal["approval_hash"],"state":"approved","priority":0,"created_at_ms":1})
}
fn evidence_gate(app: &mut App, screen: &mut TestScreen) {
    keys(app,screen,&[Key::Down,Key::Enter,Key::Down,Key::Enter,Key::Enter,Key::Enter,Key::Down,Key::Enter,Key::Enter,Key::Enter]);
}
fn type_note(app: &mut App, screen: &mut TestScreen, note: &str) {
    for character in note.chars() {app.handle(Key::Char(character),screen);} app.handle(Key::Enter,screen);
}
#[test]
fn paused_and_unavailable_states_are_not_off() {
    let host=Host::new(vec![chief("paused",true)]);let (mut app,mut screen)=host.open();
    assert!(rendered(&mut app,120,45).contains("Paused"));
    host.snapshot.lock().unwrap()["records"]=json!([chief("active",false)]);
    app.handle(Key::Esc,&mut screen);screen.picks.push_back("-\tchief".into());app.handle(Key::Char(':'),&mut screen);
    assert!(rendered(&mut app,120,45).contains("Unavailable"));
    keys(&mut app,&mut screen,&[Key::Down,Key::Down,Key::Down,Key::Enter]);
    let controls=rendered(&mut app,120,45);assert!(controls.contains("Disable chief")&&!controls.contains("Select chief"),"{controls}");
    app.handle(Key::Enter,&mut screen);let review=rendered(&mut app,150,60);
    for expected in ["verified-native-runtime","grant-epoch-1","Workers and teams continue","grant-hash"] {assert!(review.contains(expected),"{review}");}
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);
    assert_eq!(host.mutations()[0]["action"]["expected_epoch"],"grant-epoch-1");
}
#[test]
fn worker_binding_reviews_existing_epoch_and_does_not_grant_admission() {
    let host=Host::new(vec![worker(),json!({"kind":"binding","session":worker()["session"],"epoch":"binding-epoch","revision":7,"role":"qa","state":"bound","approval_hash":"role-hash"})]);
    let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Down,Key::Down,Key::Enter,Key::Enter]);
    let detail=rendered(&mut app,160,65);assert!(detail.contains("host_native_observation_missing")&&detail.contains("runtime_report_not_independently_proven"),"{detail}");
    keys(&mut app,&mut screen,&[Key::Enter]);
    assert!(rendered(&mut app,120,45).contains("Bind role"));
    keys(&mut app,&mut screen,&[Key::Enter,Key::Down,Key::Enter]);
    let review=rendered(&mut app,160,65);for expected in ["binding-epoch","verified-native-runtime","Does not grant readiness"] {assert!(review.contains(expected),"{review}");}
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);
    let request=&host.mutations()[0]["action"];assert_eq!(request["action"],"bind_worker");assert_eq!(request["role"],"developer");assert_eq!(request["expected_epoch"],"binding-epoch");
}
#[test]
fn attestation_requires_a_separate_initially_unchecked_assertion() {
    let host=Host::new(vec![approved()]);let (mut app,mut screen)=host.open();evidence_gate(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Down,Key::Enter]);type_note(&mut app,&mut screen,"I personally checked");
    let assertion=rendered(&mut app,160,65);assert!(assertion.contains("[ ]")&&assertion.contains("I attest this exact gate passed"),"{assertion}");
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);assert!(host.mutations().is_empty());
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Enter]);
    app.handle(Key::Char('y'),&mut screen);assert!(host.mutations().is_empty());
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);
    let request=&host.mutations()[0]["action"];assert_eq!(request["action"],"attest_evidence");assert_eq!(request["evidence"]["source"],"host_attestation");
    assert_eq!(request["evidence"]["result"],json!({"passed":true,"note":"I personally checked"}));
}
#[test]
fn protected_source_picker_never_accepts_a_path_or_result_editor() {
    let work=approved();let source=json!({"kind":"source","id":"protected-source-1","evidence":{"id":"protected-source-1","work":work["brief"]["work"],"kind":"tests",
        "commit":"a".repeat(40),"source":"host_attestation","result":{"passed":true,"note":"protected result"}},"approval_hash":work["approval_hash"],"can_import":true,"imported":0,"created_at_ms":1,"unavailable_reason":null});
    let host=Host::new(vec![work,source]);let (mut app,mut screen)=host.open();evidence_gate(&mut app,&mut screen);
    app.handle(Key::Enter,&mut screen);assert!(rendered(&mut app,150,60).contains("protected-source-1"));
    app.handle(Key::Enter,&mut screen);let review=rendered(&mut app,160,80);
    for expected in ["protected result","protected-source-1",&"a".repeat(40),"revision"] {assert!(review.contains(expected),"{review}");}
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);let request=&host.mutations()[0]["action"];
    assert_eq!(request["action"],"import_evidence");assert_eq!(request["source_id"],"protected-source-1");assert!(request.get("result").is_none()&&request.get("path").is_none());
}
#[test]
fn host_run_gate_cannot_be_substituted_by_host_attestation() {
    let mut work=approved();work["brief"]["gates"][0]["source"]=json!("host_run");
    let host=Host::new(vec![work]);let (mut app,mut screen)=host.open();evidence_gate(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Down,Key::Enter,Key::Enter,Key::Char('y')]);
    assert!(rendered(&mut app,150,50).contains("not an approved host_attestation"));assert!(host.mutations().is_empty());
}
fn decision() -> Value {
    json!({"kind":"decision","id":"decision-1","scope_key":"scope-1","decision_key":"uncertain_delivery","work":approved()["brief"]["work"],"assignment":"assignment-1",
        "question":"Confirm ownership","recommendation":"Check worker (chief recommendation)","state":"open","revision":4,"resolution":null,"created_at_ms":1,"resolved_at_ms":null})
}
fn assignment() -> Value {
    json!({"kind":"assignment","id":"assignment-1","work":approved()["brief"]["work"],"worker_identity":worker()["session"],"worker":"ses_3f2a9c0b1ffeQkTz8mWn4rJd2L",
        "worker_runtime":"verified-native-runtime","worker_epoch":"worker-epoch","chief_grant":"grant-1","chief_epoch":"grant-epoch","state":"uncertain","revision":5,"created_at_ms":1,"finished_at_ms":null})
}
#[test]
fn safe_stop_requires_note_assertion_and_exact_assignment_before_confirmation() {
    let host=Host::new(vec![decision(),assignment()]);let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Enter,Key::Enter,Key::Enter,Key::Down,Key::Enter,Key::Down,Key::Enter]);
    type_note(&mut app,&mut screen,"Worker was stopped");
    let assertion=rendered(&mut app,160,70);assert!(assertion.contains("[ ]")&&assertion.contains("worker-epoch"),"{assertion}");
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);assert!(host.mutations().is_empty());
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Enter,Key::Enter,Key::Char('y')]);
    let request=&host.mutations()[0]["action"];assert_eq!(request["assignment"],"assignment-1");assert_eq!(request["expected_revision"],5);
    assert_eq!(request["evidence"],json!({"kind":"safe_stop","note":"Worker was stopped","confirmed_stopped":true}));
}

#[test]
fn exact_launch_and_message_recovery_keep_separate_unchecked_stop_consent() {
    for (kind,scope,action,field) in [("legacy_launch","host-launch-target-1","reconcile_launch","token"),("inbox","host-delivery-target-1","reconcile_delivery","message")] {
        let mut decision=decision();decision["assignment"]=Value::Null;decision["scope_key"]=json!(scope);
        let target=json!({"kind":kind,"id":"target-1","state":"uncertain","revision":3,"project":PROJECT,"task_id":"1234abcd","worker":null,"recipient":worker()["session"]});
        let host=Host::new(vec![decision,target]);let (mut app,mut screen)=host.open();
        keys(&mut app,&mut screen,&[Key::Enter,Key::Enter,Key::Enter,Key::Down,Key::Enter,Key::Down,Key::Enter]);
        type_note(&mut app,&mut screen,"Checked this exact target");
        let assertion=rendered(&mut app,160,70);
        assert!(assertion.contains("[ ]") && assertion.contains("target-1"),"{assertion}");
        if kind=="legacy_launch" {assert!(assertion.contains("worker identity may be unknown"));}
        keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);assert!(host.mutations().is_empty());
        keys(&mut app,&mut screen,&[Key::Char(' '),Key::Enter]);app.handle(Key::Char('y'),&mut screen);assert!(host.mutations().is_empty());
        keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);
        let request=&host.mutations()[0]["action"];
        assert_eq!(request["action"],action);assert_eq!(request[field],"target-1");assert_eq!(request["expected_revision"],3);
    }
}
#[test]
fn partial_batch_failure_reports_every_result_and_leaves_remaining_items_unsent() {
    let mut second=proposal("p2",1);second["brief"]["work"]["task"]["task_id"]=json!("2345abcd");
    let typed:cockpit::chief::WorkBrief=serde_json::from_value(second["brief"].clone()).unwrap();second["approval_hash"]=json!(typed.approval_hash().unwrap());
    let mut third=second.clone();third["id"]=json!("p3");third["brief"]["work"]["task"]["task_id"]=json!("3456abcd");
    let typed:cockpit::chief::WorkBrief=serde_json::from_value(third["brief"].clone()).unwrap();third["approval_hash"]=json!(typed.approval_hash().unwrap());
    let host=Host::new(vec![proposal("p1",1),second,third]);host.results.lock().unwrap().extend([json!({"state":"approved"}),json!({"error":"storage failure"})]);
    let (mut app,mut screen)=host.open();awaiting(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Down,Key::Char(' '),Key::Down,Key::Char(' '),Key::Char('a'),Key::Enter,Key::Char('y')]);
    assert_eq!(host.mutations().len(),2);
    let mut all=String::new();for _ in 0..80 {all.push_str(&rendered(&mut app,65,20));app.handle(Key::PageDown,&mut screen);}
    for expected in ["Host result","storage failure","UNSENT","3456abcd"] {assert!(all.contains(expected),"missing {expected}: {all}");}
    assert_eq!(host.mutations().len(),2);
}

#[test]
fn host_sqlite_numeric_live_flag_preserves_pause_resume_controls() {
    let mut grant=chief("active",true);grant["live_verified"]=json!(1);
    let host=Host::new(vec![grant]);let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Down,Key::Down,Key::Down,Key::Enter]);
    assert!(rendered(&mut app,120,45).contains("Pause chief"));
}
#[test]
fn suspended_confirmation_requires_separate_confirmation_again_on_return() {
    let host=Host::new(vec![proposal("p1",1)]);let (mut app,mut screen)=host.open();awaiting(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Char('a'),Key::Enter,Key::Tab]);
    screen.picks.push_back("-\tchief".into());app.handle(Key::Char(':'),&mut screen);
    app.handle(Key::Char('y'),&mut screen);assert!(host.mutations().is_empty());
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);assert_eq!(host.mutations().len(),1);
}
#[test]
fn disappeared_proposal_does_not_retarget_the_next_row_or_keep_batch_consent() {
    let host=Host::new(vec![proposal("p1",1)]);let (mut app,mut screen)=host.open();awaiting(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Char('a'),Key::Enter]);
    host.snapshot.lock().unwrap()["records"]=json!([proposal("replacement",2)]);
    keys(&mut app,&mut screen,&[Key::Char('y'),Key::Esc,Key::Enter,Key::Char('a'),Key::Enter,Key::Char('y')]);
    assert!(host.mutations().is_empty());
    let text=rendered(&mut app,120,45);assert!(text.contains("Selected: 0"),"{text}");
}
#[test]
fn unrelated_telemetry_does_not_invalidate_an_exact_frozen_brief() {
    let host=Host::new(vec![proposal("p1",1),worker()]);let (mut app,mut screen)=host.open();awaiting(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Char('a'),Key::Enter]);
    host.snapshot.lock().unwrap()["generation"]=json!("telemetry-generation-2");
    host.snapshot.lock().unwrap()["records"][1]["observed_at_ms"]=json!(123456);
    app.handle(Key::Char('y'),&mut screen);assert_eq!(host.mutations().len(),1);
}
#[test]
fn changed_gate_hash_source_or_recovery_identity_cannot_inherit_consent() {
    let host=Host::new(vec![approved()]);let (mut app,mut screen)=host.open();evidence_gate(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Down,Key::Enter]);type_note(&mut app,&mut screen,"checked");
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Enter,Key::Enter]);
    host.snapshot.lock().unwrap()["records"][0]["brief"]["gates"][0]["source"]=json!("host_run");
    app.handle(Key::Char('y'),&mut screen);assert!(host.mutations().is_empty());
}
#[test]
fn escaped_long_details_remain_inspectable_on_a_narrow_short_screen() {
    let mut record=proposal("p1",1);record["brief"]["brief"]=json!(format!("{}\u{1b}[2J \u{202e} END-OF-EXACT-BRIEF","Long bounded scope ".repeat(40)));
    // An invalid proposal still has to be inspectable honestly; no approval is attempted.
    let host=Host::new(vec![record]);let (mut app,mut screen)=host.open();awaiting(&mut app,&mut screen);app.handle(Key::Enter,&mut screen);
    let mut all=String::new();for _ in 0..250 {all.push_str(&rendered(&mut app,32,8));app.handle(Key::PageDown,&mut screen);}
    for expected in ["END-OF-EXACT-BRIEF","\\u001b","approval_hash"] {assert!(all.contains(expected),"missing {expected}: {all}");}
    assert!(!all.contains('\u{1b}')&&!all.contains('\u{202e}'));assert!(host.mutations().is_empty());
}

#[test]
fn enable_pause_and_resume_submit_only_after_exact_runtime_review_and_confirmation() {
    for (grant,expected) in [(None,"enable"),(Some(chief("active",true)),"pause"),(Some(chief("paused",true)),"resume")] {
        let mut records=vec![worker()];if let Some(grant)=grant {records.push(grant);}
        let host=Host::new(records);let (mut app,mut screen)=host.open();
        keys(&mut app,&mut screen,&[Key::Down,Key::Down,Key::Down,Key::Enter,Key::Enter]);
        if expected=="enable" {app.handle(Key::Enter,&mut screen);}
        let review=rendered(&mut app,180,80);assert!(review.contains("verified-native-runtime"),"{review}");
        app.handle(Key::Char('y'),&mut screen);assert!(host.mutations().is_empty());
        keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);
        let request=&host.mutations()[0]["action"];assert_eq!(request["action"],expected);
        if expected=="enable" {assert_eq!(request["session"],worker()["session"]);} else {assert_eq!(request["expected_epoch"],"grant-epoch-1");}
    }
}
#[test]
fn revocation_reviews_stop_notice_and_retained_ownership_not_undo() {
    let host=Host::new(vec![approved()]);let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Down,Key::Enter,Key::Down,Key::Enter,Key::Enter,Key::Enter,Key::Enter]);
    let review=rendered(&mut app,180,70);for expected in ["stop-at-safe-boundary","Completed effects are not undone","Uncertain ownership is not released"] {assert!(review.contains(expected),"{review}");}
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);assert_eq!(host.mutations()[0]["action"]["action"],"revoke_work");
    assert_eq!(host.mutations()[0]["action"]["work"],approved()["brief"]["work"]);
}
#[test]
fn resolve_and_handled_notes_never_act_as_completion_or_permission() {
    for handled in [false,true] {
        let host=Host::new(vec![decision(),assignment()]);let (mut app,mut screen)=host.open();
        keys(&mut app,&mut screen,&[Key::Enter,Key::Enter,Key::Enter]);
        if handled {keys(&mut app,&mut screen,&[Key::Down,Key::Enter]);}
        app.handle(Key::Enter,&mut screen);type_note(&mut app,&mut screen,"Observed by Theo");
        assert!(host.mutations().is_empty());let review=rendered(&mut app,180,80);
        assert!(review.contains(if handled {"retain ownership"} else {"waives no evidence"}),"{review}");
        keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);let request=&host.mutations()[0]["action"];
        if handled {assert_eq!(request["evidence"]["kind"],"handled");assert_eq!(request["expected_revision"],5);}
        else {assert_eq!(request["action"],"resolve");assert_eq!(request["expected_revision"],4);assert_eq!(request["resolution"],"Observed by Theo");}
    }
}
#[test]
fn changed_role_epoch_and_runtime_require_new_review() {
    for change_runtime in [false,true] {
        let host=Host::new(vec![worker()]);let (mut app,mut screen)=host.open();
        keys(&mut app,&mut screen,&[Key::Down,Key::Down,Key::Enter,Key::Enter,Key::Enter,Key::Enter,Key::Enter,Key::Enter]);
        if change_runtime {host.snapshot.lock().unwrap()["records"][0]["session"]["runtime"]=json!("replacement-runtime");}
        else {host.snapshot.lock().unwrap()["records"].as_array_mut().unwrap().push(json!({"kind":"binding","session":worker()["session"],"epoch":"new-epoch","state":"bound","revision":1,"role":"qa"}));}
        app.handle(Key::Char('y'),&mut screen);assert!(host.mutations().is_empty());
        assert!(rendered(&mut app,150,60).contains("changed"));
    }
}
#[test]
fn changed_source_result_cannot_inherit_import_consent() {
    let work=approved();let source=json!({"kind":"source","id":"protected-1","evidence":{"work":work["brief"]["work"],"kind":"tests","commit":"a".repeat(40),"result":{"passed":true}},"can_import":true});
    let host=Host::new(vec![work,source]);let (mut app,mut screen)=host.open();evidence_gate(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Enter,Key::Enter,Key::Enter]);
    host.snapshot.lock().unwrap()["records"][1]["evidence"]["result"]["passed"]=json!(false);
    app.handle(Key::Char('y'),&mut screen);assert!(host.mutations().is_empty());
}
#[test]
fn host_pagination_invalidates_and_restarts_without_mixing_generations() {
    let host=Host::new(vec![proposal("new",2)]);
    let mut first=snapshot(vec![proposal("old",1)]);first["has_more"]=json!(true);first["next"]=json!(1);first["generation"]=json!("old-generation");
    host.pages.lock().unwrap().extend([first,json!({"records":[],"invalidated":true,"snapshot_required":true,"next":0,"generation":"new-generation"})]);
    let (mut app,mut screen)=host.open();awaiting(&mut app,&mut screen);keys(&mut app,&mut screen,&[Key::Char(' '),Key::Char('a'),Key::Enter,Key::Char('y')]);
    assert_eq!(host.mutations()[0]["action"]["brief"]["work"]["revision"],2);
    let requests=host.requests.lock().unwrap();assert_eq!(requests[1]["after"],1);assert_eq!(requests[1]["generation"],"old-generation");assert_eq!(requests[2]["after"],0);assert_eq!(requests[2]["generation"],Value::Null);
}
#[test]
fn host_storage_failure_rejects_confirmation_and_preserves_visible_failure() {
    let host=Host::new(vec![proposal("p1",1)]);let (mut app,mut screen)=host.open();awaiting(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Char('a'),Key::Enter]);
    host.pages.lock().unwrap().push_back(json!({"error":"database unavailable\u{1b}[2J"}));app.handle(Key::Char('y'),&mut screen);
    assert!(host.mutations().is_empty());let text=rendered(&mut app,160,70);assert!(text.contains("database unavailable")&&!text.contains('\u{1b}'));
}
#[test]
fn team_bound_worker_cannot_be_rebound_or_selected_chief() {
    let host=Host::new(vec![worker(),json!({"kind":"team_member","session":worker()["session"],"role":"qa","epoch":"team-epoch","team_revision":3,"task":{"project":PROJECT,"task_id":"1234abcd"}})]);
    let (mut app,mut screen)=host.open();keys(&mut app,&mut screen,&[Key::Down,Key::Down,Key::Enter,Key::Enter,Key::Enter,Key::Enter]);
    assert!(rendered(&mut app,150,60).contains("team mapping"));assert!(host.mutations().is_empty());
    keys(&mut app,&mut screen,&[Key::Esc,Key::Esc,Key::Esc,Key::Down,Key::Enter,Key::Enter,Key::Enter]);assert!(host.mutations().is_empty());
}
#[test]
fn missing_gates_and_empty_lists_do_not_infer_applicability_or_readiness() {
    let mut work=approved();work["brief"].as_object_mut().unwrap().remove("gates");
    let host=Host::new(vec![work]);let (mut app,mut screen)=host.open();evidence_gate(&mut app,&mut screen);
    let text=rendered(&mut app,150,50);assert!(text.contains("missing gates never imply not applicable")&&text.contains("Nothing available"),"{text}");assert!(host.mutations().is_empty());
}

#[test]
fn chief_decisions_preserve_selection_scroll_and_human_prompts_on_refresh() {
    let mut first=decision();first["question"]=json!("FIRST DECISION");
    let mut second=decision();second["id"]=json!("decision-2");second["question"]=json!("Start of second decision");second["recommendation"]=json!("source report ".repeat(50));
    let host=Host::new(vec![first,second]);
    let session=worker()["session"]["session_id"].as_str().unwrap().to_string();
    cockpit::prompts::add(&session,"Human prompt stays queued").unwrap();let prompt=cockpit::prompts::oldest(&session).unwrap();
    let (mut app,mut screen)=host.open();keys(&mut app,&mut screen,&[Key::Enter,Key::Down,Key::Enter]);
    let _=rendered(&mut app,65,20);app.handle(Key::PageDown,&mut screen);
    let before=rendered(&mut app,65,20);assert!(!before.contains("FIRST DECISION"));
    app.handle(Key::Tab,&mut screen);
    host.snapshot.lock().unwrap()["records"][1]["question"]=json!("Updated host evidence on second decision");
    host.snapshot.lock().unwrap()["records"][1]["revision"]=json!(5);
    screen.picks.push_back("-\tchief".into());app.handle(Key::Char(':'),&mut screen);
    let after=rendered(&mut app,65,20);assert!(!after.contains("FIRST DECISION"));
    // The refresh must not reset this detail to its first page.
    assert!(!after.contains("Chief · Off"),"{after}");
    let mut all=after;for _ in 0..80 {app.handle(Key::PageUp,&mut screen);all.push_str(&rendered(&mut app,120,40));}
    assert!(all.contains("Updated host evidence on second decision"),"{all}");
    assert_eq!(cockpit::prompts::oldest(&session),Some(prompt));assert!(host.mutations().is_empty());
}
#[test]
fn only_unresolved_decisions_count_as_chief_attention() {
    let mut resolved=decision();resolved["state"]=json!("resolved");
    let state=snapshot(vec![decision(),resolved,json!({"kind":"event","event_kind":"progress","body":"success"}),approved()]);
    assert_eq!(cockpit::chief::view::ChiefView::attention(&state),1);
}

#[test]
fn narrow_review_keeps_enter_and_escape_instructions_accessible() {
    let host=Host::new(vec![proposal("p1",1)]);let (mut app,mut screen)=host.open();awaiting(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Char('a')]);
    let text=rendered(&mut app,40,12);assert!(text.contains("Enter confirm")&&text.contains("Esc cancel"),"{text}");
}

#[test]
fn actual_chief_view_reviews_current_records_without_loading_terminal_history() {
    use cockpit::chief::{store,ChiefAgentAction,ChiefHostAction,WorkBrief,WorkRef,WorkerState};
    use cockpit::teams::{Role,TaskKey,TransportOutcome,VerifiedSession};
    let mut connection=rusqlite::Connection::open_in_memory().unwrap();store::migrate(&mut connection).unwrap();
    let worker:VerifiedSession=serde_json::from_value(worker()["session"].clone()).unwrap();
    let mut chief=worker.clone();chief.session_id="ses_chief000000000000".into();chief.runtime="chief-runtime".into();
    let live=vec![WorkerState {session:chief.clone(),role:None,ready:false,blocked_by:vec!["host_role_unbound".into()],observed_at_ms:100},WorkerState {session:worker.clone(),role:Some(Role::Developer),ready:true,blocked_by:vec![],observed_at_ms:100}];
    store::host(&mut connection,&ChiefHostAction::Enable {request_id:"enable".into(),session:chief.clone()},&live,100).unwrap().result.unwrap();
    store::host(&mut connection,&ChiefHostAction::BindWorker {request_id:"bind".into(),session:worker.clone(),role:Role::Developer,expected_epoch:None},&live,100).unwrap().result.unwrap();
    let brief=WorkBrief {work:WorkRef {task:TaskKey {project:PROJECT.into(),task_id:"aaaaaaaa".into()},revision:1},brief:"Exact unresolved work".into(),role:Role::Developer,prerequisites:vec![],gates:vec![]};
    store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"approve".into(),approval_hash:brief.approval_hash().unwrap(),brief:brief.clone(),expected_revision:0},&live,100).unwrap().result.unwrap();
    store::verify_admission(&mut connection,&chief,"opencode-queued-v1","Controlled fixture, not live proof",100).unwrap();
    store::observe(&mut connection,&live,100).unwrap();let wake=store::claim(&mut connection,&chief,"bootstrap",100).unwrap().unwrap();
    store::finish_for(&mut connection,&chief,&wake.id,&TransportOutcome::Received {native_id:"bootstrap-native".into()},100).unwrap();
    store::agent(&mut connection,&chief,&ChiefAgentAction::Dispatch {submission_key:"assign".into(),work:brief.work,worker:worker.session_id.clone()},&live,100).unwrap().result.unwrap();
    let delivery=store::claim(&mut connection,&worker,"worker-boundary",100).unwrap().unwrap();
    store::finish_for(&mut connection,&worker,&delivery.id,&TransportOutcome::Uncertain {reason:"Lost native reply".into()},100).unwrap();
    store::observe(&mut connection,&live,100).unwrap();
    let prerequisite=WorkBrief {work:WorkRef {task:TaskKey {project:PROJECT.into(),task_id:"00000000".into()},revision:1},brief:"Old completed prerequisite".into(),role:Role::Developer,prerequisites:vec![],
        gates:vec![cockpit::chief::GateRequirement {kind:cockpit::chief::GateKind::Tests,source:"host_attestation".into(),expected:json!({"commit":"a".repeat(40)})}]};
    store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"old-prerequisite".into(),approval_hash:prerequisite.approval_hash().unwrap(),brief:prerequisite.clone(),expected_revision:0},&live,101).unwrap().result.unwrap();
    let evidence=cockpit::chief::Evidence {id:"old-prerequisite-proof".into(),work:prerequisite.work.clone(),kind:cockpit::chief::GateKind::Tests,commit:"a".repeat(40),source:"host_attestation".into(),result:json!({"passed":true,"note":"Exact prerequisite result"})};
    store::publish_source(&mut connection,&evidence,102).unwrap();
    store::host(&mut connection,&ChiefHostAction::ImportEvidence {request_id:"import-old-proof".into(),work:prerequisite.work.clone(),gate:evidence.kind,source_id:evidence.id.clone()},&live,102).unwrap().result.unwrap();
    for index in 0..4 {
        let mut candidate=evidence.clone();candidate.id=format!("later-unimported-{index}");
        store::publish_source(&mut connection,&candidate,103+index).unwrap();
    }
    // Terminal history is deliberately outside both recent windows, with no live assignment/decision anchor.
    for index in 1..=18 {
        let mut terminal=prerequisite.clone();terminal.work.task.task_id=format!("{index:08x}");terminal.gates.clear();
        connection.execute("INSERT INTO chief_work_revisions(project,task_id,revision,brief,approval_hash,state,priority,created_at) VALUES (?1,?2,1,?3,?4,'completed',0,?5)",rusqlite::params![PROJECT,terminal.work.task.task_id,serde_json::to_string(&terminal).unwrap(),terminal.approval_hash().unwrap(),200+index]).unwrap();
    }
    let proposal=ChiefAgentAction::ProposeWork {submission_key:"current-proposal".into(),proposal:cockpit::chief::WorkProposal {task:TaskKey {project:PROJECT.into(),task_id:"bbbbbbbb".into()},brief:"Current exact proposal".into(),role:Role::Developer,prerequisites:vec![prerequisite.work.clone()],gates:vec![]}};
    store::agent(&mut connection,&worker,&proposal,&live,100).unwrap().result.unwrap();
    connection.execute_batch("WITH RECURSIVE history(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM history WHERE n<26000)
        INSERT INTO chief_events(id,source,submission_key,fingerprint,kind,body,created_at) SELECT 'terminal-'||n,'host','terminal-'||n,'fixture','status','{}',n FROM history;").unwrap();
    let page=store::snapshot_page(&connection,&live,0,50,None,100).unwrap();assert_eq!(page["has_more"],false);
    let records=page["records"].as_array().unwrap();
    assert!(records.iter().any(|record|record["kind"]=="work" && record["brief"]["work"]==json!(prerequisite.work)),"pending approval must retain its exact old prerequisite");
    assert!(records.iter().any(|record|record["kind"]=="source" && record["id"]==evidence.id),"latest gate evidence must retain its exact source even beyond the recent source window");
    assert!(!records.iter().any(|record|record["kind"]=="work" && record["brief"]["work"]["task"]["task_id"]=="00000001"));
    let host=Host::new(page["records"].as_array().unwrap().clone());*host.snapshot.lock().unwrap()=page;
    let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Enter,Key::Enter]);assert!(rendered(&mut app,160,70).contains("Assignment requires host reconciliation"));
    keys(&mut app,&mut screen,&[Key::Esc,Key::Esc,Key::Down,Key::Enter,Key::Down,Key::Enter,Key::Enter]);
    assert!(rendered(&mut app,180,70).contains("Old completed prerequisite"));
    keys(&mut app,&mut screen,&[Key::Enter,Key::Down,Key::Enter,Key::Enter]);
    let gate=rendered(&mut app,180,90);for expected in ["old-prerequisite-proof","Exact prerequisite result","host_attestation"] {assert!(gate.contains(expected),"{gate}");}
    keys(&mut app,&mut screen,&[Key::Esc,Key::Esc,Key::Esc,Key::Esc,Key::Esc,Key::Up,Key::Enter]);
    keys(&mut app,&mut screen,&[Key::Char(' '),Key::Char('a')]);assert!(rendered(&mut app,160,70).contains("Current exact proposal"));
    app.handle(Key::Enter,&mut screen);
    let mut changed=evidence.clone();changed.id="prerequisite-now-failed".into();changed.result=json!({"passed":false,"note":"Gate changed during approval review"});
    store::record_evidence(&mut connection,&changed,400).unwrap();
    *host.snapshot.lock().unwrap()=store::snapshot_page(&connection,&live,0,50,None,400).unwrap();
    app.handle(Key::Char('y'),&mut screen);
    assert!(host.mutations().is_empty(),"changed prerequisite must invalidate pending approval consent");
    assert!(rendered(&mut app,160,70).contains("changed"));
    changed.id="prerequisite-restored".into();changed.result=evidence.result.clone();
    store::record_evidence(&mut connection,&changed,401).unwrap();
    *host.snapshot.lock().unwrap()=store::snapshot_page(&connection,&live,0,50,None,401).unwrap();
    keys(&mut app,&mut screen,&[Key::Esc,Key::Tab]);
    screen.picks.push_back("-\tchief".into());app.handle(Key::Char(':'),&mut screen);
    app.handle(Key::Char('a'),&mut screen);
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);
    let action:ChiefHostAction=serde_json::from_value(host.mutations()[0]["action"].clone()).unwrap();
    store::host(&mut connection,&action,&live,100).unwrap().result.unwrap();
    assert!(host.requests.lock().unwrap().len()<=6);
    let audit=store::audit_page(&connection,&live,0,50,None,100).unwrap();assert_eq!(audit["has_more"],true);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_events WHERE id LIKE 'terminal-%'",[],|row|row.get::<_,i64>(0)).unwrap(),26000);
}
#[test]
fn rebinding_a_restarted_runtime_reviews_the_retained_binding_epoch() {
    let mut old_session=worker()["session"].clone();old_session["runtime"]=json!("old-runtime");
    let host=Host::new(vec![worker(),json!({"kind":"binding","session":old_session,"epoch":"retained-binding-epoch","revision":3,"state":"bound","role":"qa","approval_hash":"old-hash"})]);
    let (mut app,mut screen)=host.open();keys(&mut app,&mut screen,&[Key::Down,Key::Down,Key::Enter,Key::Enter,Key::Enter,Key::Enter,Key::Enter,Key::Enter,Key::Char('y')]);
    assert_eq!(host.mutations()[0]["action"]["expected_epoch"],"retained-binding-epoch");
}

#[test]
fn gate_detail_identifies_accepted_qa_reviewer_handoff_and_provenance() {
    let mut work=approved();work["brief"]["gates"][0]["kind"]=json!("qa");work["brief"]["gates"][0]["source"]=json!("qa");
    let evidence=json!({"kind":"evidence","id":"qa-proof","work":work["brief"]["work"],"gate":"qa","commit":"a".repeat(40),"source":"qa",
        "source_id":"protected-qa","result":{"verdict":"pass","reviewer_epoch":"exact-reviewer-epoch","handoff_id":"exact-reviewed-handoff"},"created_at_ms":10});
    let host=Host::new(vec![work,evidence]);let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Down,Key::Enter,Key::Down,Key::Enter,Key::Enter,Key::Enter,Key::Down,Key::Enter,Key::Enter]);
    let text=rendered(&mut app,180,90);for expected in ["exact-reviewer-epoch","exact-reviewed-handoff","not independently proven command execution"] {assert!(text.contains(expected),"{text}");}
}

#[test]
fn short_narrow_lists_keep_the_selected_root_action_visible() {
    let host=Host::new(vec![]);let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Down,Key::Down,Key::Down,Key::Down]);
    assert!(rendered(&mut app,32,8).contains("Request summary"));
}
