//! Conformance against an installed Codex. Ignored by default because CI has no Codex; run
//! it with the binary of `CONFORMANCE_VERSION`:
//!
//! ```text
//! COCKPIT_CODEX_PROGRAM=~/.local/npm/bin/codex \
//!   cargo nextest run -p cockpit-codex --test conformance --run-ignored only -j 1
//! ```
//!
//! Each test runs Codex with a fresh `CODEX_HOME` holding a placeholder API key, so the
//! owner's configuration, trust and login are never read, and every model call fails as
//! `unauthorized` after Codex's retries: the hooks run before it, and the retries leave a
//! turn under way for the delivery tests. The session launcher's test needs `tmux`. One test
//! at a time (`-j 1`): several Codex starts at once outlast discovery's 10 seconds.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cockpit_codex::app_server::AppServer;
use cockpit_codex::{
    Bridge, CONFORMANCE_VERSION, Codex, HOOKS_FEATURE, LAUNCH_FLAGS, SERVER_FLAGS, TUI_FLAGS, hooks,
};
use cockpit_provider_api::{Action, ConversationId, Delivered, Provider};
use serde_json::json;

/// The installed Codex the ignored live tests run.
fn installed() -> PathBuf {
    PathBuf::from(
        std::env::var_os("COCKPIT_CODEX_PROGRAM")
            .expect("COCKPIT_CODEX_PROGRAM names the Codex program to test"),
    )
}

/// A fresh `CODEX_HOME` whose login is a placeholder API key.
fn fresh_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("auth.json"),
        r#"{"auth_mode": "apikey", "OPENAI_API_KEY": "sk-cockpit-conformance-placeholder"}"#,
    )
    .unwrap();
    home
}

