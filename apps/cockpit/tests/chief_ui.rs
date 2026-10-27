//! Approved Chief controls through real App, rendering and host IPC.
use cockpit::app::{App, Key, Screen};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::{Value, json};
use std::{collections::VecDeque, fs, io::{Read, Write}, os::unix::net::UnixListener,
    path::PathBuf, sync::{Arc, Mutex}, time::Duration};

static ENVIRONMENT: Mutex<()> = Mutex::new(());
const PROJECT: &str = "path:/home/theo/IdeaProjects/a-long-canonical-project";
fn snapshot(records: Vec<Value>) -> Value {
    json!({"records":records,"generation":"generation-1","next":0,"has_more":false,"projection":"host_review","cursor_kind":"snapshot_offset","coordination_error":null})
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
    pages: Arc<Mutex<VecDeque<Value>>>,
    /// The records the host holds after each next mutation, in order: the fixture's view of what the host changed.
    after_mutations: Arc<Mutex<VecDeque<Vec<Value>>>>,
    /// Replies to the next mutations, in order; `{"error":…}` fails one. Otherwise the host records it.
    mutation_replies: Arc<Mutex<VecDeque<Value>>>,
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
        let pages = Arc::new(Mutex::new(VecDeque::<Value>::new()));
        let (state, calls) = (snapshot.clone(),requests.clone());
        let read_pages=pages.clone();
        let after_mutations = Arc::new(Mutex::new(VecDeque::<Vec<Value>>::new()));
        let changes=after_mutations.clone();
        let mutation_replies = Arc::new(Mutex::new(VecDeque::<Value>::new()));
        let replies=mutation_replies.clone();
        std::thread::spawn(move || for mut stream in listener.incoming().flatten() {
            let mut wire = String::new(); stream.read_to_string(&mut wire).unwrap();
            let request: Value = serde_json::from_str(&wire).unwrap(); calls.lock().unwrap().push(request.clone());
            let result = if request["request"] == "chief_snapshot" { read_pages.lock().unwrap().pop_front().unwrap_or_else(||state.lock().unwrap().clone()) }
                else {
                    if let Some(records)=changes.lock().unwrap().pop_front() {state.lock().unwrap()["records"]=json!(records);}
                    replies.lock().unwrap().pop_front().unwrap_or_else(||json!({"recorded":true}))
                };
            let code = if result["code"]=="conflict" {cockpit::teams::ErrorCode::Conflict} else {cockpit::teams::ErrorCode::Storage};
            let result = if let Some(message)=result["error"].as_str() {Err(cockpit::teams::TeamError::new(code,message))} else {Ok(result)};
            let reply = serde_json::to_value(cockpit::teams::TeamReply {request_id:"fixture".into(),result}).unwrap();
            stream.write_all(reply.to_string().as_bytes()).unwrap();
        });
        Self {root,snapshot,requests,pages,after_mutations,mutation_replies,_lock:lock}
    }
    fn open(&self) -> (App, TestScreen) {
        let mut app = App::new(); let mut screen = TestScreen {picks:VecDeque::new()};
        open_chief(&mut app,&mut screen); (app,screen)
    }
    /// The tab's first needs-you row is the oldest escalation: ⏎ answers it in the inbox card.
    /// The tab's c with no chief yet: the startup screen, sending a start only where it offers one.
    fn open_startup(&self) -> (App, TestScreen) {
        let (mut app, mut screen) = self.open(); app.handle(Key::Char('c'), &mut screen); (app, screen)
    }
    fn open_inbox(&self) -> (App, TestScreen) {
        let (mut app, mut screen) = self.open(); app.handle(Key::Enter, &mut screen); (app, screen)
    }
    fn mutations(&self) -> Vec<Value> { self.requests.lock().unwrap().iter().filter(|request|request["request"]=="chief").cloned().collect() }
}
impl Drop for Host { fn drop(&mut self) { fs::remove_dir_all(&self.root).unwrap(); } }
/// The Chief lives in its own tab (5), from wherever the column is.
fn open_chief(app: &mut App, screen: &mut TestScreen) { keys(app,screen,&[Key::Esc,Key::Char('5')]); }
fn keys(app: &mut App, screen: &mut TestScreen, keys: &[Key]) {for key in keys {app.handle(key.clone(),screen);}}
fn rendered(app: &mut App, width: u16, height: u16) -> String {
    let mut buffer = Buffer::empty(Rect::new(0,0,width,height)); app.render(&mut buffer);
    buffer.content.iter().map(|cell|cell.symbol()).collect::<String>()
}
// The tab's c: with an active chief that is the Pause review.
fn first_control_review(app: &mut App, screen: &mut TestScreen) { keys(app,screen,&[Key::Char('c')]); }

#[test]
fn chief_start_is_explicit_and_repeated_keys_cannot_queue_another_launch() {
    let host = Host::new(vec![]);
    host.snapshot.lock().unwrap()["startup"] = Value::Null;
    let (mut app, mut screen) = host.open();
    assert!(rendered(&mut app, 60, 22).contains("c start chief"));
    keys(&mut app, &mut screen, &[Key::Enter,Key::Char('p')]);
    assert!(host.mutations().is_empty());
    keys(&mut app, &mut screen, &[Key::Char('c'),Key::Char('c'),Key::Enter]);
    let mutations = host.mutations();
    assert_eq!(mutations.len(), 1);
    assert_eq!(mutations[0]["action"]["action"], "start");
    assert_eq!(mutations[0]["action"]["previous_attempt"], Value::Null);
    assert!(rendered(&mut app, 60, 22).contains("Starting Chief"));
}

#[test]
fn reopening_chief_tracks_the_host_attempt_without_launching() {
    let host = Host::new(vec![]);
    host.snapshot.lock().unwrap()["startup"] = json!({"id":"attempt-1","state":"checking","can_retry":false,"error":"Waiting for verified admission"});
    let (mut app, mut screen) = host.open();
    keys(&mut app, &mut screen, &[Key::Char('c'),Key::Enter,Key::Esc]);
    open_chief(&mut app, &mut screen);
    assert!(rendered(&mut app, 80, 24).contains("◇ Chief · Checking"));
    app.handle(Key::Char('c'), &mut screen);
    let shown = rendered(&mut app, 80, 24);
    assert!(shown.contains("Checking Chief") && shown.contains("Waiting for verified admission"), "{shown}");
    assert!(host.mutations().is_empty());
}

const ADMISSION_WAIT: &str = "host live OpenCode draft/queue verification is required before chief activation";
fn checking(check: Value) -> Value {
    json!({"id":"attempt-1","state":"checking","can_retry":false,"error":ADMISSION_WAIT,"check_session":worker()["session"],"check":check})
}
fn received_check() -> Value { json!({"id":"check-1","state":"received","session":worker()["session"]}) }
fn verifications(host: &Host) -> Vec<Value> {
    host.requests.lock().unwrap().iter().filter(|request|request["request"]=="verify_chief_admission").cloned().collect()
}

#[test]
fn checking_offers_no_test_message_before_the_chief_session_is_live() {
    let host = Host::new(vec![]);
    let mut startup = checking(Value::Null); startup["check_session"] = Value::Null;
    host.snapshot.lock().unwrap()["startup"] = startup;
    let (mut app, mut screen) = host.open_startup();
    assert!(!rendered(&mut app, 100, 24).contains("Send test message"));
    keys(&mut app, &mut screen, &[Key::Char('t'),Key::Char('y'),Key::Char('n')]);
    assert!(host.mutations().is_empty() && verifications(&host).is_empty());
}

#[test]
fn t_sends_one_test_message_for_the_attempt_and_a_repeat_reuses_its_request() {
    let host = Host::new(vec![]);
    host.snapshot.lock().unwrap()["startup"] = checking(Value::Null);
    let (mut app, mut screen) = host.open_startup();
    assert!(rendered(&mut app, 100, 24).contains("[t] Send test message"));
    keys(&mut app, &mut screen, &[Key::Char('t'),Key::Char('t')]);
    let mutations = host.mutations();
    assert_eq!(mutations.len(), 2);
    assert_eq!(mutations[0]["action"]["action"], "send_check");
    assert_eq!(mutations[0]["action"]["attempt"], "attempt-1");
    assert_eq!(mutations[0]["action"]["request_id"], mutations[1]["action"]["request_id"], "the host replays a repeated request");
    host.snapshot.lock().unwrap()["startup"] = checking(json!({"id":"check-1","state":"queued","session":worker()["session"]}));
    app.handle(Key::Char('t'), &mut screen);
    assert_eq!(host.mutations().len(), 2, "no second test while one is on its way");
    assert!(verifications(&host).is_empty());
}

#[test]
fn y_without_a_received_test_message_does_nothing() {
    for check in [Value::Null, json!({"id":"check-1","state":"queued","session":worker()["session"]})] {
        let host = Host::new(vec![]);
        host.snapshot.lock().unwrap()["startup"] = checking(check);
        let (mut app, mut screen) = host.open_startup();
        app.handle(Key::Char('y'), &mut screen);
        assert!(verifications(&host).is_empty());
    }
}

#[test]
fn a_received_test_message_asks_theo_and_y_verifies_that_exact_session() {
    let host = Host::new(vec![]);
    host.snapshot.lock().unwrap()["startup"] = checking(received_check());
    let (mut app, mut screen) = host.open_startup();
    let shown = rendered(&mut app, 100, 24);
    for expected in ["Test message sent. Look at the Chief pane: did it appear?","[y] It arrived","[n] Didn't arrive"] {assert!(shown.contains(expected), "{expected}: {shown}");}
    app.handle(Key::Enter, &mut screen);
    assert!(verifications(&host).is_empty(), "Enter confirms nothing");
    keys(&mut app, &mut screen, &[Key::Char('y'),Key::Char('y')]);
    let recorded = verifications(&host);
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["session"], worker()["session"]);
    assert_eq!(recorded[0]["adapter"], "opencode-queued-v1");
    assert!(recorded[0]["evidence"].as_str().unwrap().contains("check-1"));
}

#[test]
fn n_does_not_verify_and_offers_another_test_message() {
    let host = Host::new(vec![]);
    host.snapshot.lock().unwrap()["startup"] = checking(received_check());
    let (mut app, mut screen) = host.open_startup();
    app.handle(Key::Char('n'), &mut screen);
    assert!(verifications(&host).is_empty());
    let shown = rendered(&mut app, 100, 24);
    assert!(shown.contains("Not verified. You can send another test message.") && shown.contains("[t] Send test message"), "{shown}");
    app.handle(Key::Char('y'), &mut screen);
    assert!(verifications(&host).is_empty(), "y after n confirms nothing");
    app.handle(Key::Char('t'), &mut screen);
    assert_eq!(host.mutations()[0]["action"]["action"], "send_check");
}

#[test]
fn checking_names_the_admission_wait_in_plain_words() {
    let host = Host::new(vec![]);
    host.snapshot.lock().unwrap()["startup"] = json!({"id":"attempt-1","state":"checking","can_retry":false,
        "error":"host live OpenCode draft/queue verification is required before chief activation"});
    let (mut app, _) = host.open_startup();
    let shown = rendered(&mut app, 100, 24);
    assert!(shown.contains("Waiting to confirm Chief can receive messages from your agents."), "{shown}");
    assert!(!shown.contains("draft/queue"), "{shown}");
}

#[test]
fn the_startup_screen_hides_the_status_view_behind_it() {
    let host = Host::new(vec![]);
    host.snapshot.lock().unwrap()["startup"] = json!({"id":"attempt-1","state":"checking","can_retry":false,"error":"Waiting for verified admission"});
    let (mut app, mut screen) = host.open();
    let (width, height) = (80, 24);
    let startup_copy = ["‹ Chief","Checking Chief…","Its session may have started. Waiting to confirm that Chief is ready.",
        "Waiting for verified admission","You can leave this view while we check.","PgUp/Dn scroll · esc back"];
    let owned_rows = 1..usize::from(height-1);
    keys(&mut app, &mut screen, &[Key::Esc, Key::Char('s')]);
    app.build();
    let status = lines_of(&mut app, width, height);
    assert!(status[owned_rows.clone()].iter().filter(|line|!line.trim().is_empty() && !startup_copy.contains(&line.trim())).count() > 3,
        "the fixture must put Status text where the startup screen draws: {status:#?}");
    open_chief(&mut app, &mut screen);
    app.handle(Key::Char('c'), &mut screen);
    let startup = lines_of(&mut app, width, height);
    for row in owned_rows {
        let line = startup[row].trim();
        assert!(line.is_empty() || startup_copy.contains(&line), "Status text survives on row {row}: {line:?}\n{startup:#?}");
    }
    assert_one_background(&mut app, width, height, 1..height-1, &[]);
}

/// Apart from the designed highlights in `highlights`, the cleared cells and the text cells of `rows`
/// share one background: no boxes behind the text.
fn assert_one_background(app: &mut App, width: u16, height: u16, rows: std::ops::Range<u16>, highlights: &[ratatui::style::Color]) {
    let mut buffer = Buffer::empty(Rect::new(0,0,width,height)); app.render(&mut buffer);
    let mut text = std::collections::BTreeSet::new(); let mut blank = std::collections::BTreeSet::new();
    for row in rows { for column in 0..width {
        let cell = &buffer[(column,row)];
        if highlights.contains(&cell.bg) {continue;}
        if cell.symbol().trim().is_empty() {blank.insert(format!("{:?}",cell.bg));} else {text.insert(format!("{:?}",cell.bg));}
    }}
    assert_eq!(text.len(), 1, "text backgrounds: {text:?}");
    assert_eq!(text, blank, "text and cleared cells differ in background");
}

#[test]
fn the_chief_view_box_has_one_background_behind_its_text() {
    let host = Host::new(vec![chief("active",true)]);
    let (mut app, _) = host.open();
    assert_one_background(&mut app, 100, 30, 2..28, &[ratatui::style::Color::Indexed(cockpit::app::BG_HIGHLIGHT)]);
}

#[test]
fn chief_retry_names_the_failed_attempt_and_stays_disabled_until_new_evidence() {
    let host = Host::new(vec![]);
    host.snapshot.lock().unwrap()["startup"] = json!({"id":"attempt-1","state":"failed","can_retry":true,"error":"Launcher missing"});
    let (mut app, mut screen) = host.open();
    assert!(rendered(&mut app, 60, 22).contains("c try again"));
    keys(&mut app, &mut screen, &[Key::Char('c'),Key::Char('c')]);
    assert_eq!(host.mutations().len(), 1);
    assert_eq!(host.mutations()[0]["action"]["previous_attempt"], "attempt-1");
    assert!(!rendered(&mut app, 60, 22).contains("Try again"));
}

