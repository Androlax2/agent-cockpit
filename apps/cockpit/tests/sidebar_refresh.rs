//! Exercise idle scheduling through the real terminal entry point.
mod common;

use common::sandbox::write_executable;
use common::{Terminal, contains};
use std::os::{fd::AsRawFd, unix::fs::PermissionsExt};
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Sidebar {
    terminal: Terminal,
    root: PathBuf,
}

impl Sidebar {
    fn new() -> Self {
        Self::with_status("")
    }

    fn with_status(status_url: &str) -> Self {
        Self::configured(status_url, |_| {})
    }

    fn configured(status_url: &str, setup: impl FnOnce(&Path)) -> Self {
        let root =
            std::env::temp_dir().join(format!("sidebar-refresh-{}", cockpit::util::token_hex(6)));
        for directory in ["home", "run", "state", "bin"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        for name in ["cockpit", "hyprctl", "pgrep", "git", "gh"] {
            write_executable(&root.join("bin").join(name), "#!/bin/sh\nexit 0\n");
        }
        write_executable(
            &root.join("bin/tmux"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
                root.join("tmux.calls").display()
            ),
        );
        write_executable(
            &root.join("bin/cockpit"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
                root.join("cockpit.calls").display()
            ),
        );
        fs::write(root.join("run/cockpit.json"), "{\"sessions\":[]}").unwrap();
        setup(&root);
        let terminal = Terminal::start(
            Path::new(env!("CARGO_BIN_EXE_cockpit-sidebar")),
            &[],
            &[
                ("HOME", &root.join("home")),
                ("XDG_RUNTIME_DIR", &root.join("run")),
                ("COCKPIT_STATE_HOME", &root.join("state")),
                ("XDG_STATE_HOME", &root.join("state")),
                ("PATH", &root.join("bin")),
                ("CLAUDE_STATUS_URL", Path::new(status_url)),
                ("CODEX_STATUS_URL", Path::new("")),
                ("CLAUDE_AUDIT_JOURNAL", Path::new("off")),
                ("COCKPIT_OPENCODE_DB", &root.join("missing.db")),
                ("COCKPIT_DISK_MOUNTS", Path::new("")),
            ],
        );
        Self { root, terminal }
    }

    fn gathers(&self) -> usize {
        fs::read_to_string(self.root.join("tmux.calls"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("list-panes "))
            .count()
    }

    fn atomic_snapshot(&self, project: &str) {
        let snapshot = serde_json::json!({"sessions": [{"session": "aaaaaaaa-0000-4000-8000-000000000000",
            "state": "working", "project": project, "cwd": self.root, "since": 1, "started": 1, "tmux_pane": "%1"}]});
        fs::write(self.root.join("run/snapshot.tmp"), snapshot.to_string()).unwrap();
        fs::rename(
            self.root.join("run/snapshot.tmp"),
            self.root.join("run/cockpit.json"),
        )
        .unwrap();
    }
}

impl Drop for Sidebar {
    fn drop(&mut self) {
        let _ = self.terminal.child.kill();
        let _ = self.terminal.child.wait();
        let _ = fs::set_permissions(
            self.root.join("home/.cache/claude"),
            fs::Permissions::from_mode(0o700),
        );
        fs::remove_dir_all(&self.root).unwrap();
    }
}

struct TeamHost {
    paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
    has_focus_error: std::sync::Arc<std::sync::atomic::AtomicBool>,
    requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    snapshots: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[test]
fn a_stalled_team_snapshot_does_not_block_arrow_navigation() {
    let mut host = None;
    let mut sidebar = Sidebar::configured("", |root| {
        host = Some(TeamHost::start(root, "1111aaaa", true));
        let sessions: Vec<_> = [
            ("aaaaaaaa-0000-4000-8000-000000000000", "first", 1),
            ("bbbbbbbb-0000-4000-8000-000000000000", "second", 2),
        ]
        .into_iter()
        .map(|(session, directory, started)| {
            serde_json::json!({
                "session": session, "state": "working", "project": "navigation",
                "cwd": root.join(directory), "label": directory,
                "since": started, "started": started, "tmux_pane": "%1"
            })
        })
        .collect();
        fs::write(
            root.join("run/cockpit.json"),
            serde_json::json!({"sessions": sessions}).to_string(),
        )
        .unwrap();
    });
    let host = host.unwrap();
    wait_for_team_request(&mut sidebar, &host);
    sidebar.terminal.screen.clear();
    sidebar.terminal.type_keys(b"\x1b[Bi");
    sidebar.terminal.pump(0.6, Some(b"WHERE & WHO"));
    assert!(
        contains(&sidebar.terminal.screen, b"WHERE & WHO"),
        "Down/details input waited for team IPC"
    );
    assert!(
        contains(&sidebar.terminal.screen, b"/second"),
        "Down did not select the second session"
    );
    sidebar.terminal.type_keys(b"\x1b");
    sidebar.terminal.pump(0.1, None);
    sidebar.terminal.screen.clear();
    sidebar.terminal.type_keys(b"\x1b[Ai");
    sidebar.terminal.pump(0.6, Some(b"WHERE & WHO"));
    assert!(
        contains(&sidebar.terminal.screen, b"/first"),
        "Up/details input waited for team IPC"
    );
    assert_eq!(
        host.snapshots(),
        1,
        "navigation started extra snapshot work"
    );
}

impl TeamHost {
    fn start(root: &Path, task_id: &str, is_paused: bool) -> Self {
        use std::{
            io::{Read, Write},
            os::unix::net::UnixListener,
            sync::{
                Arc,
                atomic::{AtomicBool, AtomicUsize, Ordering},
            },
            time::Duration,
        };
        let folder = root.join("run/claude-team-host");
        fs::create_dir_all(&folder).unwrap();
        let listener = UnixListener::bind(folder.join("host.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let paused = Arc::new(AtomicBool::new(is_paused));
        let has_focus_error = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let snapshots = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_paused = paused.clone();
        let worker_focus_error = has_focus_error.clone();
        let worker_requests = requests.clone();
        let worker_snapshots = snapshots.clone();
        let worker_stopped = stopped.clone();
        let snapshot = serde_json::json!({
            "members": [], "sessions": [], "messages": [], "escalations": [],
            "spawns": [], "sleeps": [], "handoffs": [], "qa_plans": [],
            "chief_session": null, "chief_badge": {}, "chief_answers": [],
            "proposals": [{"proposal_id": "fixture", "project": "path:/r/project-a",
                "task_id": task_id, "status": "pending", "reason": "Review team links",
                "mapping": "[]", "mapping_hash": "fixture", "revision": 1,
                "created_by": "agent", "parent_id": null}]
        });
        let worker = std::thread::spawn(move || {
            while !worker_stopped.load(Ordering::Acquire) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_millis(100)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_millis(100)))
                    .unwrap();
                let mut wire = String::new();
                if stream.read_to_string(&mut wire).is_err() {
                    continue;
                }
                let request: serde_json::Value = serde_json::from_str(&wire).unwrap();
                assert!(
                    matches!(
                        request["request"].as_str(),
                        Some("snapshot" | "chief_focus")
                    ),
                    "unexpected background request: {request}"
                );
                worker_requests.fetch_add(1, Ordering::Release);
                if request["request"] == "snapshot" {
                    worker_snapshots.fetch_add(1, Ordering::Release);
                }
                while worker_paused.load(Ordering::Acquire) {
                    if worker_stopped.load(Ordering::Acquire) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                let reply = cockpit::teams::TeamReply {
                    request_id: "snapshot".into(),
                    result: if request["request"] == "chief_focus"
                        && worker_focus_error.load(Ordering::Acquire)
                    {
                        Err(cockpit::teams::TeamError::new(
                            cockpit::teams::ErrorCode::Unavailable,
                            "fixture focus failure",
                        ))
                    } else {
                        Ok(if request["request"] == "snapshot" {
                            snapshot.clone()
                        } else {
                            serde_json::json!({"recorded": true})
                        })
                    },
                };
                let _ = stream.write_all(serde_json::to_string(&reply).unwrap().as_bytes());
            }
        });
        Self {
            paused,
            has_focus_error,
            requests,
            snapshots,
            stopped,
            worker: Some(worker),
        }
    }

    fn snapshots(&self) -> usize {
        self.snapshots.load(std::sync::atomic::Ordering::Acquire)
    }

    fn release(&self) {
        self.paused
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

impl Drop for TeamHost {
    fn drop(&mut self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn sidebar_with_team_host(task_id: &str, paused: bool) -> (Sidebar, TeamHost) {
    let mut host = None;
    let sidebar = Sidebar::configured("", |root| {
        host = Some(TeamHost::start(root, task_id, paused))
    });
    (sidebar, host.unwrap())
}

fn wait_for_team_request(sidebar: &mut Sidebar, host: &TeamHost) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while host.snapshots() == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "sidebar never requested team state"
        );
        sidebar.terminal.pump(0.02, None);
    }
}

#[test]
fn a_stalled_team_snapshot_does_not_block_sidebar_help() {
    let (mut sidebar, host) = sidebar_with_team_host("1111aaaa", true);
    wait_for_team_request(&mut sidebar, &host);
    sidebar.terminal.screen.clear();
    sidebar.terminal.type_keys(b"?");
    sidebar.terminal.pump(0.6, Some("Help ·".as_bytes()));
    assert!(
        contains(&sidebar.terminal.screen, "Help ·".as_bytes()),
        "team IPC blocked keyboard input"
    );
}

#[test]
fn a_stalled_focus_report_does_not_block_sidebar_help() {
    let mut host = None;
    let mut sidebar = Sidebar::configured("", |root| {
        write_executable(
            &root.join("bin/hyprctl"),
            "#!/bin/sh\nprintf '%s\\n' '{\"id\":11}'\n",
        );
        host = Some(TeamHost::start(root, "1111aaaa", true));
    });
    let host = host.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while host.requests.load(std::sync::atomic::Ordering::Acquire) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "sidebar never reported focus"
        );
        sidebar.terminal.pump(0.02, None);
    }
    sidebar.terminal.screen.clear();
    sidebar.terminal.type_keys(b"?");
    sidebar.terminal.pump(0.6, Some("Help ·".as_bytes()));
    assert!(
        contains(&sidebar.terminal.screen, "Help ·".as_bytes()),
        "focus IPC blocked keyboard input"
    );
}

#[test]
fn a_failed_focus_report_does_not_suppress_the_team_snapshot() {
    let mut host = None;
    let mut sidebar = Sidebar::configured("", |root| {
        write_executable(
            &root.join("bin/hyprctl"),
            "#!/bin/sh\nprintf '%s\\n' '{\"id\":11}'\n",
        );
        let team_host = TeamHost::start(root, "1111aaaa", false);
        team_host
            .has_focus_error
            .store(true, std::sync::atomic::Ordering::Release);
        host = Some(team_host);
    });
    let host = host.unwrap();
    sidebar.terminal.type_keys(b"4");
    sidebar.terminal.pump(1.5, Some(b"1111aaaa"));
    assert!(
        contains(&sidebar.terminal.screen, b"1111aaaa"),
        "focus failure suppressed independent team data"
    );
    assert_eq!(
        host.snapshots(),
        1,
        "focus failure prevented the snapshot request"
    );
}

#[test]
fn an_idle_sidebar_shares_one_team_snapshot_between_maintenance_ticks() {
    let (mut sidebar, host) = sidebar_with_team_host("1111aaaa", false);
    wait_for_team_request(&mut sidebar, &host);
    sidebar.terminal.pump(3.5, None);
    assert_eq!(
        host.snapshots(),
        1,
        "maintenance multiplied background snapshot requests"
    );
}

#[test]
fn a_replaced_team_socket_cannot_publish_its_old_snapshot() {
    let (mut sidebar, old_host) = sidebar_with_team_host("1111aaaa", true);
    wait_for_team_request(&mut sidebar, &old_host);
    fs::remove_file(sidebar.root.join("run/claude-team-host/host.sock")).unwrap();
    let new_host = TeamHost::start(&sidebar.root, "2222bbbb", false);
    old_host.release();
    sidebar.terminal.type_keys(b"4");
    sidebar.terminal.pump(1.5, Some(b"2222bbbb"));
    assert!(
        !contains(&sidebar.terminal.screen, b"1111aaaa"),
        "obsolete endpoint result reached the screen"
    );
    assert!(
        contains(&sidebar.terminal.screen, b"2222bbbb"),
        "replacement endpoint was not refreshed"
    );
    assert_eq!(
        new_host.snapshots(),
        1,
        "replacement fetch was not coalesced"
    );
}

#[test]
fn an_idle_sidebar_does_not_gather_sessions_four_times_per_second() {
    let mut sidebar = Sidebar::new();
    sidebar.terminal.pump(1.0, None);
    assert!(
        contains(&sidebar.terminal.screen, b"Todo"),
        "sidebar did not start"
    );
    let before = sidebar.gathers();
    assert!(before > 0, "initial session gather never ran");
    sidebar.terminal.pump(2.0, None);
    assert!(
        sidebar.gathers() - before <= 1,
        "idle sidebar still polls tmux: {} gathers",
        sidebar.gathers() - before
    );
}

#[test]
fn unopened_chief_controls_do_not_add_host_snapshot_polling() {
    use std::{
        io::{Read, Write},
        os::unix::net::UnixListener,
        sync::{Arc, Mutex},
    };
    let requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let captured = requests.clone();
    let mut sidebar = Sidebar::configured("", move |root| {
        let folder = root.join("run/claude-team-host");
        fs::create_dir_all(&folder).unwrap();
        let listener = UnixListener::bind(folder.join("host.sock")).unwrap();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut wire = String::new();
                if stream.read_to_string(&mut wire).is_err() {
                    continue;
                }
                let request: serde_json::Value = serde_json::from_str(&wire).unwrap();
                captured.lock().unwrap().push(request);
                let reply = cockpit::teams::TeamReply {
                    request_id: "fixture".into(),
                    result: Ok(serde_json::json!({"members":[]})),
                };
                let _ = stream.write_all(serde_json::to_string(&reply).unwrap().as_bytes());
            }
        });
    });
    sidebar.terminal.pump(1.0, None);
    sidebar.terminal.pump(1.0, None);
    let requests = requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|request| request["request"] == "snapshot")
    );
    assert!(
        !requests
            .iter()
            .any(|request| request["request"] == "chief_snapshot" || request["request"] == "chief")
    );
}