fn codex_output(home: &Path, arguments: &[&str]) -> String {
    let output = Command::new(installed())
        .args(arguments)
        .env("CODEX_HOME", home)
        .output()
        .unwrap();
    assert!(output.status.success(), "{arguments:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn documents(help: &str, flag: &str) -> bool {
    help.split(|character: char| character.is_whitespace() || character == ',')
        .any(|word| word == flag)
}

fn wait_until<T>(what: &str, mut found: impl FnMut() -> Option<T>) -> T {
    let ends_at = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(value) = found() {
            return value;
        }
        assert!(Instant::now() < ends_at, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "needs an installed Codex (COCKPIT_CODEX_PROGRAM)"]
fn the_installed_codex_is_the_pinned_version() {
    let codex = Codex::discover(&installed(), Some(common::bridge())).unwrap();

    assert_eq!(codex.version(), Some(CONFORMANCE_VERSION));
}

#[test]
#[ignore = "needs an installed Codex (COCKPIT_CODEX_PROGRAM)"]
fn the_installed_codex_documents_every_flag_a_launch_passes() {
    let home = tempfile::tempdir().unwrap();
    for command in [&["--help"][..], &["resume", "--help"][..]] {
        let help = codex_output(home.path(), command);

        for flag in LAUNCH_FLAGS.into_iter().chain(TUI_FLAGS) {
            assert!(documents(&help, flag), "{flag} is not in {command:?}");
        }
    }
    let help = codex_output(home.path(), &["app-server", "--help"]);
    for flag in SERVER_FLAGS {
        assert!(documents(&help, flag), "{flag} is not in app-server --help");
    }
}

#[test]
#[ignore = "needs an installed Codex (COCKPIT_CODEX_PROGRAM)"]
fn the_installed_codex_has_the_hooks_feature() {
    let home = tempfile::tempdir().unwrap();
    let features = codex_output(home.path(), &["features", "list"]);

    assert!(
        features
            .lines()
            .any(|line| line.split_whitespace().next() == Some(HOOKS_FEATURE)),
        "{features}"
    );
}

/// The built bridge, reporting through a `cockpit` that records each report's arguments and
/// launch token, one line per argument, a blank line after each report.
struct Recorder {
    dir: tempfile::TempDir,
}

impl Recorder {
    fn new() -> Recorder {
        let dir = tempfile::tempdir().unwrap();
        common::script(
            dir.path(),
            "cockpit",
            "{ printf '%s\\n' \"$COCKPIT_LAUNCH_TOKEN\" \"$@\"; echo; } >> \"$(dirname \"$0\")/reports\"",
        );
        Recorder { dir }
    }

    fn bridge(&self) -> Bridge {
        Bridge {
            session_program: PathBuf::from(env!("CARGO_BIN_EXE_cockpit-codex-session")),
            hook_program: PathBuf::from(env!("CARGO_BIN_EXE_cockpit-codex-hook")),
            cockpit_program: self.dir.path().join("cockpit"),
        }
    }

    /// Each report: the launch token, then the arguments.
    fn reports(&self) -> Vec<Vec<String>> {
        let text = std::fs::read_to_string(self.dir.path().join("reports")).unwrap_or_default();
        text.split("\n\n")
            .filter(|report| !report.trim().is_empty())
            .map(|report| report.lines().map(str::to_owned).collect())
            .collect()
    }

    /// Waits for the first report whose arguments contain `word`.
    fn report_with(&self, word: &str) -> Vec<String> {
        wait_until(&format!("a report with {word}"), || {
            self.reports()
                .into_iter()
                .find(|report| report.iter().any(|argument| argument == word))
        })
    }
}

/// `codex app-server` on a unix socket, with the hooks a launch subscribes, as the session
/// launcher runs it ([`cockpit_codex::server_arguments`]), and a connection to it.
struct LiveServer {
    child: Child,
    session_dir: tempfile::TempDir,
    work: tempfile::TempDir,
    connection: AppServer,
    _home: tempfile::TempDir,
}

impl LiveServer {
    fn start(bridge: &Bridge) -> LiveServer {
        let home = fresh_home();
        let session_dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let socket = cockpit_codex::delivery::socket_path(session_dir.path());
        let child = Command::new(installed())
            .args(cockpit_codex::server_arguments(
                &socket,
                &hooks::subscription(&bridge.hook_command()),
            ))
            .current_dir(work.path())
            .env("CODEX_HOME", home.path())
            .env("COCKPIT_LAUNCH_TOKEN", "token-live")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let connection = wait_until("the app-server listening", || {
            AppServer::connect(&socket, "cockpit-conformance", Duration::from_secs(30)).ok()
        });
        LiveServer {
            child,
            session_dir,
            work,
            connection,
            _home: home,
        }
    }

    fn start_thread(&mut self) -> ConversationId {
        let started = self
            .connection
            .call(
                "thread/start",
                json!({"cwd": self.work.path().to_str().unwrap()}),
            )
            .unwrap();
        ConversationId::new(started["thread"]["id"].as_str().unwrap()).unwrap()
    }

    /// `None` also while Codex cannot list the turns yet, just after the first message.
    fn last_turn_status(&mut self, thread: &ConversationId) -> Option<String> {
        let page = self
            .connection
            .call(
                "thread/turns/list",
                json!({"threadId": thread.as_str(), "limit": 1, "sortDirection": "desc"}),
            )
            .ok()?;
        page["data"][0]["status"].as_str().map(str::to_owned)
    }
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        let _ = terminate(&mut self.child);
    }
}

/// Stops Codex's npm wrapper and the server it runs: `kill -TERM`, which it forwards.
fn terminate(child: &mut Child) -> std::io::Result<()> {
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()?;
    let ends_at = Instant::now() + Duration::from_secs(5);
    while Instant::now() < ends_at {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.kill()?;
    child.wait().map(|_| ())
}

/// Codex runs a hook only when its trusted hash matches: a wrong `hooks::trusted_hash`
/// would leave every Cockpit hook `untrusted`, so events would silently never arrive. Also
/// proves the adapter's WebSocket client speaks to the real app-server.
#[test]
#[ignore = "needs an installed Codex (COCKPIT_CODEX_PROGRAM)"]
fn codex_trusts_every_hook_a_launch_subscribes() {
    let recorder = Recorder::new();
    let mut server = LiveServer::start(&recorder.bridge());

    let listed = server
        .connection
        .call(
            "hooks/list",
            json!({"cwds": [server.work.path().to_str().unwrap()]}),
        )
        .unwrap();

    let entries = listed["data"][0]["hooks"].as_array().unwrap();
    for event in hooks::HOOK_EVENTS {
        let key = hooks::trust_key(&event);
        let entry = entries
            .iter()
            .find(|entry| entry["key"] == key.as_str())
            .unwrap_or_else(|| panic!("{key} is not listed: {listed}"));
        assert_eq!(entry["trustStatus"], "trusted", "{entry}");
        assert_eq!(
            entry["currentHash"],
            hooks::trusted_hash(&event, &recorder.bridge().hook_command()).as_str()
        );
    }
}

#[test]
#[ignore = "needs an installed Codex (COCKPIT_CODEX_PROGRAM)"]
fn a_delivered_message_starts_a_turn_whose_prompt_reports_through_the_bridge() {
    let recorder = Recorder::new();
    let mut server = LiveServer::start(&recorder.bridge());
    let thread = server.start_thread();
    let codex = Codex::new(installed(), None, Some(recorder.bridge()));

    let delivered = codex
        .deliver(&Action::Submit("hello"), server.session_dir.path(), &thread)
        .unwrap();

    assert_eq!(delivered, Delivered::StartedTurn);
    assert_eq!(
        recorder.report_with("--prompt"),
        [
            "token-live",
            "agent",
            "report",
            "--state",
            "working",
            "--prompt",
            "--conversation",
            thread.as_str()
        ]
    );
}

#[test]
#[ignore = "needs an installed Codex (COCKPIT_CODEX_PROGRAM)"]
fn a_message_during_a_turn_joins_it_and_a_cancel_interrupts_it() {
    let recorder = Recorder::new();
    let mut server = LiveServer::start(&recorder.bridge());
    let thread = server.start_thread();
    let codex = Codex::new(installed(), None, Some(recorder.bridge()));
    let session_dir = server.session_dir.path().to_path_buf();
    codex
        .deliver(&Action::Submit("hello"), &session_dir, &thread)
        .unwrap();
    wait_until("the turn under way", || {
        (server.last_turn_status(&thread).as_deref() == Some("inProgress")).then_some(())
    });

    let joined = codex
        .deliver(&Action::Submit("and then"), &session_dir, &thread)
        .unwrap();
    let cancelled = codex
        .deliver(&Action::Cancel, &session_dir, &thread)
        .unwrap();

    assert_eq!(
        (joined, cancelled),
        (Delivered::JoinedTurn, Delivered::Interrupted)
    );
    wait_until("the turn interrupted", || {
        (server.last_turn_status(&thread).as_deref() == Some("interrupted")).then_some(())
    });
}

/// A failed turn sends no hook (measured); the session launcher's watcher reports it.
#[test]
#[ignore = "needs an installed Codex (COCKPIT_CODEX_PROGRAM)"]
fn a_failed_turn_is_followed_to_its_outcome() {
    let recorder = Recorder::new();
    let mut server = LiveServer::start(&recorder.bridge());
    let thread = server.start_thread();
    let socket = cockpit_codex::delivery::socket_path(server.session_dir.path());
    let (sender, reports) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut watcher = AppServer::connect(&socket, "watcher", Duration::from_secs(30)).unwrap();
        watcher.set_timeout(None).unwrap();
        cockpit_codex::turns::watch_failures(&mut watcher, |report| {
            let _ = sender.send(report);
        });
    });
    std::thread::sleep(Duration::from_millis(500));

    server
        .connection
        .call(
            "turn/start",
            json!({"threadId": thread.as_str(), "input": [{"type": "text", "text": "hello"}]}),
        )
        .unwrap();

    // The error's kind depends on the network: `unauthorized` or `httpConnectionFailed`.
    let report = reports.recv_timeout(Duration::from_secs(180)).unwrap();
    assert_eq!(
        report[..6],
        [
            "agent",
            "report",
            "--state",
            "waiting",
            "--outcome",
            "failed"
        ]
    );
    assert_eq!(report[6], "--outcome-reason");
    assert_eq!(report[8..], ["--conversation", thread.as_str()]);
}