#[test]
fn a_failed_prestart_snapshot_read_blocks_chief_start() {
    let host = Host::new(vec![]);
    host.snapshot.lock().unwrap()["startup"] = Value::Null;
    let (mut app, mut screen) = host.open();
    host.pages.lock().unwrap().push_back(json!({"error":"host disconnected"}));
    app.handle(Key::Char('c'), &mut screen);
    assert!(host.mutations().is_empty());
    let shown = rendered(&mut app, 80, 24);
    assert!(shown.contains("unavailable") && !shown.contains("[c] Start Chief"), "{shown}");
}

#[test]
fn chief_startup_keeps_existing_decisions_reachable_without_a_session_picker() {
    let host = Host::new(vec![decision()]);
    host.snapshot.lock().unwrap()["startup"] = Value::Null;
    let (mut app, mut screen) = host.open();
    let shown = rendered(&mut app, 100, 30);
    assert!(shown.contains("NEEDS YOU · 1") && shown.contains("Confirm ownership"), "{shown}");
    assert!(!shown.contains("pick") && !shown.contains("Chief controls"), "{shown}");
    keys(&mut app, &mut screen, &[Key::Enter,Key::Enter]);
    assert!(host.mutations().is_empty());
}

#[test]
fn an_ended_attempt_offers_start_chief_again() {
    let host = Host::new(vec![]);
    host.snapshot.lock().unwrap()["startup"] = json!({"id":"attempt-1","state":"ended","can_retry":false,"error":null});
    let (mut app, mut screen) = host.open();
    assert!(rendered(&mut app, 60, 22).contains("c start chief"));
    app.handle(Key::Char('c'), &mut screen);
    assert_eq!(host.mutations()[0]["action"]["previous_attempt"], Value::Null);
}

#[test]
fn an_unconfirmed_start_is_resent_with_the_same_request_and_never_claims_a_session() {
    let host = Host::new(vec![]);
    host.snapshot.lock().unwrap()["startup"] = Value::Null;
    host.mutation_replies.lock().unwrap().push_back(json!({"error":"acknowledgement lost"}));
    let (mut app, mut screen) = host.open();
    app.handle(Key::Char('c'), &mut screen);
    let shown = rendered(&mut app, 80, 24);
    assert!(shown.contains("Cockpit hasn't confirmed this start yet.") && shown.contains("It can't start a second Chief.") && !shown.contains("may have started"), "{shown}");
    app.handle(Key::Char('c'), &mut screen);
    let mutations = host.mutations();
    assert_eq!(mutations.len(), 2);
    assert_eq!(mutations[0]["action"]["request_id"], mutations[1]["action"]["request_id"]);
}

#[test]
fn closing_and_reopening_keeps_an_unacknowledged_chief_start_as_the_same_request() {
    let host = Host::new(vec![]);
    host.snapshot.lock().unwrap()["startup"] = Value::Null;
    host.mutation_replies.lock().unwrap().extend([json!({"error":"acknowledgement lost"}),json!({"error":"acknowledgement lost again"})]);
    let (mut app, mut screen) = host.open();
    keys(&mut app, &mut screen, &[Key::Char('c'),Key::Esc]);
    open_chief(&mut app, &mut screen);
    assert_eq!(host.mutations().len(), 1);
    app.handle(Key::Char('c'), &mut screen);
    let shown = rendered(&mut app, 80, 24);
    assert!(shown.contains("Cockpit hasn't confirmed this start yet.") && shown.contains("acknowledgement lost again"), "{shown}");
    let mutations = host.mutations();
    assert_eq!(mutations[0]["action"]["request_id"], mutations[1]["action"]["request_id"]);
}

#[test]
fn the_chief_tab_shows_its_state_and_its_keys_and_never_approves() {
    let host = Host::new(vec![chief("active",true)]);
    host.snapshot.lock().unwrap()["today"] = json!({"since_ms":1,"answered":5,"escalated":1});
    name_the_session(&host, "Chief: run agents without you · agent-cockpit");
    let (mut app,_) = host.open();
    let text = lines_of(&mut app,120,45).join("\n");
    for expected in ["◆ Chief · On · answered 5 today · 1 sent to you","Runs as Chief: run agents without you · agent-cockpit · opencode",
        " c  pause"," p  pick another chief"," r  summary now"," l  answer log"," h  habits","TODAY","Nothing decided yet today."] {
        assert!(text.contains(expected),"{expected}: {text}");
    }
    for forbidden in ["Approve","Attest","Bind role","Workers and roles","Chief controls"] {assert!(!text.contains(forbidden),"{forbidden}: {text}");}
}


#[test]
fn narrow_review_and_confirmation_keep_cancel_and_action_instructions_visible() {
    let host=Host::new(vec![chief("active",true)]); let (mut app,mut screen)=host.open(); first_control_review(&mut app,&mut screen);
    let review=rendered(&mut app,24,8);
    assert!(review.contains("Esc cancel"),"{review}");
    assert!(review.contains("Enter"),"{review}");
    app.handle(Key::Enter,&mut screen);
    let confirmation=rendered(&mut app,24,8);
    assert!(confirmation.contains("Esc"),"{confirmation}");
    assert!(confirmation.contains("y submit"),"{confirmation}");
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
fn root_cancel_restores_focus_and_tab_does_not_submit() {
    let host=Host::new(vec![chief("active",true)]); let (mut app,mut screen)=host.open(); first_control_review(&mut app,&mut screen);
    app.handle(Key::Enter,&mut screen);
    app.focus=1;
    let before=app.focus; app.handle(Key::Tab,&mut screen); assert_ne!(app.focus,before);
    app.handle(Key::BackTab,&mut screen); assert_eq!(app.focus,before);
    app.handle(Key::Char('n'),&mut screen); assert!(host.mutations().is_empty());
}
#[test]
fn a_suspended_review_needs_its_confirmation_again_after_tab_cycling() {
    for key in [Key::Tab, Key::BackTab] {
        let host=Host::new(vec![chief("active",true)]); let (mut app,mut screen)=host.open();
        first_control_review(&mut app,&mut screen);
        app.handle(Key::Enter,&mut screen);
        let before=app.focus;
        app.handle(key.clone(),&mut screen);
        assert_ne!(app.focus,before,"{key:?} leaves the Chief for another panel");
        assert!(host.mutations().is_empty());
        open_chief(&mut app,&mut screen);
        app.handle(Key::Char('c'),&mut screen);
        app.handle(Key::Char('y'),&mut screen);
        assert!(host.mutations().is_empty(),"suspended confirmation must be reset");
        app.handle(Key::Esc,&mut screen);
        assert_eq!(app.focus,6,"closing the review returns to the Chief tab");
    }
}

#[test]
fn summary_request_does_not_enable_progress_notifications() {
    let host=Host::new(vec![json!({"kind":"summary","state":"completed","last_completed_at_ms":12345,
        "last_result":{"provenance":"chief_narrative_unverified","sections":{"completed":"Worker claim only"}}})]);
    let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Char('r'),Key::Enter,Key::Enter]);
    let requests=host.mutations(); assert_eq!(requests.len(),1); assert_eq!(requests[0]["action"]["action"],"request_summary");
    let text=rendered(&mut app,120,45); assert!(text.contains("12345")&&text.contains("chief_narrative_unverified"),"{text}");
}

fn worker() -> Value {
    json!({"kind":"worker","session":{"session_id":"ses_3f2a9c0b1ffeQkTz8mWn4rJd2L","agent":"opencode","project":PROJECT,"runtime":"verified-native-runtime"},
        "role":null,"ready":false,"blocked_by":["host_native_observation_missing","admission_not_verified","pending_human_input"],
        "observed_at_ms":1,"observation_provenance":"daemon_derived_availability","native_observation":{"source":"runtime_report","runtime_report":{"provenance":"runtime_report_not_independently_proven"}}})
}

#[test]
fn status_shows_one_chief_line_pointing_to_the_chief_tab() {
    let host = Host::new(vec![chief("active", true), answer("answer-1", "received"), escalation(None)]);
    host.snapshot.lock().unwrap()["today"] = json!({"since_ms":1,"answered":205,"escalated":3});
    let mut app = App::new();
    let mut screen = TestScreen::default();
    app.handle(Key::Char('s'), &mut screen);
    app.build();
    let text = lines_of(&mut app, 100, 80).join("\n");
    let line = text.lines().find(|line| line.contains("◆ On")).unwrap_or_else(|| panic!("{text}"));
    assert!(line.contains("◆ On · 205 answered today · 1 needs you") && line.contains(" 5 ") && line.contains("Chief tab"), "{line}");
    assert!(!text.contains("Last: Use option B.") && !text.contains("Runs as"), "{text}");
}


#[test]
fn status_does_not_call_a_missing_chief_host_off() {
    let host = Host::new(vec![]);
    std::fs::remove_file(cockpit::paths::team_host_socket()).unwrap();
    let mut app = App::new();
    let mut screen = TestScreen::default();
    app.handle(Key::Char('s'), &mut screen);
    app.build();
    let text = rendered(&mut app, 100, 80);
    assert!(text.contains("Chief unavailable"), "{text}");
    drop(host);
}
fn chief(state: &str, live: bool) -> Value {
    json!({"kind":"chief","session":worker()["session"],"epoch":"grant-epoch-1","revision":2,"state":state,"live_verified":live,"approval_hash":"grant-hash","created_at_ms":1})
}
fn type_note(app: &mut App, screen: &mut TestScreen, note: &str) {
    for character in note.chars() {app.handle(Key::Char(character),screen);} app.handle(Key::Enter,screen);
}
#[test]
fn paused_and_unavailable_states_are_not_off() {
    let host=Host::new(vec![chief("paused",true)]);let (mut app,mut screen)=host.open();
    let paused=rendered(&mut app,120,45);
    assert!(paused.contains("◇ Chief · Paused · c resume") && paused.contains("While it is off, sessions ask you directly."),"{paused}");
    host.snapshot.lock().unwrap()["records"]=json!([chief("active",false)]);
    app.handle(Key::Esc,&mut screen);open_chief(&mut app,&mut screen);
    let unavailable=rendered(&mut app,120,45);
    assert!(unavailable.contains("Unavailable") && unavailable.contains(" c  disable"),"{unavailable}");
    app.handle(Key::Char('c'),&mut screen);let review=rendered(&mut app,150,60);
    for expected in ["verified-native-runtime","grant-epoch-1","Workers and teams continue","grant-hash"] {assert!(review.contains(expected),"{review}");}
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);
    assert_eq!(host.mutations()[0]["action"]["expected_epoch"],"grant-epoch-1");
}

fn decision() -> Value {
    json!({"kind":"decision","id":"decision-1","scope_key":"scope-1","decision_key":"uncertain_delivery","work":null,"assignment":null,
        "question":"Confirm ownership","recommendation":"Check worker (chief recommendation)","state":"open","revision":4,"resolution":null,"created_at_ms":1,"resolved_at_ms":null})
}
fn uncertain_delivery() -> (Value,Value) {
    let mut decision=decision();decision["scope_key"]=json!("host-delivery-delivery-1");
    (decision,json!({"kind":"inbox","id":"delivery-1","state":"uncertain","revision":5,"project":PROJECT,"task_id":"1234abcd","worker":null,"recipient":worker()["session"]}))
}

