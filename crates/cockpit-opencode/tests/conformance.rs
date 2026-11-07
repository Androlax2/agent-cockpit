//! Conformance against an installed OpenCode. Ignored by default because CI has no
//! OpenCode; run it with the OpenCode to check (the nightly job runs the latest, #732):
//!
//! ```text
//! COCKPIT_OPENCODE_PROGRAM=~/.opencode/bin/opencode \
//!   cargo nextest run -p cockpit-opencode --test conformance --run-ignored only
//! ```

mod common;

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cockpit_opencode::{OpenCode, Server};
use cockpit_provider_api::{Conversation, LaunchRequest, Provider};

#[test]
#[ignore = "needs an installed OpenCode (COCKPIT_OPENCODE_PROGRAM)"]
fn the_installed_opencode_prints_a_version_the_adapter_reads() {
    let opencode = OpenCode::discover(&common::installed(), None).unwrap();

    assert!(opencode.version().is_some());
}

#[test]
#[ignore = "needs an installed OpenCode (COCKPIT_OPENCODE_PROGRAM)"]
fn the_installed_opencode_documents_every_flag_the_adapter_passes() {
    let output = Command::new(common::installed())
        .arg("--help")
        .output()
        .unwrap();
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    for flag in cockpit_opencode::LAUNCH_FLAGS {
        assert!(help.contains(flag), "{flag} is not in --help");
    }
}

/// A variable the binary no longer reads would silently drop the password or the plugin.
#[test]
#[ignore = "needs an installed OpenCode (COCKPIT_OPENCODE_PROGRAM)"]
fn the_installed_opencode_reads_every_variable_the_adapter_sets() {
    let binary = std::fs::read(common::installed()).unwrap();

    for name in cockpit_opencode::OPENCODE_VARIABLES {
        assert!(
            binary
                .windows(name.len())
                .any(|window| window == name.as_bytes()),
            "{name} is not in the binary"
        );
    }
}

/// The server a launch's words start answers its own password only, with its version, and
/// lists native commands. `serve` runs the same server as the TUI without a terminal.
#[test]
#[ignore = "needs an installed OpenCode (COCKPIT_OPENCODE_PROGRAM)"]
fn a_launched_server_answers_its_password_and_lists_commands() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let discovered = OpenCode::discover(&common::installed(), None).unwrap();
    let opencode = OpenCode::new(common::installed(), None, None);
    let launch = opencode
        .launch(&LaunchRequest {
            working_dir: project.path(),
            session_dir: project.path(),
            conversation: &Conversation::New,
            first_message: None,
            effort: None,
            model: None,
            remote_control: false,
        })
        .unwrap();
    let server = Server::from_launch(&launch).unwrap();
    let _running = Running(
        Command::new(&launch.program)
            .arg("serve")
            .args(&launch.arguments)
            .envs(launch.environment.iter().map(|(name, value)| (name, value)))
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env("XDG_STATE_HOME", home.path().join("state"))
            .env("XDG_CACHE_HOME", home.path().join("cache"))
            .current_dir(project.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let version = wait_for_health(&server, Duration::from_secs(30));
    let wrong = Server::new(server.port(), "wrong".to_owned())
        .health()
        .unwrap_err();
    let commands = server.commands().unwrap();

    assert_eq!(Some(version.as_str()), discovered.version());
    assert!(wrong.reason.contains("refused"), "{wrong}");
    assert!(!commands.is_empty());
}

fn wait_for_health(server: &Server, deadline: Duration) -> String {
    let ends_at = Instant::now() + deadline;
    loop {
        match server.health() {
            Ok(version) => return version,
            Err(error) if Instant::now() > ends_at => panic!("the server never answered: {error}"),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

/// Kills the server when the test ends, passed or not.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// What the daemon checks at its start on a version not on the tested list (#732); the
/// nightly job runs it on the latest release.
#[test]
#[ignore = "needs an installed OpenCode (COCKPIT_OPENCODE_PROGRAM)"]
fn the_installed_opencode_passes_the_runtime_probe() {
    let opencode = OpenCode::discover(&common::installed(), None).unwrap();

    let found = cockpit_opencode::probe(&common::installed(), opencode.version().unwrap()).unwrap();

    assert!(found.is_clean(), "{}", found.summary());
}
