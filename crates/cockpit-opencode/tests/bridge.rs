//! The in-sandbox bridge, run as the plugin runs it: the event on standard input, the
//! `cockpit` program as its argument. A fake `cockpit` records what it was asked.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct FakeCockpit {
    dir: tempfile::TempDir,
}

impl FakeCockpit {
    /// A `cockpit` that records its arguments and launch token, then exits with `status`.
    fn exiting_with(status: i32) -> FakeCockpit {
        let dir = tempfile::tempdir().unwrap();
        common::script(
            dir.path(),
            "cockpit",
            &format!(
                "printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/arguments\"\n\
                 printf '%s' \"$COCKPIT_LAUNCH_TOKEN\" > \"$(dirname \"$0\")/token\"\n\
                 echo 'reported for session s000000000001'\nexit {status}"
            ),
        );
        FakeCockpit { dir }
    }

    fn program(&self) -> PathBuf {
        self.dir.path().join("cockpit")
    }

    /// The arguments it was run with; `None` when it never ran.
    fn arguments(&self) -> Option<Vec<String>> {
        let text = std::fs::read_to_string(self.dir.path().join("arguments")).ok()?;
        Some(text.lines().map(str::to_owned).collect())
    }

    fn token(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("token")).unwrap()
    }
}

fn run_bridge(cockpit: &Path, event: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cockpit-opencode-bridge"))
        .arg(cockpit)
        .env("COCKPIT_LAUNCH_TOKEN", "token-1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn an_ended_turn_reports_its_outcome_through_the_cockpit_program() {
    let cockpit = FakeCockpit::exiting_with(0);

    let output = run_bridge(
        &cockpit.program(),
        &common::fixture("message-assistant-stop"),
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        cockpit.arguments().unwrap(),
        [
            "agent",
            "report",
            "--state",
            "waiting",
            "--outcome",
            "completed",
            "--conversation",
            "ses_1c4f2e8a3ffeJ7qkR2mWbXzD0v",
        ]
    );
    assert_eq!(cockpit.token(), "token-1");
}

/// OpenCode shows nothing of a plugin's child, but a bridge that talks would still be noise.
#[test]
fn the_bridge_prints_nothing_on_standard_output() {
    let cockpit = FakeCockpit::exiting_with(0);

    let output = run_bridge(&cockpit.program(), &common::fixture("session-status-busy"));

    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
}

#[test]
fn an_event_that_proves_no_activity_runs_no_report() {
    let cockpit = FakeCockpit::exiting_with(0);

    let output = run_bridge(&cockpit.program(), &common::fixture("tool-running"));

    assert!(output.status.success(), "{output:?}");
    assert_eq!(cockpit.arguments(), None);
}

#[test]
fn a_refused_report_exits_1_naming_it() {
    let cockpit = FakeCockpit::exiting_with(3);

    let output = run_bridge(&cockpit.program(), &common::fixture("session-status-idle"));

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).starts_with("cockpit-opencode-bridge: "),
        "{output:?}"
    );
}

#[test]
fn an_event_that_is_not_json_exits_1() {
    let cockpit = FakeCockpit::exiting_with(0);

    let output = run_bridge(&cockpit.program(), "not json");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(cockpit.arguments(), None);
}

#[test]
fn the_bridge_answers_its_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_cockpit-opencode-bridge"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        cockpit_opencode::bridge_version_line()
    );
}