#[test]
fn exact_launch_and_message_recovery_keep_separate_unchecked_stop_consent() {
    for (kind,scope,action,field) in [("legacy_launch","host-launch-target-1","reconcile_launch","token"),("inbox","host-delivery-target-1","reconcile_delivery","message")] {
        let mut decision=decision();decision["scope_key"]=json!(scope);
        let target=json!({"kind":kind,"id":"target-1","state":"uncertain","revision":3,"project":PROJECT,"task_id":"1234abcd","worker":null,"recipient":worker()["session"]});
        let host=Host::new(vec![decision,target]);let (mut app,mut screen)=host.open();
        keys(&mut app,&mut screen,&[Key::Char('x')]);
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
fn host_sqlite_numeric_live_flag_preserves_pause_resume_controls() {
    let mut grant=chief("active",true);grant["live_verified"]=json!(1);
    let host=Host::new(vec![grant]);let (mut app,mut screen)=host.open();
    assert!(rendered(&mut app,120,45).contains("◆ Chief · On"));
    app.handle(Key::Char('c'),&mut screen);
    assert!(rendered(&mut app,120,45).contains("New chief actions and automatic wake-ups stop."));
}
#[test]
fn suspended_confirmation_requires_separate_confirmation_again_on_return() {
    let host=Host::new(vec![chief("active",true)]);let (mut app,mut screen)=host.open();first_control_review(&mut app,&mut screen);
    keys(&mut app,&mut screen,&[Key::Enter,Key::Tab]);
    open_chief(&mut app,&mut screen);
    app.handle(Key::Char('c'),&mut screen);
    app.handle(Key::Char('y'),&mut screen);assert!(host.mutations().is_empty());
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);assert_eq!(host.mutations().len(),1);
}
#[test]
fn escaped_long_details_remain_inspectable_on_a_narrow_short_screen() {
    let mut record=decision();record["recommendation"]=json!(format!("{}\u{1b}[2J \u{202e} END-OF-EXACT-DECISION","Long bounded scope ".repeat(40)));
    let host=Host::new(vec![record]);let (mut app,mut screen)=host.open();keys(&mut app,&mut screen,&[Key::Char('i')]);
    let mut all=String::new();for _ in 0..250 {all.push_str(&rendered(&mut app,32,8));app.handle(Key::PageDown,&mut screen);}
    for expected in ["END-OF-EXACT-DECISION","\\u001b","decision_key"] {assert!(all.contains(expected),"missing {expected}: {all}");}
    assert!(!all.contains('\u{1b}')&&!all.contains('\u{202e}'));assert!(host.mutations().is_empty());
}

#[test]
fn enable_pause_and_resume_submit_only_after_exact_runtime_review_and_confirmation() {
    for (grant,expected) in [(None,"enable"),(Some(chief("active",true)),"pause"),(Some(chief("paused",true)),"resume")] {
        let mut ready=worker();ready["ready"]=json!(true);ready["blocked_by"]=json!([]);
        let mut records=vec![ready];if let Some(grant)=grant {records.push(grant);}
        let host=Host::new(records);let (mut app,mut screen)=host.open();
        if expected=="enable" {keys(&mut app,&mut screen,&[Key::Char('p'),Key::Enter]);} else {app.handle(Key::Char('c'),&mut screen);}
        let review=rendered(&mut app,180,80);assert!(review.contains("verified-native-runtime"),"{review}");
        app.handle(Key::Char('y'),&mut screen);assert!(host.mutations().is_empty());
        keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);
        let request=&host.mutations()[0]["action"];assert_eq!(request["action"],expected);
        if expected=="enable" {assert_eq!(request["session"],worker()["session"]);} else {assert_eq!(request["expected_epoch"],"grant-epoch-1");}
    }
}
#[test]
fn resolve_and_handled_notes_never_act_as_completion_or_permission() {
    for handled in [false,true] {
        let (delivery_decision,delivery)=uncertain_delivery();
        let host=Host::new(vec![delivery_decision,delivery]);let (mut app,mut screen)=host.open();
        keys(&mut app,&mut screen,&[Key::Char('i'),Key::Enter]);
        if handled {keys(&mut app,&mut screen,&[Key::Down,Key::Enter]);}
        app.handle(Key::Enter,&mut screen);type_note(&mut app,&mut screen,"Observed by Theo");
        assert!(host.mutations().is_empty());let review=rendered(&mut app,180,80);
        assert!(review.contains(if handled {"retain ownership"} else {"Grants no authority"}),"{review}");
        keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);let request=&host.mutations()[0]["action"];
        if handled {assert_eq!(request["evidence"]["kind"],"handled");assert_eq!(request["expected_revision"],5);}
        else {assert_eq!(request["action"],"resolve");assert_eq!(request["expected_revision"],4);assert_eq!(request["resolution"],"Observed by Theo");}
    }
}
#[test]
fn host_pagination_invalidates_and_restarts_without_mixing_generations() {
    let mut current=decision();current["question"]=json!("Current decision question");
    let host=Host::new(vec![current]);
    let mut stale=decision();stale["question"]=json!("Stale decision question");
    let mut first=snapshot(vec![stale]);first["has_more"]=json!(true);first["next"]=json!(1);first["generation"]=json!("old-generation");
    host.pages.lock().unwrap().extend([first,json!({"records":[],"invalidated":true,"snapshot_required":true,"next":0,"generation":"new-generation"})]);
    let (mut app,mut screen)=host.open();app.handle(Key::Enter,&mut screen);
    let text=rendered(&mut app,120,45);assert!(text.contains("Current decision question")&&!text.contains("Stale decision question"),"{text}");
    let requests=host.requests.lock().unwrap();assert_eq!(requests[1]["after"],1);assert_eq!(requests[1]["generation"],"old-generation");assert_eq!(requests[2]["after"],0);assert_eq!(requests[2]["generation"],Value::Null);
}
#[test]
fn host_storage_failure_rejects_confirmation_and_preserves_visible_failure() {
    let host=Host::new(vec![chief("active",true)]);let (mut app,mut screen)=host.open();first_control_review(&mut app,&mut screen);
    app.handle(Key::Enter,&mut screen);
    host.pages.lock().unwrap().push_back(json!({"error":"database unavailable\u{1b}[2J"}));app.handle(Key::Char('y'),&mut screen);
    assert!(host.mutations().is_empty());let text=rendered(&mut app,160,70);assert!(text.contains("database unavailable")&&!text.contains('\u{1b}'));
}
#[test]
fn team_bound_worker_cannot_be_selected_chief() {
    let host=Host::new(vec![worker(),json!({"kind":"team_member","session":worker()["session"],"role":"qa","epoch":"team-epoch","team_revision":3,"task":{"project":PROJECT,"task_id":"1234abcd"}})]);
    let (mut app,mut screen)=host.open();keys(&mut app,&mut screen,&[Key::Char('p')]);
    assert!(rendered(&mut app,150,60).contains("Nothing available"));
    keys(&mut app,&mut screen,&[Key::Enter,Key::Enter]);assert!(host.mutations().is_empty());
}

#[test]
fn chief_decisions_preserve_selection_scroll_and_human_prompts_on_refresh() {
    let mut first=decision();first["question"]=json!("FIRST DECISION");
    let mut second=decision();second["id"]=json!("decision-2");second["question"]=json!("Start of second decision");second["recommendation"]=json!("source report ".repeat(50));
    let host=Host::new(vec![first,second]);
    let session=worker()["session"]["session_id"].as_str().unwrap().to_string();
    cockpit::prompts::add(&session,"Human prompt stays queued").unwrap();let prompt=cockpit::prompts::oldest(&session).unwrap();
    let (mut app,mut screen)=host.open();keys(&mut app,&mut screen,&[Key::Char('j'),Key::Char('i')]);
    let _=rendered(&mut app,65,20);app.handle(Key::PageDown,&mut screen);
    let before=rendered(&mut app,65,20);assert!(!before.contains("FIRST DECISION"));
    app.handle(Key::Tab,&mut screen);
    host.snapshot.lock().unwrap()["records"][1]["question"]=json!("Updated host evidence on second decision");
    host.snapshot.lock().unwrap()["records"][1]["revision"]=json!(5);
    open_chief(&mut app,&mut screen);
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
    let state=snapshot(vec![decision(),resolved,json!({"kind":"event","event_kind":"progress","body":"success"})]);
    assert_eq!(cockpit::chief::view::ChiefView::attention(&state,chrono::Utc::now().timestamp_millis()),1);
}

#[test]
fn narrow_review_keeps_enter_and_escape_instructions_accessible() {
    let host=Host::new(vec![chief("active",true)]);let (mut app,mut screen)=host.open();first_control_review(&mut app,&mut screen);
    let text=rendered(&mut app,40,12);assert!(text.contains("Enter confirm")&&text.contains("Esc cancel"),"{text}");
}

#[test]
fn actual_chief_view_reviews_current_records_without_loading_terminal_history() {
    use cockpit::chief::{store,ChiefAgentAction,ChiefHostAction,WorkerState};
    use cockpit::teams::{TransportOutcome,VerifiedSession};
    let mut connection=rusqlite::Connection::open_in_memory().unwrap();store::migrate(&mut connection).unwrap();
    let worker:VerifiedSession=serde_json::from_value(worker()["session"].clone()).unwrap();
    let mut chief=worker.clone();chief.session_id="ses_chief000000000000".into();chief.runtime="chief-runtime".into();
    let live:Vec<_>=[chief.clone(),worker.clone()].into_iter().map(|session|WorkerState {session,role:None,ready:true,blocked_by:vec![],observed_at_ms:100}).collect();
    store::host(&mut connection,&ChiefHostAction::Enable {request_id:"enable".into(),session:chief.clone()},&live,100).unwrap().result.unwrap();
    store::verify_admission(&mut connection,&chief,"opencode-queued-v1","Controlled fixture, not live proof",100).unwrap();
    store::observe(&mut connection,&live,100).unwrap();let wake=store::claim(&mut connection,&chief,"bootstrap",100).unwrap().expect("durable chief wake");
    store::finish_for(&mut connection,&chief,&wake.id,&TransportOutcome::Received {native_id:"bootstrap-native".into()},100).unwrap();
    let ask=|key:&str,question:&str| ChiefAgentAction::Decision {key:key.into(),question:question.into(),recommendation:"Check the exact record".into()};
    for (key,question) in [("old-question","Old resolved question"),("current-question","Current open question")] {
        store::agent(&mut connection,&chief,&ask(key,question),&live,100).unwrap().result.unwrap();
    }
    connection.execute_batch("WITH RECURSIVE history(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM history WHERE n<26000)
        INSERT INTO chief_events(id,source,submission_key,fingerprint,kind,body,created_at) SELECT 'terminal-'||n,'host','terminal-'||n,'fixture','status','{}',n FROM history;").unwrap();
    let decision_with=|connection:&rusqlite::Connection,question:&str| connection.query_row("SELECT id,revision FROM chief_decisions WHERE question=?1",[question],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?))).unwrap();
    let (old_id,old_revision)=decision_with(&connection,"Old resolved question");
    store::host(&mut connection,&ChiefHostAction::Resolve {request_id:"resolve-old".into(),decision:old_id,expected_revision:old_revision,resolution:"Already handled".into()},&live,101).unwrap().result.unwrap();
    let page=store::snapshot_page(&connection,&live,0,50,None,102).unwrap();assert_eq!(page["has_more"],false);
    let records=page["records"].as_array().unwrap();
    assert!(records.iter().any(|record|record["kind"]=="decision" && record["question"]=="Current open question"),"open decisions must be retained");
    assert!(!records.iter().any(|record|record["kind"]=="decision" && record["question"]=="Old resolved question"),"resolved history must not be loaded");
    let host=Host::new(records.clone());*host.snapshot.lock().unwrap()=page;
    let (mut app,mut screen)=host.open();
    let list=rendered(&mut app,160,70);
    assert!(list.contains("Current open question")&&!list.contains("Old resolved question"),"{list}");
    keys(&mut app,&mut screen,&[Key::Char('i'),Key::Enter,Key::Enter]);type_note(&mut app,&mut screen,"Checked by Theo");
    assert!(rendered(&mut app,160,70).contains("Current open question"));app.handle(Key::Enter,&mut screen);
    store::agent(&mut connection,&chief,&ask("current-question","Materially changed question"),&live,400).unwrap().result.unwrap();
    *host.snapshot.lock().unwrap()=store::snapshot_page(&connection,&live,0,50,None,400).unwrap();
    app.handle(Key::Char('y'),&mut screen);
    assert!(host.mutations().is_empty(),"a decision changed during review must invalidate pending consent");
    assert!(rendered(&mut app,160,70).contains("changed"));
    keys(&mut app,&mut screen,&[Key::Esc,Key::Esc,Key::Esc,Key::Char('i'),Key::Enter,Key::Enter]);type_note(&mut app,&mut screen,"Checked the changed question");
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);
    let action:ChiefHostAction=serde_json::from_value(host.mutations()[0]["action"].clone()).unwrap();
    store::host(&mut connection,&action,&live,401).unwrap().result.unwrap();
    assert!(host.requests.lock().unwrap().len()<=8);
    let audit=store::audit_page(&connection,&live,0,50,None,402).unwrap();assert_eq!(audit["has_more"],true);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_events WHERE id LIKE 'terminal-%'",[],|row|row.get::<_,i64>(0)).unwrap(),26000);
}

#[test]
fn a_short_narrow_chief_tab_keeps_the_selected_row_visible() {
    let records: Vec<Value>=(1..=4).map(|number| {let mut record=decision();record["id"]=json!(format!("decision-{number}"));record["question"]=json!(format!("Question number {number}"));record}).collect();
    let host=Host::new(records);let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Char('j'),Key::Char('j'),Key::Char('j')]);
    assert!(rendered(&mut app,32,12).contains("Question number 4"));
}


fn escalation(suggestion: Option<&str>) -> Value {
    let mut record = decision();
    for (field, value) in [("id", json!("decision-esc")), ("scope_key", json!("chief-epoch-1")), ("decision_key", json!("stop-stop-1")),
        ("session_id", worker()["session"]["session_id"].clone()), ("stop_id", json!("stop-1")), ("category", json!("scope")),
        ("question", json!("Which option do you want?")), ("recommendation", json!("Changing the API is outside the task.")),
        ("suggested_answer", json!(suggestion)), ("revision", json!(3))] {
        record[field] = value;
    }
    record
}

#[test]
fn a_decision_with_a_suggestion_offers_send_suggestion_and_write_my_own() {
    let host=Host::new(vec![escalation(Some("Keep the old API."))]);let (mut app,mut screen)=host.open();
    app.handle(Key::Char('i'),&mut screen);
    let detail=rendered(&mut app,160,60);
    for expected in ["scope","Changing the API is outside the task.","Which option do you want?","Keep the old API."] {assert!(detail.contains(expected),"{expected}: {detail}");}
    app.handle(Key::Enter,&mut screen);
    let actions=rendered(&mut app,160,40);
    assert!(actions.contains("Send the suggestion")&&actions.contains("Write my own answer"),"{actions}");
    keys(&mut app,&mut screen,&[Key::Enter,Key::Enter,Key::Char('y')]);
    let request=&host.mutations()[0]["action"];
    assert_eq!(request["action"],"answer_decision");assert_eq!(request["decision"],"decision-esc");
    assert_eq!(request["expected_revision"],3);assert_eq!(request["answer"],"Keep the old API.");
}

#[test]
fn a_decision_without_a_suggestion_only_offers_to_write_an_answer() {
    let host=Host::new(vec![escalation(None)]);let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Char('i'),Key::Enter]);
    let actions=rendered(&mut app,160,40);
    assert!(actions.contains("Write my own answer")&&!actions.contains("Send the suggestion"),"{actions}");
}

#[test]
fn a_plain_decision_offers_no_answer_to_an_agent() {
    let host=Host::new(vec![decision()]);let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Char('i'),Key::Enter]);
    let actions=rendered(&mut app,160,40);
    assert!(actions.contains("Resolve with note")&&!actions.contains("Write my own answer")&&!actions.contains("Send the suggestion"),"{actions}");
}

#[test]
fn the_answer_review_shows_the_exact_text_to_send() {
    let host=Host::new(vec![escalation(Some("Keep the old API."))]);let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Char('i'),Key::Enter,Key::Down,Key::Enter]);
    type_note(&mut app,&mut screen,"Use option B, then run the tests.");
    assert!(host.mutations().is_empty(),"typing the answer sends nothing");
    let review=rendered(&mut app,220,80);
    for expected in ["Use option B, then run the tests.","Answer from Theo, sent through the cockpit Chief view."] {
        assert!(review.contains(expected),"{expected}: {review}");
    }
    for forbidden in ["Chief answer on Theo's behalf.","still need Theo"] {
        assert!(!review.contains(forbidden),"{forbidden}: {review}");
    }
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);
    let request=&host.mutations()[0]["action"];
    assert_eq!(request["action"],"answer_decision");assert_eq!(request["answer"],"Use option B, then run the tests.");
}

#[test]
fn an_escalation_changed_during_review_invalidates_the_answer() {
    let host=Host::new(vec![escalation(Some("Keep the old API."))]);let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Enter,Key::Enter,Key::Enter,Key::Enter,Key::Enter]);
    host.snapshot.lock().unwrap()["records"][0]["revision"]=json!(4);
    host.snapshot.lock().unwrap()["records"][0]["suggested_answer"]=json!("Change the API instead.");
    app.handle(Key::Char('y'),&mut screen);
    assert!(host.mutations().is_empty(),"a decision changed during review must invalidate pending consent");
}