/// The whole session: the launcher in a terminal runs the app-server and the TUI, the first
/// message's prompt reports through the bridge with the launch's token, and the app-server
/// stops with the TUI.
#[test]
#[ignore = "needs an installed Codex (COCKPIT_CODEX_PROGRAM) and tmux"]
fn the_session_launcher_runs_a_conversation_in_its_terminal() {
    let recorder = Recorder::new();
    let home = fresh_home();
    let session_dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let codex = Codex::new(installed(), None, Some(recorder.bridge()));
    let command = codex
        .launch(&cockpit_provider_api::LaunchRequest {
            working_dir: work.path(),
            session_dir: session_dir.path(),
            conversation: &cockpit_provider_api::Conversation::New,
            first_message: Some("hello"),
            effort: None,
            remote_control: false,
        })
        .unwrap();
    let tmux_socket = session_dir.path().join("tmux");
    let tmux = |arguments: &[&std::ffi::OsStr]| {
        Command::new("tmux")
            .arg("-S")
            .arg(&tmux_socket)
            .args(arguments)
            .status()
            .unwrap()
    };
    let _tmux_server = TmuxServer(tmux_socket.clone());
    let mut words: Vec<std::ffi::OsString> = vec![
        "new-session".into(),
        "-d".into(),
        "-x".into(),
        "160".into(),
        "-y".into(),
        "45".into(),
        "-c".into(),
        work.path().into(),
        "-e".into(),
        format!("CODEX_HOME={}", home.path().display()).into(),
        "-e".into(),
        "COCKPIT_LAUNCH_TOKEN=token-session".into(),
        command.program.clone().into(),
    ];
    words.extend(command.arguments.iter().cloned());
    let words: Vec<&std::ffi::OsStr> = words.iter().map(std::ffi::OsString::as_os_str).collect();
    assert!(tmux(&words).success());

    let report = recorder.report_with("--prompt");
    // The first Ctrl-C interrupts the turn, the next ones quit the TUI.
    for _ in 0..3 {
        tmux(&["send-keys".as_ref(), "C-c".as_ref()]);
        std::thread::sleep(Duration::from_secs(1));
    }
    let socket = cockpit_codex::delivery::socket_path(session_dir.path());
    let is_stopped = (0..100).any(|_| {
        std::thread::sleep(Duration::from_millis(100));
        std::os::unix::net::UnixStream::connect(&socket).is_err()
    });

    assert_eq!(
        &report[..6],
        [
            "token-session",
            "agent",
            "report",
            "--state",
            "working",
            "--prompt"
        ]
    );
    assert!(is_stopped, "the app-server outlived the TUI");
}

/// Kills the test's tmux server, and the launcher in it, even when the test fails.
struct TmuxServer(PathBuf);

impl Drop for TmuxServer {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(&self.0)
            .arg("kill-server")
            .status();
    }
}
