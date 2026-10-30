//! The column driven through its App, as the Python column's tests did: a scratch HOME, runtime
//! and state folder, stand-ins on PATH that log their calls (cockpit, tmux, gh, fzf,
//! hyprctl, osd-show), and a screen that answers questions from a script.

use crate::agents;
use crate::app::{App, Key, Screen, SettingsRow};
use crate::costs;
use crate::errors::ErrorReporter;
use crate::reviews::{self, Issue, Review};
use crate::sessions;
use crate::todo::{Entry, Item, TodoList};
use crate::usage::{self, BudgetAlerts};
use crate::util;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

static ENVIRONMENT: Mutex<()> = Mutex::new(());

fn check_fixture_listener_lifetimes(test_name: &str, replace: bool, unlink: bool) {
    if std::env::var("COCKPIT_FIXTURE_LIFETIME_TEST").as_deref() != Ok(test_name) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("tests::{test_name}"), "--nocapture"])
            .env("COCKPIT_FIXTURE_LIFETIME_TEST", test_name)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let socket_count = || {
        fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(|entry| fs::read_link(entry.unwrap().path()).ok())
            .filter(|target| target.to_string_lossy().starts_with("socket:"))
            .count()
    };
    let baseline = socket_count();
    for iteration in 0..12 {
        let fixture = Fixture::new();
        drop(fixture.app());
        if replace {
            for _ in 0..3 {
                team_host(&fixture, team_snapshot(), vec![]);
                drop(fixture.app());
            }
            assert_eq!(
                socket_count(),
                baseline + 1,
                "replacement leaked listeners at iteration {iteration}"
            );
        }
        if unlink {
            fs::remove_file(crate::paths::team_host_socket()).unwrap();
        }
        drop(fixture);
        let remaining = socket_count();
        eprintln!("iteration {iteration}: baseline={baseline}, remaining={remaining}");
        assert_eq!(
            remaining, baseline,
            "fixture leaked listeners at iteration {iteration}"
        );
    }
}

#[test]
fn successive_app_fixtures_release_their_listeners() {
    check_fixture_listener_lifetimes(
        "successive_app_fixtures_release_their_listeners",
        false,
        false,
    );
}

#[test]
fn replacing_fixture_servers_releases_previous_listeners() {
    check_fixture_listener_lifetimes(
        "replacing_fixture_servers_releases_previous_listeners",
        true,
        false,
    );
}

#[test]
fn unlinked_fixture_sockets_do_not_prevent_server_shutdown() {
    check_fixture_listener_lifetimes(
        "unlinked_fixture_sockets_do_not_prevent_server_shutdown",
        false,
        true,
    );
}

fn check_fixture_server_panic(test_name: &str, unwind: bool) {
    if std::env::var("COCKPIT_FIXTURE_PANIC_TEST").as_deref() == Ok(test_name) {
        use std::io::Read;
        let fixture = Fixture::new();
        fixture.serve(|_| panic!("intentional fixture server failure"));
        let mut stream =
            std::os::unix::net::UnixStream::connect(crate::paths::team_host_socket()).unwrap();
        stream.read_to_end(&mut Vec::new()).unwrap();
        if unwind {
            panic!("intentional test failure");
        }
        drop(fixture);
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &format!("tests::{test_name}"), "--nocapture"])
        .env("COCKPIT_FIXTURE_PANIC_TEST", test_name)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(101),
        "expected ordinary test failure, got {}: {stderr}",
        output.status
    );
    assert!(
        stderr.contains("intentional fixture server failure"),
        "{stderr}"
    );
    if unwind {
        assert!(stderr.contains("intentional test failure"), "{stderr}");
        assert!(
            stderr.contains("fixture server panicked during test unwinding"),
            "{stderr}"
        );
    }
}

mod continuity;
mod errors_audit;
mod layout_input;
#[path = "../tests/common/link_fixture.rs"]
mod link_fixture;
mod menus;
mod reviews_ideas;
mod session_lifecycle;
mod sessions_ui;
mod status_usage;
mod storage_lifecycle;
mod subscription_queue;
mod teams_chief_tabs;
mod teams_links;
mod todo_ui;
use crate::teams::store::Connection as LinkConnection;

#[test]
fn fixture_server_panic_during_test_unwind_does_not_abort() {
    check_fixture_server_panic(
        "fixture_server_panic_during_test_unwind_does_not_abort",
        true,
    );
}

#[test]
fn fixture_server_panic_without_test_unwind_fails_the_test() {
    check_fixture_server_panic(
        "fixture_server_panic_without_test_unwind_fails_the_test",
        false,
    );
}
const SESSION_A: &str = "aaaaaaaa-0000-4000-8000-000000000000";
const SESSION_B: &str = "bbbbbbbb-0000-4000-8000-000000000000";
const OPENCODE_SESSION: &str = "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L";

/// A scratch machine for one test. Tests run one at a time: they all change the environment.
struct Fixture {
    root: PathBuf,
    server: RefCell<Option<FixtureServer>>,
    _lock: MutexGuard<'static, ()>,
}

struct FixtureServer {
    stopping: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl FixtureServer {
    fn stop(self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.thread.thread().unpark();
        if let Err(error) = self.thread.join() {
            if std::thread::panicking() {
                use std::io::Write;
                let _ = writeln!(
                    std::io::stderr(),
                    "fixture server panicked during test unwinding: {error:?}"
                );
            } else {
                std::panic::resume_unwind(error);
            }
        }
    }
}

impl Fixture {
    fn new() -> Self {
        let lock = ENVIRONMENT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!("sidebar-test-{}", util::token_hex(6)));
        for folder in ["home", "run", "state", "bin"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        let path = format!("{}:/usr/bin:/bin", root.join("bin").display());
        // SAFETY: the lock above keeps every test that touches the environment one at a time.
        unsafe {
            std::env::set_var("HOME", root.join("home"));
            std::env::set_var("XDG_RUNTIME_DIR", root.join("run"));
            std::env::set_var("XDG_STATE_HOME", root.join("state"));
            // The audit log goes under state/; never into the real journal.
            std::env::set_var("CLAUDE_AUDIT_JOURNAL", "off");
            std::env::set_var("PATH", path);
            std::env::set_var("CLAUDE_STATUS_URL", "");
            std::env::set_var("CODEX_STATUS_URL", "");
            // The real disk's fill would add a warning to the sessions panel; a test sets its own.
            std::env::set_var("COCKPIT_DISK_MOUNTS", "");
            // H and the costs read opencode's database: never Theo's, wherever XDG_DATA_HOME points.
            std::env::set_var(
                "COCKPIT_OPENCODE_DB",
                root.join("home/.local/share/opencode/opencode.db"),
            );
            // COCKPIT_STATE_HOME too: set in opencode sessions, it would win over the
            // XDG_STATE_HOME above and point the tests at Theo's real state.
            for name in [
                "COCKPIT_STATE_HOME",
                "COCKPIT_HISTORY",
                "COCKPIT_TRANSCRIPTS",
                "CLAUDE_TODO_UPDATES",
                "CLAUDE_REVIEWS_FIXTURE",
                "COCKPIT_APPROVAL_ID",
                "COCKPIT_ANSWERS_KEY",
                "COCKPIT_ANSWERS_DIR",
                "COCKPIT_RUNS_DIR",
                "COCKPIT_REMOTE_DIR",
                "COCKPIT_REMOTE_ADDRESS",
                "COCKPIT_TODO_PATH",
                "COCKPIT_TODO_UPDATES",
                "COCKPIT_DESIGNS_DIR",
                "COCKPIT_DOTFILES_WORK",
                "COCKPIT_GREYWALL_WORK",
                "COCKPIT_GREYPROXY_WORK",
                "COCKPIT_REVIEWS_FIXTURE",
                "COCKPIT_CLONES",
                "COCKPIT_BOOT_TIME",
                "COCKPIT_GREYPROXY_DB",
                "CLAUDE_GREYWALL_WORK",
                "CLAUDE_GREYPROXY_WORK",
                "HYPRLAND_INSTANCE_SIGNATURE",
            ] {
                std::env::remove_var(name);
            }
        }
        let fixture = Fixture {
            root,
            server: RefCell::new(None),
            _lock: lock,
        };
        // fzf too: the real one opens a picker on the terminal running the tests and waits there
        // (make test on the host). Its stand-in picks nothing: a test that needs a pick shims its own.
        for name in ["cockpit", "hyprctl", "osd-show", "xdg-open", "tmux", "fzf"] {
            fixture.shim(name, "");
        }
        fixture
    }