const SESSION: &str = "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L";
const ANSWER_TIME_MS: i64 = 1_761_000_000_000;
fn answer(id: &str, delivery: &str) -> Value {
    json!({"kind":"answer","id":id,"stop_id":"stop-1","session_id":SESSION,"project":PROJECT,"task_id":"1234abcd",
        "question":"Which option do you want?","answer":"Use option B.","reason":"The task names option B.","source":"chief",
        "delivery":delivery,"created_at_ms":ANSWER_TIME_MS,"overrule":null})
}
fn clock(milliseconds: i64) -> String {
    chrono::DateTime::from_timestamp_millis(milliseconds).unwrap().with_timezone(&chrono::Local).format("%H:%M").to_string()
}
fn name_the_session(host: &Host, name: &str) { host.snapshot.lock().unwrap()["session_names"] = json!({SESSION: name}); }
fn lines_of(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let mut buffer = Buffer::empty(Rect::new(0,0,width,height)); app.render(&mut buffer);
    (0..height).map(|row|(0..width).map(|column|buffer[(column,row)].symbol().to_string()).collect::<String>()).collect()
}
fn cell_of(app: &mut App, needle: &str) -> ratatui::buffer::Cell { cell_at(app,needle,0) }
/// The cell `offset` characters into the first screen line that holds `needle`.
fn cell_at(app: &mut App, needle: &str, offset: usize) -> ratatui::buffer::Cell {
    let (width,height)=(120,45); let mut buffer = Buffer::empty(Rect::new(0,0,width,height)); app.render(&mut buffer);
    for row in 0..height {
        let line: String = (0..width).map(|column|buffer[(column,row)].symbol().to_string()).collect();
        if let Some(byte) = line.find(needle) { return buffer[((line[..byte].chars().count()+offset) as u16,row)].clone(); }
    }
    panic!("{needle} is not on the screen");
}
fn open_answers(host: &Host) -> (App, TestScreen) {
    let (mut app,mut screen) = host.open(); keys(&mut app,&mut screen,&[Key::Char('l')]); (app,screen)
}

#[test]
fn the_chief_tab_counts_what_needs_you() {
    let host=Host::new(vec![decision(),escalation(None),answer("answer-1","received"),answer("answer-2","queued"),answer("answer-3","uncertain")]);
    let (mut app,_)=host.open();
    let text=rendered(&mut app,120,45);
    assert!(text.contains("NEEDS YOU · 2"),"{text}");
    assert!(!text.contains("Decisions"),"{text}");
}


#[test]
fn the_answers_page_lists_rows_newest_first() {
    let mut oldest=answer("answer-old","received");oldest["created_at_ms"]=json!(ANSWER_TIME_MS-7_200_000);oldest["question"]=json!("Oldest question");
    let mut newest=answer("answer-new","uncertain");newest["created_at_ms"]=json!(ANSWER_TIME_MS);newest["question"]=json!("Newest question");newest["answer"]=json!("Use\noption B.");
    let mut waiting=answer("answer-wait","queued");waiting["created_at_ms"]=json!(ANSWER_TIME_MS-3_600_000);waiting["question"]=json!("Waiting question");
    let host=Host::new(vec![oldest,newest,waiting]);name_the_session(&host,"Fix login · a-project");
    let (mut app,_)=open_answers(&host);
    let text=lines_of(&mut app,120,45).join("\n");
    let positions: Vec<usize>=["Newest question","Waiting question","Oldest question"].iter().map(|question|text.find(question).unwrap_or_else(||panic!("{question}: {text}"))).collect();
    assert!(positions[0]<positions[1]&&positions[1]<positions[2],"{text}");
    for expected in [format!("{} · Fix login · a-project · Newest question",clock(ANSWER_TIME_MS)).as_str(),"Chief: Use option B.","not sure · The task names option B.","waiting · The task names option B.","sent · The task names option B."] {
        assert!(text.contains(expected),"{expected}: {text}");
    }
    assert!(text.contains("Answers · 3")||text.contains("Chief · Answers"),"{text}");
    assert!(text.contains("o overrule · ⏎ reason · esc back"),"{text}");
    assert_eq!(cell_of(&mut app,"not sure").fg,ratatui::style::Color::Indexed(cockpit::app::RED));
    let reason=cell_at(&mut app,"waiting · The task names option B.","waiting · ".chars().count());
    assert!(reason.fg!=ratatui::style::Color::Indexed(cockpit::app::RED)&&reason.modifier.contains(ratatui::style::Modifier::DIM),"only the word not sure is red");
    let sent=cell_of(&mut app,"waiting ·");
    assert!(sent.modifier.contains(ratatui::style::Modifier::DIM)&&sent.fg!=ratatui::style::Color::Indexed(cockpit::app::RED));
}

#[test]
fn an_overruled_answer_shows_the_correction_delivery_on_its_third_line() {
    let mut overruled=answer("answer-1","received");
    overruled["overrule"]=json!({"id":"overrule-1","correction":"Use option C.","delivery":"queued","created_at_ms":ANSWER_TIME_MS+1000});
    let host=Host::new(vec![overruled]);let (mut app,_)=open_answers(&host);
    let text=rendered(&mut app,120,45);
    assert!(text.contains("overruled · waiting"),"{text}");
}

#[test]
fn an_empty_answers_page_says_no_chief_answers_yet() {
    let host=Host::new(vec![]);let (mut app,_)=open_answers(&host);
    let text=rendered(&mut app,120,45);
    assert!(text.contains("No Chief answers yet."),"{text}");
}

#[test]
fn a_long_answer_row_stays_on_one_line_and_escaped() {
    let mut long=answer("answer-1","received");long["answer"]=json!(format!("{}\u{202e}\u{1b}[2J END",
        "Very long answer. ".repeat(200)));
    let host=Host::new(vec![long]);let (mut app,_)=open_answers(&host);
    let lines=lines_of(&mut app,100,30);
    assert_eq!(lines.iter().filter(|line|line.contains("Chief: Very long answer.")).count(),1,"{lines:#?}");
    assert!(lines.iter().find(|line|line.contains("Chief: Very long")).unwrap().trim_end().trim_end_matches('│').trim_end().ends_with('…'),"{lines:#?}");
    let text=lines.join("\n");assert!(!text.contains('\u{202e}')&&!text.contains('\u{1b}'),"{text}");
}

#[test]
fn overruling_shows_the_exact_text_then_sends_it() {
    let host=Host::new(vec![answer("answer-1","received")]);let (mut app,mut screen)=open_answers(&host);
    app.handle(Key::Char('o'),&mut screen);
    type_note(&mut app,&mut screen,"Use option C instead.");
    assert!(host.mutations().is_empty(),"typing the correction sends nothing");
    let review=rendered(&mut app,260,90);
    for expected in ["Correction from Theo to an earlier Chief answer (","\"Use option B.\"","Use option C instead.",
        "Correction from Theo, sent through the cockpit Chief view. Treat it as a new instruction inside your current task."] {
        assert!(review.contains(expected),"{expected}: {review}");
    }
    app.handle(Key::Char('y'),&mut screen);assert!(host.mutations().is_empty(),"y does nothing before the separate confirmation");
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);
    let requests=host.mutations();assert_eq!(requests.len(),1);
    let request=&requests[0]["action"];
    assert_eq!(request["action"],"overrule");assert_eq!(request["answer"],"answer-1");assert_eq!(request["correction"],"Use option C instead.");
}

#[test]
fn command_s_sends_the_visible_correction_after_a_fresh_read() {
    let host=Host::new(vec![answer("answer-1","received")]);let (mut app,mut screen)=open_answers(&host);
    keys(&mut app,&mut screen,&[Key::Char('o'),Key::Paste("Use option C instead.".into())]);
    let text=rendered(&mut app,100,40);
    for expected in ["Use option B.","Becomes a habit", "Use option C instead.","⌘S"] {assert!(text.contains(expected),"{expected}: {text}");}
    app.handle(Key::Ctrl('s'),&mut screen);
    assert_eq!(host.mutations().len(),1);
    assert_eq!(host.mutations()[0]["action"]["correction"],"Use option C instead.");
    app.handle(Key::Ctrl('s'),&mut screen);
    assert_eq!(host.mutations().len(),1,"a second save must not resend");
}

#[test]
fn command_s_refuses_a_correction_when_its_answer_changed() {
    let host=Host::new(vec![answer("answer-1","received")]);let (mut app,mut screen)=open_answers(&host);
    keys(&mut app,&mut screen,&[Key::Char('o'),Key::Paste("Use C.".into())]);
    host.snapshot.lock().unwrap()["records"][0]["overrule"]=json!({"correction":"Already corrected"});
    app.handle(Key::Ctrl('s'),&mut screen);
    assert!(host.mutations().is_empty());
    assert!(rendered(&mut app,100,40).contains("changed"));
}

#[test]
fn a_long_correction_editor_keeps_the_draft_and_habit_preview_scrollable() {
    let mut long=answer("answer-1","received");long["question"]=json!("Question ".repeat(150));long["answer"]=json!("Original answer ".repeat(100));
    let host=Host::new(vec![long]);let (mut app,mut screen)=open_answers(&host);
    keys(&mut app,&mut screen,&[Key::Char('o'),Key::Paste("Use C.".into())]);
    let text=rendered(&mut app,40,14);
    assert!(text.contains("Use C."),"{text}");
    assert!(text.contains("Becomes a habit"),"{text}");
    for _ in 0..60 {app.handle(Key::PageUp,&mut screen);}
    assert!(rendered(&mut app,40,14).contains("Question"));
    for _ in 0..60 {app.handle(Key::PageDown,&mut screen);}
    assert!(rendered(&mut app,40,14).contains("Use C."));
    assert!(host.mutations().is_empty());
}

#[test]
fn chief_shortcuts_open_their_scoped_pages_without_approving() {
    for (key,expected) in [('l',"Chief · Answers"),('h',"Chief · Habits"),('p',"Pick another Chief"),('c',"Frozen review"),('r',"Requested summary")] {
        let host=Host::new(vec![chief("active",true)]);let (mut app,mut screen)=host.open();
        app.handle(Key::Char(key),&mut screen);
        assert!(rendered(&mut app,100,60).contains(expected),"shortcut {key}");
        assert_eq!(host.mutations().len(),usize::from(key=='r'));
    }
}

#[test]
fn the_chief_tab_h_opens_habits_without_any_change() {
    let host=Host::new(vec![]);let mut app=App::new();let mut screen=TestScreen::default();
    keys(&mut app,&mut screen,&[Key::Char('5'),Key::Char('h')]);
    assert!(rendered(&mut app,100,40).contains("No habits yet"));
    assert!(host.mutations().is_empty());
}


#[test]
fn the_chief_picker_names_unsupported_sessions_and_cannot_enable_them() {
    let mut unsupported=worker();unsupported["session"]["agent"]=json!("claude");
    let host=Host::new(vec![unsupported]);name_the_session(&host,"NAS proxy recovery · homelab");
    let (mut app,mut screen)=host.open();app.handle(Key::Char('p'),&mut screen);
    let text=rendered(&mut app,100,40);
    assert!(text.contains("NAS proxy recovery"),"{text}");
    assert!(text.contains("no native inbox"),"{text}");
    keys(&mut app,&mut screen,&[Key::Enter,Key::Enter,Key::Char('y')]);
    assert!(host.mutations().is_empty());
}

#[test]
fn a_current_chief_is_not_silently_replaced_from_the_picker() {
    let mut other=worker();other["session"]["session_id"]=json!("ses_other00000000000000000000");other["ready"]=json!(true);other["blocked_by"]=json!([]);
    let host=Host::new(vec![chief("active",true),other]);let (mut app,mut screen)=host.open();
    app.handle(Key::Char('p'),&mut screen);
    assert!(rendered(&mut app,120,40).contains("disable the current Chief"));
    keys(&mut app,&mut screen,&[Key::Enter,Key::Enter,Key::Char('y')]);
    assert!(host.mutations().is_empty());
}

#[test]
fn uncertain_delivery_shortcuts_start_evidence_only_on_the_selected_row() {
    for key in ['y','x'] {
        let (decision,delivery)=uncertain_delivery();
        let mut today=answer("answer-today","received");today["created_at_ms"]=json!(chrono::Utc::now().timestamp_millis());
        let host=Host::new(vec![decision,delivery,today]);let (mut app,mut screen)=host.open();
        let text=rendered(&mut app,120,50);
        assert!(text.contains(" y  it arrived") && text.contains(" x  stop safely") && text.contains(" i  details"),"{text}");
        app.handle(Key::Char('j'),&mut screen);
        app.handle(Key::Char(key),&mut screen);
        assert!(!rendered(&mut app,120,50).contains("Nonempty note"),"today's answer is selected");
        app.handle(Key::Char('k'),&mut screen);
        app.handle(Key::Char(key),&mut screen);
        assert!(rendered(&mut app,120,50).contains("Nonempty note"));
        assert!(host.mutations().is_empty());
    }
}


#[test]
fn the_detail_action_overrules_an_uncertain_answer_too() {
    let host=Host::new(vec![answer("answer-1","uncertain")]);let (mut app,mut screen)=open_answers(&host);
    app.handle(Key::Enter,&mut screen);
    let detail=rendered(&mut app,160,60);
    for expected in ["Which option do you want?","Use option B.","The task names option B.","not sure"] {assert!(detail.contains(expected),"{expected}: {detail}");}
    keys(&mut app,&mut screen,&[Key::Enter,Key::Enter]);
    type_note(&mut app,&mut screen,"Use option C.");
    keys(&mut app,&mut screen,&[Key::Enter,Key::Char('y')]);
    assert_eq!(host.mutations()[0]["action"]["action"],"overrule");
}

#[test]
fn an_answer_not_sent_yet_says_why_it_cannot_be_overruled() {
    let mut overruled=answer("answer-4","received");
    overruled["overrule"]=json!({"id":"overrule-1","correction":"Use option C.","delivery":"received","created_at_ms":1});
    let mut theirs=answer("answer-5","received");theirs["source"]=json!("theo");
    for (record,reason) in [(answer("answer-1","queued"),"Not sent yet"),(answer("answer-2","cancelled"),"Cancelled"),
            (overruled,"Already overruled"),(theirs,"This answer is yours")] {
        let host=Host::new(vec![record]);let (mut app,mut screen)=open_answers(&host);
        app.handle(Key::Char('o'),&mut screen);
        let text=rendered(&mut app,120,45);
        assert!(text.contains(reason),"{reason}: {text}");
        app.handle(Key::Char('x'),&mut screen);
        assert!(!rendered(&mut app,120,45).contains("Your correction"),"no note box opens");
        keys(&mut app,&mut screen,&[Key::Enter,Key::Enter]);
        assert!(host.mutations().is_empty());
    }
}

