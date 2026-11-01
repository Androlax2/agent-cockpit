//! `cockpit agent reports` shows the agent's own question waiting under an open dialog as
//! `also-waiting` (#250), driven as a process against a scripted daemon socket. The daemon's
//! side is proven in `cockpit-daemon/tests/agent_question_resets.rs`.

use std::os::unix::fs::DirBuilderExt;
use std::process::Output;

mod scripted;

use cockpit_protocol::{AgentState, Outcome, QuestionSource, Reply, SessionReport};
use scripted::{scripted_daemon, text};

async fn agent_reports(report: SessionReport) -> Output {
    let home = tempfile::tempdir().unwrap();
    let socket = home.path().join("run/cockpit/daemon.sock");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(socket.parent().unwrap())
        .unwrap();
    let _daemon = scripted_daemon(
        &socket,
        Outcome::Ok(Reply::AgentReports {
            reports: vec![report],
        }),
    );
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
    command
        .args(["agent", "reports"])
        .env_clear()
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", home.path().join("run"));
    tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap()
}

fn dialog(covered_question: Option<&str>) -> SessionReport {
    SessionReport {
        session: "s1".to_owned(),
        state: Some(AgentState::Waiting),
        label: None,
        doing: None,
        needs_input: Some("Allow Bash?".to_owned()),
        question_source: Some(QuestionSource::Dialog),
        covered_question: covered_question.map(str::to_owned),
        outcome: None,
        reported_at: 1,
    }
}

#[tokio::test]
async fn the_owner_sees_the_agents_question_waiting_under_a_dialog() {
    let output = agent_reports(dialog(Some("Which database?"))).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s1 waiting dialog=\"Allow Bash?\" also-waiting=\"Which database?\"\n"
    );
}

#[tokio::test]
async fn a_dialog_over_no_question_shows_nothing_else_waiting() {
    let output = agent_reports(dialog(None)).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "s1 waiting dialog=\"Allow Bash?\"\n");
}
