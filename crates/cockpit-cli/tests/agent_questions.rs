//! `cockpit agent report --dialog` and `--prompt`, and the owner's view naming a question's
//! source (#182, decided in #200), driven as processes against a scripted daemon socket. The
//! daemon's side is proven in `cockpit-daemon/tests/agent_questions.rs`.

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    AgentReport, AgentState, Command, Outcome, QuestionSource, Reply, SessionReport,
};
use scripted::{scripted_daemon, text};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    /// Where the daemon puts a socket, `relative` to its socket directory.
    fn socket(&self, relative: &str) -> PathBuf {
        let path = self.dir.path().join("run/cockpit").join(relative);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    /// Runs `cockpit` with `arguments` in this home, off the test's runtime threads.
    async fn cockpit(&self, arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"));
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

fn dialog_report() -> SessionReport {
    SessionReport {
        session: "s1".to_owned(),
        state: Some(AgentState::Waiting),
        label: None,
        doing: None,
        needs_input: Some("Allow Bash?".to_owned()),
        question_source: Some(QuestionSource::Dialog),
        outcome: None,
        reported_at: 1,
    }
}

#[tokio::test]
async fn reports_send_a_dialog_and_a_new_prompt_as_their_own_fields() {
    let cases: [(&[&str], AgentReport); 2] = [
        (
            &["--state", "waiting", "--dialog", "Allow Bash?"],
            AgentReport {
                state: Some(AgentState::Waiting),
                dialog: Some("Allow Bash?".to_owned()),
                ..AgentReport::default()
            },
        ),
        (
            &["--state", "working", "--prompt"],
            AgentReport {
                state: Some(AgentState::Working),
                is_new_prompt: true,
                ..AgentReport::default()
            },
        ),
    ];
    for (flags, expected) in cases {
        let home = Home::new();
        let daemon = scripted_daemon(
            &home.socket("agent/agent.sock"),
            Outcome::Ok(Reply::AgentReported {
                report: Box::new(dialog_report()),
            }),
        );
        let arguments: Vec<&str> = ["agent", "report"]
            .into_iter()
            .chain(flags.iter().copied())
            .collect();

        let output = home.cockpit(&arguments).await;

        assert!(
            output.status.success(),
            "{flags:?}: {}",
            text(&output.stderr)
        );
        assert_eq!(
            daemon.await.unwrap(),
            Command::AgentReport {
                launch_token: None,
                report: expected,
            },
            "{flags:?}"
        );
    }
}

#[tokio::test]
async fn the_owner_sees_a_dialogs_question_as_a_dialog() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.socket("daemon.sock"),
        Outcome::Ok(Reply::AgentReports {
            reports: vec![dialog_report()],
        }),
    );

    let output = home.cockpit(&["agent", "reports"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "s1 waiting dialog=\"Allow Bash?\"\n");
}

#[tokio::test]
async fn a_repeated_prompt_flag_exits_2_before_any_connection() {
    let home = Home::new();

    let output = home
        .cockpit(&["agent", "report", "--prompt", "--prompt"])
        .await;

    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).contains("--prompt given twice"),
        "{}",
        text(&output.stderr)
    );
}
