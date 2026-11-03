//! #417: native events from the pinned harness, with a local model and no account.

#[path = "common/bounded_command.rs"]
mod bounded_command;
mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use cockpit_opencode::{Bridge, CONFORMANCE_VERSION, OpenCode, PLUGIN_SOURCE};
use cockpit_provider_api::{Conversation, LaunchRequest, Provider};
use serde_json::json;

#[test]
#[ignore = "needs approved OpenCode 1.19.5 and Python; isolated local model, no account access"]
fn the_native_plugin_captures_tool_permission_and_aborted_turns() {
    assert!(
        std::env::var_os("OPENCODE_CONFIG_CONTENT").is_none(),
        "unset inherited OPENCODE_CONFIG_CONTENT for this isolated test"
    );
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let program = common::installed().canonicalize().unwrap();
    let version_home = root.join("version-home");
    std::fs::create_dir(&version_home).unwrap();
    let version = bounded_command::output(
        Command::new(&program)
            .arg("--version")
            .env_clear()
            .env("HOME", &version_home)
            .env("XDG_CONFIG_HOME", version_home.join("config"))
            .env("XDG_DATA_HOME", version_home.join("data"))
            .env("XDG_CACHE_HOME", version_home.join("cache"))
            .env("XDG_STATE_HOME", version_home.join("state"))
            .current_dir(&version_home),
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(version.status.success(), "{version:?}");
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        CONFORMANCE_VERSION,
    );
    let work = root.join("work");
    std::fs::create_dir(&work).unwrap();
    let plugin = root.join("cockpit plugin.mjs");
    std::fs::write(&plugin, PLUGIN_SOURCE).unwrap();
    let script = root.join("native_plugin.py");
    std::fs::write(&script, include_str!("common/native_plugin.py")).unwrap();
    let recorder = root.join("record-bridge");
    let cockpit = root.join("cockpit-fixture");
    for program in [&recorder, &cockpit] {
        std::fs::copy(&script, program).unwrap();
        std::fs::set_permissions(program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let provider = OpenCode::new(
        program,
        Some(CONFORMANCE_VERSION.to_owned()),
        Some(Bridge {
            plugin,
            bridge_program: recorder,
            cockpit_program: cockpit,
        }),
    );
    let launch = provider
        .launch(&LaunchRequest {
            working_dir: &work,
            session_dir: root,
            conversation: &Conversation::New,
            first_message: None,
            effort: None,
            model: None,
            remote_control: false,
        })
        .unwrap();
    let config = root.join("launch.json");
    std::fs::write(&config, json!({
        "program": launch.program,
        "arguments": launch.arguments.iter().map(|v| v.to_str().unwrap()).collect::<Vec<_>>(),
        "environment": launch.environment.iter().map(|(k,v)| (k.to_str().unwrap(),v.to_str().unwrap())).collect::<std::collections::BTreeMap<_,_>>(),
        "bridge": env!("CARGO_BIN_EXE_torlek-opencode-bridge"),
        "observed_version": CONFORMANCE_VERSION,
    }).to_string()).unwrap();
    let mut command = Command::new("python3");
    command
        .arg(&script)
        .arg("run")
        .arg(&config)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", root.join("home"))
        .current_dir(root);
    if let Some(record) = std::env::var_os("COCKPIT_OPENCODE_CAPTURE_DIR") {
        command.env("COCKPIT_OPENCODE_CAPTURE_DIR", record);
    }
    let output = run_probe(&mut command, root, Duration::from_secs(90)).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
}

fn run_probe(command: &mut Command, root: &Path, timeout: Duration) -> Result<Output, String> {
    let result = bounded_command::output(command, timeout);
    // A deadline/output-limit kill skips Python's finally block. Restore permissions
    // after its entire process group has stopped so TempDir can still remove the fixture.
    match std::fs::set_permissions(
        root.join("home/config/opencode"),
        std::fs::Permissions::from_mode(0o755),
    ) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot restore fixture config permissions: {error}"
            ));
        }
    }
    result
}

#[test]
fn a_forced_probe_timeout_leaves_the_fixture_removable() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("home/config/opencode");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join(".gitignore"), "node_modules\n").unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o555)).unwrap();

    let result = run_probe(
        Command::new("/bin/sh").args(["-c", "sleep 30 & wait"]),
        dir.path(),
        Duration::from_millis(150),
    );
    let cleanup = std::fs::remove_dir_all(dir.path());
    // A failing regression must still remove its own deliberately read-only fixture.
    if cleanup.is_err() {
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755)).unwrap();
        dir.close().unwrap();
    }
    assert!(result.unwrap_err().contains("timed out"));
    assert!(
        cleanup.is_ok(),
        "forced probe stop stranded its fixture: {cleanup:?}"
    );
}