    /// A stand-in command: logs "name arguments" to calls.log, then runs `body` (sh).
    fn shim(&self, name: &str, body: &str) {
        let path = self.root.join("bin").join(name);
        let log = self.root.join("calls.log");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"{name} $*\" >> '{}'\n{body}\n",
                log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn calls(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.root.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with(&format!("{name} ")))
            .map(str::to_string)
            .collect()
    }

    /// Waits for calls made by a command run in the background.
    fn wait_for_calls(&self, name: &str, count: usize) -> Vec<String> {
        for _ in 0..100 {
            let calls = self.calls(name);
            if calls.len() >= count {
                return calls;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        self.calls(name)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn snapshot(&self, sessions: Value) {
        self.write(
            "run/cockpit.json",
            &json!({"sessions": sessions}).to_string(),
        );
    }

    fn view(&self, session: &str) {
        self.write("run/cockpit.current", session);
    }

    fn app(&self) -> App {
        self.scheduler();
        let mut app = App::new();
        app.build();
        app
    }

    fn scheduler(&self) {
        use std::io::{Read, Write};
        let path = crate::paths::team_host_socket();
        if path.exists() {
            return;
        }
        let mut connection = rusqlite::Connection::open_in_memory().unwrap();
        crate::chief::store::migrate(&mut connection).unwrap();
        self.serve(move |mut stream| {
            let mut text = String::new();
            if stream.read_to_string(&mut text).is_err() {
                return;
            }
            let request: crate::teams::HostRequest = serde_json::from_str(&text).unwrap();
            crate::chief::store::reconcile_todos(&mut connection, &TodoList::load(), 0).unwrap();
            let result = match request {
                crate::teams::HostRequest::Snapshot => Ok(json!({"members":[]})),
                crate::teams::HostRequest::ReserveManual { task_id, directory } => {
                    let task = crate::teams::TaskKey {
                        project: format!("path:{directory}"),
                        task_id,
                    };
                    crate::chief::store::reserve_legacy_kind(&mut connection, &task, None, true, 0)
                        .map(|token| json!({"reservation":token}))
                }
                crate::teams::HostRequest::ReleaseManual {
                    task_id,
                    directory,
                    token,
                } => crate::chief::store::release_manual(
                    &mut connection,
                    &format!("path:{directory}"),
                    &task_id,
                    &token,
                )
                .map(|_| json!({"released_manual_only":true})),
                crate::teams::HostRequest::SettleReservation {
                    token,
                    expected_revision,
                    outcome,
                } => crate::chief::store::settle_launch(
                    &mut connection,
                    &token,
                    expected_revision,
                    &outcome,
                ),
                crate::teams::HostRequest::ReserveTask {
                    task_id,
                    directory,
                    worker,
                    ..
                } => {
                    let task = crate::teams::TaskKey {
                        project: format!("path:{directory}"),
                        task_id,
                    };
                    crate::chief::store::reserve_legacy(
                        &mut connection,
                        &task,
                        worker.as_deref(),
                        0,
                    )
                    .map(|token| json!({"reservation":token}))
                }
                crate::teams::HostRequest::ReserveResume { session, directory } => {
                    let task = crate::teams::TaskKey {
                        project: format!("path:{directory}"),
                        task_id: format!("resume:{session}"),
                    };
                    crate::chief::store::reserve_resume(&mut connection, &task, &session, 0)
                        .map(|token| json!({"reservation":token}))
                }
                _ => Ok(json!({"recorded":true})),
            };
            let reply = crate::teams::TeamReply {
                request_id: "fixture".into(),
                result,
            };
            let _ = stream.write_all(serde_json::to_string(&reply).unwrap().as_bytes());
        });
    }

    fn serve(&self, mut handle: impl FnMut(std::os::unix::net::UnixStream) + Send + 'static) {
        if let Some(server) = self.server.borrow_mut().take() {
            server.stop();
        }
        let path = crate::paths::team_host_socket();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stopping = Arc::new(AtomicBool::new(false));
        let server_stopping = stopping.clone();
        let thread = std::thread::spawn(move || {
            while !server_stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => handle(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        // Tests may unlink the socket, so shutdown must not reconnect by pathname.
                        std::thread::park_timeout(Duration::from_millis(1));
                    }
                    Err(error) => panic!("accept fixture request: {error}"),
                }
            }
        });
        *self.server.borrow_mut() = Some(FixtureServer { stopping, thread });
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(server) = self.server.get_mut().take() {
            server.stop();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A screen that draws nothing, answers questions from `keys` and prompts from `typed`.
#[derive(Default)]
struct FakeScreen {
    keys: VecDeque<Key>,
    typed: VecDeque<String>,
}

impl FakeScreen {
    fn answering(keys: &[Key]) -> Self {
        Self {
            keys: keys.iter().cloned().collect(),
            ..Self::default()
        }
    }

    fn typing(text: &[&str]) -> Self {
        Self {
            typed: text.iter().map(|t| t.to_string()).collect(),
            ..Self::default()
        }
    }
}

impl Screen for FakeScreen {
    fn draw(&mut self, _app: &mut App) {}
    fn next_key(&mut self, _timeout: Option<Duration>) -> Option<Key> {
        self.keys.pop_front()
    }
    fn outside(&mut self, action: &mut dyn FnMut() -> Option<String>) -> Option<String> {
        match self.typed.pop_front() {
            Some(text) => Some(text),
            None => action(),
        }
    }
}

/// Each line's texts and its right-aligned parts.
fn texts_with_badges(app: &App, panel: usize) -> Vec<String> {
    app.panels[panel]
        .lines
        .iter()
        .map(|line| {
            format!(
                "{} {}",
                line.texts(),
                line.right
                    .iter()
                    .map(|part| part.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
        .collect()
}

fn texts(app: &App, panel: usize) -> Vec<String> {
    app.panels[panel]
        .lines
        .iter()
        .chain(&app.panels[panel].fixed_bottom)
        .map(|line| line.texts())
        .collect()
}

fn quota_texts(app: &App) -> Vec<String> {
    let mut rows = vec![String::new(); app.panels[0].items];
    let mut provider = String::new();
    for line in &app.panels[0].lines {
        let text = line.texts();
        if text == "Claude" || text == "Codex" {
            provider = text.clone();
        }
        if let Some(index) = line.item {
            if rows[index].is_empty() {
                rows[index] = format!("{provider} · {text}");
            } else {
                rows[index].push_str(&format!(" {text}"));
            }
        }
    }
    rows
}

fn selected_session_badges(app: &App) -> Vec<String> {
    app.panels[1]
        .fixed_bottom
        .iter()
        .flat_map(|line| line.right.iter().map(|part| part.text.clone()))
        .collect()
}

fn render(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
    app.render(&mut buffer);
    buffer
}

fn session(id: &str, state: &str, project: &str, cwd: &str, started: i64) -> Value {
    json!({"session": id, "state": state, "project": project, "cwd": cwd, "since": started, "started": started, "tmux_pane": format!("%{}", started % 90)})
}

fn external_session(id: &str, state: &str, project: &str, cwd: &str, started: i64) -> Value {
    let mut row = session(id, state, project, cwd, started);
    row["tmux_pane"] = Value::Null;
    row["agent"] = json!("opencode");
    row
}

// ---- Sessions ----

/// `w` on the selected card: Theo's note, typed as `text`.
fn write_note(app: &mut App, text: &str) {
    app.focus_panel(1);
    app.handle(Key::Char('w'), &mut FakeScreen::typing(&[text]));
    app.redraw();
}

fn continuity_action(app: &mut App, action: &str, answers: &[&str]) {
    let (panel, action) = action.split_once('\t').expect("action context");
    let panel = panel.parse::<usize>().unwrap_or(1);
    app.focus_panel(panel);
    let key = match action {
        "updates" => 'u',
        "queue" => 'Q',
        "reviewed" => 'v',
        "defer-20m" | "bring-back" => 'l',
        "task-trail" => {
            if panel == 3 {
                'T'
            } else {
                't'
            }
        }
        _ => panic!("unknown test action {action}"),
    };
    app.handle(Key::Char(key), &mut FakeScreen::typing(answers));
    app.redraw();
}

fn continuity_result_row(turn: &str, state: &str) -> Value {
    let mut row = session(SESSION_A, state, "shop", "/r/shop", 1);
    row["repository"] = json!("acme/shop");
    row["branch"] = json!("feat/vat");
    row["last_result"] = json!({"id":turn,"completed_at":util::now_secs(),"outcome":"tests passed","repository":"acme/shop","branch":"feat/vat"});
    row
}

fn select_status_action(app: &mut App, label: &str) {
    app.focus_panel(0);
    let services = app.panels[0]
        .lines
        .iter()
        .position(|line| line.texts() == "SERVICES")
        .unwrap();
    let target = app.panels[0]
        .lines
        .iter()
        .skip(services)
        .find(|line| line.texts().contains(label) && line.item.is_some())
        .and_then(|line| line.item)
        .expect("selectable Status action");
    app.panels[0].selected = target;
}

fn vanish_selected_session(fixture: &Fixture, loading: Option<&str>) {
    fixture.snapshot(json!([
        session(SESSION_A, "waiting", "runway", "/r/runway", 1),
        session(SESSION_B, "waiting", "runway", "/r/runway", 2)
    ]));
    let mut app = fixture.app();
    app.focus_panel(1);
    let shown = app.sessions[app.panels[1].selected].session.clone();
    let other = if shown == SESSION_A {
        SESSION_B
    } else {
        SESSION_A
    };
    if let Some(label) = loading {
        fixture.write("run/cockpit-loading", label);
    }
    fixture.snapshot(json!([session(other, "waiting", "runway", "/r/runway", 2)]));
    app.build();
}

fn continuity_saved(fixture: &Fixture) -> Value {
    serde_json::from_str(
        &fs::read_to_string(fixture.path("state/cockpit/continuity.json")).unwrap(),
    )
    .unwrap()
}

fn continuity_question_row(id: &str, question: &str) -> Value {
    let mut row = session(
        id,
        "needs-you",
        "shop",
        "/r/shop",
        if id == SESSION_A { 1 } else { 2 },
    );
    row["needs_input"] = json!(true);
    row["request"] = json!(question);
    row
}

fn settings_screen_rows(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|row| {
            (0..buffer.area.width)
                .map(|column| buffer[(column, row)].symbol())
                .collect()
        })
        .collect()
}

fn model_catalog(fixture: &Fixture, variants: Value) {
    let response = json!({"location":{"directory":"/r/runway"},"data":[{
        "providerID":"openai", "id":"gpt-6.1-sol", "modelID":"gpt-6.1-sol", "name":"GPT-6.1 Sol",
        "capabilities":{"tools":true}, "variants":variants, "time":{"released":0}, "cost":[],
        "status":"active", "enabled":true, "limit":{"context":200000,"output":32000}
    }]});
    fixture.shim(
        "cockpit",
        &format!(
            "if [[ $1 == opencode-models ]]; then echo '{}'; fi",
            response
        ),
    );
}

fn choose_loaded_preference(app: &mut App, fixture: &Fixture, selection: &str) {
    for _ in 0..100 {
        let mut screen = if selection.is_empty() {
            FakeScreen::default()
        } else {
            FakeScreen::typing(&[selection])
        };
        app.handle(Key::Enter, &mut screen);
        if fixture
            .calls("cockpit")
            .iter()
            .any(|call| call.contains(" setting "))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("preference was never saved: {}", app.message);
}

/// The phone page as the column sees it: Tailscale names the computer, qrencode draws a 2×2 code,
/// and devices.json holds `phones`.
fn phone_page(fixture: &Fixture, phones: Value) {
    fixture.shim(
        "tailscale",
        r#"echo '{"Self": {"DNSName": "box.tail1234.ts.net."}}'"#,
    );
    fixture.shim("qrencode", "printf '##  \\n  ##\\n'");
    fixture.write(
        "state/claude-remote/devices.json",
        &json!({"devices": phones}).to_string(),
    );
}

fn phone(name: &str, hash: &str) -> Value {
    json!({"sha256": hash, "name": name, "paired_at": 1, "last_seen": util::now_secs()})
}

fn pairing_digits(app: &App) -> String {
    app.pairing.as_ref().unwrap().digits.clone()
}

fn open_settings_row(app: &mut App, wanted: SettingsRow) {
    app.handle(Key::Char('S'), &mut FakeScreen::default());
    for _ in 0..100 {
        let menu = app.settings_menu.as_ref().expect("settings are open");
        if menu.row(menu.selected) == wanted {
            return;
        }
        app.handle(Key::Down, &mut FakeScreen::default());
    }
    panic!("no settings row {wanted:?}");
}

/// Every line of one card (its item), joined.
fn card_text(app: &App, item: usize) -> String {
    app.panels[1]
        .lines
        .iter()
        .filter(|line| line.item == Some(item))
        .map(|line| line.texts())
        .collect::<Vec<_>>()
        .join("\n")
}

fn card_age(app: &App, item: usize) -> String {
    app.panels[1]
        .lines
        .iter()
        .find(|line| line.item == Some(item))
        .expect("a card")
        .right
        .iter()
        .map(|part| part.text.as_str())
        .collect()
}

/// A note `cockpit sleep` left for a session that exited at its prompt.
fn asleep_note(fixture: &Fixture, session: &str, idle_since: i64, slept_at: i64) {
    fixture.write(&format!("state/cockpit/asleep/{session}"), &json!({
        "agent": "claude", "project": "runway", "cwd": "/r/runway", "title": "Fix the chart",
        "idle_since": idle_since, "slept_at": slept_at, "remote": false, "started": idle_since - 100,
    }).to_string());
}

fn asleep_badge(app: &App) -> String {
    let card = app.panels[1]
        .lines
        .iter()
        .find(|line| line.item == Some(0))
        .expect("a card");
    card.right.iter().map(|part| part.text.as_str()).collect()
}

fn idle_rows(idle_s: &[(&str, i64)]) -> Value {
    let now = util::now_secs();
    json!(
        idle_s
            .iter()
            .enumerate()
            .map(|(index, (id, idle))| {
                let mut row = session(
                    id,
                    "waiting",
                    &format!("project-{index}"),
                    &format!("/r/project-{index}"),
                    index as i64 + 1,
                );
                row["since"] = json!(now - idle);
                row
            })
            .collect::<Vec<Value>>()
    )
}

/// A team daemon whose teams have no member: membership is known and keeps no one awake.
fn no_team_members(fixture: &Fixture) {
    let mut snapshot = team_snapshot();
    snapshot["members"] = json!([]);
    team_host(fixture, snapshot, vec![]);
}

/// One waiting session idle for `idle_s`, with a project of its own per id; team membership known.
fn sleep_fixture(idle_s: &[(&str, i64)]) -> (Fixture, App) {
    let fixture = Fixture::new();
    fixture.snapshot(idle_rows(idle_s));
    no_team_members(&fixture);
    let mut app = fixture.app();
    app.check_team();
    (fixture, app)
}

fn buffer_rows(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|row| {
            (0..buffer.area.width)
                .map(|column| buffer[(column, row)].symbol())
                .collect()
        })
        .collect()
}

// The session that asked for the apply is still in its turn when Ctrl+R comes (2026-10-19: the
// dotfiles session, twice): it must not be left on its old sandbox.
fn note_restart(fixture: &Fixture, session_id: &str, since: i64, project: &str, cwd: &str) {
    fixture.write(
        &format!("run/cockpit.restarting/{session_id}"),
        &format!("{since}\n{project}\n{cwd}\n%1\n"),
    );
}

/// The pid a stand-in wrote to `relative`, once it has.
fn pid_written_to(fixture: &Fixture, relative: &str) -> u32 {
    for _ in 0..250 {
        if let Some(pid) = fs::read_to_string(fixture.path(relative))
            .ok()
            .and_then(|text| text.trim().parse().ok())
        {
            return pid;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("nothing wrote its pid to {relative}");
}

/// A zombie keeps its /proc entry, in state Z, until its parent waits on it.
fn is_reaped(pid: u32) -> bool {
    for _ in 0..250 {
        if !Path::new(&format!("/proc/{pid}")).exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

// ---- The Team view: Theo's approvals, on the host-only team socket ----

type HostRequests = std::sync::Arc<Mutex<Vec<Value>>>;

const TEAM_PROJECT: &str = "remote:git@github.com:example/project-a.git";

/// A fake daemon on the host-only team socket: snapshots answer `snapshot`, controls take the
/// next of `replies` (success by default). Records every request.
fn team_host(fixture: &Fixture, snapshot: Value, replies: Vec<Value>) -> HostRequests {
    use std::io::{Read, Write};
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    crate::chief::store::migrate(&mut connection).unwrap();
    let requests = HostRequests::default();
    let recorded = requests.clone();
    let mut replies: VecDeque<Value> = replies.into();
    fixture.serve(move |mut stream| {
        let mut text = String::new();
        let _ = stream.read_to_string(&mut text);
        let request: Value = serde_json::from_str(&text).unwrap();
        let result = if request["request"] == "snapshot" {
            json!({"Ok": snapshot})
        } else if matches!(
            request["request"].as_str(),
            Some("reserve_task" | "reserve_resume")
        ) {
            let task = crate::teams::TaskKey {
                project: format!("path:{}", request["directory"].as_str().unwrap()),
                task_id: request["task_id"].as_str().unwrap_or("resume").into(),
            };
            json!(
                crate::chief::store::reserve_legacy(
                    &mut connection,
                    &task,
                    request["worker"].as_str(),
                    0
                )
                .map(|token| json!({"reservation": token}))
            )
        } else if request["request"] == "settle_reservation" {
            json!(crate::chief::store::settle_launch(
                &mut connection,
                request["token"].as_str().unwrap(),
                request["expected_revision"].as_i64().unwrap(),
                request["outcome"].as_str().unwrap()
            ))
        } else {
            replies
                .pop_front()
                .unwrap_or(json!({"Ok": {"status": "ok"}}))
        };
        recorded.lock().unwrap().push(request);
        let _ = stream.write_all(
            json!({"request_id": "x", "result": result})
                .to_string()
                .as_bytes(),
        );
    });
    requests
}

fn team_snapshot() -> Value {
    let mapping = json!([{"session_id": SESSION_A, "role": "designer", "agent": "claude", "runtime": "r1"},
        {"session_id": OPENCODE_SESSION, "role": "developer", "agent": "opencode", "runtime": "r2"}]).to_string();
    json!({"proposals": [{"proposal_id": "p1", "project": TEAM_PROJECT, "task_id": "1d3fd318", "mapping": mapping, "mapping_hash": "h1",
            "revision": 2, "status": "pending", "reason": "Review this task", "sender": OPENCODE_SESSION, "created_by": "agent", "parent_id": null}],
        "members": [{"session_id": SESSION_B, "role": "qa", "agent": "claude", "project": TEAM_PROJECT, "task_id": "0123abcd", "runtime": "r", "epoch": "e"}],
        "messages": [], "escalations": [],
        "sessions": [{"session_id": SESSION_A, "agent": "claude", "project": TEAM_PROJECT, "project_name": "project-a", "label": "", "state": "waiting"},
            {"session_id": OPENCODE_SESSION, "agent": "opencode", "project": TEAM_PROJECT, "project_name": "project-a", "label": "", "state": "waiting"},
            {"session_id": SESSION_B, "agent": "claude", "project": TEAM_PROJECT, "project_name": "project-a", "label": "", "state": "waiting"}]})
}

const SIDEBAR_PROJECT: &str = "path:/r/project-a";

fn sidebar_snapshot() -> Value {
    let mut state = team_snapshot();
    state["proposals"] = json!([]);
    state["members"] = json!([
        {"session_id":SESSION_A,"role":"designer","agent":"claude","project":SIDEBAR_PROJECT,"task_id":"1d3fd318","runtime":"r1","epoch":"e1"},
        {"session_id":OPENCODE_SESSION,"role":"developer","agent":"opencode","project":SIDEBAR_PROJECT,"task_id":"1d3fd318","runtime":"r2","epoch":"e2"}]);
    for session in state["sessions"].as_array_mut().unwrap() {
        session["project"] = json!(SIDEBAR_PROJECT);
        session["cwd"] = json!("/r/project-a");
    }
    state["handoffs"] = json!([]);
    state["qa_plans"] = json!([]);
    state["spawns"] = json!([]);
    state
}

fn sidebar_sessions(fixture: &Fixture) {
    let mut designer = session(SESSION_A, "waiting", "project-a", "/r/project-a", 1);
    designer["label"] = json!("Design session");
    let mut developer = session(OPENCODE_SESSION, "working", "project-a", "/r/project-a", 2);
    developer["label"] = json!("Build session");
    developer["agent"] = json!("opencode");
    let mut independent = session(SESSION_B, "waiting", "project-a", "/r/project-a", 3);
    independent["label"] = json!("Independent session");
    fixture.snapshot(json!([designer, developer, independent]));
}

fn sidebar_select(app: &mut App, text: &str) {
    app.panels[1].selected = app.panels[1]
        .lines
        .iter()
        .find(|line| line.item.is_some() && line.texts().contains(text))
        .unwrap_or_else(|| panic!("missing {text}: {:?}", texts(app, 1)))
        .item
        .unwrap();
}

fn sidebar_fold(app: &mut App) {
    sidebar_select(app, "› 1d3fd318");
    app.handle(Key::Enter, &mut FakeScreen::default());
    app.redraw();
}

fn open_team_view(app: &mut App) {
    {
        app.focus_panel(1);
        app.handle(Key::Char('k'), &mut FakeScreen::default());
    }
    assert!(app.team_view.is_some(), "{}", app.message);
}

fn controls(requests: &HostRequests) -> Vec<Value> {
    requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request["request"] == "control")
        .map(|request| request["action"].clone())
        .collect()
}

/// Real link authorization/lifecycle behind an isolated host socket; never a live daemon.
fn link_ui_host(
    fixture: &Fixture,
) -> (
    Vec<crate::teams::VerifiedSession>,
    crate::teams::TaskKey,
    Value,
) {
    use crate::teams::{LinkProposal, VerifiedSession, links, store};
    use std::io::{Read, Write};
    let first = fixture.path("first");
    let second = fixture.path("second");
    for project in [&first, &second] {
        fs::create_dir_all(project.join(".git")).unwrap();
    }
    let mut todos = TodoList::load();
    let task = crate::teams::TaskKey {
        project: format!("path:{}", first.display()),
        task_id: todos.add("first", first.to_str().unwrap(), "Shared work"),
    };
    let live = vec![
        VerifiedSession {
            session_id: SESSION_A.into(),
            agent: "claude".into(),
            project: task.project.clone(),
            runtime: "first-runtime".into(),
        },
        VerifiedSession {
            session_id: OPENCODE_SESSION.into(),
            agent: "opencode".into(),
            project: format!("path:{}", second.display()),
            runtime: "second-runtime".into(),
        },
    ];
    let mut connection = store::open(&crate::paths::team_database()).unwrap();
    let pending = links::propose(
        &mut connection,
        &live[0],
        &LinkProposal {
            submission_key: "ui-request".into(),
            task: task.clone(),
            peer_session_id: live[1].session_id.clone(),
            purpose: "Coordinate builds".into(),
        },
        &live,
        0,
    )
    .unwrap()
    .result
    .unwrap();
    let folder = fixture.path("run/claude-team-host");
    fs::create_dir_all(&folder).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(folder.join("host.sock")).unwrap();
    let verified = live.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut text = String::new();
            stream.read_to_string(&mut text).unwrap();
            let request: crate::teams::HostRequest = serde_json::from_str(&text).unwrap();
            let result = match request {
                crate::teams::HostRequest::Snapshot => {
                    store::snapshot(&connection, None, None, 100).map(|mut state| {
                        state["sessions"] = json!(verified);
                        state
                    })
                }
                crate::teams::HostRequest::Control { action } => {
                    store::control(&mut connection, &action, &verified, 100_000)
                        .and_then(|reply| reply.result)
                }
                _ => panic!("unexpected host request"),
            };
            stream
                .write_all(
                    serde_json::to_string(&crate::teams::TeamReply {
                        request_id: "ui".into(),
                        result,
                    })
                    .unwrap()
                    .as_bytes(),
                )
                .unwrap();
        }
    });
    (live, task, pending)
}

fn queue_link_note(live: &[crate::teams::VerifiedSession]) -> (String, String) {
    let mut connection = crate::teams::store::open(&crate::paths::team_database()).unwrap();
    let link_id: String = connection
        .query_row("SELECT id FROM links WHERE state='active'", [], |row| {
            row.get(0)
        })
        .unwrap();
    let receipt = crate::teams::links::accept(
        &mut connection,
        &live[0],
        &crate::teams::LinkSubmission {
            link_id: link_id.clone(),
            submission_key: "ui-note".into(),
            kind: crate::teams::MessageKind::Notice,
            body: "Coordinate shared work".into(),
            thread_id: None,
        },
        live,
        200_000,
    )
    .unwrap()
    .result
    .unwrap();
    (link_id, receipt["message_id"].as_str().unwrap().into())
}

/// An opencode database with the session_v2 columns H reads: (id, folder, title, parent, updated ms, archived).
type PastSessionRow<'a> = (&'a str, &'a str, &'a str, Option<&'a str>, i64, Option<i64>);

fn opencode_database_with(fixture: &Fixture, rows: &[PastSessionRow<'_>]) {
    let path = fixture.root.join("home/.local/share/opencode/opencode.db");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let database = rusqlite::Connection::open(&path).unwrap();
    database.execute_batch("create table session_v2 (id text primary key, directory text not null, title text, parent_id text,
                            time_updated integer not null, time_archived integer)").unwrap();
    for (id, directory, title, parent, updated, archived) in rows {
        database
            .execute(
                "insert into session_v2 values (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![id, directory, title, parent, updated, archived],
            )
            .unwrap();
    }
}

// ---- Status ----

fn app_with_billing(fixture: &Fixture, replies: &[(&str, Option<&str>, Option<f64>)]) -> App {
    let archive = costs::open_archive(&fixture.path("state/cockpit/usage.sqlite")).unwrap();
    let stamp = chrono::Local::now();
    for (key, billing, cost) in replies {
        archive.execute("insert into replies values (?, ?, ?, ?, 'shop', 'session', 0, 1000, 0, 0, 0, 'standard', ?)",
            rusqlite::params![key, stamp.date_naive().to_string(), stamp.to_rfc3339(), if cost.is_some() { "claude-opus-5-5" } else { "unpriced-model" }, cost]).unwrap();
        if let Some(billing) = billing {
            archive
                .execute(
                    "insert into reply_billing values (?, ?)",
                    rusqlite::params![key, billing],
                )
                .unwrap();
        }
    }
    drop(archive);
    let mut app = fixture.app();
    app.costs.tick();
    for _ in 0..200 {
        if !app.costs.is_loading() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!app.costs.is_loading(), "usage scan did not finish");
    app.build();
    app
}
fn status_rows(app: &mut App) -> Vec<String> {
    app.focus_panel(0);
    let buffer = render(app, 80, 45);
    (0..45)
        .map(|y| {
            (0..80)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

// ---- Costs ----

fn digest_with_chief(sessions: &[sessions::Session], chief: &costs::ChiefSection) -> String {
    let fixture = Fixture::new();
    let archive = costs::open_archive(&fixture.path("state/usage.sqlite")).unwrap();
    let digest = costs::digest_lines(
        &archive,
        sessions,
        &[],
        chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap(),
        chief,
    )
    .unwrap()
    .join("\n");
    regex::Regex::new(r"\x1b\[[0-9;]*m")
        .unwrap()
        .replace_all(&digest, "")
        .to_string()
}

fn local_ms(day: u32, hour: u32, minute: u32) -> i64 {
    use chrono::TimeZone;
    chrono::Local
        .with_ymd_and_hms(2026, 10, day, hour, minute, 0)
        .unwrap()
        .timestamp_millis()
}

fn digest_answer(
    session_id: &str,
    question: &str,
    answer: &str,
) -> crate::chief::store::DigestAnswer {
    crate::chief::store::DigestAnswer {
        session_id: session_id.into(),
        question: question.into(),
        answer: answer.into(),
    }
}

fn digest_habit(text: &str, project: Option<&str>) -> crate::chief::store::DigestHabit {
    crate::chief::store::DigestHabit {
        text: text.into(),
        project: project.map(str::to_string),
        created_at_ms: 0,
    }
}

fn digest_with_habits(
    answered: u32,
    habits: Vec<crate::chief::store::DigestHabit>,
    new_habits: u32,
) -> String {
    let lines = (0..answered)
        .map(|index| digest_answer("ses_abcdefghijkl", &format!("Question {index}?"), "Yes."))
        .collect();
    let digest = crate::chief::store::ChiefDigest {
        answered,
        escalated: 0,
        overruled: 0,
        more: 0,
        lines,
        habits,
        new_habits,
    };
    digest_with_chief(
        &[],
        &costs::ChiefSection::Answers {
            since_ms: local_ms(18, 8, 12),
            digest,
        },
    )
}

// ---- Pull requests ----

/// A session working in a checkout of acme/shop, its pull requests fetched once (gh answering none).
fn reviews_fetched_while_a_shop_session_works(fixture: &Fixture) -> App {
    let shop = fixture.path("repos/shop");
    fs::create_dir_all(&shop).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(&shop)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(&shop)
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/acme/shop.git",
        ])
        .status()
        .unwrap();
    fixture.shim("gh", "echo '[]'");
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "shop",
        &shop.display().to_string(),
        1
    )]));
    fixture.view(SESSION_A);
    let mut app = fixture.app();
    app.reviews.tick();
    wait_for_fetch(fixture, &app, 1);
    app
}

/// Waits until gh has listed pull requests `count` times and the fetch is over.
fn wait_for_fetch(fixture: &Fixture, app: &App, count: usize) -> usize {
    for _ in 0..150 {
        let listed = fixture
            .calls("gh")
            .iter()
            .filter(|call| call.starts_with("gh pr list"))
            .count();
        if listed >= count && !app.reviews.current().2 {
            return listed;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    fixture
        .calls("gh")
        .iter()
        .filter(|call| call.starts_with("gh pr list"))
        .count()
}

fn end_the_shop_sessions_turn(fixture: &Fixture) {
    // A new modification time: the column re-reads the snapshot only when it changes.
    std::thread::sleep(Duration::from_millis(20));
    let shop = fixture.path("repos/shop").display().to_string();
    fixture.snapshot(json!([session(SESSION_A, "waiting", "shop", &shop, 1)]));
}

/// gh answering one pull request for any repository, after `slow_seconds` for o/a.
fn shim_gh_slow_for_a(fixture: &Fixture, slow_seconds: &str) {
    let recent = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let pull = format!(
        r#"[{{"number": 3, "title": "Fix", "url": "u", "author": {{"login": "sam"}}, "updatedAt": "{recent}"}}]"#
    );
    fixture.shim("gh", &format!("case \"$*\" in\n  issue*) echo '[]' ;;\n  *o/a*) sleep {slow_seconds}; echo '{pull}' ;;\n  *) echo '{pull}' ;;\nesac"));
}

fn wait_for_rows(app: &App) -> Vec<Review> {
    for _ in 0..150 {
        let (rows, _, loading) = app.reviews.current();
        if !rows.is_empty() && !loading {
            return rows;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Vec::new()
}

// ---- Todo ----

fn type_in_app(app: &mut App, text: &str) {
    for character in text.chars() {
        app.handle(Key::Char(character), &mut FakeScreen::default());
    }
}

fn buffer_text(buffer: &Buffer) -> String {
    (0..buffer.area.height)
        .map(|row| {
            (0..buffer.area.width)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const CLIPBOARD_PNG: &str = r"case $1 in --list-types) echo text/plain; echo image/png ;; *) printf '\211PNG\r\n\032\nimage-bytes' ;; esac";

fn issue(number: i64, title: &str, labels: &[&str]) -> Issue {
    Issue {
        number,
        title: title.into(),
        url: String::new(),
        repository: "runway".into(),
        full_repository: "o/runway".into(),
        updated: format!("2026-10-1{number}T00:00:00Z"),
        labels: labels.iter().map(|l| l.to_string()).collect(),
    }
}

fn declare_role(fixture: &Fixture, session: &str, role: &str) {
    fixture.write(&format!("home/.cache/claude/designs/{session}.role"), role);
}

fn design_item(app: &mut App, project: &str, cwd: &str, text: &str) -> String {
    let id = app.todos.add(project, cwd, text);
    app.todos
        .items
        .iter_mut()
        .find(|item| item.id() == id)
        .unwrap()
        .0
        .insert("kind".into(), json!("design"));
    id
}

fn hand_over_first_item(app: &mut App) {
    hand_over_first_item_with(app, &["claude"]);
}

/// h on the first item, `typed` answering the agent pick if a new session starts.
fn hand_over_first_item_with(app: &mut App, typed: &[&str]) {
    app.build();
    app.focus_panel(3);
    app.panels[3].selected = 0;
    app.handle(Key::Char('h'), &mut FakeScreen::typing(typed));
}

/// A claude-todo update as the OSD daemon queues it: with the folder the sender's sandbox started in.
fn queue_update(update: Value, started_in: &Path) {
    let mut update = update.as_object().unwrap().clone();
    update.insert("started_in".into(), json!(started_in.display().to_string()));
    let path = crate::paths::todo_updates();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let previous = fs::read_to_string(&path).unwrap_or_default();
    fs::write(&path, format!("{previous}{}\n", Value::Object(update))).unwrap();
}

/// A project folder under the fixture, a repository.
fn project_folder(fixture: &Fixture, name: &str) -> PathBuf {
    let folder = fixture.path(name);
    fs::create_dir_all(folder.join(".git")).unwrap();
    fs::create_dir_all(folder.join("src")).unwrap();
    folder
}

// ---- Errors ----

fn reporter(fixture: &Fixture) -> ErrorReporter {
    fixture.write(
        "state/cockpit/log",
        "2026-10-18T10:00:00+02:00 ERROR manager: logged before the column started\n",
    );
    let mut reporter = ErrorReporter::new();
    reporter.gaming = || false;
    reporter
}

fn append_log(fixture: &Fixture, lines: &[&str]) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(fixture.path("state/cockpit/log"))
        .unwrap();
    for line in lines {
        writeln!(file, "{line}").unwrap();
    }
}

// ---- The log ----

// ---- Design sessions ----

const CANVAS: &str = "https://claude.ai/artifact/d31175a3-dc24-4166-aa81-2ccdf5efe83a";

// ---- The queue ----

fn queue_fixture(state: &str, idle_s: i64) -> (Fixture, App) {
    let fixture = Fixture::new();
    let since = util::now() as i64 - idle_s;
    fixture.snapshot(json!([session(
        SESSION_A,
        state,
        "runway",
        "/r/runway",
        since
    )]));
    let mut app = fixture.app();
    app.gaming = || false;
    (fixture, app)
}

fn prepare_subscription_hold() {
    use crate::subscription_fallback::{Binding, Observation, RoutingStore, Selection};
    use crate::usage::SubscriptionCapacity::{Available, Exhausted};
    let source = Binding {
        session: SESSION_A.into(),
        runtime: "runtime-source".into(),
        native_thread: None,
        selection: Selection {
            agent: "claude".into(),
            provider: "anthropic".into(),
            model: "claude-opus-4-7".into(),
            variant: Some("high".into()),
            account: "a".repeat(64),
        },
    };
    let alternate = Selection {
        agent: "codex".into(),
        provider: "openai".into(),
        model: "gpt-6.1-sol".into(),
        variant: Some("high".into()),
        account: "b".repeat(64),
    };
    let mut store = RoutingStore::load(&crate::paths::subscription_routes()).unwrap();
    store
        .track(source.clone(), "/r/runway", "1797cdd6", 100)
        .unwrap();
    store
        .reconcile(
            &[Observation {
                binding: source,
                session: sessions::Session::from_value(&session(
                    SESSION_A,
                    "waiting",
                    "runway",
                    "/r/runway",
                    1,
                ))
                .unwrap(),
                current_capacity: Exhausted,
                original_capacity: Exhausted,
                alternate: Some((alternate, Available)),
                has_pending_work: true,
                prompt_clear: true,
                manual_override: false,
            }],
            101,
        )
        .unwrap();
}

fn activate_subscription_replacement() -> &'static str {
    use crate::subscription_fallback::{Binding, Phase, RoutingStore};
    let mut routes = RoutingStore::load(&crate::paths::subscription_routes()).unwrap();
    let Phase::Pending(pending) = routes.route(SESSION_A).unwrap().phase.clone() else {
        unreachable!()
    };
    let replacement = "cdx_c214e8c0c9458f09ea4300974071bcad955995188f33264e341800f8a2bb5fcb";
    routes.mark_retired(SESSION_A, &pending.generation).unwrap();
    routes
        .mark_submitted(SESSION_A, &pending.generation)
        .unwrap();
    routes
        .record_runtime(
            &pending.generation,
            Binding {
                session: replacement.into(),
                runtime: "runtime-replacement".into(),
                selection: pending.target,
                native_thread: Some(SESSION_A.into()),
            },
            "/r/runway",
        )
        .unwrap();
    routes.finish(SESSION_A, &pending.generation).unwrap();
    routes
        .confirm_initial_admission(SESSION_A, &pending.generation)
        .unwrap();
    routes
        .start_initial_turn(SESSION_A, &pending.generation)
        .unwrap();
    drop(routes);
    replacement
}

/// A host that refuses the reservation of `refused` with `refusal` and grants every other one.
fn queue_host_refusing(fixture: &Fixture, refused: String, refusal: &'static str) -> HostRequests {
    use std::io::{Read, Write};
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    crate::chief::store::migrate(&mut connection).unwrap();
    let requests = HostRequests::default();
    let recorded = requests.clone();
    fixture.serve(move |mut stream| {
        let mut text = String::new();
        let _ = stream.read_to_string(&mut text);
        let request: Value = serde_json::from_str(&text).unwrap();
        let result = match request["request"].as_str() {
            Some("snapshot") => json!({"Ok": {"members": []}}),
            Some("reserve_task") if request["task_id"] == refused.as_str() => {
                json!({"Err": {"code": "conflict", "message": refusal}})
            }
            Some("reserve_task") => {
                let task = crate::teams::TaskKey {
                    project: format!("path:{}", request["directory"].as_str().unwrap()),
                    task_id: request["task_id"].as_str().unwrap().into(),
                };
                json!(
                    crate::chief::store::reserve_legacy(
                        &mut connection,
                        &task,
                        request["worker"].as_str(),
                        0
                    )
                    .map(|token| json!({"reservation": token}))
                )
            }
            Some("settle_reservation") => json!(crate::chief::store::settle_launch(
                &mut connection,
                request["token"].as_str().unwrap(),
                request["expected_revision"].as_i64().unwrap(),
                request["outcome"].as_str().unwrap()
            )),
            _ => json!({"Ok": {"recorded": true}}),
        };
        recorded.lock().unwrap().push(request);
        let _ = stream.write_all(
            json!({"request_id": "x", "result": result})
                .to_string()
                .as_bytes(),
        );
    });
    requests
}

/// An idle session and two queued items of its project, the first ahead of the second.
fn two_queued_items(fixture: &Fixture) -> (App, String, String) {
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "runway",
        "/r/runway",
        util::now_secs() - 600
    )]));
    let mut app = fixture.app();
    app.gaming = || false;
    let first = app
        .todos
        .add("runway", "/r/runway", "stuck at the head of the queue");
    app.todos.change_priority(&first, 1);
    app.todos.toggle_queued(&first);
    let second = app.todos.add("runway", "/r/runway", "next in the queue");
    app.todos.toggle_queued(&second);
    (app, first, second)
}

// ---- Notices, badges and the missing states (DESIGN.md "Round 3", frame 2081) ----

fn first_card_parts(app: &App) -> Vec<String> {
    let card = app.panels[1]
        .lines
        .iter()
        .find(|line| line.item == Some(0))
        .expect("a card");
    card.right.iter().map(|part| part.text.clone()).collect()
}

fn blocked_card(fixture: &Fixture) -> App {
    fixture.write("state/cockpit/todo.json", &json!({"items": [
        {"id": "1234abcd", "project": "opusline", "cwd": "/r/opusline", "text": "Resume deployment", "status": "todo", "session": SESSION_A, "after": ["abcd1234"]},
        {"id": "abcd1234", "project": "opusline", "cwd": "/r/opusline", "text": "Fix profile drift", "status": "todo"}
    ]}).to_string());
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "opusline",
        "/r/opusline",
        1
    )]));
    let mut app = fixture.app();
    app.focus_panel(1);
    app
}

fn first_hint_keys(app: &App, count: usize) -> Vec<&'static str> {
    app.sessions_hints()
        .into_iter()
        .take(count)
        .map(|(key, _)| key)
        .collect()
}

/// The Todo panel opens on Open: two tabs further is To do.
fn show_to_do_tab(app: &mut App) {
    app.focus_panel(3);
    for _ in 0..2 {
        app.handle(Key::Char(']'), &mut FakeScreen::default());
    }
}

fn report_blocked(fixture: &Fixture, project: &str, what: &str) {
    let reporter = fixture.root.join(project);
    fs::create_dir_all(&reporter).unwrap();
    queue_update(
        json!({"action": "blocked", "text": what, "why": "greywall hides ~/.ssh", "session": "s1"}),
        &reporter,
    );
}

/// A report from SESSION_A, which worked in `cwd` (running or since ended), filed from `started_in`.
fn report_blocked_by_session_a(
    fixture: &Fixture,
    started_in: &Path,
    cwd: &Path,
    is_running: bool,
) -> (TodoList, String) {
    fs::create_dir_all(started_in).unwrap();
    let cwd = cwd.display().to_string();
    if is_running {
        fixture.snapshot(json!([session(
            SESSION_A, "working", "opusline", &cwd, 100
        )]));
    } else {
        fixture.write(
            "state/cockpit/recent-sessions.json",
            &json!({SESSION_A: {"cwd": cwd, "project": "opusline", "last_seen": 100}}).to_string(),
        );
    }
    queue_update(
        json!({"action": "blocked", "text": "run the API tests", "why": "no Docker daemon", "session": SESSION_A}),
        started_in,
    );
    let mut todos = TodoList::load();
    todos.apply_updates();
    let id = todos
        .items
        .iter()
        .find(|item| item.is_blocked_report())
        .expect("no report")
        .id();
    (todos, id)
}

fn stack_checkout(fixture: &Fixture, repo: &str) -> PathBuf {
    let checkout = fixture.root.join("home/IdeaProjects").join(repo);
    fs::create_dir_all(checkout.join(".git")).unwrap();
    checkout
}

fn greywall_checkout(fixture: &Fixture) -> PathBuf {
    stack_checkout(fixture, "greywall")
}

fn report_to_stack(fixture: &Fixture, project: &str, repo: &str, update: Value) {
    let reporter = project_folder(fixture, project);
    let mut update = update.as_object().unwrap().clone();
    update.insert("action".into(), json!("stack"));
    update.insert("repo".into(), json!(repo));
    update
        .entry("why")
        .or_insert(json!("denyWrite re-exposes masked files"));
    update.insert("session".into(), json!("s1"));
    queue_update(Value::Object(update), &reporter);
}

fn report_to_greywall(fixture: &Fixture, project: &str, update: Value) {
    report_to_stack(fixture, project, "greywall", update);
}

fn greywall_items(todos: &TodoList) -> Vec<&Item> {
    todos
        .items
        .iter()
        .filter(|item| item.project() == "greywall")
        .collect()
}

fn project_checkout(fixture: &Fixture, name: &str) -> PathBuf {
    let checkout = fixture.root.join("home/IdeaProjects").join(name);
    fs::create_dir_all(checkout.join(".git")).unwrap();
    checkout
}

// ---- Todos handed between projects (claude-todo add --project) ----

/// greywall (where SESSION_A runs) and greyproxy, both checkouts in ~/IdeaProjects.
fn greywall_and_greyproxy(fixture: &Fixture) -> (PathBuf, PathBuf) {
    let greywall = project_checkout(fixture, "greywall");
    let greyproxy = project_checkout(fixture, "greyproxy");
    fixture.snapshot(json!([session(
        SESSION_A,
        "working",
        "greywall",
        &greywall.display().to_string(),
        100
    )]));
    (greywall, greyproxy)
}

fn file_in_greyproxy(greywall: &Path, title: &str, extra: Value) {
    let mut update =
        json!({"action": "add", "project": "greyproxy", "text": title, "session": SESSION_A});
    update
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    queue_update(update, greywall);
}

fn item_titled<'a>(todos: &'a TodoList, title: &str) -> &'a Item {
    todos
        .items
        .iter()
        .find(|item| item.text() == title)
        .unwrap_or_else(|| panic!("{title:?} not filed"))
}