#[test]
fn a_background_provider_result_appears_before_the_next_maintenance_tick() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (send, receive) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        assert!(
            stream.read(&mut request).unwrap() > 0,
            "client closed before sending a request"
        );
        receive.recv().unwrap();
        let body = r#"{"status":{"indicator":"minor","description":"Worker-ready"}}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let mut sidebar = Sidebar::with_status(&url);
    sidebar.terminal.pump(1.2, None);
    send.send(()).unwrap();
    server.join().unwrap();
    sidebar.terminal.pump(0.4, Some(b"Worker-ready"));
    assert!(
        contains(&sidebar.terminal.screen, b"Worker-ready"),
        "background result waited for maintenance"
    );
}

#[test]
fn atomic_snapshot_replacements_update_the_real_sidebar_promptly() {
    let mut sidebar = Sidebar::new();
    sidebar.terminal.pump(1.0, None);
    // Names that share no prefix: the terminal redraws only the cells that change.
    for project in ["First-watch", "Second-watch"] {
        sidebar.terminal.screen.clear();
        sidebar.atomic_snapshot(project);
        sidebar.terminal.pump(0.6, Some(project.as_bytes()));
        assert!(
            contains(&sidebar.terminal.screen, project.as_bytes()),
            "snapshot change missed: {project}"
        );
    }
}

