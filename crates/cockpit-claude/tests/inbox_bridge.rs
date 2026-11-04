//! #703: the hook bridge's inbox waiter, run as Claude Code runs an `asyncRewake` hook: the
//! payload on standard input, the `cockpit` program and `--inbox` as its arguments. A fake
//! `cockpit` answers `agent inbox` and records what it was asked.

mod common;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const MESSAGE: &str = "[Torlek] Message from session s000000000002 (a collaborator, not the owner): \
                       Base it on main.";

/// A `cockpit` whose `agent inbox` prints `printed` and exits with `status`, recording its
/// arguments and launch token in `dir`.
fn fake_cockpit(dir: &Path, printed: &str, status: i32) {
    common::script(
        dir,
        "cockpit",
        &format!(
            "printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/arguments\"\n\
             printf '%s' \"$TORLEK_LAUNCH_TOKEN\" > \"$(dirname \"$0\")/token\"\n\
             printf '{printed}'\nexit {status}"
        ),
    );
}

fn run_waiter(dir: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_torlek-claude-hook"))
        .arg(dir.join("cockpit"))
        .arg("--inbox")
        .env("TORLEK_LAUNCH_TOKEN", "token-1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(common::fixture("stop").as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn a_collected_message_wakes_the_model_on_standard_error_with_status_2() {
    let dir = tempfile::tempdir().unwrap();
    fake_cockpit(dir.path(), &format!("{MESSAGE}\\n"), 0);

    let output = run_waiter(dir.path());

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("{MESSAGE}\n")
    );
    assert!(output.stdout.is_empty(), "{output:?}");
}

#[test]
fn the_waiter_asks_agent_inbox_with_the_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    fake_cockpit(dir.path(), "", 0);

    run_waiter(dir.path());

    let arguments = std::fs::read_to_string(dir.path().join("arguments")).unwrap();
    assert_eq!(arguments.lines().collect::<Vec<_>>(), ["agent", "inbox"]);
    let token = std::fs::read_to_string(dir.path().join("token")).unwrap();
    assert_eq!(token, "token-1");
}

/// A newer waiter took over: nothing wakes the model.
#[test]
fn a_superseded_waiter_ends_quietly() {
    let dir = tempfile::tempdir().unwrap();
    fake_cockpit(dir.path(), "", 0);

    let output = run_waiter(dir.path());

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

/// A failure is a hook error, never status 2: it must not wake the model with an error text
/// as if it were a message.
#[test]
fn a_failed_inbox_exits_1_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    fake_cockpit(dir.path(), "", 126);

    let output = run_waiter(dir.path());

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("`torlek agent inbox` failed"),
        "{output:?}"
    );
}

/// With status 2 the model reads all of the hook's standard error, so only the message goes
/// there, never what `agent inbox` warned about while it waited.
#[test]
fn only_the_message_wakes_the_model_never_the_inbox_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    common::script(
        dir.path(),
        "cockpit",
        &format!("echo 'torlek: a warning' >&2\nprintf '%s\\n' '{MESSAGE}'"),
    );

    let output = run_waiter(dir.path());

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("{MESSAGE}\n")
    );
}

#[test]
fn a_failed_inbox_names_what_it_said() {
    let dir = tempfile::tempdir().unwrap();
    common::script(
        dir.path(),
        "cockpit",
        "echo 'torlek: cannot reach the Torlek daemon' >&2\nexit 126",
    );

    let output = run_waiter(dir.path());

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cannot reach the Torlek daemon"),
        "{output:?}"
    );
}
