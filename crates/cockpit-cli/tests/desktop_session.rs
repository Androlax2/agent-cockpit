//! `cockpit desktop-session import` driven as a process against a scripted daemon socket:
//! the variables it sends from its own environment, and what it says back (#307). That a
//! launched agent then gets them is proven in `cockpit-daemon/tests/agent_environment.rs`.

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;

mod scripted;

use cockpit_protocol::{Command, Outcome, Reply};
use scripted::{scripted_daemon, text};

/// Runs `cockpit desktop-session import` in a fresh home with `variables` as its only extra
/// environment, against a daemon that answers `withheld`; returns its output and request.
async fn import(variables: &[(&str, &str)], withheld: bool) -> (Output, Command) {
    let home = tempfile::tempdir().unwrap();
    let runtime_dir: PathBuf = home.path().join("run");
    let socket = runtime_dir.join("cockpit-dev/daemon.sock");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(socket.parent().unwrap())
        .unwrap();
    let daemon = scripted_daemon(
        &socket,
        Outcome::Ok(Reply::DesktopSessionImported { withheld }),
    );
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
    command
        .args(["desktop-session", "import"])
        .env_clear()
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .envs(variables.iter().copied());
    let output = tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap();
    (output, daemon.await.unwrap())
}

#[tokio::test]
async fn import_sends_the_desktop_variables_this_shell_has_and_no_other() {
    let (output, sent) = import(
        &[
            ("DISPLAY", ":1"),
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("TERM", "xterm"),
        ],
        false,
    )
    .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "");
    assert_eq!(
        sent,
        Command::DesktopSessionImport {
            variables: [
                ("DISPLAY".to_owned(), ":1".to_owned()),
                ("WAYLAND_DISPLAY".to_owned(), "wayland-1".to_owned()),
            ]
            .into(),
        }
    );
}

#[tokio::test]
async fn import_into_a_daemon_that_withholds_the_desktop_session_says_so_and_succeeds() {
    let (output, _) = import(&[("DISPLAY", ":1")], true).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("agents.desktop_session = \"withheld\""),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn import_from_a_shell_without_a_desktop_session_warns_that_agents_get_none() {
    let (output, sent) = import(&[], false).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("no desktop session variable is set here"),
        "{}",
        text(&output.stderr)
    );
    assert_eq!(
        sent,
        Command::DesktopSessionImport {
            variables: Default::default(),
        }
    );
}
