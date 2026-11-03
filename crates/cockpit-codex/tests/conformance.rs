//! Native 0.166.0 capability evidence, independent of production pin promotion.
//! Ignored native tests require explicit approval and COCKPIT_CODEX_PROGRAM.
//! They use scratch homes, local canned responses, no credentials and private tmux.

#[path = "common/approval_model.rs"]
#[allow(dead_code, reason = "shared with approval-routing native tests")]
mod approval_model;
#[path = "common/approval_preflight.rs"]
mod approval_preflight;
#[path = "common/bounded_command.rs"]
mod bounded_command;
mod common;

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use approval_model::{Model, Scenario};
use cockpit_codex::app_server::AppServer;
use cockpit_codex::{Bridge, Codex, HOOKS_FEATURE, LAUNCH_FLAGS, SERVER_FLAGS, TUI_FLAGS, hooks};
use cockpit_provider_api::{Action, ConversationId, Delivered, Provider};
use rustix::process::{Pid, Signal, kill_process_group};
use serde_json::json;

const NATIVE_VERSION: &str = "0.166.0";

/// The installed Codex the ignored live tests run.
fn installed() -> PathBuf {
    PathBuf::from(
        std::env::var_os("COCKPIT_CODEX_PROGRAM")
            .expect("COCKPIT_CODEX_PROGRAM names the Codex program to test"),
    )
}

fn fresh_home(model: &Model, work: &Path) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            r#"
model = "mock-model"
model_provider = "fixture"
approval_policy = "on-request"
approvals_reviewer = "user"
sandbox_mode = "read-only"
notify = []
cli_auth_credentials_store = "file"
check_for_update_on_startup = false
[features]
enable_request_compression = false
shell_snapshot = false
plugins = false
apps = false
memories = false
[skills]
include_instructions = false
[model_providers.fixture]
name = "Isolated conformance fixture"
base_url = "{}"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0
[projects.{}]
trust_level = "trusted"
"#,
            model.url,
            json!(work.display().to_string())
        ),
    )
    .unwrap();
    home
}

