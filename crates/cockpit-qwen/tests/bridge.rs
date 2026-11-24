//! The in-sandbox hook bridge, run as Qwen Code runs it: the payload on standard input, the
//! `torlek` program as its argument. A fake `torlek` records what it was asked.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BRIEF: &str = "# Torlek launch brief";

struct FakeTorlek {
    dir: tempfile::TempDir,
}

impl FakeTorlek {
    /// A `torlek` whose reports record their arguments and launch token and exit with
    /// `status`, and whose `brief` prints [`BRIEF`] and exits with `brief_status`.
    fn exiting_with(status: i32, brief_status: i32) -> FakeTorlek {
        let dir = tempfile::tempdir().unwrap();
        common::script(
            dir.path(),
            "torlek",
            &format!(
                "if [ \"$1\" = brief ]; then printf '{BRIEF}\\n'; exit {brief_status}; fi\n\
                 printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/arguments\"\n\
                 printf '%s' \"$TORLEK_LAUNCH_TOKEN\" > \"$(dirname \"$0\")/token\"\n\
                 exit {status}"
            ),
        );
        FakeTorlek { dir }
    }

    fn program(&self) -> PathBuf {
        self.dir.path().join("torlek")
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

fn run_hook(torlek: &Path, payload: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_torlek-qwen-hook"))
        .arg(torlek)
        .env("TORLEK_LAUNCH_TOKEN", "launch-token-1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn a_prompt_reports_working_for_its_conversation_with_the_launch_token() {
    let torlek = FakeTorlek::exiting_with(0, 0);
    let output = run_hook(&torlek.program(), &common::fixture("user-prompt-submit"));

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        torlek.arguments().unwrap(),
        [
            "agent",
            "report",
            "--state",
            "working",
            "--prompt",
            "--conversation",
            common::FIXTURE_SESSION
        ]
    );
    assert_eq!(torlek.token(), "launch-token-1");
    assert!(output.stdout.is_empty());
}

#[test]
fn a_session_start_reports_and_prints_the_brief_as_additional_context() {
    let torlek = FakeTorlek::exiting_with(0, 0);
    let output = run_hook(&torlek.program(), &common::fixture("session-start-startup"));

    assert!(output.status.success(), "{output:?}");
    assert!(torlek.arguments().is_some());
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        answer["hookSpecificOutput"]["additionalContext"],
        format!("{BRIEF}\n")
    );
    assert_eq!(
        answer["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
}

/// Exit 2 would block Qwen Code's stop or prompt.
#[test]
fn a_failed_report_exits_1_never_2() {
    let torlek = FakeTorlek::exiting_with(3, 0);
    let output = run_hook(&torlek.program(), &common::fixture("stop"));

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed"));
}

#[test]
fn a_failed_brief_still_reports_and_exits_1() {
    let torlek = FakeTorlek::exiting_with(0, 4);
    let output = run_hook(&torlek.program(), &common::fixture("session-start-startup"));

    assert_eq!(output.status.code(), Some(1));
    assert!(torlek.arguments().is_some());
    assert!(output.stdout.is_empty());
}

#[test]
fn an_event_that_proves_nothing_runs_no_report() {
    let torlek = FakeTorlek::exiting_with(0, 0);
    let output = run_hook(&torlek.program(), &common::fixture("session-end-exit"));

    assert!(output.status.success(), "{output:?}");
    assert_eq!(torlek.arguments(), None);
}

#[test]
fn a_payload_that_is_not_json_exits_1() {
    let torlek = FakeTorlek::exiting_with(0, 0);
    let output = run_hook(&torlek.program(), "not json");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(torlek.arguments(), None);
}

#[test]
fn version_prints_the_hook_s_version_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_torlek-qwen-hook"))
        .arg(cockpit_qwen::HOOK_VERSION_FLAG)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        cockpit_qwen::hook_version_line()
    );
}
