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
            std::env::set_var("CLAUDE_RUNS_DIR", root.join("queue"));
            std::env::set_var("CLAUDE_GUIDES_DIR", root.join("queue"));
            std::env::set_var("CLAUDE_ANSWERS_DIR", root.join("answers"));
            for name in ["CLAUDE_SESSIONS_CLONES", "HYPRLAND_INSTANCE_SIGNATURE"] {
                std::env::remove_var(name);
            }
        }
        Fixture { root, _lock: lock }
    }

    fn queue(&self) -> PathBuf {
        self.root.join("queue")
    }

    /// Theo's answers, "runs" or "guides" (cockpit::approval::answers_dir).
    fn answers(&self, kind: &str) -> PathBuf {
        let directory = self.root.join("answers").join(kind);
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

// ---- The registry ----

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
fn publishing_signals_waybar() {
    let mut bench = Bench::new();
    bench.report(SESSION_A, json!({"state": "waiting"}));
    assert!(bench.commands().contains(&vec!["pkill".to_string(), "-RTMIN+16".into(), "waybar".into()]));
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

const RUN_A: &str = "aaaaaaaaaaaa";
const RUN_B: &str = "bbbbbbbbbbbb";

fn started_sessions(bench: &Bench) -> Vec<String> {
    bench.commands().iter().filter(|command| command[..2] == ["tmux", "new-session"]).map(|command| command[4].clone()).collect()
}

#[test]
fn a_bad_request_id_is_refused() {
    let mut bench = Bench::new();
    assert!(bench.registry.run_request("../../etc").is_err());
}

#[test]
fn a_request_starts_in_its_own_tmux_session_one_at_a_time() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    assert_eq!(started_sessions(&bench), ["claude-run-aaaaaaaaaaaa"]);
    assert!(bench.commands().contains(&vec!["claude-sessions".to_string(), "approval-pane".into(), RUN_A.into()]));
}

#[test]
fn a_finished_request_closes_the_pane_and_starts_the_next() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_request(RUN_B).unwrap();
    bench.registry.run_phase(RUN_A, "done");
    assert!(bench.commands().contains(&vec!["claude-sessions".to_string(), "approval-pane".into(), "close".into()]));
    assert_eq!(bench.registry.runs.current, RUN_B);
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
    assert_eq!(bench.registry.runs.current, RUN_A);
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
}

#[test]
fn a_request_without_a_heartbeat_counts_from_its_filing() {
    let mut bench = Bench::new();
    fs::write(bench.fixture.queue().join(format!("{RUN_A}.request")), "{}").unwrap();
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
fn the_window_stays_open_while_the_command_runs() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    bench.registry.tick_queues();
    assert_eq!(bench.windows().len(), 1);
    assert!(!bench.windows()[0].borrow().closed);
}

#[test]
fn a_window_closed_during_the_run_is_not_reopened_until_the_last_question() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    bench.windows()[0].borrow_mut().closed = true;
    bench.registry.tick_queues();
    bench.registry.tick_queues();
    assert_eq!(bench.windows().len(), 1);
    bench.registry.run_phase(RUN_A, "asking");
    assert_eq!(bench.windows().len(), 2);
}

#[test]
fn the_running_window_closes_when_a_game_starts() {
    let mut bench = Bench::new();
    bench.registry.run_request(RUN_A).unwrap();
    bench.registry.run_phase(RUN_A, "running");
    bench.world.borrow_mut().gaming = true;
    bench.registry.tick_queues();
    assert!(bench.windows()[0].borrow().closed);
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
fn a_sandbox_may_only_report_and_request_on_its_fifo() {
    for command in ["run-request", "guide-request"] {
        assert!(allowed_from_sandbox(command), "{command}");
    }
    // Reports only come through the socket, where the sender is known.
    for command in ["session", "sessions-clear", "focus-next", "notice", "notice-clear", "run-phase", "volume"] {
        assert!(!allowed_from_sandbox(command), "{command}");
    }
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
    bench.registry.report_from(report, 500);
    assert_eq!(bench.registry.sessions[SESSION_A].target.get("tmux_pane").map(String::as_str), Some("%7"));
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
    bench.registry.report_from(report("waiting"), 500);
    bench.registry.report_from(report("needs-you"), 600);
    let entry = &bench.registry.sessions[SESSION_A];
    assert_eq!((entry.target.get("tmux_pane").map(String::as_str), entry.state.as_str()), (Some("%1"), "waiting"));
}

#[test]
fn a_report_from_an_unplaceable_sender_is_refused() {
    let mut bench = Bench::new();
    let report = parse_session(&json!({"session": SESSION_A, "project": "p", "state": "working"}).to_string()).unwrap();
    bench.registry.report_from(report, 777);
    assert!(bench.registry.sessions.is_empty());
}