fn codex_output(home: &Path, arguments: &[&str]) -> String {
    let output = bounded_command::output(
        native_command(&installed(), home).args(arguments),
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(output.status.success(), "{arguments:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn native_command(program: &Path, home: &Path) -> Command {
    let mut command = Command::new(program);
    command.env_clear().current_dir(home).envs([
        ("PATH", PathBuf::from("/usr/bin:/bin")),
        ("HOME", home.to_path_buf()),
        ("CODEX_HOME", home.to_path_buf()),
        ("XDG_CONFIG_HOME", home.join("config")),
        ("XDG_DATA_HOME", home.join("data")),
        ("XDG_CACHE_HOME", home.join("cache")),
        ("XDG_STATE_HOME", home.join("state")),
    ]);
    command.env("TERM", "xterm-256color");
    command
}

fn documents(help: &str, flag: &str) -> bool {
    help.split(|character: char| character.is_whitespace() || character == ',')
        .any(|word| word == flag)
}

#[test]
fn native_helpers_do_not_inherit_credentials_or_configuration() {
    if std::env::var_os("COCKPIT_CONFORMANCE_TEST_CHILD").is_some() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(codex_output(home.path(), &["--version"]), "isolated\n");
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let program = common::script(
        fixture.path(),
        "codex",
        r#"if [ -n "${OPENAI_API_KEY+x}${OPENAI_BASE_URL+x}${HTTP_PROXY+x}${OPENCODE_CONFIG_CONTENT+x}" ] || [ "$HOME" != "$CODEX_HOME" ]; then
echo inherited
else
echo isolated
fi"#,
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "native_helpers_do_not_inherit_credentials_or_configuration",
            "--nocapture",
        ])
        .env("COCKPIT_CONFORMANCE_TEST_CHILD", "1")
        .env("COCKPIT_CODEX_PROGRAM", program)
        .env("OPENAI_API_KEY", "fixture-not-a-credential")
        .env("OPENAI_BASE_URL", "fixture-invalid-endpoint")
        .env("HTTP_PROXY", "fixture-invalid-proxy")
        .env("OPENCODE_CONFIG_CONTENT", "fixture-inherited-config")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
fn the_installed_codex_is_the_validated_version() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        codex_output(home.path(), &["--version"]).trim(),
        format!("codex-cli {NATIVE_VERSION}")
    );
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
            "{ printf '%s\\n' \"$TORLEK_LAUNCH_TOKEN\" \"$@\"; echo; } >> \"$(dirname \"$0\")/reports\"",
        );
        Recorder { dir }
    }

    fn bridge(&self) -> Bridge {
        Bridge {
            session_program: PathBuf::from(env!("CARGO_BIN_EXE_torlek-codex-session")),
            hook_program: PathBuf::from(env!("CARGO_BIN_EXE_torlek-codex-hook")),
            cockpit_program: self.dir.path().join("cockpit"),
        }
    }

    /// Each report: the launch token, then the arguments.
    fn reports(&self) -> Vec<Vec<String>> {
        let text = std::fs::read_to_string(self.dir.path().join("reports")).unwrap_or_default();
        text.split_inclusive("\n\n")
            .filter_map(|report| report.strip_suffix("\n\n"))
            .filter(|report| !report.is_empty())
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

#[test]
fn a_report_is_not_observable_until_its_record_is_complete() {
    let recorder = Recorder::new();
    let path = recorder.dir.path().join("reports");
    std::fs::write(&path, "token\nagent\nreport\n--outcome\ncompleted\n").unwrap();
    assert!(recorder.reports().is_empty());
    std::fs::write(
        &path,
        "token\nagent\nreport\n--outcome\ncompleted\n--conversation\nthread\n\n",
    )
    .unwrap();
    assert_eq!(
        recorder.reports()[0].last().map(String::as_str),
        Some("thread")
    );
}

/// `codex app-server` on a unix socket, with the hooks a launch subscribes, as the session
/// launcher runs it ([`cockpit_codex::server_arguments`]), and a connection to it.
struct LiveServer {
    _server: Server,
    session_dir: tempfile::TempDir,
    work: tempfile::TempDir,
    connection: AppServer,
    _home: tempfile::TempDir,
    model: Model,
}

impl LiveServer {
    fn start(bridge: &Bridge) -> LiveServer {
        let model = Model::conformance(Scenario::Failure);
        let session_dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let home = fresh_home(&model, work.path());
        let (server, connection) =
            start_server(bridge, home.path(), work.path(), session_dir.path(), &model);
        LiveServer {
            _server: server,
            session_dir,
            work,
            connection,
            _home: home,
            model,
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

fn start_server(
    bridge: &Bridge,
    home: &Path,
    work: &Path,
    session_dir: &Path,
    model: &Model,
) -> (Server, AppServer) {
    assert_eq!(
        codex_output(home, &["--version"]).trim(),
        format!("codex-cli {NATIVE_VERSION}")
    );
    let socket = cockpit_codex::delivery::socket_path(session_dir);
    let reports = cockpit_codex::reports::LaunchReports::new(&socket).unwrap();
    let child = Server {
        child: native_command(&installed(), home)
            .args(cockpit_codex::server_arguments(
                &socket,
                &hooks::subscription(&bridge.hook_command()),
            ))
            .current_dir(work)
            .process_group(0)
            .env("TORLEK_LAUNCH_TOKEN", "token-live")
            .env(cockpit_codex::reports::STATE_VARIABLE, reports.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
        _reports: reports,
    };
    let mut connection = wait_until("the app-server listening", || {
        AppServer::connect(&socket, "cockpit-conformance", Duration::from_secs(1)).ok()
    });
    connection
        .set_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    if let Err(reason) = approval_preflight::check(&mut connection, home, work, Some(&model.url)) {
        let config = connection
            .call("config/read", json!({"includeLayers": true, "cwd": work}))
            .unwrap();
        panic!(
            "{reason}; isolated effective keymap: {}",
            config["config"]["tui"]["keymap"]
        );
    }
    (child, connection)
}

struct Server {
    child: Child,
    // Native hooks need the launcher's serialization too, before appending records.
    _reports: cockpit_codex::reports::LaunchReports,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Err(error) = terminate(&mut self.child) {
            eprintln!("cannot clean up native conformance server: {error}");
        }
    }
}

fn terminate(child: &mut Child) -> std::io::Result<()> {
    // Keep the leader waitable until its whole group is stopped, avoiding PID reuse.
    let pid = Pid::from_child(child);
    let _ = kill_process_group(pid, Signal::TERM);
    std::thread::sleep(Duration::from_millis(100));
    match kill_process_group(pid, Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => {}
        Err(error) => return Err(error.into()),
    }
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
        .deliver(
            &Action::Submit("hello"),
            &common::target(server.session_dir.path()),
            &thread,
        )
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
        .deliver(
            &Action::Submit("hello"),
            &common::target(&session_dir),
            &thread,
        )
        .unwrap();
    wait_until("the turn under way", || {
        (server.last_turn_status(&thread).as_deref() == Some("inProgress")).then_some(())
    });

    let joined = codex
        .deliver(
            &Action::Submit("and then"),
            &common::target(&session_dir),
            &thread,
        )
        .unwrap();
    let cancelled = codex
        .deliver(&Action::Cancel, &common::target(&session_dir), &thread)
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
    let (ready, connected) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut watcher = AppServer::connect(&socket, "watcher", Duration::from_secs(30)).unwrap();
        ready.send(()).unwrap();
        cockpit_codex::turns::watch_failures(&mut watcher, Duration::from_secs(30), |report| {
            let _ = sender.send(report);
        });
    });
    connected.recv_timeout(Duration::from_secs(5)).unwrap();
    server.model.release_response();

    server
        .connection
        .call(
            "turn/start",
            json!({"threadId": thread.as_str(), "input": [{"type": "text", "text": "hello"}]}),
        )
        .unwrap();

    let report = reports.recv_timeout(Duration::from_secs(30)).unwrap();
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
    let model = Model::conformance(Scenario::Success);
    let work = tempfile::tempdir().unwrap();
    let home = fresh_home(&model, work.path());
    let session =
        TerminalSession::start(&recorder.bridge(), home.path(), work.path(), &model, None);
    let report = recorder.report_with("--prompt");
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
    let completed = recorder.report_with("completed");
    assert_eq!(completed.last(), report.last());
    session.quit();
}

#[test]
#[ignore = "needs explicitly approved native Codex 0.166.0 and tmux; local model only"]
fn the_session_launcher_resumes_the_same_completed_conversation() {
    let recorder = Recorder::new();
    let model = Model::conformance(Scenario::Success);
    let work = tempfile::tempdir().unwrap();
    let home = fresh_home(&model, work.path());
    let session =
        TerminalSession::start(&recorder.bridge(), home.path(), work.path(), &model, None);
    let completed = recorder.report_with("completed");
    let conversation = ConversationId::new(completed.last().unwrap()).unwrap();
    session.quit();
    drop(session);
    let prior_prompts = recorder
        .reports()
        .iter()
        .filter(|report| report.iter().any(|word| word == "--prompt"))
        .count();
    let resumed = TerminalSession::start(
        &recorder.bridge(),
        home.path(),
        work.path(),
        &model,
        Some(&conversation),
    );
    let reports = wait_until("a resumed prompt report", || {
        let reports = recorder.reports();
        (reports
            .iter()
            .filter(|report| report.iter().any(|word| word == "--prompt"))
            .count()
            > prior_prompts)
            .then_some(reports)
    });
    let prompt = reports
        .iter()
        .rfind(|report| report.iter().any(|word| word == "--prompt"))
        .unwrap();
    assert_eq!(
        prompt.last().map(String::as_str),
        Some(conversation.as_str())
    );
    wait_until("the resumed completed outcome", || {
        let reports = recorder.reports();
        (reports
            .iter()
            .filter(|report| {
                report.iter().any(|word| word == "completed")
                    && report.last().map(String::as_str) == Some(conversation.as_str())
            })
            .count()
            == 2)
            .then_some(())
    });
    resumed.quit();
}

struct TerminalSession {
    dir: PathBuf,
    home: PathBuf,
    pane: Option<Pid>,
}

impl TerminalSession {
    fn start(
        bridge: &Bridge,
        home: &Path,
        work: &Path,
        model: &Model,
        resumed: Option<&ConversationId>,
    ) -> Self {
        let preflight_dir = tempfile::tempdir().unwrap();
        // The actual launcher starts its TUI immediately; check the same scratch
        // configuration with the same server arguments before any thread can start.
        drop(start_server(
            bridge,
            home,
            work,
            preflight_dir.path(),
            model,
        ));
        let mut session = Self {
            dir: tempfile::tempdir().unwrap().keep(),
            home: home.to_owned(),
            pane: None,
        };
        let conversation = resumed
            .cloned()
            .map(cockpit_provider_api::Conversation::Resume)
            .unwrap_or(cockpit_provider_api::Conversation::New);
        let command = Codex::new(installed(), None, Some(bridge.clone()))
            .launch(&cockpit_provider_api::LaunchRequest {
                working_dir: work,
                session_dir: &session.dir,
                conversation: &conversation,
                first_message: Some("Reply READY only. Do not use tools."),
                effort: None,
                model: None,
                remote_control: false,
            })
            .unwrap();
        session.launch(work, &command.program, &command.arguments);
        session
    }

    fn launch(&mut self, work: &Path, program: &Path, arguments: &[std::ffi::OsString]) {
        assert!(
            Path::new("/proc/self/stat").is_file(),
            "native terminal cleanup requires Linux /proc"
        );
        std::fs::write(self.dir.join("tmux.conf"), "set-option -g exit-empty off\n").unwrap();
        // Keep the pane group leader alive until cleanup, even after the launcher
        // exits. No child starts until the parent has recorded the group's identity.
        let leader = common::script(
            &self.dir,
            "pane-leader",
            r#"gate=$1; shift
while [ ! -f "$gate" ]; do sleep 0.01; done
trap ':' HUP INT TERM
"$@"
while :; do sleep 1; done"#,
        );
        let mut launch = self.tmux();
        launch
            .args([
                "new-session",
                "-s",
                "conformance",
                "-d",
                "-x",
                "160",
                "-y",
                "45",
                "-P",
                "-F",
                "#{pane_pid}",
                "-c",
            ])
            .arg(work)
            .arg("/bin/sh")
            .arg(leader)
            .arg(self.dir.join("start"))
            .args(["/usr/bin/env", "-i"]);
        for (name, value) in native_command(program, &self.home).get_envs() {
            let mut assignment = name.to_os_string();
            assignment.push("=");
            assignment.push(value.unwrap());
            launch.arg(assignment);
        }
        launch
            .arg("TORLEK_LAUNCH_TOKEN=token-session")
            .arg(program)
            .args(arguments);
        let output = bounded_command::output(&mut launch, Duration::from_secs(10)).unwrap();
        assert!(output.status.success(), "{output:?}");
        self.pane = Some(
            Pid::from_raw(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap(),
            )
            .unwrap(),
        );
        std::fs::write(self.dir.join("start"), "").unwrap();
    }

    fn tmux(&self) -> Command {
        let mut command = native_command(Path::new("tmux"), &self.home);
        command
            .arg("-S")
            .arg(self.dir.join("tmux"))
            .arg("-f")
            .arg(self.dir.join("tmux.conf"));
        command
    }

    fn quit(&self) {
        for _ in 0..3 {
            let output = bounded_command::output(
                self.tmux()
                    .args(["send-keys", "-t", "=conformance:0.0", "C-c"]),
                Duration::from_secs(5),
            )
            .unwrap();
            if !output.status.success() {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        let socket = cockpit_codex::delivery::socket_path(&self.dir);
        wait_until("the app-server stopping with the TUI", || {
            std::os::unix::net::UnixStream::connect(&socket)
                .is_err()
                .then_some(())
        });
    }

    fn cleanup(&mut self) -> Result<(), String> {
        if let Some(group) = self.pane.take() {
            for signal in [Signal::TERM, Signal::KILL] {
                match kill_process_group(group, signal) {
                    Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                    Err(error) => return Err(format!("cannot stop pane group: {error}")),
                }
                if signal == Signal::TERM {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while group_is_running(group).map_err(|error| error.to_string())? {
                if Instant::now() >= deadline {
                    return Err("pane process group did not stop".to_owned());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        if self.dir.join("tmux").exists() {
            let output =
                bounded_command::output(self.tmux().arg("kill-server"), Duration::from_secs(3))?;
            if !output.status.success() {
                return Err(format!("cannot stop private tmux: {output:?}"));
            }
        }
        std::fs::remove_dir_all(&self.dir).map_err(|error| error.to_string())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            let reason = format!(
                "{error}; retained terminal fixture at {}",
                self.dir.display()
            );
            if std::thread::panicking() {
                eprintln!("{reason}");
            } else {
                panic!("{reason}");
            }
        }
    }
}

fn group_is_running(group: Pid) -> std::io::Result<bool> {
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .is_none()
        {
            continue;
        }
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let (_, fields) = stat
            .rsplit_once(") ")
            .ok_or_else(|| std::io::Error::other("invalid process stat"))?;
        let mut fields = fields.split_whitespace();
        let state = fields.next();
        let process_group = fields.nth(1).and_then(|value| value.parse::<i32>().ok());
        if process_group == Some(group.as_raw_nonzero().get()) && state != Some("Z") {
            return Ok(true);
        }
    }
    Ok(false)
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "needs tmux; isolated fake processes only"]
fn a_failed_terminal_check_stops_descendants_that_ignore_hangup() {
    let home = tempfile::tempdir().unwrap();
    let mut session = TerminalSession {
        dir: tempfile::tempdir().unwrap().keep(),
        home: home.path().to_owned(),
        pane: None,
    };
    let program = common::script(
        home.path(),
        "stubborn",
        r#"trap '' HUP TERM
sh -c 'echo $$ > "$HOME/child.pid"; exec sleep 30' &
wait"#,
    );
    session.launch(home.path(), &program, &[]);
    let pid: i32 = wait_until("the stubborn child", || {
        std::fs::read_to_string(home.path().join("child.pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    });
    let pane = bounded_command::output(
        session.tmux().args([
            "display-message",
            "-p",
            "-t",
            "=conformance:0.0",
            "#{pane_pid}",
        ]),
        Duration::from_secs(3),
    )
    .unwrap();
    let group = Pid::from_raw(
        String::from_utf8(pane.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap(),
    )
    .unwrap();
    let failure = std::panic::catch_unwind(|| {
        let _session = session;
        panic!("forced fixture failure");
    });
    assert!(failure.is_err());
    let running = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, fields)| !fields.starts_with('Z'))
    });
    // Rescue the RED case before asserting, so the regression itself never leaks.
    if running {
        let _ = kill_process_group(group, Signal::KILL);
    }
    assert!(
        !running,
        "a failed check left its hangup-resistant child running"
    );
}