#[test]
fn changing_the_viewed_session_bypasses_ordinary_refresh_coalescing() {
    const FIRST: &str = "aaaaaaaa-0000-4000-8000-000000000000";
    const SECOND: &str = "bbbbbbbb-0000-4000-8000-000000000000";
    let mut sidebar = Sidebar::new();
    sidebar.terminal.pump(1.0, None);
    let sessions: Vec<_> = [FIRST, SECOND].into_iter().map(|session| serde_json::json!({
        "session": session, "state": "working", "project": "test-project", "cwd": sidebar.root,
        "since": 1, "started": 1, "tmux_pane": "%1"
    })).collect();
    fs::write(
        sidebar.root.join("run/cockpit.json"),
        serde_json::json!({"sessions": sessions}).to_string(),
    )
    .unwrap();
    sidebar.terminal.pump(0.5, None);
    for session in [FIRST, SECOND] {
        fs::write(sidebar.root.join("run/view.tmp"), session).unwrap();
        fs::rename(
            sidebar.root.join("run/view.tmp"),
            sidebar.root.join("run/cockpit.current"),
        )
        .unwrap();
        sidebar.terminal.pump(0.18, None);
        let calls = fs::read_to_string(sidebar.root.join("cockpit.calls")).unwrap();
        let last = calls
            .lines()
            .rev()
            .find(|line| line.starts_with("label-state "));
        assert_eq!(
            last,
            Some(format!("label-state working {session}").as_str()),
            "viewed session lagged behind"
        );
    }
}