#[test]
fn a_changed_answer_invalidates_an_open_overrule_review() {
    let host=Host::new(vec![answer("answer-1","uncertain")]);let (mut app,mut screen)=open_answers(&host);
    app.handle(Key::Char('o'),&mut screen);type_note(&mut app,&mut screen,"Use option C.");app.handle(Key::Enter,&mut screen);
    host.snapshot.lock().unwrap()["records"][0]["delivery"]=json!("received");
    app.handle(Key::Char('y'),&mut screen);
    assert!(host.mutations().is_empty(),"an answer changed during review must invalidate pending consent");
    assert!(rendered(&mut app,160,70).contains("changed"));
}

#[test]
fn answers_name_the_session_not_its_id() {
    let mut escalated=escalation(None);escalated["session_id"]=json!(SESSION);
    let host=Host::new(vec![answer("answer-1","received"),escalated]);name_the_session(&host,"Fix the login bug · a-long-canonical-project");
    let (mut app,mut screen)=host.open_inbox();
    let needs_you=rendered(&mut app,160,45);
    assert!(needs_you.contains("a-long-canonical-project › Fix the login bug")&&!needs_you.contains(SESSION),"{needs_you}");
    keys(&mut app,&mut screen,&[Key::Char('i')]);
    let detail=rendered(&mut app,160,60);assert!(detail.contains("Session: Fix the login bug · a-long-canonical-project"),"{detail}");
    keys(&mut app,&mut screen,&[Key::Esc,Key::Esc,Key::Char('l')]);
    let list=rendered(&mut app,160,45);assert!(list.contains("Fix the login bug · a-long-canonical-project")&&!list.contains(SESSION),"{list}");
    app.handle(Key::Enter,&mut screen);
    let detail=rendered(&mut app,160,60);assert!(detail.contains("Fix the login bug · a-long-canonical-project")&&!detail.contains(SESSION),"{detail}");
}

#[test]
fn a_session_without_a_name_falls_back_to_the_end_of_its_id() {
    let host=Host::new(vec![answer("answer-1","received")]);let (mut app,_)=open_answers(&host);
    let text=rendered(&mut app,160,45);
    assert!(text.contains("ses_…Jd2L")||text.contains(&format!("ses_…{}",&SESSION[SESSION.len()-6..])),"{text}");
    assert!(!text.contains(SESSION),"{text}");
}

#[test]
fn an_agent_chosen_session_label_is_escaped_and_cut_to_one_line() {
    let host=Host::new(vec![answer("answer-1","received")]);
    name_the_session(&host,&format!("Line one\nline two \u{202e}{} · project","x".repeat(400)));
    let (mut app,_)=open_answers(&host);
    let lines=lines_of(&mut app,160,45);
    let text=lines.join("\n");assert!(!text.contains('\u{202e}'),"{text}");
    assert_eq!(lines.iter().filter(|line|line.contains("Line one line two")).count(),1,"{text}");
    assert!(!text.contains(&"x".repeat(350)),"{text}");
}

#[test]
fn session_names_from_every_page_of_the_snapshot_are_kept() {
    let host=Host::new(vec![answer("answer-1","received")]);
    let mut app=App::new();let mut screen=TestScreen::default();
    let mut first=snapshot(vec![]);first["has_more"]=json!(true);first["next"]=json!(1);first["session_names"]=json!({"ses_other0000000000":"Other · elsewhere"});
    let mut second=snapshot(vec![answer("answer-1","received")]);second["session_names"]=json!({SESSION:"Second page name · project"});
    host.pages.lock().unwrap().extend([first,second]);
    keys(&mut app,&mut screen,&[Key::Char('5'),Key::Char('l')]);
    let text=rendered(&mut app,160,45);assert!(text.contains("Second page name · project"),"{text}");
}

#[test]
fn the_answers_page_can_be_limited_to_one_session() {
    let mut other=answer("answer-2","received");other["session_id"]=json!("ses_other0000000000");other["question"]=json!("Question of another session");
    let mut state=snapshot(vec![answer("answer-1","received"),other]);state["session_names"]=json!({SESSION:"Fix the login bug · project"});
    let mut view=cockpit::chief::view::ChiefView::new(state);view.open_answers(Some(SESSION.into()));
    let (title,lines)=view.list_window(30);
    assert_eq!(title,"Answers · Fix the login bug · project");
    let text=lines.iter().map(|line|line.text.clone()).collect::<Vec<_>>().join("\n");
    assert_eq!(lines[0].text,format!("> {} · Which option do you want?",clock(ANSWER_TIME_MS)));
    assert_eq!(lines[1].text,"  Chief: Use option B.");
    assert!(text.contains("Which option do you want?")&&!text.contains("Question of another session"),"{text}");
    assert_eq!(view.footer(),"o overrule · ⏎ reason · esc back");
}

#[test]
fn the_answers_page_of_one_session_states_the_card_count_and_its_rule() {
    let mut state=snapshot(vec![answer("answer-1","received")]);state["session_names"]=json!({SESSION:"Fix the login bug · project"});
    let mut view=cockpit::chief::view::ChiefView::new(state);
    view.open_answers(Some(SESSION.into()));view.card_count=4;
    assert_eq!(view.list_window(30).0,"Answers · Fix the login bug · project · Chief ✓4 this task");
    view.card_count=0;
    assert_eq!(view.list_window(30).0,"Answers · Fix the login bug · project");
}

#[test]
fn the_selected_row_stays_on_screen_below_a_stale_line() {
    let decisions: Vec<Value>=(0..10).map(|number|{let mut record=decision();record["id"]=json!(format!("decision-{number}"));record["question"]=json!(format!("Question {number}"));record}).collect();
    let mut view=cockpit::chief::view::ChiefView::new(snapshot(decisions));
    view.handle(Key::Enter);
    for _ in 0..9 {view.handle(Key::Down);}
    view.read_failed("host went away".into());
    let (_,lines)=view.list_window(8);
    let text=lines.iter().map(|line|line.text.clone()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("> ")&&text.contains("Question 9"),"{text}");
    assert!(lines.len()<=8,"{text}");
}

#[test]
fn a_tall_row_at_the_bottom_still_fits_the_window() {
    let answers: Vec<Value>=(0..5).map(|number|{let mut record=answer(&format!("answer-{number}"),"received");record["created_at_ms"]=json!(ANSWER_TIME_MS-number*60_000);record["question"]=json!(format!("Question {number}"));record}).collect();
    let mut view=cockpit::chief::view::ChiefView::new(snapshot(answers));
    view.open_answers(None);
    for _ in 0..4 {view.handle(Key::Down);}
    let (_,lines)=view.list_window(5);
    let text=lines.iter().map(|line|line.text.clone()).collect::<Vec<_>>().join("\n");
    assert!(lines.len()<=5&&text.contains("> ")&&text.contains("Question 4")&&text.contains("sent ·"),"{text}");
}

#[test]
fn an_escalation_detail_shows_no_raw_record_json() {
    let mut escalated=escalation(Some("Keep the old API."));escalated["session_id"]=json!(SESSION);
    let host=Host::new(vec![escalated]);name_the_session(&host,"Fix the login bug · project");
    let (mut app,mut screen)=host.open();keys(&mut app,&mut screen,&[Key::Char('i')]);
    let detail=rendered(&mut app,160,60);
    assert!(detail.contains("Session: Fix the login bug · project")&&detail.contains("Keep the old API."),"{detail}");
    for forbidden in ["session_id",SESSION,"{","\"kind\""] {assert!(!detail.contains(forbidden),"{forbidden}: {detail}");}
}

#[test]
fn the_answers_count_shows_a_plus_when_the_snapshot_hit_its_cap() {
    for (count,expected) in [(199,"Answers · 199"),(200,"Answers · 200+")] {
        let answers: Vec<Value>=(0..count).map(|number|answer(&format!("answer-{number}"),"received")).collect();
        let host=Host::new(answers);let (mut app,_)=open_answers(&host);
        let text=rendered(&mut app,120,45);assert!(text.contains(expected)&&(count==200||!text.contains("199+")),"{expected}: {text}");
    }
}

fn older_clock(milliseconds: i64) -> String {
    chrono::DateTime::from_timestamp_millis(milliseconds).unwrap().with_timezone(&chrono::Local).format("%a %d %b %H:%M").to_string()
}
fn summary_record(previous_completed_at_ms: Value) -> Value {
    json!({"kind":"summary","state":"completed","last_completed_at_ms":ANSWER_TIME_MS,"previous_completed_at_ms":previous_completed_at_ms,
        "last_result":{"provenance":"chief_narrative_unverified","sections":{"completed":"Worker claim only"}}})
}
fn answer_at(id: &str, question: &str, created_at_ms: i64) -> Value {
    let mut record=answer(id,"received");record["question"]=json!(question);record["created_at_ms"]=json!(created_at_ms);record
}
fn open_summary(host: &Host) -> (App, TestScreen) {
    let (mut app,mut screen)=host.open();
    keys(&mut app,&mut screen,&[Key::Char('r')]);
    (app,screen)
}

#[test]
fn a_summary_lists_the_answers_since_the_previous_summary() {
    let since=ANSWER_TIME_MS-3_600_000;
    let mut records=vec![summary_record(json!(since)),answer_at("old","Before the last summary",since-1_000)];
    for index in 0..7 {records.push(answer_at(&format!("new-{index}"),&format!("Question {index}"),since+1_000*(index+1)));}
    let mut theo=answer_at("theo","Theo answered this",since+500);theo["source"]=json!("theo");records.push(theo);
    let mut cancelled=answer_at("cancelled","Never delivered",since+600);cancelled["delivery"]=json!("cancelled");records.push(cancelled);
    let host=Host::new(records);name_the_session(&host,"Fix login · shop");
    let (mut app,_)=open_summary(&host);
    let text=lines_of(&mut app,160,60).join("\n");
    assert!(text.contains(&format!("Answers since {}",older_clock(since))),"{text}");
    assert!(text.contains("Fix login · shop · Question 6 → Use option B."),"{text}");
    assert!(text.contains("Fix login · shop · Question 2 → Use option B."),"{text}");
    for hidden in ["Question 1 →","Question 0 →","Before the last summary","Theo answered this","Never delivered"] {assert!(!text.contains(hidden),"{hidden}: {text}");}
    assert!(text.contains("+2 more"),"{text}");
}

#[test]
fn a_summary_without_a_previous_one_looks_back_24_hours() {
    let now=chrono::Utc::now().timestamp_millis();
    let host=Host::new(vec![summary_record(Value::Null),answer_at("recent","Asked an hour ago",now-3_600_000),answer_at("stale","Asked two days ago",now-172_800_000)]);
    let (mut app,_)=open_summary(&host);
    let text=lines_of(&mut app,160,60).join("\n");
    assert!(text.contains("Answers since")&&text.contains("Asked an hour ago → Use option B."),"{text}");
    assert!(!text.contains("Asked two days ago"),"{text}");
}

#[test]
fn a_summary_says_when_the_chief_answered_nothing_since() {
    let host=Host::new(vec![summary_record(json!(ANSWER_TIME_MS)),answer_at("old","Long ago",ANSWER_TIME_MS-1_000)]);
    let (mut app,_)=open_summary(&host);
    let text=rendered(&mut app,160,60);
    assert!(text.contains(&format!("No Chief answers since {}",older_clock(ANSWER_TIME_MS))),"{text}");
}

const PROJECT_NAME: &str = "a-long-canonical-project";
const SEEN_AT_MS: i64 = ANSWER_TIME_MS;
fn habit(id: &str, text: &str, project: Option<&str>, created_at_ms: i64) -> Value {
    json!({"kind":"habit","id":id,"text":text,"project":project,"origin_project":PROJECT,"source":"overrule","source_kind":"overrule",
        "source_id":format!("overrule-{id}"),"reason":null,"state":"active","created_at_ms":created_at_ms,"changed_at_ms":created_at_ms,
        "session_id":SESSION,"stopped_at_ms":null})
}
fn deleted(mut record: Value) -> Value { record["state"]=json!("deleted"); record }
fn habits_seen_at(milliseconds: i64) {
    let path=cockpit::paths::chief_habits_seen_at(); fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path,milliseconds.to_string()).unwrap();
}
fn open_habits(host: &Host) -> (App, TestScreen) {
    let (mut app,mut screen)=host.open(); app.handle(Key::Char('h'),&mut screen); (app,screen)
}
fn selected_line(app: &mut App) -> String {
    lines_of(app,120,45).into_iter().find(|line|line.contains("│ > ")).unwrap_or_default()
}

#[test]
fn the_chief_tab_shows_habits_and_how_many_are_new() {
    let host=Host::new(vec![habit("habit-old","Old habit.",Some(PROJECT),SEEN_AT_MS-1_000),habit("habit-a","New habit A.",Some(PROJECT),SEEN_AT_MS+1_000),
        habit("habit-b","New habit B.",None,SEEN_AT_MS+2_000),deleted(habit("habit-gone","Deleted habit.",None,SEEN_AT_MS+3_000))]);
    habits_seen_at(SEEN_AT_MS);
    let (mut app,_)=host.open();
    let text=rendered(&mut app,120,45);
    assert!(text.contains(" h  habits (2 new)"),"{text}");
}


#[test]
fn the_chief_tab_shows_no_new_count_when_every_habit_was_seen() {
    let host=Host::new(vec![habit("habit-old","Old habit.",Some(PROJECT),SEEN_AT_MS-1_000)]);
    habits_seen_at(SEEN_AT_MS);
    let (mut app,_)=host.open();
    let text=rendered(&mut app,120,45);
    assert!(text.contains(" h  habits")&&!text.contains("new)"),"{text}");
}


#[test]
fn without_a_seen_time_habits_of_the_last_24_hours_are_new() {
    let now=chrono::Utc::now().timestamp_millis();
    let host=Host::new(vec![habit("habit-recent","Recent habit.",None,now-3_600_000),habit("habit-stale","Stale habit.",None,now-172_800_000)]);
    let (mut app,_)=host.open();
    assert!(rendered(&mut app,120,45).contains(" h  habits (1 new)"));
}

