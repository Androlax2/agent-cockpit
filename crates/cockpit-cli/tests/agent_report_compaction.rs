//! `cockpit agent report --compaction*` and how `cockpit agent reports` shows a compaction
//! (#782), driven as processes against a scripted daemon socket.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    AgentReport, AgentState, Command, CompactionPhase, CompactionReport, CompactionStatus,
    CompactionTrigger, CompactionView, Outcome, Reply, SessionReport,
};
use scripted::{scripted_daemon, text};

/// A home whose runtime directory holds the daemon's socket directory.
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

fn session_report(compaction: Option<CompactionView>) -> SessionReport {
    SessionReport {
        compaction,
        session: "s1".to_owned(),
        state: Some(AgentState::Waiting),
        label: None,
        doing: None,
        needs_input: None,
        question_source: None,
        covered_question: None,
        held_by: None,
        outcome: None,
        reported_at: 1,
    }
}

#[tokio::test]
async fn a_compaction_start_sends_its_trigger_and_size() {
    let home = tempfile::tempdir().unwrap();
    let daemon = scripted_daemon(
        &socket(home.path(), "agent/agent.sock"),
        Outcome::Ok(Reply::AgentReported {
            report: Box::new(session_report(None)),
        }),
    );

    let output = cockpit(
        home.path(),
        &[
            "agent",
            "report",
            "--compaction",
            "started",
            "--compaction-trigger",
            "auto",
            "--compaction-tokens-before",
            "268364",
        ],
    )
    .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::AgentReport {
            launch_token: None,
            report: AgentReport {
                compaction: Some(CompactionReport {
                    phase: CompactionPhase::Started,
                    trigger: Some(CompactionTrigger::Auto),
                    tokens_before: Some(268_364),
                    tokens_after: None,
                }),
                ..AgentReport::default()
            },
        }
    );
}

#[tokio::test]
async fn each_compaction_phase_flag_sends_its_phase() {
    let cases = [
        ("started", CompactionPhase::Started),
        ("ended", CompactionPhase::Ended),
        ("failed", CompactionPhase::Failed),
    ];
    for (flag, phase) in cases {
        let home = tempfile::tempdir().unwrap();
        let daemon = scripted_daemon(
            &socket(home.path(), "agent/agent.sock"),
            Outcome::Ok(Reply::AgentReported {
                report: Box::new(session_report(None)),
            }),
        );

        let output = cockpit(home.path(), &["agent", "report", "--compaction", flag]).await;

        assert!(output.status.success(), "{flag}: {}", text(&output.stderr));
        assert_eq!(
            daemon.await.unwrap(),
            Command::AgentReport {
                launch_token: None,
                report: AgentReport {
                    compaction: Some(CompactionReport {
                        phase,
                        trigger: None,
                        tokens_before: None,
                        tokens_after: None,
                    }),
                    ..AgentReport::default()
                },
            },
            "{flag}"
        );
    }
}

#[tokio::test]
async fn malformed_compaction_arguments_exit_2_before_any_connection() {
    let cases: [&[&str]; 7] = [
        &["--compaction", "paused"],
        &["--compaction-trigger", "auto"],
        &["--compaction-tokens-after", "10"],
        &[
            "--compaction",
            "started",
            "--compaction-trigger",
            "sometimes",
        ],
        &["--compaction", "ended", "--compaction-tokens-after", "many"],
        &["--compaction", "started", "--state", "working"],
        &["--compaction", "started", "--compaction", "ended"],
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

#[tokio::test]
async fn the_owner_sees_the_latest_compaction_with_its_sizes_and_seconds() {
    let home = tempfile::tempdir().unwrap();
    let _daemon = scripted_daemon(
        &socket(home.path(), "daemon.sock"),
        Outcome::Ok(Reply::AgentReports {
            reports: vec![session_report(Some(CompactionView {
                status: CompactionStatus::Completed,
                trigger: Some(CompactionTrigger::Manual),
                tokens_before: Some(268_364),
                tokens_after: Some(30_470),
                seconds: Some(41),
                at: 1,
            }))],
        }),
    );

    let output = cockpit(home.path(), &["agent", "reports"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s1 waiting compaction=completed compaction-trigger=manual \
         compaction-tokens-before=268364 compaction-tokens-after=30470 compaction-seconds=41\n"
    );
}
