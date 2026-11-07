mod candidate;
mod common;

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, ErrorCode, Reply};
use common::{GENEROUS, TestHome};
use serde_json::{Value, json};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn set(level: &str, duration: Option<&str>) -> Command {
    let mut wire = json!({"kind":"logs-level-set","directives":level});
    if let Some(duration) = duration {
        wire["duration"] = json!(duration);
    }
    serde_json::from_value(wire).unwrap()
}
fn action(kind: &str) -> Command {
    serde_json::from_value(json!({"kind":kind})).unwrap()
}
async fn status(owner: &Connection) -> Value {
    serde_json::to_value(
        owner
            .request(action("logs-level-status"), GENEROUS)
            .await
            .unwrap(),
    )
    .unwrap()
}
/// Writes an owner configuration with database backups off: a backup is heavy database work
/// that, under a debug or trace override, floods the very log these tests read (#406).
fn without_database_backups(config_file: &std::path::Path) {
    std::fs::create_dir_all(config_file.parent().unwrap()).unwrap();
    std::fs::write(config_file, "[storage]\ndatabase_backups_kept = 0\n").unwrap();
}

/// The owner's configuration file a [`TestHome`] daemon reads.
fn owner_config(home: &TestHome) -> std::path::PathBuf {
    home.home()
        .join("config")
        .join(common::BUILD_PROFILE_DIR)
        .join("config.toml")
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}