#[test]
fn h_opens_the_habits_page() {
    let host=Host::new(vec![habit("habit-a","Always run the tests first.",Some(PROJECT),SEEN_AT_MS)]);
    let (mut app,_)=open_habits(&host);
    let text=rendered(&mut app,120,45);
    for expected in ["Chief · Habits","Always run the tests first.","x delete · p project/everywhere · ⏎ source · esc back"] {assert!(text.contains(expected),"{expected}: {text}");}
}

#[test]
fn habit_rows_show_new_text_scope_source_and_time() {
    let mut answered=habit("habit-b","Prefer small commits.",None,SEEN_AT_MS-60_000);answered["source"]=json!("theo_answer");answered["source_kind"]=json!("theo_answer");
    let mut messaged=habit("habit-c","Write tests first.",None,SEEN_AT_MS-120_000);messaged["source"]=json!("theo_answer");messaged["source_kind"]=json!("theo_message");
    let host=Host::new(vec![habit("habit-a","Always run the tests first.",Some(PROJECT),SEEN_AT_MS+60_000),answered,messaged]);
    habits_seen_at(SEEN_AT_MS);
    let (mut app,_)=open_habits(&host);
    let text=lines_of(&mut app,120,45).join("\n");
    for expected in ["new Always run the tests first.",&format!("{PROJECT_NAME} · from your overrule · {}",clock(SEEN_AT_MS+60_000)),
        &format!("everywhere · from your answer · {}",clock(SEEN_AT_MS-60_000)),&format!("everywhere · from your message · {}",clock(SEEN_AT_MS-120_000))] {
        assert!(text.contains(expected),"{expected}: {text}");
    }
    assert!(!text.contains("new Prefer small commits.")&&text.contains("Prefer small commits."),"an old habit has no new tag: {text}");
    let tag=cell_of(&mut app,"new Always");
    assert_eq!(tag.fg,ratatui::style::Color::Indexed(cockpit::app::MAGENTA));
    assert!(tag.modifier.contains(ratatui::style::Modifier::BOLD),"new is bold");
    let habit_text=cell_at(&mut app,"new Always","new ".chars().count());
    assert_ne!(habit_text.fg,ratatui::style::Color::Indexed(cockpit::app::MAGENTA),"only the word new is purple");
    // This project used to hash to amber: project colours never use it.
    let project=cell_of(&mut app,&format!("{PROJECT_NAME} · from"));
    assert_eq!(project.fg,ratatui::style::Color::Indexed(cockpit::app::project_color(PROJECT_NAME)));
    assert_ne!(project.fg,ratatui::style::Color::Indexed(cockpit::app::YELLOW),"never amber");
    let scope=cell_of(&mut app,"everywhere · from your answer");
    assert!(scope.modifier.contains(ratatui::style::Modifier::DIM)&&scope.fg!=ratatui::style::Color::Indexed(cockpit::app::MAGENTA),"everywhere is dim: line 2 of an unselected row");
    let mut buffer=Buffer::empty(Rect::new(0,0,120,45));app.render(&mut buffer);
    let page_rows=lines_of(&mut app,120,45).iter().enumerate().filter(|(_,line)|line.contains("from your")||line.contains(" first.")||line.contains("commits.")).map(|(row,_)|row as u16).collect::<Vec<_>>();
    assert_eq!(page_rows.len(),6);
    for row in page_rows {assert!((0..120).all(|column|buffer[(column,row)].fg!=ratatui::style::Color::Indexed(cockpit::app::YELLOW)),"never amber");}
}

#[test]
fn a_habit_text_is_escaped_to_one_line() {
    let host=Host::new(vec![habit("habit-a","Keep\nit \u{202e}short\u{1b}[2J.",None,SEEN_AT_MS)]);
    let (mut app,_)=open_habits(&host);
    let text=rendered(&mut app,120,45);
    assert!(text.contains("Keep it ")&&!text.contains('\u{202e}')&&!text.contains('\u{1b}'),"{text}");
}

#[test]
fn an_empty_habit_list_explains_where_habits_come_from() {
    let host=Host::new(vec![deleted(habit("habit-gone","Deleted habit.",None,SEEN_AT_MS))]);
    let (mut app,_)=open_habits(&host);
    let text=rendered(&mut app,120,45);
    assert!(text.contains("No habits yet. The Chief learns them from your answers and overrules.")&&!text.contains("Deleted habit."),"{text}");
}

#[test]
fn x_deletes_at_once_and_u_undoes() {
    let first=habit("habit-a","Always run the tests first.",Some(PROJECT),SEEN_AT_MS);
    let host=Host::new(vec![first.clone()]);
    host.after_mutations.lock().unwrap().extend([vec![deleted(first.clone())],vec![first]]);
    let (mut app,mut screen)=open_habits(&host);
    app.handle(Key::Char('x'),&mut screen);
    let mutations=host.mutations();assert_eq!(mutations.len(),1,"x submits without a review");
    assert_eq!((mutations[0]["action"]["action"].as_str(),mutations[0]["action"]["habit"].as_str()),(Some("delete_habit"),Some("habit-a")));
    let text=rendered(&mut app,120,45);assert!(text.contains("Deleted · u undo")&&!text.contains("CONFIRM")&&!text.contains("Host result"),"{text}");
    app.handle(Key::Char('u'),&mut screen);
    let mutations=host.mutations();assert_eq!(mutations.len(),2);
    assert_eq!((mutations[1]["action"]["action"].as_str(),mutations[1]["action"]["habit"].as_str()),(Some("restore_habit"),Some("habit-a")));
    let text=rendered(&mut app,120,45);assert!(text.contains("Always run the tests first.")&&!text.contains("Deleted · u undo"),"{text}");
}

#[test]
fn the_undo_offer_ends_with_the_next_key() {
    let first=habit("habit-a","Always run the tests first.",Some(PROJECT),SEEN_AT_MS);
    let host=Host::new(vec![first.clone(),habit("habit-b","Prefer small commits.",None,SEEN_AT_MS-1_000)]);
    host.after_mutations.lock().unwrap().push_back(vec![deleted(first),habit("habit-b","Prefer small commits.",None,SEEN_AT_MS-1_000)]);
    let (mut app,mut screen)=open_habits(&host);
    keys(&mut app,&mut screen,&[Key::Char('x'),Key::Char('j')]);
    assert!(!rendered(&mut app,120,45).contains("u undo"));
    app.handle(Key::Char('u'),&mut screen);
    assert_eq!(host.mutations().len(),1,"u does nothing once another key was pressed");
}

#[test]
fn after_deleting_a_habit_the_next_one_is_selected() {
    let records=vec![habit("habit-a","Habit A.",None,SEEN_AT_MS),habit("habit-b","Habit B.",None,SEEN_AT_MS-1_000),habit("habit-c","Habit C.",None,SEEN_AT_MS-2_000)];
    let host=Host::new(records.clone());
    let mut without_b=records.clone();without_b[1]=deleted(without_b[1].clone());
    host.after_mutations.lock().unwrap().extend([without_b,records]);
    let (mut app,mut screen)=open_habits(&host);
    keys(&mut app,&mut screen,&[Key::Down,Key::Char('x')]);
    assert!(selected_line(&mut app).contains("Habit C."),"{:#?}",lines_of(&mut app,120,45));
    app.handle(Key::Char('u'),&mut screen);
    let lines=lines_of(&mut app,120,45);
    assert!(selected_line(&mut app).contains("Habit B."),"u restores it in place and selects it: {lines:#?}");
    let positions: Vec<usize>=["Habit A.","Habit B.","Habit C."].iter().map(|text|lines.iter().position(|line|line.contains(text)).unwrap()).collect();
    assert!(positions[0]<positions[1]&&positions[1]<positions[2],"{lines:#?}");
}

#[test]
fn deleting_the_last_habit_shows_the_empty_text() {
    let only=habit("habit-a","Always run the tests first.",None,SEEN_AT_MS);
    let host=Host::new(vec![only.clone()]);host.after_mutations.lock().unwrap().push_back(vec![deleted(only)]);
    let (mut app,mut screen)=open_habits(&host);
    app.handle(Key::Char('x'),&mut screen);
    let text=rendered(&mut app,120,45);
    assert!(text.contains("No habits yet.")&&text.contains("Deleted · u undo")&&!text.contains("Always run the tests first."),"{text}");
}

#[test]
fn p_toggles_a_habit_between_its_project_and_everywhere() {
    for (project,everywhere) in [(Some(PROJECT),true),(None,false)] {
        let host=Host::new(vec![habit("habit-a","Always run the tests first.",project,SEEN_AT_MS)]);
        let (mut app,mut screen)=open_habits(&host);
        app.handle(Key::Char('p'),&mut screen);
        let mutations=host.mutations();assert_eq!(mutations.len(),1,"p submits without a review");
        let action=&mutations[0]["action"];
        assert_eq!((action["action"].as_str(),action["habit"].as_str(),action["everywhere"].as_bool()),(Some("rescope_habit"),Some("habit-a"),Some(everywhere)));
        assert!(!rendered(&mut app,120,45).contains("Host result"));
    }
}

#[test]
fn a_failed_habit_change_says_so_and_offers_no_undo() {
    let host=Host::new(vec![habit("habit-a","Always run the tests first.",None,SEEN_AT_MS)]);
    let (mut app,mut screen)=open_habits(&host);
    host.mutation_replies.lock().unwrap().push_back(json!({"error":"no project to scope to"}));
    app.handle(Key::Char('p'),&mut screen);
    let text=rendered(&mut app,120,45);
    assert!(text.contains("no project to scope to")&&!text.contains("u undo"),"{text}");
    host.mutation_replies.lock().unwrap().push_back(json!({"error":"habit not found"}));
    app.handle(Key::Char('x'),&mut screen);
    let text=rendered(&mut app,120,45);
    assert!(text.contains("habit not found")&&!text.contains("u undo"),"{text}");
}

#[test]
fn enter_shows_where_a_habit_came_from() {
    let mut corrected=answer("answer-1","received");
    corrected["overrule"]=json!({"id":"overrule-habit-a","correction":"Never touch the public API.","delivery":"received","created_at_ms":ANSWER_TIME_MS+1000});
    let mut theirs=answer("answer-theo","received");theirs["source"]=json!("theo");theirs["answer"]=json!("Keep the old endpoint.");
    let mut from_answer=habit("habit-b","Keep the old endpoint.",None,SEEN_AT_MS);from_answer["source"]=json!("theo_answer");from_answer["source_kind"]=json!("theo_answer");
    from_answer["source_id"]=json!("answer-theo");from_answer["reason"]=json!("Theo said so twice.");
    let mut from_message=habit("habit-c","Write tests first.",None,SEEN_AT_MS);from_message["source"]=json!("theo_answer");from_message["source_kind"]=json!("theo_message");
    from_message["source_id"]=json!("stop-9");from_message["stopped_at_ms"]=json!(ANSWER_TIME_MS);
    for (record,expected) in [
        (habit("habit-a","Never touch the public API.",Some(PROJECT),SEEN_AT_MS),vec!["From your overrule","Your correction: Never touch the public API.","The Chief's answer it corrected: Use option B.","Session: Fix login · shop"]),
        (from_answer,vec!["From your answer","Your answer: Keep the old endpoint.","Why the Chief added it: Theo said so twice.","Session: Fix login · shop"]),
        (from_message,vec!["From your message","Session: Fix login · shop",&format!("Stopped at {}",cockpit::chief::view::since_label(ANSWER_TIME_MS,chrono::Local::now().date_naive()))]),
    ] {
        let host=Host::new(vec![record,corrected.clone(),theirs.clone()]);name_the_session(&host,"Fix login · shop");
        let (mut app,mut screen)=open_habits(&host);
        app.handle(Key::Enter,&mut screen);
        let detail=rendered(&mut app,200,60);
        for line in expected {assert!(detail.contains(line),"{line}: {detail}");}
        for forbidden in ["\"kind\"","source_id"] {assert!(!detail.contains(forbidden),"{forbidden}: {detail}");}
    }
}

#[test]
fn the_overrule_review_previews_the_exact_habit_it_stores() {
    let correction=format!("{}\u{202e} END","Use option C and keep the tests green. ".repeat(20));
    for (project,scope) in [(json!(PROJECT),PROJECT_NAME),(Value::Null,"everywhere")] {
        let mut sent=answer("answer-1","received");sent["project"]=project;
        let mut view=cockpit::chief::view::ChiefView::new(snapshot(vec![sent]));
        view.open_answers(None);
        view.handle(Key::Char('o'));view.handle(Key::Paste(correction.clone()));view.handle(Key::Enter);
        let review=view.detail().join("\n");
        let stored=cockpit::chief::store::habit_text(&correction);
        assert!(stored.chars().count()==500&&stored.ends_with('…'),"the fixture is cut");
        let expected=format!("Adds a habit: {} · {scope}",cockpit::approval::escape_hiding(&stored,"",""));
        assert!(review.contains(&expected),"{expected}\n{review}");
        assert!(!review.contains('\u{202e}'),"{review}");
    }
}

#[test]
fn an_answer_row_names_the_habits_it_used() {
    let mut used=answer("answer-1","received");
    used["habits"]=json!([{"id":"habit-a","text":"Always run the tests first."},{"id":"habit-b","text":"Prefer small commits."},{"id":"habit-c","text":"Keep replies short."}]);
    let mut single=answer("answer-2","received");single["created_at_ms"]=json!(ANSWER_TIME_MS-60_000);single["habits"]=json!([{"id":"habit-b","text":"Prefer small commits."}]);
    let mut none=answer("answer-3","received");none["created_at_ms"]=json!(ANSWER_TIME_MS-120_000);none["habits"]=json!([]);
    let host=Host::new(vec![used,single,none]);let (mut app,_)=open_answers(&host);
    let lines=lines_of(&mut app,160,45);let text=lines.join("\n");
    assert!(text.contains("sent · The task names option B. · habits: Always run the tests first. +2"),"{text}");
    assert!(text.contains("sent · The task names option B. · habits: Prefer small commits.│")||lines.iter().any(|line|line.trim_end().trim_end_matches('│').trim_end().ends_with("habits: Prefer small commits.")),"{text}");
    assert_eq!(lines.iter().filter(|line|line.contains("habits:")).count(),2,"an answer without habits names none: {text}");
}

