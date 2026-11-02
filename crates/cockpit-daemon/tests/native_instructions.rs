//! #419: real daemon launches, native harnesses and loopback model input. No accounts.

mod real_daemon;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cockpit_protocol::{Command as Request, Reply};
use real_daemon::{Home, cockpit_program};
use serde_json::{Value, json};

const PROMPT: &str = "Reply with only OK.";

#[test]
#[cfg(target_os = "linux")]
fn the_instruction_fixture_refuses_unsafe_inputs_and_false_evidence() {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new("python3")
        .args(["-I", "-B", "-m", "unittest", "discover", "-s"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/instruction_fixture"))
        .args(["-p", "test_*.py", "-v"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home.path())
        .current_dir(home.path())
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "fixture safety tests: {status}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("fixture safety tests timed out");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit Claude 2.1.316 path; local model, fresh home, no account"]
async fn claude_receives_the_daemon_brief_and_native_skill() {
    probe("claude", "COCKPIT_CLAUDE_PROGRAM", "2.1.316").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit Codex 0.166.0 path; local model, fresh home, no account"]
async fn codex_receives_the_daemon_brief_without_claiming_a_native_skill() {
    probe("codex", "COCKPIT_CODEX_PROGRAM", "0.166.0").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit approved OpenCode 1.19.5 path; local model, fresh home, no account"]
async fn opencode_receives_the_daemon_brief_and_native_skill() {
    probe("opencode", "COCKPIT_OPENCODE_PROGRAM", "1.19.5").await;
}

async fn probe(provider: &str, variable: &str, version: &str) {
    let program = std::fs::canonicalize(std::env::var_os(variable).expect(variable)).unwrap();
    for sibling in [
        "cockpit",
        "cockpit-claude-hook",
        "cockpit-codex-hook",
        "cockpit-codex-session",
        "cockpit-opencode-bridge",
    ] {
        assert!(
            cockpit_program().with_file_name(sibling).is_file(),
            "build {sibling}"
        );
    }
    let home = Home::new();
    let script = home.path().join("native.py");
    std::fs::write(&script, include_str!("instruction_fixture/native.py")).unwrap();
    let config = home.path().join("fixture.json");
    std::fs::write(
        &config,
        json!({
            "root": home.path(), "provider": provider, "program": program,
            "version": version, "prompt": PROMPT,
        })
        .to_string(),
    )
    .unwrap();
    // All discovered programs are fixtures. No fallback to an owner's provider on PATH.
    for name in ["claude", "codex", "opencode"] {
        let wrapper = home.path().join("bin").join(name);
        let body = if name == provider {
            format!(
                "#!/bin/sh\nexec python3 {} launch {} \"$@\"\n",
                quote(&script),
                quote(&config)
            )
        } else {
            "#!/bin/sh\nexit 1\n".to_owned()
        };
        std::fs::write(&wrapper, body).unwrap();
        std::fs::set_permissions(wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _permissions = WritableOnDrop(home.path().join("config/opencode"));
    let mut model = Model {
        root: home.path().to_owned(),
        child: Command::new("python3")
            .arg(&script)
            .arg("serve")
            .arg(&config)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home.path())
            .current_dir(home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(home.path().join("fixture.stderr")).unwrap())
            .spawn()
            .unwrap(),
    };
    let ready = home.path().join("ready");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(
            model.child.try_wait().unwrap().is_none(),
            "{}",
            diagnostics(&home)
        );
        assert!(
            Instant::now() < deadline,
            "fixture did not start: {}",
            diagnostics(&home)
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let path = format!("{}:/usr/bin:/bin", home.path().join("bin").display());
    let _daemon = home.start_with(&[("PATH", &path), ("TERM", "xterm-256color")]);
    let client = home.connect().await;
    // The Python fixture creates this repository with isolated Git configuration.
    // Home::project currently inherits the caller's Git environment (separate follow-up).
    let project = home.path().join("projects/native-instructions");
    let added = client
        .request(
            Request::ProjectAdd {
                path: project.display().to_string(),
                name: None,
            },
            real_daemon::GENEROUS,
        )
        .await;
    assert!(
        matches!(added, Ok(Reply::ProjectAdded { .. })),
        "project add: {added:?}"
    );
    let reply = client
        .request(
            Request::SessionNew {
                dir: project.display().to_string(),
                agent: Some(provider.to_owned()),
                prompt: Some(PROMPT.to_owned()),
                clone: None,
            },
            Duration::from_secs(15),
        )
        .await;
    let Ok(Reply::Session { session }) = reply else {
        panic!("native session: {reply:?}\n{}", diagnostics(&home));
    };
    let deadline = Instant::now() + Duration::from_secs(35);
    let captured = loop {
        if let Some(request) = read_capture(&home.path()).unwrap() {
            break request;
        }
        assert!(
            model.child.try_wait().unwrap().is_none(),
            "{}",
            diagnostics(&home)
        );
        assert!(
            Instant::now() < deadline,
            "no native model input: {}",
            diagnostics(&home)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let context = captured["context"].as_str().unwrap();
    for expected in [
        "# Cockpit launch brief",
        &format!("Session {}", session.id),
        project.to_str().unwrap(),
        "cockpit ask run --reason TEXT COMMAND",
        &cockpit_agent_context::manifest().id,
        "Prose grants nothing",
        "profile dev",
    ] {
        assert!(
            context.contains(expected),
            "missing {expected:?} in native context: {captured}"
        );
    }
    assert_eq!(captured["prompt"], PROMPT);
    assert!(
        captured["mcp"].as_bool().unwrap(),
        "MCP tools missing: {captured}"
    );
    assert_eq!(
        captured["skill"],
        provider != "codex",
        "native skill: {captured}"
    );
    let bound = real_daemon::listed(&client, &session.id).await;
    assert!(
        bound.conversation.is_some(),
        "native conversation was not bound"
    );
    model.finish().unwrap();
    let ended = client
        .request(
            Request::SessionEnd {
                session: session.id.clone(),
            },
            Duration::from_secs(15),
        )
        .await;
    assert!(
        matches!(
            ended,
            Ok(Reply::Session { .. }) | Ok(Reply::SessionAction { .. })
        ),
        "session end: {ended:?}"
    );
    let provenance: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("provenance.json")).unwrap())
            .unwrap();
    let receipt = json!({
        "provider": provider, "observed": provenance, "mode": if provider == "claude" { "native-print" } else { "native-tui" },
        "transport": "loopback canned model; real daemon and agent channel; fake sandbox engines",
        "brief": true, "mcp": captured["mcp_advertisement"], "native_cockpit_skill": captured["skill"],
        "signed_in_validation": "not run", "main_request_number": captured["number"],
        "main_request_sha256": captured["request_sha256"],
    });
    println!("{receipt}");
    if let Some(dir) = std::env::var_os("COCKPIT_INSTRUCTION_CAPTURE_DIR") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{provider}.json")),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
    }
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

fn diagnostics(home: &Home) -> String {
    [
        "fixture.stderr",
        "daemon.stderr",
        "launch.log",
        "error.json",
    ]
    .into_iter()
    .map(|name| {
        format!(
            "{name}: {}",
            std::fs::read_to_string(home.path().join(name)).unwrap_or_default()
        )
    })
    .collect::<Vec<_>>()
    .join("\n")
}

struct Model {
    child: Child,
    root: std::path::PathBuf,
}

impl Model {
    fn finish(&mut self) -> Result<(), String> {
        stop_native(&self.root)?;
        std::fs::write(self.root.join("drain"), "")
            .map_err(|error| format!("cannot request model drain: {error}"))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|error| format!("cannot wait for model drain: {error}"))?
            {
                if !status.success() {
                    return Err(format!("model fixture exited with {status}"));
                }
                read_capture(&self.root)?;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("model fixture did not finish draining within ten seconds".to_owned());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

fn stop_native(root: &Path) -> Result<(), String> {
    // Cooperative stop reaches wrappers even if the daemon died during a failed test.
    // Each wrapper retains its native child's waitable PID until its group is killed.
    std::fs::write(root.join("stop"), "")
        .map_err(|error| format!("cannot request native stop: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut entries = match std::fs::read_dir(root.join("probe-processes")) {
            Ok(entries) => entries,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && !root.join("ready").exists() =>
            {
                return Ok(());
            }
            Err(error) => return Err(format!("cannot inspect native cleanup: {error}")),
        };
        match entries.next() {
            None => return Ok(()),
            Some(Err(error)) => return Err(format!("cannot inspect native marker: {error}")),
            Some(Ok(_)) => {}
        }
        if Instant::now() >= deadline {
            return Err("native fixture cleanup did not acknowledge stop".to_owned());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        let cleanup = stop_native(&self.root);
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Err(error) = cleanup {
            if std::thread::panicking() {
                eprintln!("{error}");
            } else {
                panic!("{error}");
            }
        }
    }
}

struct WritableOnDrop(std::path::PathBuf);

fn read_capture(root: &Path) -> Result<Option<Value>, String> {
    let read = |name: &str| match std::fs::read_to_string(root.join(name)) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("cannot read fixture {name}: {error}")),
    };
    let error = read("error.json")?;
    if !error.is_empty() {
        return Err(format!("native model fixture failed: {error}"));
    }
    for line in read("requests.jsonl")?.split_inclusive('\n') {
        // A concurrent writer may not yet have completed its final record.
        if !line.ends_with('\n') {
            break;
        }
        let request: Value = serde_json::from_str(line)
            .map_err(|error| format!("invalid native capture: {error}"))?;
        if request["main"] == true {
            return Ok(Some(request));
        }
    }
    Ok(None)
}

#[test]
fn a_valid_main_capture_never_hides_a_model_fixture_failure() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("requests.jsonl"), "{\"main\":true}\n").unwrap();
    std::fs::write(root.path().join("error.json"), "unexpected model request").unwrap();
    assert!(
        read_capture(root.path())
            .unwrap_err()
            .contains("unexpected model request")
    );
}

#[test]
fn model_shutdown_requires_a_successful_exit_without_late_errors() {
    for (outcome, expected_error) in [
        ("late-error", Some("late model error")),
        ("failed-exit", Some("model fixture exited")),
        ("success", None),
    ] {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("probe-processes")).unwrap();
        std::fs::write(root.path().join("requests.jsonl"), "{\"main\":true}\n").unwrap();
        let mut model = Model {
            root: root.path().to_owned(),
            child: Command::new("python3")
                .args([
                    "-I",
                    "-B",
                    "-c",
                    r"import pathlib, sys, time
root = pathlib.Path.cwd()
deadline = time.monotonic() + 5
while not (root / 'drain').exists():
    if time.monotonic() >= deadline:
        raise RuntimeError('no drain request')
    time.sleep(0.01)
if sys.argv[1] == 'late-error':
    (root / 'error.json').write_text('late model error')
sys.exit(7 if sys.argv[1] == 'failed-exit' else 0)",
                    outcome,
                ])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .current_dir(root.path())
                .stdin(Stdio::null())
                .spawn()
                .unwrap(),
        };
        let result = model.finish();
        match expected_error {
            Some(expected) => assert!(
                result.unwrap_err().contains(expected),
                "shutdown outcome {outcome}"
            ),
            None => result.unwrap(),
        }
    }
}

#[test]
fn cleanup_filesystem_errors_are_not_success() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("stop")).unwrap();
    assert!(
        stop_native(root.path())
            .unwrap_err()
            .contains("cannot request native stop")
    );
    std::fs::remove_dir(root.path().join("stop")).unwrap();
    std::fs::write(root.path().join("probe-processes"), "not a directory").unwrap();
    assert!(
        stop_native(root.path())
            .unwrap_err()
            .contains("cannot inspect native cleanup")
    );
}

impl Drop for WritableOnDrop {
    fn drop(&mut self) {
        if self.0.exists() {
            std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}