/// A scheduler that refuses every automatic reservation and records each request.
fn refusing_scheduler(fixture: &Fixture) -> HostRequests {
    use std::io::{Read, Write};
    let requests = HostRequests::default();
    let recorded = requests.clone();
    fixture.serve(move |mut stream| {
        let mut text = String::new();
        let _ = stream.read_to_string(&mut text);
        let request: Value = serde_json::from_str(&text).unwrap();
        let result = if request["request"] == "reserve_task" {
            json!({"Err": {"code": "conflict", "message": "automatic legacy admission is no longer ready"}})
        } else { json!({"Ok": {"recorded": true}}) };
        recorded.lock().unwrap().push(request);
        let _ = stream.write_all(json!({"request_id": "x", "result": result}).to_string().as_bytes());
    });
    requests
}

fn reserve_requests(requests: &HostRequests) -> usize {
    requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request["request"] == "reserve_task")
        .count()
}

/// What the daemon records when the Chief asks for a session for `id` (chief::store::request_session_start).
fn chief_asks_for_a_session(id: &str, created_at_ms: i64) {
    let connection = crate::teams::store::open(&crate::paths::team_database()).unwrap();
    connection.execute("INSERT INTO chief_session_starts(id,epoch,project,task_id,reason,state,created_at) VALUES ('request-1','epoch','path:/p',?1,'Urgent.','requested',?2)",
        rusqlite::params![id, created_at_ms]).unwrap();
}