#[test]
fn opening_the_habits_page_clears_new() {
    let host=Host::new(vec![habit("habit-a","Always run the tests first.",None,SEEN_AT_MS+60_000)]);
    habits_seen_at(SEEN_AT_MS);
    let before=chrono::Utc::now().timestamp_millis();
    let (mut app,mut screen)=open_habits(&host);
    assert!(rendered(&mut app,120,45).contains("new Always run the tests first."),"the page tags what was new when it opened");
    let saved: i64=fs::read_to_string(cockpit::paths::chief_habits_seen_at()).unwrap().trim().parse().unwrap();
    assert!(saved>=before,"opening the page saves when Theo saw the habits");
    app.handle(Key::Esc,&mut screen);
    let tab=rendered(&mut app,120,45);
    assert!(tab.contains(" h  habits")&&!tab.contains("new)"),"{tab}");
}

#[test]
fn no_project_colour_is_amber() {
    for project in ["a-long-canonical-project","vitals","agent-cockpit","dotfiles","shop","webshop","x","yy"] {
        assert_ne!(cockpit::app::project_color(project),cockpit::app::YELLOW,"{project}");
    }
}

#[test]
fn opening_habits_after_a_failed_read_keeps_them_new() {
    let host=Host::new(vec![habit("habit-a","Always run the tests first.",None,SEEN_AT_MS+60_000)]);
    habits_seen_at(SEEN_AT_MS);
    let (mut app,mut screen)=host.open();
    app.handle(Key::Tab,&mut screen);
    host.pages.lock().unwrap().push_back(json!({"error":"database unavailable"}));
    app.handle(Key::Esc,&mut screen);
    app.handle(Key::Char('c'),&mut screen);
    app.handle(Key::Char('h'),&mut screen);
    let saved=fs::read_to_string(cockpit::paths::chief_habits_seen_at()).unwrap();
    assert_eq!(saved.trim(),SEEN_AT_MS.to_string(),"a stale page must not mark habits as seen");
}

#[test]
fn opening_habits_saves_when_the_habits_were_read() {
    let host=Host::new(vec![habit("habit-a","Always run the tests first.",None,SEEN_AT_MS+60_000)]);
    let (mut app,mut screen)=host.open();
    std::thread::sleep(Duration::from_millis(30));
    let after_read=chrono::Utc::now().timestamp_millis();
    app.handle(Key::Char('h'),&mut screen);
    let saved: i64=fs::read_to_string(cockpit::paths::chief_habits_seen_at()).unwrap().trim().parse().unwrap();
    assert!(saved<after_read,"a habit added after the read stays new: saved {saved}, read before {after_read}");
}

#[test]
fn an_open_view_rereads_when_a_digest_showed_the_habits() {
    let host=Host::new(vec![habit("habit-a","Always run the tests first.",None,SEEN_AT_MS+60_000)]);
    habits_seen_at(SEEN_AT_MS);
    let (mut app,mut screen)=host.open();
    assert!(rendered(&mut app,120,45).contains(" h  habits (1 new)"));
    habits_seen_at(SEEN_AT_MS+120_000);
    keys(&mut app,&mut screen,&[Key::Char('r'),Key::Esc]);
    let tab=rendered(&mut app,120,45);
    assert!(tab.contains(" h  habits")&&!tab.contains("new)"),"a refresh picks up the digest's seen time: {tab}");
}

#[test]
fn saving_when_habits_were_seen_never_moves_it_back() {
    let _host=Host::new(vec![]);
    habits_seen_at(SEEN_AT_MS);
    cockpit::chief::save_habits_seen(SEEN_AT_MS-60_000).unwrap();
    assert_eq!(fs::read_to_string(cockpit::paths::chief_habits_seen_at()).unwrap().trim(),SEEN_AT_MS.to_string());
    cockpit::chief::save_habits_seen(SEEN_AT_MS+60_000).unwrap();
    assert_eq!(fs::read_to_string(cockpit::paths::chief_habits_seen_at()).unwrap().trim(),(SEEN_AT_MS+60_000).to_string());
}

#[test]
fn saving_when_habits_were_seen_overwrites_a_future_or_garbage_note() {
    let _host=Host::new(vec![]);
    let future=chrono::Utc::now().timestamp_millis()+86_400_000;
    for note in [future.to_string(),"not a time".to_string()] {
        let path=cockpit::paths::chief_habits_seen_at(); fs::create_dir_all(path.parent().unwrap()).unwrap(); fs::write(&path,&note).unwrap();
        cockpit::chief::save_habits_seen(SEEN_AT_MS).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap().trim(),SEEN_AT_MS.to_string(),"{note}");
    }
}

#[test]
fn opening_habits_never_moves_the_seen_time_back_in_memory() {
    use cockpit::chief::view::ChiefView;
    let mut view=ChiefView::new(json!({"records":[habit("habit-a","Always run the tests first.",None,SEEN_AT_MS+60_000)],"read_at_ms":SEEN_AT_MS}));
    view.habits_seen_ms=Some(SEEN_AT_MS+120_000);
    view.handle(Key::Char('h'));
    assert_eq!(view.habits_seen_ms,Some(SEEN_AT_MS+120_000));
}

#[test]
fn a_failed_restore_selects_the_row_it_left() {
    let first=habit("habit-a","Habit A.",None,SEEN_AT_MS);let second=habit("habit-b","Habit B.",None,SEEN_AT_MS-1_000);
    let host=Host::new(vec![first.clone(),second.clone()]);
    host.after_mutations.lock().unwrap().push_back(vec![deleted(first),second]);
    let (mut app,mut screen)=open_habits(&host);
    app.handle(Key::Char('x'),&mut screen);
    host.mutation_replies.lock().unwrap().push_back(json!({"error":"habit not found"}));
    app.handle(Key::Char('u'),&mut screen);
    assert!(rendered(&mut app,120,45).contains("habit not found"));
    assert!(selected_line(&mut app).contains("Habit B."),"{:#?}",lines_of(&mut app,120,45));
}

// ---- Chief inbox (DESIGN.md "Chief inbox", frames 3097-3222) ----

fn inbox_question(id: &str, revision: i64, suggestion: Option<&str>) -> Value {
    let mut record = escalation(suggestion);
    record["id"] = json!(id); record["revision"] = json!(revision);
    record["category"] = json!("not_sure");
    record["recommendation"] = json!("The fix is in, but osd won't restart while your guide is open.");
    record["created_at_ms"] = json!(chrono::Utc::now().timestamp_millis() - 240_000);
    record
}
fn two_steps(id: &str) -> Value {
    let mut record = inbox_question(id, 3, Some("Go on."));
    record["situation"] = json!("The agent can't finish its last check.");
    record["steps"] = json!([{"text":"Close your Stadeo guide"},{"text":"Tell the agent to go on","message":"Go on: restart osd and run your check."}]);
    record
}
fn inbox_host(records: Vec<Value>) -> Host {
    let host = Host::new(records);
    name_the_session(&host, "Persist Stadeo team · agent-cockpit");
    host.snapshot.lock().unwrap()["today"] = json!({"answered":5,"escalated":1});
    host
}

#[test]
fn status_chief_opens_the_question_card_when_the_chief_needs_theo() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on."))]);
    let (mut app, _) = host.open_inbox();
    let card = rendered(&mut app, 100, 40);
    for expected in ["! Chief needs you · 1 of 1", "agent-cockpit › Persist Stadeo team", "Not sure · asked 4m ago", "WHAT'S GOING ON",
        "osd won't restart", "◆ CHIEF SUGGESTS · 1 step", "1 Go on.", "do this, step by step", "write my own", "later", "full details",
        "Written by the Chief: check it before acting."] {
        assert!(card.contains(expected), "{expected}: {card}");
    }
}

#[test]
fn without_a_question_status_chief_opens_the_chief_root() {
    let host = inbox_host(vec![]);
    let (mut app, _) = host.open_inbox();
    assert!(!rendered(&mut app, 100, 40).contains("Chief needs you"));
}

#[test]
fn a_deferred_question_is_not_counted_until_it_comes_back() {
    let mut deferred = inbox_question("decision-b", 3, Some("Wait."));
    deferred["later_until_ms"] = json!(chrono::Utc::now().timestamp_millis() + 600_000);
    let mut returned = inbox_question("decision-c", 3, Some("Retry."));
    returned["later_until_ms"] = json!(chrono::Utc::now().timestamp_millis() - 1_000);
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on.")), deferred, returned]);
    let (mut app, _) = host.open_inbox();
    assert!(rendered(&mut app, 100, 40).contains("! Chief needs you · 1 of 2 · 1 later"));
}

fn put_off(id: &str) -> Value {
    let mut question = inbox_question(id, 3, Some("Wait."));
    question["later_until_ms"] = json!(chrono::Utc::now().timestamp_millis() + 600_000);
    question
}

#[test]
fn the_root_counts_a_put_off_question_apart_from_the_ones_that_need_theo() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on.")), put_off("decision-b")]);
    let (mut app, mut screen) = host.open_inbox();
    app.handle(Key::Esc, &mut screen);
    assert!(rendered(&mut app, 100, 40).contains("NEEDS YOU · 1 · 1 later"));
}

#[test]
fn a_question_put_off_while_on_screen_counts_after_the_open_ones() {
    use cockpit::chief::{inbox::Line, view::ChiefView};
    let mut view = ChiefView::new(snapshot(vec![inbox_question("decision-a", 3, Some("Go on.")), inbox_question("decision-b", 3, Some("Allow it.")), put_off("decision-c")]));
    assert!(view.open_inbox());
    view.refresh(snapshot(vec![put_off("decision-a"), inbox_question("decision-b", 3, Some("Allow it.")), put_off("decision-c")]));
    let Some(Line::Text(header)) = view.inbox_lines().into_iter().next() else { panic!("the card starts with its header") };
    assert_eq!(header[0].0, "! Chief needs you · 2 of 2 · 1 later");
}

#[test]
fn a_question_put_off_waits_in_the_needs_you_count_and_offers_nothing_to_answer() {
    let host = inbox_host(vec![put_off("decision-a")]);
    let (mut app, _) = host.open_inbox();
    let tab = rendered(&mut app, 100, 40);
    assert!(tab.contains("NEEDS YOU · 0 · 1 later") && !tab.contains("Wait.") && !tab.contains("Chief needs you"), "{tab}");
}


#[test]
fn steps_are_done_one_by_one_and_only_y_on_the_last_sends_its_exact_message() {
    let host = inbox_host(vec![two_steps("decision-a")]);
    let (mut app, mut screen) = host.open_inbox();
    assert!(rendered(&mut app, 100, 40).contains("◆ CHIEF SUGGESTS · 2 steps"));
    app.handle(Key::Char('a'), &mut screen);
    let step = rendered(&mut app, 100, 40);
    assert!(step.contains("STEP 1 OF 2") && step.contains("Close your Stadeo guide") && step.contains("done, next step") && step.contains("THEN"), "{step}");
    keys(&mut app, &mut screen, &[Key::Char('y'), Key::Enter]);
    assert!(host.mutations().is_empty());
    app.handle(Key::Char('d'), &mut screen);
    let last = rendered(&mut app, 100, 40);
    assert!(last.contains("✓ Close your Stadeo guide") && last.contains("STEP 2 OF 2 · SEND TO THE AGENT")
        && last.contains("Go on: restart osd and run your check.") && last.contains("Enter never sends."), "{last}");
    app.handle(Key::Enter, &mut screen);
    assert!(host.mutations().is_empty());
    app.handle(Key::Char('y'), &mut screen);
    let mutations = host.mutations();
    assert_eq!(mutations.len(), 1);
    let action = &mutations[0]["action"];
    assert_eq!((&action["action"], &action["decision"], &action["expected_revision"], &action["answer"]),
        (&json!("answer_decision"), &json!("decision-a"), &json!(3), &json!("Go on: restart osd and run your check.")));
}

#[test]
fn without_chief_steps_the_suggestion_is_one_step_and_the_reason_the_situation() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on."))]);
    let (mut app, mut screen) = host.open_inbox();
    app.handle(Key::Char('a'), &mut screen);
    assert!(rendered(&mut app, 100, 40).contains("STEP 1 OF 1 · SEND TO THE AGENT"));
    app.handle(Key::Char('y'), &mut screen);
    assert_eq!(host.mutations()[0]["action"]["answer"], "Go on.");
}

#[test]
fn a_question_without_a_suggestion_offers_only_write_my_own() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, None)]);
    let (mut app, mut screen) = host.open_inbox();
    let card = rendered(&mut app, 100, 40);
    assert!(!card.contains("do this, step by step") && card.contains("write my own"), "{card}");
    app.handle(Key::Char('a'), &mut screen);
    assert!(!rendered(&mut app, 100, 40).contains("STEP 1"));
}

#[test]
fn write_my_own_sends_exactly_what_theo_typed_with_cmd_s() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on."))]);
    let (mut app, mut screen) = host.open_inbox();
    app.handle(Key::Char('w'), &mut screen);
    app.handle(Key::Paste("Wait, I'll restart osd myself.".into()), &mut screen);
    let box_ = rendered(&mut app, 100, 40);
    assert!(box_.contains("YOUR ANSWER") && box_.contains("Goes to agent-cockpit › Persist Stadeo team, exactly as written."), "{box_}");
    app.handle(Key::Enter, &mut screen);
    assert!(host.mutations().is_empty());
    app.handle(Key::Ctrl('s'), &mut screen);
    assert_eq!(host.mutations()[0]["action"]["answer"], "Wait, I'll restart osd myself.\n");
}

#[test]
fn edit_first_prefills_the_message_and_esc_returns_to_the_last_step_unchanged() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on."))]);
    let (mut app, mut screen) = host.open_inbox();
    keys(&mut app, &mut screen, &[Key::Char('a'), Key::Char('e')]);
    assert!(rendered(&mut app, 100, 40).contains("YOUR ANSWER"));
    keys(&mut app, &mut screen, &[Key::Backspace, Key::Esc]);
    let last = rendered(&mut app, 100, 40);
    assert!(last.contains("SEND TO THE AGENT") && last.contains("Go on."), "{last}");
    app.handle(Key::Char('y'), &mut screen);
    assert_eq!(host.mutations()[0]["action"]["answer"], "Go on.");
}

#[test]
fn after_sending_the_inbox_waits_for_n_before_the_next_question() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on.")), inbox_question("decision-b", 2, Some("Allow it."))]);
    host.after_mutations.lock().unwrap().push_back(vec![inbox_question("decision-b", 2, Some("Allow it."))]);
    let (mut app, mut screen) = host.open_inbox();
    keys(&mut app, &mut screen, &[Key::Char('a'), Key::Char('y')]);
    let sent = rendered(&mut app, 100, 40);
    assert!(sent.contains("✓ Sent to agent-cockpit › Persist Stadeo team") && sent.contains("NEXT · 1 OF 1") && sent.contains("next question")
        && sent.contains("stop here") && !sent.contains("WHAT'S GOING ON"), "{sent}");
    app.handle(Key::Char('n'), &mut screen);
    let next = rendered(&mut app, 100, 40);
    assert!(next.contains("! Chief needs you · 1 of 1") && next.contains("Allow it."), "{next}");
}