#[tokio::test]
async fn timed_override_changes_the_installed_filter_and_expires_to_startup_baseline() {
    let mut home = TestHome::new();
    without_database_backups(&owner_config(&home));
    home.log_level = Some("error".into());
    let mut daemon = home.start();
    let owner = Connection::open(&home.socket_path(), GENEROUS)
        .await
        .unwrap();
    assert_eq!(status(&owner).await["directives"], "error");
    let before = epoch_ms();
    refuse_handshake(&home).await;
    owner.request(set("info", None), GENEROUS).await.unwrap();
    refuse_handshake(&home).await;
    let deadline = Instant::now() + GENEROUS;
    while !std::fs::read_to_string(home.log_file())
        .unwrap()
        .contains("refused a handshake")
    {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let active = status(&owner).await;
    assert_eq!(active["directives"], "info");
    assert_eq!(active["reverts_to"], "error");
    let expiry = active["expires_at_unix_ms"].as_u64().unwrap();
    assert!((before + 300_000..=epoch_ms() + 300_000).contains(&expiry));
    owner
        .request(set("debug", Some("1s")), GENEROUS)
        .await
        .unwrap();
    let limit = Instant::now() + GENEROUS;
    while !status(&owner).await["expires_at_unix_ms"].is_null() {
        assert!(Instant::now() < limit, "override did not expire");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(status(&owner).await["directives"], "error");
    refuse_handshake(&home).await;
    drop(owner);
    assert!(
        std::process::Command::new("kill")
            .args(["-TERM", &daemon.child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(daemon.child.wait().unwrap().success());
    let log = std::fs::read_to_string(home.log_file()).unwrap();
    assert_eq!(
        log.matches("refused a handshake").count(),
        1,
        "the running filter must change, then return to error: {log}"
    );
    assert!(
        log.contains("log level override set") && log.contains("log level reverted"),
        "{log}"
    );
}

#[tokio::test]
async fn old_timers_do_not_revert_replacements_and_restart_clears_the_override() {
    let mut home = TestHome::new();
    without_database_backups(&owner_config(&home));
    home.log_level = Some("warn".into());
    let daemon = home.start();
    let owner = Connection::open(&home.socket_path(), GENEROUS)
        .await
        .unwrap();
    owner
        .request(set("debug", Some("1s")), GENEROUS)
        .await
        .unwrap();
    owner
        .request(set("trace", Some("1m")), GENEROUS)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(status(&owner).await["directives"], "trace");
    owner
        .request(set("debug", Some("1s")), GENEROUS)
        .await
        .unwrap();
    owner
        .request(action("logs-level-revert"), GENEROUS)
        .await
        .unwrap();
    owner
        .request(set("info", Some("1m")), GENEROUS)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(status(&owner).await["directives"], "info");
    drop(owner);
    daemon.kill();
    let restarted = home.start();
    let owner = Connection::open(&home.socket_path(), GENEROUS)
        .await
        .unwrap();
    assert_eq!(
        status(&owner).await,
        json!({"kind":"logs-level","directives":"warn","expires_at_unix_ms":null,"reverts_to":null})
    );
    drop(owner);
    restarted.kill();
}

#[tokio::test]
async fn all_log_commands_are_refused_on_the_agent_socket_without_changing_level() {
    let home = TestHome::new();
    let _daemon = home.start();
    let owner = Connection::open(&home.socket_path(), GENEROUS)
        .await
        .unwrap();
    let agent = Connection::open(&home.agent_socket_path(), GENEROUS)
        .await
        .unwrap();
    let before = status(&owner).await;
    for command in [
        action("logs-level-status"),
        set("loud", Some("0s")),
        action("logs-level-revert"),
    ] {
        let answer = agent.request(command, GENEROUS).await;
        assert!(
            matches!(answer, Err(ClientError::Daemon(ref e)) if e.code == ErrorCode::NotAuthorized),
            "{answer:?}"
        );
        assert_eq!(status(&owner).await, before);
    }
}

#[tokio::test]
async fn an_owner_without_the_capability_cannot_read_set_or_revert_levels() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let home = TestHome::new();
    let _daemon = home.start();
    let owner = Connection::open(&home.socket_path(), GENEROUS)
        .await
        .unwrap();
    let before = status(&owner).await;
    let stream = tokio::net::UnixStream::connect(home.socket_path())
        .await
        .unwrap();
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    write
        .write_all(b"{\"protocol_versions\":[1],\"capabilities\":[]}\n")
        .await
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(GENEROUS, read.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    for command in [
        action("logs-level-status"),
        set("debug", None),
        action("logs-level-revert"),
    ] {
        let wire = json!({"id":1,"command":command}).to_string() + "\n";
        write.write_all(wire.as_bytes()).await.unwrap();
        line.clear();
        tokio::time::timeout(GENEROUS, read.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            reply["outcome"]["error"]["code"], "not-negotiated",
            "{reply}"
        );
        assert_eq!(status(&owner).await, before);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn trace_override_keeps_fake_provider_content_out_of_logs_and_the_existing_bundle() {
    use cockpit_daemon::{Instance, Limits, Options, Routes};
    use cockpit_protocol::BugBundleView;
    use cockpit_provider_api::{ProviderName, fake::FakeProvider};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    let dir = tempfile::Builder::new()
        .prefix("log canary ")
        .tempdir()
        .unwrap();
    let root = dir.path().canonicalize().unwrap();
    without_database_backups(&root.join("home/.config/torlek/config.toml"));
    let _tmux = PrivateTmux(root.clone());
    let state_dir = root.join("state/cockpit");
    let log_dir = cockpit_diagnostics::log_dir_in(&root.join("state"), "daemon").unwrap();
    let mut log_options =
        cockpit_diagnostics::Options::new(log_dir.clone(), "warn".parse().unwrap());
    // Keep the whole short trace window below the existing 64 MiB bundle bound.
    log_options.limits.max_file_bytes = 16 * 1024 * 1024;
    log_options.limits.max_files = 3;
    let diagnostics = cockpit_diagnostics::start(log_options).unwrap();
    tracing::dispatcher::set_global_default(diagnostics.dispatch().clone()).unwrap();
    let program = root.join("agent");
    let marker = root.join("environment-seen");
    std::fs::write(
        &program,
        r#"#!/bin/sh
echo output-canary-137
printf '%s' "$CONTENT_CANARY" > "$CANARY_MARKER"
exec sleep 60
"#,
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let agent_log = root.join("agent.log");
    let provider =
        FakeProvider::new(ProviderName::new("fake").unwrap(), agent_log.clone()).running(program);
    let mut wiring = common::wiring_in(&root, vec![Arc::new(provider)]);
    wiring.sessions.base_environment.extend([
        ("CONTENT_CANARY".into(), "environment-canary-137".into()),
        ("CANARY_MARKER".into(), marker.as_os_str().to_owned()),
    ]);
    let environment = wiring.environment.clone();
    let daemon = Instance::acquire(Options {
        runtime_dir: root.join("run/cockpit"),
        state_dir,
        limits: Limits::default(),
    })
    .unwrap()
    .start(wiring, |capabilities| {
        Arc::new(Routes::new(capabilities, environment).with_log_level(diagnostics.level().clone()))
    })
    .await
    .unwrap();
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    owner
        .request(set("trace", Some("1m")), GENEROUS)
        .await
        .unwrap();
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    for args in [
        vec!["init", "--quiet", "--initial-branch=main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=f@example.com",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .current_dir(&project)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    owner
        .request(
            Command::ProjectAdd {
                path: project.display().to_string(),
                name: None,
            },
            GENEROUS,
        )
        .await
        .unwrap();
    let started = owner
        .request(
            Command::SessionNew {
                dir: project.display().to_string(),
                agent: Some("fake".into()),
                prompt: Some("prompt-canary-137".into()),
                clone: None,
                model: None,
                effort: None,
                compaction_threshold: None,
            },
            GENEROUS,
        )
        .await
        .unwrap();
    let Reply::Session { session } = started else {
        panic!("{started:?}")
    };
    let launches = common::once_logged(1, || FakeProvider::launches(&agent_log)).await;
    assert_eq!(launches[0].message, "prompt-canary-137");
    let deadline = Instant::now() + GENEROUS;
    while !marker.exists() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        std::fs::read_to_string(marker).unwrap(),
        "environment-canary-137"
    );
    owner
        .request(
            Command::SessionEnd {
                session: session.id,
            },
            GENEROUS,
        )
        .await
        .unwrap();
    tracing::debug!(target: "cockpit_daemon", "fixture filter control");
    // Drain before inspecting every retained rotation and the existing bundle.
    assert!(diagnostics.shutdown(GENEROUS));
    let filed = owner
        .request(
            Command::BugReportNew {
                description: "runtime diagnostics fixture".into(),
                title: None,
                steps: String::new(),
                since: None,
                cli_version: None,
            },
            GENEROUS,
        )
        .await
        .unwrap();
    let Reply::BugReport { report } = filed else {
        panic!("{filed:?}")
    };
    let BugBundleView::Written { path, .. } = &report.bundle else {
        panic!("{:?}", report.bundle)
    };
    let bundle = std::fs::read(path).unwrap();
    let log: Vec<u8> = std::fs::read_dir(&log_dir)
        .unwrap()
        .flat_map(|entry| {
            let entry = entry.unwrap();
            std::fs::read(entry.path()).unwrap()
        })
        .collect();
    let text = String::from_utf8_lossy(&log);
    assert!(
        text.contains("log level override set"),
        "the start of the canary window must not have rotated away"
    );
    assert!(text.contains("fixture filter control"));
    for content in [
        "prompt-canary-137",
        "output-canary-137",
        "environment-canary-137",
    ] {
        for bytes in [&log, &bundle] {
            assert!(
                !bytes
                    .windows(content.len())
                    .any(|part| part == content.as_bytes()),
                "hosted content escaped: {content}"
            );
        }
    }
    drop(owner);
    drop(daemon);
}

struct PrivateTmux(std::path::PathBuf);
impl Drop for PrivateTmux {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .arg("-S")
            .arg(self.0.join("tmux.sock"))
            .arg("kill-server")
            .env_remove("TMUX")
            .output();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_sink_does_not_hold_log_control_or_ping() {
    use cockpit_daemon::{Instance, Limits, Options, Routes};
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let logs = root.path().join("logs");
    std::fs::create_dir(&logs).unwrap();
    let fifo = logs.join("cockpit.log");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let held_fifo = std::fs::File::options()
        .read(true)
        .write(true)
        .open(fifo)
        .unwrap();
    let mut options = cockpit_diagnostics::Options::new(logs, "info".parse().unwrap());
    options.queue_records = 1;
    let diagnostics = cockpit_diagnostics::start(options).unwrap();
    tracing::dispatcher::with_default(diagnostics.dispatch(), || {
        for index in 0..20_000 {
            tracing::info!(index, padding = "x".repeat(200), "fixture operation");
        }
    });
    assert!(diagnostics.dropped_records() > 0);
    let daemon = Instance::acquire(Options {
        runtime_dir: root.path().join("run"),
        state_dir: root.path().join("state/cockpit"),
        limits: Limits::default(),
    })
    .unwrap()
    .start(common::wiring_in(root.path(), Vec::new()), |capabilities| {
        Arc::new(
            Routes::new(capabilities, cockpit_config::Environment::default())
                .with_log_level(diagnostics.level().clone()),
        )
    })
    .await
    .unwrap();
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    for command in [
        action("logs-level-status"),
        set("trace", None),
        action("logs-level-revert"),
        Command::Ping,
    ] {
        owner
            .request(command, Duration::from_secs(1))
            .await
            .unwrap();
    }
    drop(owner);
    drop(daemon);
    assert!(!diagnostics.shutdown(Duration::from_millis(100)));
    drop(held_fifo);
}

async fn refuse_handshake(home: &TestHome) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut stream = tokio::net::UnixStream::connect(home.socket_path())
        .await
        .unwrap();
    stream
        .write_all(b"{\"protocol_versions\":[99]}\n")
        .await
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(GENEROUS, BufReader::new(stream).read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert!(line.contains("refused"), "{line}");
}
