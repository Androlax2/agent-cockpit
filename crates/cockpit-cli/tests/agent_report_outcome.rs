//! `cockpit agent report --outcome` and how `cockpit agent reports` shows it (#183), driven as
//! processes against a scripted daemon socket.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    AgentReport, AgentState, Command, Outcome, Reply, SessionReport, TurnOutcome,
};
use scripted::{scripted_daemon, text};

/// A home whose runtime directory holds the daemon's socket directory.
fn runtime_dir(home: &Path) -> PathBuf {
    home.join("run")
}

fn socket(home: &Path, relative: &str) -> PathBuf {
    let path = runtime_dir(home).join("cockpit").join(relative);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path.parent().unwrap())
        .unwrap();
    path
}

async fn cockpit(home: &Path, arguments: &[&str]) -> Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
    command
        .args(arguments)
        .env_clear()
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", runtime_dir(home));
    tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap()
}

fn session_report(outcome: Option<TurnOutcome>) -> SessionReport {
    SessionReport {
        session: "s1".to_owned(),
        state: Some(AgentState::Waiting),
        label: None,
        doing: None,
        needs_input: None,
        question_source: None,
        outcome,
        reported_at: 1,
    }
}

#[tokio::test]
async fn each_outcome_flag_sends_its_outcome() {
    let cases: [(&[&str], TurnOutcome); 4] = [
        (&["--outcome", "completed"], TurnOutcome::Completed),
        (&["--outcome", "quota-stopped"], TurnOutcome::QuotaStopped),
        (
            &["--outcome", "failed"],
            TurnOutcome::Failed { reason: None },
        ),
        (
            &["--outcome-reason", "server_error", "--outcome", "failed"],
            TurnOutcome::Failed {
                reason: Some("server_error".to_owned()),
            },
        ),
    ];
    for (flags, outcome) in cases {
        let home = tempfile::tempdir().unwrap();
        let daemon = scripted_daemon(
            &socket(home.path(), "agent/agent.sock"),
            Outcome::Ok(Reply::AgentReported {
                report: Box::new(session_report(None)),
            }),
        );
        let arguments: Vec<&str> = ["agent", "report", "--state", "waiting"]
            .into_iter()
            .chain(flags.iter().copied())
            .collect();

        let output = cockpit(home.path(), &arguments).await;

        assert!(
            output.status.success(),
            "{flags:?}: {}",
            text(&output.stderr)
        );
        assert_eq!(
            daemon.await.unwrap(),
            Command::AgentReport {
                launch_token: None,
                report: AgentReport {
                    state: Some(AgentState::Waiting),
                    outcome: Some(outcome),
                    ..AgentReport::default()
                },
            },
            "{flags:?}"
        );
    }
}

#[tokio::test]
async fn malformed_outcome_arguments_exit_2_before_any_connection() {
    let cases: [&[&str]; 5] = [
        &["--outcome", "abandoned"],
        &["--outcome-reason", "server_error"],
        &["--outcome", "completed", "--outcome-reason", "server_error"],
        &["--outcome", "failed", "--outcome", "completed"],
        &["--outcome"],
    ];
    let home = tempfile::tempdir().unwrap();
    for flags in cases {
        let arguments: Vec<&str> = ["agent", "report"]
            .into_iter()
            .chain(flags.iter().copied())
            .collect();

        let output = cockpit(home.path(), &arguments).await;

        assert_eq!(output.status.code(), Some(2), "{flags:?}");
        assert!(
            text(&output.stderr).contains("usage:"),
            "{flags:?}: {}",
            text(&output.stderr)
        );
    }
}

#[tokio::test]
async fn an_unknown_outcome_is_named_even_with_a_reason() {
    let home = tempfile::tempdir().unwrap();

    let output = cockpit(
        home.path(),
        &[
            "agent",
            "report",
            "--outcome",
            "finished",
            "--outcome-reason",
            "x",
        ],
    )
    .await;

    assert!(
        text(&output.stderr).starts_with("cockpit: --outcome finished: use completed"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn the_owner_sees_a_failed_turn_and_its_reason() {
    let home = tempfile::tempdir().unwrap();
    let _daemon = scripted_daemon(
        &socket(home.path(), "daemon.sock"),
        Outcome::Ok(Reply::AgentReports {
            reports: vec![session_report(Some(TurnOutcome::Failed {
                reason: Some("server_error".to_owned()),
            }))],
        }),
    );

    let output = cockpit(home.path(), &["agent", "reports"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s1 waiting outcome=failed outcome-reason=\"server_error\"\n"
    );
}