#[test]
fn the_last_sent_question_leaves_the_empty_card_with_todays_count() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on."))]);
    host.after_mutations.lock().unwrap().push_back(vec![]);
    let (mut app, mut screen) = host.open_inbox();
    keys(&mut app, &mut screen, &[Key::Char('a'), Key::Char('y'), Key::Char('n')]);
    let empty = rendered(&mut app, 100, 40);
    assert!(empty.contains("✓ Nothing needs you") && empty.contains("The Chief answered 5 questions today.") && empty.contains("what it answered"), "{empty}");
}

#[test]
fn later_puts_the_question_off_through_the_host_without_answering() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on."))]);
    let (mut app, mut screen) = host.open_inbox();
    app.handle(Key::Char('l'), &mut screen);
    let mutations = host.mutations();
    assert_eq!(mutations.len(), 1);
    assert_eq!((&mutations[0]["action"]["action"], &mutations[0]["action"]["decision"], &mutations[0]["action"]["expected_revision"]),
        (&json!("later_decision"), &json!("decision-a"), &json!(3)));
}

#[test]
fn a_question_that_changed_while_on_a_step_returns_to_the_card_and_never_sends_the_old_revision() {
    let host = inbox_host(vec![two_steps("decision-a")]);
    let (mut app, mut screen) = host.open_inbox();
    keys(&mut app, &mut screen, &[Key::Char('a'), Key::Char('d')]);
    let mut changed = two_steps("decision-a"); changed["revision"] = json!(4);
    host.snapshot.lock().unwrap()["records"] = json!([changed]);
    app.handle(Key::Char('y'), &mut screen);
    assert!(host.mutations().is_empty());
    let card = rendered(&mut app, 100, 40);
    assert!(card.contains("This question changed. Read it again.") && card.contains("WHAT'S GOING ON"), "{card}");
}

#[test]
fn full_details_keeps_the_raw_source_and_esc_returns_to_the_card() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on."))]);
    let (mut app, mut screen) = host.open_inbox();
    app.handle(Key::Char('i'), &mut screen);
    let detail = rendered(&mut app, 120, 50);
    assert!(detail.contains("Why the Chief escalated") && detail.contains("untrusted narrative"), "{detail}");
    app.handle(Key::Esc, &mut screen);
    assert!(rendered(&mut app, 100, 40).contains("! Chief needs you"));
}

#[test]
fn a_narrow_column_wraps_the_card_and_keeps_its_keys() {
    let mut record = two_steps("decision-a");
    record["situation"] = json!("The fix is in, but the agent can't finish its last check: osd won't restart while your Stadeo guide is open, so it waits for you.");
    let host = inbox_host(vec![record]);
    let (mut app, mut screen) = host.open_inbox();
    app.handle(Key::Char('a'), &mut screen);
    app.handle(Key::Char('d'), &mut screen);
    let last = lines_of(&mut app, 40, 22).join("\n");
    assert!(last.contains("send") && last.contains("edit first"), "{last}");
}

#[test]
fn the_sent_card_names_where_the_answer_went_when_it_was_the_last_question() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on."))]);
    host.after_mutations.lock().unwrap().push_back(vec![]);
    let (mut app, mut screen) = host.open_inbox();
    keys(&mut app, &mut screen, &[Key::Char('a'), Key::Char('y')]);
    let sent = rendered(&mut app, 100, 40);
    assert!(sent.contains("✓ Sent to agent-cockpit › Persist Stadeo team"), "{sent}");
}

#[test]
fn the_sent_card_names_the_answered_session_not_the_next_one() {
    let mut other = inbox_question("decision-b", 2, Some("Allow it."));
    other["session_id"] = json!("ses_otherSession000000000");
    other["created_at_ms"] = json!(chrono::Utc::now().timestamp_millis() - 60_000);
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on.")), other.clone()]);
    host.snapshot.lock().unwrap()["session_names"] = json!({SESSION: "Persist Stadeo team · agent-cockpit", "ses_otherSession000000000": "Game launcher · stray-coop"});
    host.after_mutations.lock().unwrap().push_back(vec![other]);
    let (mut app, mut screen) = host.open_inbox();
    keys(&mut app, &mut screen, &[Key::Char('a'), Key::Char('y')]);
    let sent = rendered(&mut app, 100, 40);
    assert!(sent.contains("✓ Sent to agent-cockpit › Persist Stadeo team") && sent.contains("stray-coop › Game launcher"), "{sent}");
}

#[test]
fn a_short_column_keeps_the_question_keys_and_says_there_is_more() {
    let mut record = two_steps("decision-a");
    record["situation"] = json!("The fix is in, but the agent can't finish its last check: osd won't restart while your Stadeo guide is open, so it waits for you to close it first.");
    let host = inbox_host(vec![record]);
    let (mut app, _) = host.open_inbox();
    let card = lines_of(&mut app, 40, 18).join("\n");
    assert!(card.contains("do this") && card.contains("write my own") && card.contains("more"), "{card}");
}

#[test]
fn the_empty_card_shows_a_question_that_arrives_while_it_is_open() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on."))]);
    host.after_mutations.lock().unwrap().push_back(vec![]);
    let (mut app, mut screen) = host.open_inbox();
    keys(&mut app, &mut screen, &[Key::Char('a'), Key::Char('y'), Key::Char('n')]);
    assert!(rendered(&mut app, 100, 40).contains("Nothing needs you"));
    app.handle(Key::Tab, &mut screen);
    host.snapshot.lock().unwrap()["records"] = json!([inbox_question("decision-new", 1, Some("Retry."))]);
    open_chief(&mut app, &mut screen);
    app.handle(Key::Enter, &mut screen);
    let card = rendered(&mut app, 100, 40);
    assert!(card.contains("! Chief needs you · 1 of 1") && card.contains("Retry."), "{card}");
}

#[test]
fn a_definite_refusal_says_nothing_was_sent() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on."))]);
    host.mutation_replies.lock().unwrap().push_back(json!({"error":"decision revision is stale or already answered","code":"conflict"}));
    let (mut app, mut screen) = host.open_inbox();
    keys(&mut app, &mut screen, &[Key::Char('a'), Key::Char('y')]);
    let card = rendered(&mut app, 100, 40);
    assert!(card.contains("decision revision is stale") && card.contains("Nothing was sent.") && !card.contains("may have arrived"), "{card}");
}

#[test]
fn a_typed_answer_survives_its_question_changing() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on."))]);
    let (mut app, mut screen) = host.open_inbox();
    app.handle(Key::Char('w'), &mut screen);
    app.handle(Key::Paste("Wait, I'll restart osd myself.".into()), &mut screen);
    let mut changed = inbox_question("decision-a", 4, Some("Go on."));
    changed["recommendation"] = json!("The guide closed; osd can restart now.");
    host.snapshot.lock().unwrap()["records"] = json!([changed]);
    app.handle(Key::Ctrl('s'), &mut screen);
    assert!(host.mutations().is_empty(), "the old revision is never sent");
    let card = rendered(&mut app, 100, 40);
    assert!(card.contains("The question changed; your draft is kept below.") && card.contains("osd can restart now")
        && card.contains("Wait, I'll restart osd myself."), "{card}");
    app.handle(Key::Char('w'), &mut screen);
    app.handle(Key::Ctrl('s'), &mut screen);
    let sent = &host.mutations()[0]["action"];
    assert_eq!((&sent["answer"], &sent["expected_revision"]), (&json!("Wait, I'll restart osd myself."), &json!(4)));
}

#[test]
fn leaving_write_my_own_keeps_the_draft_for_later() {
    let host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on."))]);
    let (mut app, mut screen) = host.open_inbox();
    app.handle(Key::Char('w'), &mut screen);
    app.handle(Key::Paste("Half an answer".into()), &mut screen);
    app.handle(Key::Esc, &mut screen);
    assert!(rendered(&mut app, 100, 40).contains("Half an answer"));
    app.handle(Key::Char('w'), &mut screen);
    app.handle(Key::Paste(", finished.".into()), &mut screen);
    app.handle(Key::Ctrl('s'), &mut screen);
    assert_eq!(host.mutations()[0]["action"]["answer"], "Half an answer, finished.");
}

#[test]
fn the_chief_tab_h_opens_habits_even_when_a_question_waits() {
    let _host = inbox_host(vec![inbox_question("decision-a", 3, Some("Go on.")), habit("habit-a","Always run the tests first.",Some(PROJECT),SEEN_AT_MS)]);
    let mut app = App::new(); let mut screen = TestScreen::default();
    keys(&mut app, &mut screen, &[Key::Esc, Key::Char('5'), Key::Char('h')]);
    let shown = rendered(&mut app, 100, 40);
    assert!(shown.contains("Always run the tests first.") && !shown.contains("! Chief needs you"), "{shown}");
}

fn answered_today(id: &str, text: &str, seconds_ago: i64) -> Value {
    let mut record = answer(id, "received");
    record["created_at_ms"] = json!(chrono::Utc::now().timestamp_millis() - seconds_ago * 1000);
    record["answer"] = json!(text);
    record
}

#[test]
fn the_chief_tab_feed_lists_today_newest_first_and_folds_the_rest() {
    let mut records = vec![chief("active", true)];
    for index in 0..5 { records.push(answered_today(&format!("today-{index}"), &format!("Answer number {index}"), 10 - index)); }
    let mut old = answer("old", "received"); old["created_at_ms"] = json!(chrono::Utc::now().timestamp_millis() - 2 * 86_400_000); old["answer"] = json!("Old answer");
    records.push(old);
    let mut theo = answered_today("theo", "Mine", 20); theo["source"] = json!("theo");
    records.push(theo);
    let host = Host::new(records); name_the_session(&host, "Fix login · shop");
    let (mut app, _) = host.open();
    let text = lines_of(&mut app, 120, 45).join("\n");
    let positions: Vec<usize> = ["Answered Fix login · shop: Answer number 4", "Answered Fix login · shop: Answer number 3", "Answered Fix login · shop: Answer number 1"]
        .iter().map(|line| text.find(line).unwrap_or_else(|| panic!("{line}: {text}"))).collect();
    assert!(positions[0] < positions[1] && positions[1] < positions[2], "{text}");
    assert!(!text.contains("Answer number 0") && !text.contains("Old answer") && text.contains("+ 2 earlier · l answer log"), "{text}");
}

#[test]
fn theos_own_answers_and_escalations_sent_today_show_in_the_feed() {
    let mut theo = answered_today("theo", "Mine", 20); theo["source"] = json!("theo");
    let mut sent = escalation(None); sent["created_at_ms"] = json!(chrono::Utc::now().timestamp_millis());
    let host = Host::new(vec![chief("active", true), theo, sent]); name_the_session(&host, "Fix login · shop");
    let (mut app, _) = host.open();
    let text = lines_of(&mut app, 120, 45).join("\n");
    assert!(text.contains("You answered Fix login · shop: Mine") && text.contains("Sent to you: Scope · Which option do you want?"), "{text}");
}

#[test]
fn now_names_the_session_that_has_not_read_the_chiefs_answer_yet() {
    let mut waiting = answered_today("waiting", "Use option B.", 180); waiting["delivery"] = json!("queued");
    let host = Host::new(vec![chief("active", true), waiting]); name_the_session(&host, "Fix login · shop");
    let (mut app, _) = host.open();
    let text = lines_of(&mut app, 120, 45).join("\n");
    assert!(text.contains("NOW · 1") && text.contains("◐ Waiting for Fix login · shop to read its answer"), "{text}");
}

#[test]
fn o_on_an_answer_in_the_feed_overrules_that_answer_and_never_sends_without_review() {
    let host = Host::new(vec![chief("active", true), answered_today("today-1", "Use option B.", 5)]);
    let (mut app, mut screen) = host.open();
    app.handle(Key::Char('o'), &mut screen);
    let note = rendered(&mut app, 120, 45);
    assert!(note.contains("Overrule: Which option do you want?") && note.contains("Chief answered: Use option B."), "{note}");
    assert!(host.mutations().is_empty());
}

#[test]
fn enter_on_an_answer_whose_session_ended_says_so_and_opens_nothing() {
    let host = Host::new(vec![chief("active", true), answered_today("today-1", "Use option B.", 5)]);
    let (mut app, mut screen) = host.open();
    app.handle(Key::Enter, &mut screen);
    assert!(rendered(&mut app, 120, 45).contains("That session is no longer running"));
}

#[test]
fn enter_on_a_needs_you_question_opens_that_question_not_the_oldest() {
    let mut older = escalation(None); older["id"] = json!("decision-old"); older["question"] = json!("The older question?"); older["created_at_ms"] = json!(1);
    let mut newer = escalation(None); newer["id"] = json!("decision-new"); newer["question"] = json!("The newer question?"); newer["created_at_ms"] = json!(2);
    let host = Host::new(vec![chief("active", true), older, newer]);
    let (mut app, mut screen) = host.open();
    keys(&mut app, &mut screen, &[Key::Char('j'), Key::Enter]);
    let card = rendered(&mut app, 120, 45);
    assert!(card.contains("Chief needs you · 2 of 2"), "{card}");
}

#[test]
fn a_question_sent_today_opens_its_session_never_the_chief_root() {
    let mut sent = escalation(None); sent["state"] = json!("answered"); sent["created_at_ms"] = json!(chrono::Utc::now().timestamp_millis());
    let host = Host::new(vec![chief("active", true), sent]);
    let (mut app, mut screen) = host.open();
    assert!(rendered(&mut app, 120, 45).contains("Sent to you: Scope"));
    app.handle(Key::Enter, &mut screen);
    let shown = rendered(&mut app, 120, 45);
    assert!(shown.contains("That session is no longer running") && !shown.contains("Request summary"), "{shown}");
}

#[test]
fn the_chief_badge_is_the_grant_on_every_tab_even_when_its_session_is_unavailable() {
    let host = Host::new(vec![chief("active", false)]);
    let (mut app, _) = host.open();
    let text = rendered(&mut app, 120, 45);
    assert!(text.contains(" 5  Chief  ● ") && text.contains("Unavailable"), "{text}");
}
