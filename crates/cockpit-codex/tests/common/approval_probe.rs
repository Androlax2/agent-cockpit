//! Exact-version, isolated Codex/TUI probe through Provider::deliver.

#[path = "approval_model.rs"]
mod approval_model;
#[path = "approval_preflight.rs"]
mod approval_preflight;
#[path = "bounded_command.rs"]
mod bounded_command;

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cockpit_codex::Codex;
use cockpit_codex::app_server::AppServer;
use cockpit_provider_api::{Action, ConversationId, Delivered, Provider};
use rustix::process::{Pid, Signal, kill_process_group};
use serde_json::json;

use crate::common;
use crate::recording_proxy::{Direction, Entry, RecordingProxy};

const VERSION: &str = "0.166.0";
const APPROVAL: &str = "item/commandExecution/requestApproval";

/// No auth files, inherited environment, real model endpoint, or owner tmux server.
pub fn run(steer: bool) {
    probe(steer, None, None);
}

/// Only through the explicitly approved host command documented in the adapter README.
pub fn run_signed_in() {
    assert_eq!(
        std::env::var("COCKPIT_CODEX_ALLOW_SIGNED_IN").as_deref(),
        Ok("1"),
        "signed-in account usage must be explicitly authorized"
    );
    let home = PathBuf::from(
        std::env::var_os("COCKPIT_CODEX_SIGNED_IN_HOME").expect("explicit signed-in CODEX_HOME"),
    );
    assert!(
        home.is_absolute() && home.is_dir(),
        "signed-in CODEX_HOME must be an existing absolute directory"
    );
    probe(false, Some(home), None);
}

/// Exercise the live preflight with a credential-free home and a sentinel MCP program.
pub fn enabled_mcp_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("mcp-started");
    let mcp = common::script(
        home.path(),
        "sentinel",
        &format!("touch '{}'", marker.display()),
    );
    std::fs::write(home.path().join("config.toml"), format!(
        "model_provider = \"fixture\"\n[model_providers.fixture]\nname = \"fixture\"\nbase_url = \"http://127.0.0.1:9/v1\"\nwire_api = \"responses\"\n[mcp_servers.sentinel]\ncommand = {}\n", json!(mcp.display().to_string())
    )).unwrap();
    refused(home.path(), "signed-in probe refuses enabled MCP servers");
    assert!(
        !marker.exists(),
        "the configured MCP server must never start"
    );
}

pub fn notification_is_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("notified");
    let notify = common::script(
        dir.path(),
        "notify",
        &format!("touch '{}'", marker.display()),
    );
    probe(false, None, Some(&notify));
    assert!(
        !marker.exists(),
        "inherited notification command must not run"
    );
}

pub fn execution_rules_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("rules")).unwrap();
    std::fs::write(
        dir.path().join("rules/fixture.rules"),
        "prefix_rule(pattern=[\"/usr/bin/printf\"], decision=\"allow\")\n",
    )
    .unwrap();
    refused(
        dir.path(),
        "signed-in probe refuses inherited execution rules",
    );
}

pub fn custom_provider_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"),
        "model_provider = \"fixture\"\n[model_providers.fixture]\nname = \"fixture\"\nbase_url = \"http://127.0.0.1:9/v1\"\nwire_api = \"responses\"\n").unwrap();
    refused(
        dir.path(),
        "custom provider or endpoint is not signed-in conformance",
    );
}

pub fn signed_out_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "cli_auth_credentials_store = \"file\"\n",
    )
    .unwrap();
    refused(dir.path(), "a signed-in OpenAI account is required");
}

pub fn custom_keymap_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"),
        "[tui.keymap]\n\"list.cancel\" = \"q\"\n\"approval.decline\" = \"n\"\n\"approval.approve\" = \"esc\"\n").unwrap();
    refused(
        dir.path(),
        "custom TUI keymaps need a separate conformance plan",
    );
}

pub fn project_settings_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("project-mcp-started");
    let mcp = common::script(
        dir.path(),
        "sentinel",
        &format!("touch '{}'", marker.display()),
    );
    for persisted_trust in [false, true] {
        refused_with_project(
            dir.path(),
            if persisted_trust {
                "signed-in probe refuses enabled MCP servers"
            } else {
                "project configuration needs a separate conformance plan"
            },
            |work| {
                if persisted_trust {
                    trust_fixture_project(dir.path(), work);
                } else {
                    std::fs::write(
                        dir.path().join("config.toml"),
                        "cli_auth_credentials_store = \"file\"\n",
                    )
                    .unwrap();
                }
                let config = work.join(".codex");
                std::fs::create_dir(&config).unwrap();
                std::fs::write(
                    config.join("config.toml"),
                    format!(
                        "[mcp_servers.sentinel]\ncommand = {}\n",
                        json!(mcp.display().to_string())
                    ),
                )
                .unwrap();
            },
        );
    }
    assert!(!marker.exists(), "project MCP server must never start");
    refused_with_project(
        dir.path(),
        "signed-in probe refuses inherited execution rules",
        |work| {
            trust_fixture_project(dir.path(), work);
            let config = work.join(".codex");
            std::fs::create_dir_all(config.join("rules")).unwrap();
            std::fs::write(config.join("config.toml"), "").unwrap();
            std::fs::write(
                config.join("rules/fixture.rules"),
                "prefix_rule(pattern=[\"/usr/bin/printf\"], decision=\"allow\")\n",
            )
            .unwrap();
        },
    );
}

