//! Conformance against an installed Codex. Ignored by default because CI has no Codex; run
//! it with the binary of `CONFORMANCE_VERSION`:
//!
//! ```text
//! COCKPIT_CODEX_PROGRAM=~/.local/npm/bin/codex \
//!   cargo nextest run -p cockpit-codex --test conformance --run-ignored only
//! ```
//!
//! Each test runs Codex with a fresh `CODEX_HOME`, so the owner's configuration, trust and
//! login are never read. The live turn needs no login: the hooks run before the model call,
//! which then fails.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cockpit_codex::{Bridge, CONFORMANCE_VERSION, Codex, HOOKS_FEATURE, LAUNCH_FLAGS, hooks};
use cockpit_provider_api::{Conversation, LaunchRequest, Provider};

/// The installed Codex the ignored live tests run.
fn installed() -> PathBuf {
    PathBuf::from(
        std::env::var_os("COCKPIT_CODEX_PROGRAM")
            .expect("COCKPIT_CODEX_PROGRAM names the Codex program to test"),
    )
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

        for flag in LAUNCH_FLAGS {
            assert!(help.contains(flag), "{flag} is not in {command:?}");
        }
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
}

/// The `--enable` and `--config` pairs a new launch passes, for `codex app-server`, which
/// takes the same configuration as the TUI.
fn launch_configuration(bridge: Bridge) -> Vec<String> {
    let codex = Codex::new(installed(), None, Some(bridge));
    let command = codex
        .launch(&LaunchRequest {
            working_dir: Path::new("/"),
            conversation: &Conversation::New,
            first_message: None,
            effort: None,
            remote_control: false,
        })
        .unwrap();
    let words = common::words(&command.arguments);
    words
        .windows(2)
        .filter(|pair| pair[0] == "--enable" || pair[0] == "--config")
        .flat_map(|pair| pair.to_vec())
        .collect()
}

/// `codex app-server` over standard input and output, in a fresh home.
struct AppServer {
    child: Child,
    input: ChildStdin,
    lines: mpsc::Receiver<serde_json::Value>,
    next_id: u64,
    _home: tempfile::TempDir,
}

impl AppServer {
    fn start(configuration: &[String], work: &Path) -> AppServer {
        let home = tempfile::tempdir().unwrap();
        let mut child = Command::new(installed())
            .arg("app-server")
            .args(configuration)
            .current_dir(work)
            .env("CODEX_HOME", home.path())
            .env("COCKPIT_LAUNCH_TOKEN", "token-live")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in output.lines().map_while(Result::ok) {
                if let Ok(message) = serde_json::from_str(&line)
                    && sender.send(message).is_err()
                {
                    return;
                }
            }
        });
        let mut server = AppServer {
            child,
            input,
            lines,
            next_id: 0,
            _home: home,
        };
        server.request(
            "initialize",
            serde_json::json!({"clientInfo": {"name": "cockpit-conformance", "version": "0"}}),
        );
        server.send(serde_json::json!({"method": "initialized"}));
        server
    }

    fn send(&mut self, message: serde_json::Value) {
        writeln!(self.input, "{message}").unwrap();
        self.input.flush().unwrap();
    }

    /// The result of `method`, waiting up to 30 seconds and skipping notifications.
    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(serde_json::json!({"id": id, "method": method, "params": params}));
        let ends_at = Instant::now() + Duration::from_secs(30);
        loop {
            let remaining = ends_at.saturating_duration_since(Instant::now());
            let message = self
                .lines
                .recv_timeout(remaining)
                .unwrap_or_else(|_| panic!("{method} did not answer"));
            if message["id"] == id {
                assert!(message.get("error").is_none(), "{method}: {message}");
                return message["result"].clone();
            }
        }
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Codex runs a hook only when its trusted hash matches: a wrong `hooks::trusted_hash`
/// would leave every Cockpit hook `untrusted`, so events would silently never arrive.
#[test]
#[ignore = "needs an installed Codex (COCKPIT_CODEX_PROGRAM)"]
fn codex_trusts_every_hook_a_launch_subscribes() {
    let recorder = Recorder::new();
    let work = tempfile::tempdir().unwrap();
    let mut server = AppServer::start(&launch_configuration(recorder.bridge()), work.path());

    let listed = server.request(
        "hooks/list",
        serde_json::json!({"cwds": [work.path().to_str().unwrap()]}),
    );

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
fn a_real_turns_prompt_reports_working_through_the_bridge_naming_its_thread() {
    let recorder = Recorder::new();
    let work = tempfile::tempdir().unwrap();
    let mut server = AppServer::start(&launch_configuration(recorder.bridge()), work.path());
    let started = server.request(
        "thread/start",
        serde_json::json!({"cwd": work.path().to_str().unwrap()}),
    );
    let thread = started["thread"]["id"].as_str().unwrap().to_owned();

    server.request(
        "turn/start",
        serde_json::json!({"threadId": thread, "input": [{"type": "text", "text": "hello"}]}),
    );

    let ends_at = Instant::now() + Duration::from_secs(30);
    let prompt_report = loop {
        let reports = recorder.reports();
        if let Some(report) = reports
            .into_iter()
            .find(|report| report.contains(&"--prompt".to_owned()))
        {
            break report;
        }
        assert!(
            Instant::now() < ends_at,
            "no prompt report: {:?}",
            recorder.reports()
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(
        prompt_report,
        [
            "token-live",
            "agent",
            "report",
            "--state",
            "working",
            "--prompt",
            "--conversation",
            &thread
        ]
    );
}
