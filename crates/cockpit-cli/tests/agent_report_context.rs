//! The harness's own context from the command line (#479): `cockpit agent report
//! --context-tokens N [--context-window N]` sends it with a turn's outcome, and `cockpit brief
//! --measure` prints it in tokens beside Cockpit's bytes. Driven as processes against a
//! scripted daemon socket.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    AgentReport, AgentState, Command, ContextBudgetView, ContextUsage, Outcome, Reply,
    SessionReport, TurnOutcome,
};
use scripted::{scripted_daemon, text};

fn runtime_dir(home: &Path) -> PathBuf {
    home.join("run")
}

fn socket(home: &Path, relative: &str) -> PathBuf {
    let path = runtime_dir(home).join("torlek-dev").join(relative);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path.parent().unwrap())
        .unwrap();
    path
}

async fn cockpit(home: &Path, arguments: &[&str]) -> Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_torlek"));
    command
        .args(arguments)
        .env_clear()
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", runtime_dir(home));
    tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap()
}

fn session_report() -> SessionReport {
    SessionReport {
        session: "s1".to_owned(),
        state: Some(AgentState::Waiting),
        label: None,
        doing: None,
        needs_input: None,
        question_source: None,
        covered_question: None,
        held_by: None,
        outcome: Some(TurnOutcome::Completed),
        reported_at: 1,
        compaction: None,
    }
}

#[tokio::test]
async fn a_turns_end_sends_the_harness_context_with_its_window() {
    let home = tempfile::tempdir().unwrap();
    let daemon = scripted_daemon(
        &socket(home.path(), "agent/agent.sock"),
        Outcome::Ok(Reply::AgentReported {
            report: Box::new(session_report()),
        }),
    );

    let output = cockpit(
        home.path(),
        &[
            "agent",
            "report",
            "--state",
            "waiting",
            "--outcome",
            "completed",
            "--context-tokens",
            "45123",
            "--context-window",
            "258400",
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
                outcome: Some(TurnOutcome::Completed),
                context: Some(ContextUsage {
                    tokens: 45_123,
                    window: Some(258_400),
                }),
                ..AgentReport::default()
            },
        }
    );
}

#[tokio::test]
async fn malformed_context_arguments_exit_2_before_any_connection() {
    let cases: [&[&str]; 5] = [
        &["--state", "waiting", "--context-tokens", "45123"],
        &["--outcome", "completed", "--context-window", "258400"],
        &["--outcome", "completed", "--context-tokens", "many"],
        &["--outcome", "completed", "--context-tokens", "-1"],
        &[
            "--outcome",
            "completed",
            "--context-tokens",
            "1",
            "--context-tokens",
            "2",
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
            text(&output.stderr).starts_with("torlek: "),
            "{flags:?}: {}",
            text(&output.stderr)
        );
    }
}

fn brief_reply(harness_context: Option<ContextUsage>) -> Outcome {
    Outcome::Ok(Reply::AgentBrief {
        brief: "brief".to_owned(),
        packet: None,
        manifest: "m-1".to_owned(),
        budget: Box::new(ContextBudgetView {
            brief_bytes: 1115,
            packet_bytes: 0,
            catalog_bytes: 276,
            mcp_instructions_bytes: 0,
            pack_catalog_bytes: 0,
            on_demand_bytes: 9862,
        }),
        harness_context,
    })
}

async fn measured(harness_context: Option<ContextUsage>) -> String {
    let home = tempfile::tempdir().unwrap();
    let daemon = scripted_daemon(
        &socket(home.path(), "daemon.sock"),
        brief_reply(harness_context),
    );

    let output = cockpit(home.path(), &["brief", "--measure"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    daemon.await.unwrap();
    text(&output.stdout)
}

#[tokio::test]
async fn brief_measure_prints_the_harness_tokens_beside_cockpits_bytes() {
    let printed = measured(Some(ContextUsage {
        tokens: 45_123,
        window: Some(258_400),
    }))
    .await;

    assert!(
        printed.starts_with("always loaded: 1391 bytes"),
        "{printed}"
    );
    assert!(
        printed.contains(
            "\nharness context: 45123 tokens at the latest turn's end, of a 258400-token window\n"
        ),
        "{printed}"
    );
}

#[tokio::test]
async fn brief_measure_prints_the_harness_tokens_without_a_window_it_was_not_given() {
    let printed = measured(Some(ContextUsage {
        tokens: 45_123,
        window: None,
    }))
    .await;

    assert!(
        printed.contains("\nharness context: 45123 tokens at the latest turn's end\n"),
        "{printed}"
    );
}

#[tokio::test]
async fn brief_measure_says_not_reported_yet_before_the_first_turn_ends() {
    let printed = measured(None).await;

    assert!(
        printed.contains("\nharness context: not reported yet"),
        "{printed}"
    );
}