#[test]
fn writing_the_sidebar_log_does_not_trigger_full_refreshes() {
    let mut sidebar = Sidebar::new();
    sidebar.terminal.pump(1.0, None);
    let before = sidebar.gathers();
    for _ in 0..8 {
        fs::write(
            sidebar.root.join("state/cockpit/log"),
            "unrelated log update",
        )
        .unwrap();
        sidebar.terminal.pump(0.1, None);
    }
    assert_eq!(
        sidebar.gathers(),
        before,
        "log writes caused a refresh loop"
    );
}

#[test]
fn hook_logs_and_approval_files_do_not_trigger_full_refreshes() {
    let mut sidebar = Sidebar::configured("", |root| {
        fs::create_dir_all(root.join("home/.cache/claude")).unwrap();
    });
    sidebar.terminal.pump(1.0, None);
    let before = sidebar.gathers();
    fs::create_dir(sidebar.root.join("home/.cache/claude/requests")).unwrap();
    for _ in 0..8 {
        fs::write(
            sidebar.root.join("home/.cache/claude/hook.log"),
            "unrelated hook log",
        )
        .unwrap();
        fs::write(
            sidebar
                .root
                .join("home/.cache/claude/requests/approval.json"),
            "{}",
        )
        .unwrap();
        sidebar.terminal.pump(0.1, None);
    }
    assert_eq!(
        sidebar.gathers(),
        before,
        "unrelated cache writes caused a refresh loop"
    );
}