fn trust_fixture_project(home: &Path, work: &Path) {
    // config/read discovers project layers using persisted trust. A real signed-in
    // home can trust an ancestor of TMPDIR; reproduce that only in this fresh home.
    std::fs::write(
        home.join("config.toml"),
        format!(
            "cli_auth_credentials_store = \"file\"\n[projects.{}]\ntrust_level = \"trusted\"\n",
            json!(work.display().to_string())
        ),
    )
    .unwrap();
}

fn refused(home: &Path, expected: &str) {
    refused_with_project(home, expected, |_| {});
}

fn refused_with_project(
    home: &Path,
    expected: &str,
    setup: impl FnOnce(&Path) + std::panic::UnwindSafe,
) {
    let failure = std::panic::catch_unwind(|| probe_in(false, Some(home.to_owned()), None, setup))
        .unwrap_err();
    let message = failure
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| failure.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(message.contains(expected), "{message}");
}

fn probe(steer: bool, signed_in_home: Option<PathBuf>, notification: Option<&Path>) {
    probe_in(steer, signed_in_home, notification, |_| {});
}

fn probe_in(
    steer: bool,
    signed_in_home: Option<PathBuf>,
    notification: Option<&Path>,
    setup: impl FnOnce(&Path),
) {
    let program = PathBuf::from(
        std::env::var_os("COCKPIT_CODEX_PROGRAM")
            .expect("COCKPIT_CODEX_PROGRAM names Codex 0.166.0"),
    );
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir(&work).unwrap();
    setup(&work);
    let mut environment = [
        ("PATH", "/usr/bin:/bin".to_owned()),
        ("HOME", home.display().to_string()),
        ("CODEX_HOME", home.display().to_string()),
        ("XDG_CONFIG_HOME", home.join("config").display().to_string()),
        ("XDG_DATA_HOME", home.join("data").display().to_string()),
        ("XDG_CACHE_HOME", home.join("cache").display().to_string()),
        ("XDG_STATE_HOME", home.join("state").display().to_string()),
        ("TERM", "xterm-256color".to_owned()),
    ];
    let version = bounded_command::output(
        Command::new(&program)
            .arg("--version")
            .env_clear()
            .envs(environment.iter().cloned())
            .current_dir(&work),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        format!("codex-cli {VERSION}")
    );
    let model = signed_in_home.is_none().then(approval_model::Model::start);
    if let Some(model) = &model {
        std::fs::write(
            home.join("config.toml"),
            format!(
                r#"
model = "mock-model"
model_provider = "fixture"
approval_policy = "on-request"
approvals_reviewer = "user"
sandbox_mode = "read-only"
cli_auth_credentials_store = "file"
check_for_update_on_startup = false
notify = {}
[features]
enable_request_compression = false
shell_snapshot = false
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
                json!(
                    notification
                        .map(|path| vec![path.display().to_string()])
                        .unwrap_or_default()
                ),
                model.url,
                json!(work.display().to_string())
            ),
        )
        .unwrap();
    }
    if let Some(auth_home) = &signed_in_home {
        environment
            .iter_mut()
            .find(|(key, _)| *key == "CODEX_HOME")
            .unwrap()
            .1 = auth_home.display().to_string();
    }
    let overrides = [
        "notify=[]".to_owned(),
        "approval_policy=\"on-request\"".to_owned(),
        "approvals_reviewer=\"user\"".to_owned(),
        "sandbox_mode=\"read-only\"".to_owned(),
        "features.hooks=false".to_owned(),
        "features.plugins=false".to_owned(),
        "features.apps=false".to_owned(),
        "features.memories=false".to_owned(),
        "features.shell_snapshot=false".to_owned(),
        "skills.include_instructions=false".to_owned(),
        "check_for_update_on_startup=false".to_owned(),
        format!(
            "projects.{}.trust_level=\"trusted\"",
            json!(work.display().to_string())
        ),
    ];

    let socket = dir.path().join("server.sock");
    let mut command = Command::new(&program);
    command
        .args(["app-server", "--listen"])
        .arg(format!("unix://{}", socket.display()));
    for value in &overrides {
        command.args(["--config", value]);
    }
    let stderr = if signed_in_home.is_some() {
        Stdio::null()
    } else {
        std::fs::File::create(dir.path().join("server.log"))
            .unwrap()
            .into()
    };
    let child = command
        .process_group(0)
        .env_clear()
        .envs(environment.iter().cloned())
        .current_dir(&work)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .unwrap();
    let _server = Server(child);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(mut readiness) = AppServer::connect(&socket, "readiness", Duration::from_secs(1))
        {
            // Include cwd so project settings are checked too. Empty TOML tables merge;
            // `mcp_servers={}` cannot disable inherited integrations. Both probe modes
            // refuse them before starting a thread, including when TMPDIR is in a project.
            approval_preflight::check(
                &mut readiness,
                signed_in_home.as_deref().unwrap_or(&home),
                &work,
                model.as_ref().map(|model| model.url.as_str()),
            )
            .unwrap_or_else(|reason| panic!("{reason}"));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "app-server failed: {}",
            if signed_in_home.is_some() {
                "signed-in diagnostics withheld".to_owned()
            } else {
                std::fs::read_to_string(dir.path().join("server.log")).unwrap()
            }
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let proxy = RecordingProxy::start(&dir.path().join("codex.sock"), &socket);
    let tmux = Tmux {
        socket: dir.path().join("tmux"),
        signed_in: signed_in_home.is_some(),
    };
    let mut launch = tmux.command();
    launch
        .args([
            "new-session",
            "-s",
            "approval-probe",
            "-d",
            "-x",
            "160",
            "-y",
            "45",
            "-c",
        ])
        .arg(&work);
    for (key, value) in &environment {
        launch.arg("-e").arg(format!("{key}={value}"));
    }
    launch.arg("/usr/bin/env").arg("-i");
    for (key, value) in &environment {
        launch.arg(format!("{key}={value}"));
    }
    launch.arg(&program);
    for value in &overrides {
        launch.args(["--config", value]);
    }
    launch
        .arg("--remote")
        .arg(format!("unix://{}", proxy.path().display()))
        .args(["--", "Reply READY only. Do not use any tools."]);
    assert!(
        bounded_command::output(&mut launch, Duration::from_secs(10))
            .unwrap()
            .status
            .success()
    );
    wait("initial turn completion", &proxy, &tmux, |entries| {
        entries
            .iter()
            .any(|entry| entry.body["method"] == "turn/completed")
    });
    let entries = proxy.entries();
    let owner = entries
        .iter()
        .find(|entry| entry.direction == Direction::Client && entry.body["method"] == "initialize")
        .unwrap()
        .connection;
    let started = entries
        .iter()
        .find(|entry| entry.body["method"] == "turn/started")
        .unwrap();
    let thread = ConversationId::new(started.body["params"]["threadId"].as_str().unwrap()).unwrap();
    let codex = Codex::new(program, Some(VERSION.to_owned()), Some(common::bridge()));
    if steer {
        assert_eq!(
            codex
                .deliver(
                    &Action::Submit("Start the fixture command."),
                    &common::target(dir.path()),
                    &thread
                )
                .unwrap(),
            Delivered::StartedTurn
        );
        model.as_ref().unwrap().wait_for_pending_tool();
    }
    let prompt = "Approval-routing conformance: make exactly one exec_command call with cmd '/usr/bin/printf cockpit-approval-fixture', sandbox_permissions 'require_escalated', and justification 'Conformance fixture: refuse this harmless command.' Do not run any other commands or read files. The owner will cancel this request; do not retry it.";
    let delivered = codex
        .deliver(
            &Action::Submit(prompt),
            &common::target(dir.path()),
            &thread,
        )
        .unwrap();
    assert_eq!(
        delivered,
        if steer {
            Delivered::JoinedTurn
        } else {
            Delivered::StartedTurn
        }
    );
    if let Some(model) = &model {
        model.wait_for_pending_tool();
    }
    let delivery = proxy
        .entries()
        .iter()
        .rfind(|entry| {
            entry.direction == Direction::Client
                && entry.body["method"] == if steer { "turn/steer" } else { "turn/start" }
                && entry.body["params"]["threadId"] == thread.as_str()
        })
        .expect("the delivery request was recorded")
        .connection;
    assert_ne!(delivery, owner);
    wait("delivery disconnect", &proxy, &tmux, |entries| {
        entries.iter().any(|entry| {
            entry.connection == delivery
                && entry.direction == Direction::Client
                && entry.disconnected
        })
    });
    if let Some(model) = &model {
        model.release_tool();
    }
    wait("owner approval", &proxy, &tmux, |entries| {
        entries
            .iter()
            .any(|entry| entry.connection == owner && entry.body["method"] == APPROVAL)
    });
    let approval = proxy
        .entries()
        .into_iter()
        .find(|entry| entry.connection == owner && entry.body["method"] == APPROVAL)
        .unwrap()
        .body;
    assert_eq!(approval["params"]["threadId"], thread.as_str());
    assert!(approval["params"]["turnId"].is_string());
    let deadline = Instant::now() + Duration::from_secs(10);
    while !tmux
        .screen()
        .contains("Would you like to run the following command?")
    {
        assert!(
            Instant::now() < deadline,
            "TUI did not render its approval prompt"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
    // 0.166.0 quarantines pre-prompt input for up to one second (tui/input_boundary.rs).
    std::thread::sleep(Duration::from_millis(1100));
    // Escape cancels the displayed request; it never selects the default Yes.
    tmux.keys(&["Escape"]);
    wait("owner cancellation response", &proxy, &tmux, |entries| {
        assert!(
            !entries
                .iter()
                .any(|entry| entry.connection == owner && entry.disconnected),
            "owner TUI disconnected before answering"
        );
        entries.iter().any(|entry| {
            entry.connection == owner
                && entry.direction == Direction::Client
                && entry.body["id"] == approval["id"]
                && entry.body.get("result").is_some()
        })
    });
    wait("cancelled turn completion", &proxy, &tmux, |entries| {
        entries.iter().any(|entry| {
            entry.body["method"] == "turn/completed"
                && entry.body["params"]["threadId"] == thread.as_str()
                && entry.body["params"]["turn"]["id"] == approval["params"]["turnId"]
                && entry.body["params"]["turn"]["status"] == "interrupted"
        })
    });
    let entries = proxy.entries();
    let response = entries
        .iter()
        .find(|entry| {
            entry.connection == owner
                && entry.direction == Direction::Client
                && entry.body["id"] == approval["id"]
                && entry.body.get("result").is_some()
        })
        .unwrap();
    assert!(
        matches!(
            response.body["result"]["decision"].as_str(),
            Some("decline" | "cancel")
        ),
        "only a refusal is allowed"
    );
    wait("approval resolution", &proxy, &tmux, |entries| {
        entries.iter().any(|entry| {
            entry.body["method"] == "serverRequest/resolved"
                && entry.body["params"]["requestId"] == approval["id"]
        })
    });
    assert!(
        !entries.iter().any(|entry| entry.connection != owner
            && entry.direction == Direction::Client
            && entry.body.get("result").is_some()),
        "Cockpit must never answer approvals"
    );
    println!(
        "Codex {VERSION}; action={delivered:?}; owner TUI received approval and returned {}; {}",
        response.body["result"]["decision"],
        if signed_in_home.is_some() {
            "signed-in model; account usage and home writes authorized separately"
        } else {
            "delivery disconnected before tool request; local fake model, no account usage"
        }
    );
}

fn wait(what: &str, proxy: &RecordingProxy, tmux: &Tmux, found: impl Fn(&[Entry]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !found(&proxy.entries()) {
        assert!(
            Instant::now() < deadline,
            "{what} timed out; approval traffic: {:?}; fixture TUI:\n{}",
            proxy
                .entries()
                .iter()
                .filter(|entry| entry.body["method"] == APPROVAL)
                .map(|entry| entry.connection)
                .collect::<Vec<_>>(),
            tmux.diagnostic()
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}

struct Tmux {
    socket: PathBuf,
    signed_in: bool,
}

impl Tmux {
    fn command(&self) -> Command {
        let mut command = Command::new("tmux");
        command
            .arg("-S")
            .arg(&self.socket)
            .args(["-f", "/dev/null"])
            .env_remove("TMUX")
            .env_remove("TMUX_PANE");
        command
    }

    fn keys(&self, keys: &[&str]) {
        assert!(
            bounded_command::output(
                self.command()
                    .args(["send-keys", "-t", "=approval-probe:0.0"])
                    .args(keys),
                Duration::from_secs(5)
            )
            .unwrap()
            .status
            .success()
        );
    }

    fn diagnostic(&self) -> String {
        if self.signed_in {
            "signed-in TUI diagnostics withheld".to_owned()
        } else {
            self.screen()
        }
    }

    fn screen(&self) -> String {
        let output = bounded_command::output(
            self.command()
                .args(["capture-pane", "-t", "=approval-probe:0.0", "-p"]),
            Duration::from_secs(5),
        )
        .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = bounded_command::output(self.command().arg("kill-server"), Duration::from_secs(2));
    }
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        // Keep the leader unreaped until its group is stopped, so its PID cannot be reused.
        let pid = Pid::from_child(&self.0);
        let _ = kill_process_group(pid, Signal::TERM);
        std::thread::sleep(Duration::from_millis(100));
        let _ = kill_process_group(pid, Signal::KILL);
        let _ = self.0.wait();
    }
}
