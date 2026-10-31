//! `cockpit agent report --conversation` and `--conversation-ends` (#219), driven as
//! processes against a scripted daemon socket. The daemon's check is proven in
//! `cockpit-daemon/tests/agent_conversation.rs`.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Output;

mod scripted;

use cockpit_protocol::{AgentReport, AgentState, Command, Outcome, Reply, SessionReport};
use scripted::{scripted_daemon, text};

fn runtime_dir(home: &Path) -> PathBuf {
    home.join("run")
}

fn agent_socket(home: &Path) -> PathBuf {
    let path = runtime_dir(home).join("cockpit/agent/agent.sock");
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

fn reported() -> Outcome {
    Outcome::Ok(Reply::AgentReported {
        report: Box::new(SessionReport {
            session: "s1".to_owned(),
            state: Some(AgentState::Waiting),
            label: None,
            doing: None,
            needs_input: None,
            question_source: None,
            outcome: None,
            reported_at: 1,
        }),
    })
}

#[tokio::test]
async fn a_report_carries_the_conversation_it_comes_from() {
    let home = tempfile::tempdir().unwrap();
    let daemon = scripted_daemon(&agent_socket(home.path()), reported());

    let output = cockpit(
        home.path(),
        &[
            "agent",
            "report",
            "--state",
            "waiting",
            "--conversation",
            "c-1",
        ],
    )
    .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::AgentReport {
            launch_token: None,
            report: AgentReport {
                state: Some(AgentState::Waiting),
                conversation: Some("c-1".to_owned()),
                ..AgentReport::default()
            },
        }
    );
}

#[tokio::test]
async fn a_conversation_announces_its_end_alone() {
    let home = tempfile::tempdir().unwrap();
    let daemon = scripted_daemon(&agent_socket(home.path()), reported());

    let output = cockpit(
        home.path(),
        &[
            "agent",
            "report",
            "--conversation",
            "c-1",
            "--conversation-ends",
        ],
    )
    .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::AgentReport {
            launch_token: None,
            report: AgentReport {
                conversation: Some("c-1".to_owned()),
                is_conversation_ending: true,
                ..AgentReport::default()
            },
        }
    );
}

#[tokio::test]
async fn malformed_conversation_arguments_exit_2_before_any_connection() {
    let cases: [&[&str]; 5] = [
        &[
            "--conversation",
            "c-1",
            "--conversation-ends",
            "--state",
            "waiting",
        ],
        &["--conversation-ends"],
        &["--conversation", "c-1"],
        &[
            "--conversation",
            "c-1",
            "--conversation",
            "c-2",
            "--state",
            "working",
        ],
        &[
            "--conversation-ends",
            "--conversation-ends",
            "--conversation",
            "c-1",
        ],
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
