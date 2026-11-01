//! The in-sandbox hook bridge, run as Codex runs it: the payload on standard input, the
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

fn run_hook(cockpit: &Path, payload: &str, token: Option<&str>) -> Output {
    let mut hook = Command::new(env!("CARGO_BIN_EXE_cockpit-codex-hook"));
    hook.arg(cockpit)
        .env_remove("COCKPIT_LAUNCH_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(token) = token {
        hook.env("COCKPIT_LAUNCH_TOKEN", token);
    }
    let mut child = hook.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn a_prompt_reports_working_through_the_cockpit_program_with_the_launch_token() {
    let cockpit = FakeCockpit::exiting_with(0);
    let output = run_hook(
        &cockpit.program(),
        &common::fixture("user-prompt-submit"),
        Some("token-1"),
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        cockpit.arguments().unwrap(),
        [
            "agent",
            "report",
            "--state",
            "working",
            "--prompt",
            "--conversation",
            "01a19800-e8f3-70e3-b788-d9601b03debb"
        ]
    );
    assert_eq!(cockpit.token(), "token-1");
}

/// Codex reads a hook's standard output as its answer: a permission request must never be
/// approved or denied by the bridge.
#[test]
fn the_bridge_never_answers_codex() {
    let cockpit = FakeCockpit::exiting_with(0);
    let output = run_hook(
        &cockpit.program(),
        &common::fixture("permission-request"),
        None,
    );

    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn a_subagents_event_runs_nothing() {
    let cockpit = FakeCockpit::exiting_with(0);
    let output = run_hook(
        &cockpit.program(),
        &common::fixture("post-tool-use-subagent"),
        None,
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(cockpit.arguments(), None);
}

/// Exit 2 would block Codex's prompt or stop; a failed report must only be shown.
#[test]
fn a_refused_report_exits_1_and_never_2() {
    let cockpit = FakeCockpit::exiting_with(2);
    let output = run_hook(&cockpit.program(), &common::fixture("stop"), None);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
}

#[test]
fn a_payload_that_is_not_a_hook_event_exits_1_saying_why() {
    let cockpit = FakeCockpit::exiting_with(0);
    let output = run_hook(&cockpit.program(), "not json", None);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).starts_with("cockpit-codex-hook: "),
        "{output:?}"
    );
    assert_eq!(cockpit.arguments(), None);
}

#[test]
fn a_missing_cockpit_program_exits_1() {
    let output = run_hook(
        Path::new("/nonexistent/cockpit"),
        &common::fixture("stop"),
        None,
    );

    assert_eq!(output.status.code(), Some(1), "{output:?}");
}

#[test]
fn without_its_cockpit_argument_it_exits_1_with_its_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_cockpit-codex-hook"))
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("usage"),
        "{output:?}"
    );
}