#[test]
fn resize_and_keyboard_input_do_not_wait_for_the_safety_refresh() {
    let mut sidebar = Sidebar::new();
    sidebar.terminal.pump(1.0, None);
    sidebar.terminal.screen.clear();
    common::set_size(sidebar.terminal.master.as_raw_fd(), 30, 90);
    sidebar.terminal.pump(0.4, None);
    assert!(!sidebar.terminal.screen.is_empty(), "resize did not redraw");
    sidebar.terminal.screen.clear();
    sidebar.terminal.type_keys(b"?");
    sidebar.terminal.pump(0.4, None);
    assert!(
        !sidebar.terminal.screen.is_empty(),
        "keyboard input did not redraw"
    );
}

#[test]
fn the_safety_refresh_discovers_changes_that_have_no_file_notification() {
    let mut sidebar = Sidebar::new();
    sidebar.terminal.pump(1.0, None);
    let before = sidebar.gathers();
    sidebar.terminal.pump(5.0, None);
    assert_eq!(
        sidebar.gathers() - before,
        1,
        "safety refresh should gather once, not continuously"
    );
}

#[test]
fn watch_failure_falls_back_to_polling_then_recovers() {
    let mut sidebar = Sidebar::configured("", |root| {
        let cache = root.join("home/.cache/claude");
        fs::create_dir_all(&cache).unwrap();
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o000)).unwrap();
    });
    sidebar.terminal.pump(1.0, None);
    let before = sidebar.gathers();
    sidebar.terminal.pump(1.0, None);
    assert!(
        sidebar.gathers() - before >= 2,
        "unwatched sidebar stopped polling"
    );
    fs::set_permissions(
        sidebar.root.join("home/.cache/claude"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    sidebar.terminal.pump(4.0, None);
    let before = sidebar.gathers();
    sidebar.terminal.pump(1.0, None);
    assert_eq!(
        sidebar.gathers(),
        before,
        "polling did not stop after watch recovery"
    );
    let log = fs::read_to_string(sidebar.root.join("state/cockpit/log")).unwrap();
    assert!(
        log.contains("using 250 ms polling") && log.contains("input watch restored"),
        "watch failure was silent: {log}"
    );
}

#[test]
fn slow_gathers_do_not_run_back_to_back_when_reports_keep_arriving() {
    let mut sidebar = Sidebar::configured("", |root| {
        write_executable(
            &root.join("bin/tmux"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n/usr/bin/sleep 0.4\nprintf 'done\\n' >> '{}'\n",
                root.join("tmux.calls").display(),
                root.join("tmux.done").display()
            ),
        );
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !sidebar.root.join("tmux.done").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "first gather did not complete"
        );
        sidebar.terminal.pump(0.01, None);
    }
    let before = sidebar.gathers();
    for _ in 0..5 {
        fs::write(sidebar.root.join("run/cockpit.json"), "{\"sessions\":[]}").unwrap();
        sidebar.terminal.pump(0.03, None);
    }
    assert_eq!(
        sidebar.gathers(),
        before,
        "a slow gather consumed its own cooldown"
    );
}

#[test]
fn usage_failures_and_their_removal_refresh_quota_data_promptly() {
    let mut sidebar = Sidebar::new();
    sidebar.terminal.pump(1.0, None);
    for filename in ["claude-usage-error.json", "codex-usage-error.json"] {
        let path = sidebar.root.join("run").join(filename);
        let before = sidebar.gathers();
        fs::write(&path, "{}").unwrap();
        sidebar.terminal.pump(0.5, None);
        assert!(
            sidebar.gathers() > before,
            "failure cache was not watched: {filename}"
        );
        let before = sidebar.gathers();
        fs::remove_file(path).unwrap();
        sidebar.terminal.pump(0.5, None);
        assert!(
            sidebar.gathers() > before,
            "failure recovery was not watched: {filename}"
        );
    }
}

#[test]
fn switching_sessions_stays_prompt_even_when_watching_has_failed() {
    let mut sidebar = Sidebar::configured("", |root| {
        let cache = root.join("home/.cache/claude");
        fs::create_dir_all(&cache).unwrap();
        fs::set_permissions(cache, fs::Permissions::from_mode(0o000)).unwrap();
    });
    const FIRST: &str = "aaaaaaaa-0000-4000-8000-000000000000";
    const SECOND: &str = "bbbbbbbb-0000-4000-8000-000000000000";
    let sessions: Vec<_> = [FIRST, SECOND].into_iter().map(|session| serde_json::json!({
        "session": session, "state": "working", "project": "test-project", "cwd": sidebar.root,
        "since": 1, "started": 1, "tmux_pane": "%1"
    })).collect();
    fs::write(
        sidebar.root.join("run/cockpit.json"),
        serde_json::json!({"sessions": sessions}).to_string(),
    )
    .unwrap();
    fs::write(sidebar.root.join("run/cockpit.current"), FIRST).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let calls = fs::read_to_string(sidebar.root.join("cockpit.calls")).unwrap_or_default();
        if calls.contains(&format!("label-state working {FIRST}")) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "initial scope was not shown"
        );
        sidebar.terminal.pump(0.01, None);
    }
    fs::write(sidebar.root.join("run/cockpit.current"), SECOND).unwrap();
    sidebar.terminal.pump(0.15, None);
    let calls = fs::read_to_string(sidebar.root.join("cockpit.calls")).unwrap();
    assert!(
        calls.contains(&format!("label-state working {SECOND}")),
        "fallback slowed session switching"
    );
}