fn propose(fixture: &Fixture, title: &str, why: &str) {
    let _ = fs::remove_file(crate::paths::todo_updates());
    queue_update(
        json!({"action": "propose", "text": title, "why": why, "notes": "line one\nline two", "session": "s1"}),
        &fixture.root,
    );
}

fn ideas_fixture() -> (Fixture, App) {
    let fixture = Fixture::new();
    let homelab = project_checkout(&fixture, "homelab");
    let greywall = project_checkout(&fixture, "greywall");
    let cockpit = project_checkout(&fixture, "agent-cockpit");
    let mut reporter = session(
        SESSION_B,
        "waiting",
        "greywall",
        greywall.to_str().unwrap(),
        10,
    );
    reporter["label"] = json!("Check isolation");
    fixture.snapshot(json!([
        session(
            SESSION_A,
            "waiting",
            "homelab",
            homelab.to_str().unwrap(),
            1
        ),
        reporter
    ]));
    fixture.view(SESSION_A);
    queue_update(
        json!({"action": "propose", "text": "Show sandbox failures", "why": "Failures should be visible", "session": SESSION_B}),
        &greywall,
    );
    queue_update(
        json!({"action": "propose", "text": "Explain queued work", "why": "Show what happens next", "session": OPENCODE_SESSION}),
        &cockpit,
    );
    let mut app = fixture.app();
    assert_eq!(app.todos.apply_updates(), 2);
    app.todos
        .add("homelab", homelab.to_str().unwrap(), "Ordinary work");
    app.build();
    (fixture, app)
}

