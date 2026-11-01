//! Issue #39 through `cockpit workflow`: the owner's verbs against the real daemon, and a
//! launched session's verbs against a scripted agent socket (the crate graph keeps the CLI's
//! tests from launching a session; the daemon's side of those is proven in
//! `cockpit-daemon/tests/workflows.rs`).
mod common;
mod scripted;

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;

use cockpit_protocol::{
    Command, Outcome, Reply, WorkflowHandoffView, WorkflowNextView, WorkflowStatusView,
    WorkflowView,
};
use common::TestHome;
use scripted::{scripted_daemon, text};

/// The real daemon with project `app` registered and item 1 filed in it.
fn with_item() -> (TestHome, common::RunningDaemon) {
    let home = TestHome::new();
    let daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    home.cockpit_ok(&[
        "todo",
        "add",
        "Fix the login",
        "--project",
        "app",
        "--kind",
        "bug",
    ]);
    (home, daemon)
}

fn the_line(output: &str, prefix: &str) -> String {
    output
        .lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix:?} line in {output:?}"))
        .to_owned()
}

const HANDOFF: [&str; 6] = [
    "--changed",
    "Rewrote the validation",
    "--checked",
    "commit abc123",
    "--outcome",
    "12 passed",
];

#[test]
fn the_owner_starts_a_bug_on_code_change_and_the_list_shows_who_acts_next() {
    let (home, _daemon) = with_item();

    home.cockpit_ok(&["workflow", "start", "1"]);

    assert_eq!(
        home.cockpit_ok(&["workflow", "list"]),
        "1\tapp\t1\tcode-change\trunning\tstep 1 waits for a session to begin it\n"
    );
}

#[test]
fn a_step_with_requirements_completes_once_the_owner_waives_them() {
    let (home, _daemon) = with_item();
    home.cockpit_ok(&["workflow", "start", "1"]);
    home.cockpit_ok(&[&["workflow", "complete", "1"][..], &HANDOFF].concat());

    let unmet = home.cockpit_err(&[&["workflow", "complete", "1"][..], &HANDOFF].concat());
    let waived = home.cockpit_ok(
        &[
            &["workflow", "complete", "1"][..],
            &HANDOFF,
            &["--waive", "checks", "--waive", "review"],
        ]
        .concat(),
    );

    assert!(unmet.contains("checks"), "{unmet}");
    assert_eq!(
        the_line(&waived, "next:"),
        "next: step 3 waits for a session to begin it"
    );
}

#[test]
fn the_packet_carries_the_goal_and_the_previous_steps_handoff() {
    let (home, _daemon) = with_item();
    home.cockpit_ok(&["workflow", "start", "1", "--preset", "research"]);
    home.cockpit_ok(&[&["workflow", "complete", "1"][..], &HANDOFF].concat());

    let packet = home.cockpit_ok(&["workflow", "packet", "1"]);

    assert_eq!(
        the_line(&packet, "goal:"),
        "goal: item 1 Fix the login (bug, todo)"
    );
    assert_eq!(
        the_line(&packet, "handoff from"),
        "handoff from step 1 investigate (completed): changed: Rewrote the validation; \
         checked: commit abc123; outcome: 12 passed"
    );
    assert_eq!(the_line(&packet, "step "), "step 2 synthesize (ready)");
}

#[test]
fn the_presets_say_which_steps_need_evidence() {
    let (home, _daemon) = with_item();

    let presets = home.cockpit_ok(&["workflow", "presets"]);

    assert!(
        presets.contains("  2 check (agent) requires checks, review\n"),
        "{presets}"
    );
}

#[test]
fn a_completion_without_its_outcome_is_a_usage_error() {
    let home = TestHome::new();

    let output = home.cockpit(&[
        "workflow",
        "complete",
        "1",
        "--changed",
        "a",
        "--checked",
        "b",
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("--outcome is required"));
}

#[test]
fn an_unknown_blocker_action_is_a_usage_error_naming_the_known_ones() {
    let home = TestHome::new();

    let output = home.cockpit(&["workflow", "block", "1", "pay-invoice", "--detail", "x"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("answer-question, approve-operation"));
}

/// A home with only the agent socket, as a sandbox sees it.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        Sandbox {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn agent_socket(&self) -> PathBuf {
        let path = self.dir.path().join("run/cockpit/agent/agent.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    /// Runs `cockpit` with `environment`, off the test's runtime threads.
    async fn cockpit_with(&self, arguments: &[&str], environment: &[(&str, &str)]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"))
            .envs(environment.iter().copied());
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }

    /// Runs `cockpit` as an agent launched with `the-token`.
    async fn cockpit(&self, arguments: &[&str]) -> Output {
        self.cockpit_with(arguments, &[("COCKPIT_LAUNCH_TOKEN", "the-token")])
            .await
    }
}

fn begun_view() -> WorkflowView {
    WorkflowView {
        id: 3,
        item: 1,
        project: "app".to_owned(),
        preset: "code-change".to_owned(),
        preset_version: 1,
        status: WorkflowStatusView::Running,
        intent_revision: 1,
        blocked: None,
        steps: Vec::new(),
        next: WorkflowNextView::InProgress {
            position: 1,
            session: Some("s0123456789ab".to_owned()),
        },
        created_at: 10,
        changed_at: 10,
    }
}

fn answered_with_workflow() -> Outcome {
    Outcome::Ok(Reply::Workflow {
        workflow: Box::new(begun_view()),
    })
}

#[tokio::test]
async fn a_session_begins_a_step_on_the_agent_socket_with_its_launch_token() {
    let sandbox = Sandbox::new();
    let daemon = scripted_daemon(&sandbox.agent_socket(), answered_with_workflow());

    let output = sandbox.cockpit(&["workflow", "begin", "3"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::WorkflowBegin {
            launch_token: Some("the-token".to_owned()),
            workflow: 3,
        }
    );
    assert_eq!(
        the_line(&text(&output.stdout), "next:"),
        "next: session s0123456789ab works on step 1"
    );
}

#[tokio::test]
async fn a_checkpoint_goes_to_the_agent_socket_even_without_a_launch_token() {
    let sandbox = Sandbox::new();
    let daemon = scripted_daemon(&sandbox.agent_socket(), answered_with_workflow());

    let output = sandbox
        .cockpit_with(&["workflow", "checkpoint", "3", "Tests next"], &[])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::WorkflowCheckpoint {
            launch_token: None,
            workflow: 3,
            note: "Tests next".to_owned(),
        }
    );
}

#[tokio::test]
async fn a_session_releases_its_step_with_a_handoff() {
    let sandbox = Sandbox::new();
    let daemon = scripted_daemon(&sandbox.agent_socket(), answered_with_workflow());

    let output = sandbox
        .cockpit(
            &[
                &["workflow", "release", "3"][..],
                &HANDOFF,
                &["--evidence", "receipt 7"],
            ]
            .concat(),
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::WorkflowRelease {
            launch_token: Some("the-token".to_owned()),
            workflow: 3,
            handoff: Some(WorkflowHandoffView {
                changed: "Rewrote the validation".to_owned(),
                checked: "commit abc123".to_owned(),
                outcome: "12 passed".to_owned(),
                needs_human: String::new(),
                evidence: vec!["receipt 7".to_owned()],
            }),
        }
    );
}