fn open_ideas(app: &mut App) {
    app.handle(Key::Char('3'), &mut FakeScreen::default());
    assert_eq!(app.focus, 4, "3 opens the global Ideas tab");
}

/// The open tab is lit (its label bold on the selection background) while the column has the focus.
fn blue_panel_corners(app: &mut App) -> usize {
    let buffer = render(app, 120, 40);
    let tabs: String = (0..120).map(|x| buffer[(x, 1)].symbol()).collect();
    let column = tabs.find("Sessions").unwrap() as u16;
    usize::from(
        buffer[(column, 1)]
            .modifier
            .contains(ratatui::style::Modifier::BOLD)
            && !buffer[(column, 1)]
                .modifier
                .contains(ratatui::style::Modifier::DIM),
    )
}

fn craft_badges(app: &mut App) -> Vec<String> {
    let mut badges = Vec::new();
    for index in 0..app.sessions.len() {
        app.panels[1].selected = index;
        render(app, 140, 45);
        badges.extend(
            selected_session_badges(app)
                .into_iter()
                .filter(|badge| badge.starts_with('◆')),
        );
    }
    badges
}

struct CraftProcess(std::process::Child);

impl CraftProcess {
    fn new(root: &str) -> Self {
        use std::io::Read;
        let mut child = std::process::Command::new("bash")
            .args([
                "-c",
                "echo; read -r -t 60",
                "figcraft",
                "--control",
                "17979",
                "--automation-read-root",
                root,
                "--automation-write-root",
                root,
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // spawn() returns before the kernel publishes /proc/<pid>/cmdline; the child's output proves it has.
        child
            .stdout
            .take()
            .unwrap()
            .read_exact(&mut [0u8])
            .expect("figcraft fixture exited before it was ready");
        Self(child)
    }

    fn id(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for CraftProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn want_restart(session: &str) {
    let markers = crate::paths::state_dir().join("restart-when-idle");
    fs::create_dir_all(&markers).unwrap();
    fs::write(markers.join(session), "").unwrap();
}

fn audit_day_file() -> PathBuf {
    crate::audit::dir().join(format!(
        "audit-{}.jsonl",
        chrono::Local::now().format("%Y-%m-%d")
    ))
}

fn record_three_events() {
    for event in ["run.held", "run.shown", "run.ran"] {
        crate::audit::record("test", event, "ok", json!({"id": "0123456789ab"}));
    }
}

/// A transcript of replies at these minutes past 10:00 (and seconds), in session `session` run in `cwd`.
fn replies_at(fixture: &Fixture, session: &str, cwd: &str, seconds: &[i64]) {
    let lines: Vec<String> = seconds.iter().enumerate().map(|(index, offset)| {
        let at = chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap().and_hms_opt(10, 0, 0).unwrap() + chrono::Duration::seconds(*offset);
        json!({"timestamp": format!("{}Z", at.format("%Y-%m-%dT%H:%M:%S")), "requestId": format!("{session}-r{index}"), "sessionId": session,
               "cwd": cwd, "message": {"id": format!("{session}-m{index}"), "model": "claude-opus-5-5", "usage": {"output_tokens": 10}}}).to_string()
    }).collect();
    fixture.write(
        &format!("home/.claude/projects/p/{session}.jsonl"),
        &(lines.join("\n") + "\n"),
    );
}

fn archived(fixture: &Fixture) -> rusqlite::Connection {
    let mut archive = costs::open_archive(&fixture.path("state/usage.sqlite")).unwrap();
    costs::archive_transcripts(&mut archive, &fixture.path("home/.claude/projects")).unwrap();
    archive
}

// ---- opencode sessions ----

fn opencode_session(state: &str, project: &str, started: i64) -> Value {
    let mut row = session(
        OPENCODE_SESSION,
        state,
        project,
        &format!("/r/{project}"),
        started,
    );
    row["agent"] = json!("opencode");
    row
}

fn type_text(app: &mut App, text: &str) {
    for character in text.chars() {
        app.handle(Key::Char(character), &mut FakeScreen::default());
    }
}

/// An item filed in a parallel clone that has since been deleted, handed off with `key`.
fn hand_off_from_a_deleted_clone(fixture: &Fixture, key: char) -> (App, String) {
    fixture.write(
        "state/cockpit/settings.json",
        r#"{"default-agent":"opencode"}"#,
    );
    let gone_clone = crate::paths::clones_dir().join("runway/todo-0123abcd/apps");
    let mut app = fixture.app();
    let healthcheck = app.todos.add(
        "runway",
        &gone_clone.display().to_string(),
        "add a healthcheck",
    );
    app.build();
    app.focus_panel(3);
    app.panels[3].selected = 0;
    app.handle(Key::Char(key), &mut FakeScreen::default());
    (app, healthcheck)
}

// ---- Chief answers on the session cards, in the details and behind c ----

fn chief_answer(minutes_ago: i64, question: &str, answer: &str) -> Value {
    json!({"created_at_ms": (util::now_secs() - minutes_ago * 60) * 1000, "question": question, "answer": answer, "delivery": "received"})
}

/// A team host whose snapshot carries `chief_answers` for the given sessions: (session, count, recent).
fn chief_answers_host(fixture: &Fixture, counts: Vec<(&str, u32, Vec<Value>)>) {
    let mut snapshot = team_snapshot();
    snapshot["members"] = json!([]);
    snapshot["chief_answers"] = json!(counts.into_iter().map(|(session, count, recent)| json!({"session_id": session, "count": count, "recent": recent})).collect::<Vec<_>>());
    team_host(fixture, snapshot, vec![]);
}

fn card_of(app: &App, session: &str) -> crate::app::Line {
    let index = app.sidebar_rows.iter().position(|row| matches!(row, crate::teams::sidebar::SidebarRow::Session { session_id } if session_id == session)).unwrap();
    app.panels[1]
        .lines
        .iter()
        .find(|line| line.item == Some(index))
        .cloned()
        .unwrap()
}

fn open_details(app: &mut App) -> String {
    render(app, 100, 50);
    app.handle(Key::Char('i'), &mut FakeScreen::default());
    buffer_rows(&render(app, 100, 50)).join("\n")
}

// ---- Images from the latest reply (I) ----

const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nfirst";

/// A session in `work/` whose transcript holds an older reply, a question, then `latest` (split
/// in two records around a tool call, as Claude writes a turn).
fn session_with_latest_reply(fixture: &Fixture, latest: &str) -> PathBuf {
    let folder = fixture.path("work");
    fs::create_dir_all(folder.join("previews")).unwrap();
    fixture.snapshot(json!([session(
        SESSION_A,
        "waiting",
        "work",
        folder.to_str().unwrap(),
        1
    )]));
    let reply = |uuid: &str, text: &str| {
        json!({"type": "assistant", "uuid": uuid, "message": {"content": [{"type": "text", "text": text}]}}).to_string()
    };
    let lines = [
        reply("r1", "Older:\n![Old](previews/old.png)"),
        json!({"type": "user", "uuid": "u1", "message": {"content": "show me"}}).to_string(),
        reply("r2", "First part"),
        json!({"type": "user", "uuid": "t1", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}).to_string(),
        reply("r3", latest),
    ];
    fixture.write(
        &format!("home/.claude/projects/p/{SESSION_A}.jsonl"),
        &(lines.join("\n") + "\n"),
    );
    folder
}

fn viewer_calls(fixture: &Fixture) -> Vec<String> {
    fixture
        .calls("hyprctl")
        .into_iter()
        .filter(|call| call.contains("loupe"))
        .collect()
}

/// Apart from the designed highlights in `highlights`, the cleared cells and the text cells of the
/// area share one background: no boxes behind the text.
fn assert_one_background(
    buffer: &Buffer,
    rows: std::ops::Range<u16>,
    columns: std::ops::Range<u16>,
    highlights: &[ratatui::style::Color],
) {
    let (mut text, mut blank) = (
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::new(),
    );
    for row in rows {
        for column in columns.clone() {
            let cell = &buffer[(column, row)];
            if highlights.contains(&cell.bg) {
                continue;
            }
            if cell.symbol().trim().is_empty() {
                blank.insert(format!("{:?}", cell.bg));
            } else {
                text.insert(format!("{:?}", cell.bg));
            }
        }
    }
    assert_eq!(text.len(), 1, "text backgrounds: {text:?}");
    assert_eq!(text, blank, "text and cleared cells differ in background");
}

const DESIGNED_HIGHLIGHT: ratatui::style::Color =
    ratatui::style::Color::Indexed(crate::app::BG_HIGHLIGHT);

// ---- Key map (DESIGN.md "Key map", todo 86822093) ----

// ---- Session details, frame 2200 (todo 501023cd) ----

fn details_with_costs(fixture: &Fixture, costs: &[(&str, f64)]) -> App {
    let archive = costs::open_archive(&fixture.path("state/cockpit/usage.sqlite")).unwrap();
    let stamp = chrono::Local::now();
    for (index, (session, cost)) in costs.iter().enumerate() {
        archive.execute("insert into replies values (?, ?, ?, 'claude-opus-5-5', 'runway', ?, 0, 1000, 0, 0, 0, 'standard', ?)",
            rusqlite::params![format!("reply-{index}"), stamp.date_naive().to_string(), stamp.to_rfc3339(), session, cost]).unwrap();
    }
    drop(archive);
    let mut app = fixture.app();
    app.costs.tick();
    for _ in 0..200 {
        if !app.costs.is_loading() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!app.costs.is_loading(), "usage scan did not finish");
    app.build();
    app
}
