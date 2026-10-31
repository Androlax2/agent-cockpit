//! `cockpit ask run`, `cockpit ask guide`, `cockpit approval` and `cockpit guide` driven as
//! processes against a scripted daemon socket: what each sends, what it prints and the exit
//! codes agents rely on. The daemon's side is proven in `cockpit-daemon/tests/approvals.rs`.

use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Output, Stdio};

mod scripted;

use cockpit_protocol::{
    Command, ErrorCode, FoldedView, GuideStateView, GuideStepView, GuideView, HiddenView, Outcome,
    ProtocolError, Reply, RunRequestView, RunResultView, RunStateView, ShownText, Unblock,
    UnblockedBy,
};
use scripted::{scripted_conversation, text};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    fn socket(&self, relative: &str) -> PathBuf {
        let path = self.runtime_dir().join("cockpit").join(relative);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    fn agent_socket(&self) -> PathBuf {
        self.socket("agent/agent.sock")
    }

    fn owner_socket(&self) -> PathBuf {
        self.socket("daemon.sock")
    }

    /// Runs `cockpit` with `arguments` and `stdin`, as an agent launched with `the-token`
    /// that waits at most `COCKPIT_ASK_WAIT_SECONDS=2`.
    async fn cockpit(&self, arguments: &[&str], stdin: &str) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
        command
            .args(arguments)
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.runtime_dir())
            .env("COCKPIT_LAUNCH_TOKEN", "the-token")
            .env("COCKPIT_ASK_WAIT_SECONDS", "2")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let stdin = stdin.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut child = command.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(stdin.as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        })
        .await
        .unwrap()
    }

    fn cwd(&self) -> String {
        self.dir.path().to_str().unwrap().to_owned()
    }
}

fn shown(text: &str) -> ShownText {
    ShownText {
        text: text.to_owned(),
        has_hidden_characters: false,
        non_ascii: Vec::new(),
        warnings: Vec::new(),
    }
}

fn request(state: RunStateView) -> RunRequestView {
    RunRequestView {
        id: 7,
        session: "s1".to_owned(),
        revision: 1,
        state,
        command: "make test".to_owned(),
        asked_command: None,
        cwd: "/p".to_owned(),
        reason: "tests".to_owned(),
        shown_command: shown("make test"),
        shown_cwd: shown("/p"),
        shown_reason: shown("tests"),
        project: None,
        outside_project: true,
        folded_command: None,
        timeout_without_foreground: false,
        changed_build_files: Some(Vec::new()),
        digest: "d".repeat(64),
        asked_at: 1,
        expires_at: 901,
        waiting_for_input: false,
        result: None,
        note: None,
    }
}

fn run_reply(request: RunRequestView) -> Outcome {
    Outcome::Ok(Reply::RunRequest {
        request: Box::new(request),
    })
}

fn finished(exit_code: i32, output: &str) -> Outcome {
    run_reply(RunRequestView {
        result: Some(RunResultView {
            exit_code,
            output: output.to_owned(),
            omitted_lines: 0,
            truncated_bytes: 0,
            redacted: 0,
        }),
        ..request(RunStateView::Finished)
    })
}

fn guide(state: GuideStateView) -> Outcome {
    guide_reply(guide_view(state))
}

fn guide_reply(guide: GuideView) -> Outcome {
    Outcome::Ok(Reply::Guide {
        guide: Box::new(guide),
    })
}

fn guide_view(state: GuideStateView) -> GuideView {
    let step = |text: &str| GuideStepView {
        section: None,
        text: shown(text),
        open: Vec::new(),
        copy: Vec::new(),
        ask: None,
        run: None,
        answer: None,
        run_request: None,
    };
    GuideView {
        id: 3,
        session: "s1".to_owned(),
        title: shown("Connect the app"),
        cwd: "/p".to_owned(),
        shown_cwd: shown("/p"),
        project: None,
        outside_project: true,
        state,
        current_step: 1,
        steps: vec![step("Open the settings"), step("Copy the id")],
        stuck_note: (state == GuideStateView::Stuck).then(|| "no id shown".to_owned()),
        asked_at: 1,
        expires_at: 901,
    }
}

#[tokio::test]
async fn ask_run_files_the_request_with_the_token_and_exits_with_the_commands_code() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.agent_socket(),
        vec![
            run_reply(request(RunStateView::Asking)),
            finished(3, "the tests failed"),
        ],
    );

    let output = home
        .cockpit(&["ask", "run", "--reason", "tests", "make test"], "")
        .await;

    assert_eq!(output.status.code(), Some(3), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "the tests failed\n");
    let received = daemon.await.unwrap();
    assert_eq!(
        received[0],
        Command::AskRun {
            launch_token: Some("the-token".to_owned()),
            command: "make test".to_owned(),
            cwd: home.cwd(),
            reason: "tests".to_owned(),
        }
    );
    assert_eq!(
        received[1],
        Command::AskRunStatus {
            launch_token: Some("the-token".to_owned()),
            request: 7,
        }
    );
}

#[tokio::test]
async fn each_answer_keeps_its_exit_code() {
    for (outcome, expected) in [
        (run_reply(request(RunStateView::Declined)), 125),
        (run_reply(request(RunStateView::Asking)), 124),
        (
            run_reply(RunRequestView {
                note: Some("the directory moved".to_owned()),
                ..request(RunStateView::NotRun)
            }),
            1,
        ),
    ] {
        let home = Home::new();
        // A request still asking is polled until the wait ends: answer every poll.
        let daemon = scripted_conversation(&home.agent_socket(), vec![outcome; 20]);

        let output = home.cockpit(&["ask", "run", "--wait", "7"], "").await;

        assert_eq!(
            output.status.code(),
            Some(expected),
            "{}",
            text(&output.stderr)
        );
        daemon.abort();
    }
}

#[tokio::test]
async fn a_pending_request_names_the_command_that_waits_on() {
    let home = Home::new();
    let _daemon = scripted_conversation(
        &home.agent_socket(),
        vec![run_reply(request(RunStateView::Asking)); 20],
    );

    let output = home.cockpit(&["ask", "run", "--wait", "7"], "").await;

    assert_eq!(output.status.code(), Some(124));
    assert!(
        text(&output.stderr).contains("cockpit ask run --wait 7"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn an_unreachable_daemon_exits_126_with_the_fix() {
    let home = Home::new();
    home.agent_socket();

    let output = home.cockpit(&["ask", "run", "--check"], "").await;

    assert_eq!(output.status.code(), Some(126));
    assert!(
        text(&output.stderr).contains("to fix:"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn ask_check_reports_a_refusal_with_what_unblocks_it() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.agent_socket(),
        vec![Outcome::Error(ProtocolError {
            code: ErrorCode::NotAuthorized,
            message: "the session holds no run-commands grant for this".to_owned(),
            unblock: Some(Unblock {
                action: "the owner grants run-commands over session s1".to_owned(),
                by: UnblockedBy::Owner,
            }),
        })],
    );

    let output = home.cockpit(&["ask", "run", "--check"], "").await;

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("to unblock (the owner): the owner grants run-commands"),
        "{}",
        text(&output.stderr)
    );
    assert_eq!(
        daemon.await.unwrap(),
        [Command::AskCheck {
            launch_token: Some("the-token".to_owned())
        }]
    );
}

#[tokio::test]
async fn ask_guide_sends_standard_input_and_exits_3_when_the_owner_is_stuck() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.agent_socket(),
        vec![guide(GuideStateView::Asking), guide(GuideStateView::Stuck)],
    );
    let steps = "# Connect the app\nOpen the settings\nCopy the id\n";

    let output = home.cockpit(&["ask", "guide"], steps).await;

    assert_eq!(output.status.code(), Some(3), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "stuck at step 2: no id shown\n");
    assert_eq!(
        daemon.await.unwrap()[0],
        Command::AskGuide {
            launch_token: Some("the-token".to_owned()),
            guide: steps.to_owned(),
            cwd: home.cwd(),
        }
    );
}

#[tokio::test]
async fn an_ask_without_its_reason_is_a_usage_error() {
    let home = Home::new();

    let output = home.cockpit(&["ask", "run", "make test"], "").await;

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("--reason is required"));
}

#[tokio::test]
async fn the_owner_approves_naming_the_revision_and_digest_read() {
    let home = Home::new();
    let digest = "d".repeat(64);
    let daemon = scripted_conversation(
        &home.owner_socket(),
        vec![run_reply(request(RunStateView::Approved))],
    );

    let output = home
        .cockpit(
            &[
                "approval",
                "approve",
                "7",
                "--revision",
                "1",
                "--digest",
                &digest,
            ],
            "",
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(text(&output.stdout).starts_with("request 7 revision 1 approved (session s1)\n"));
    assert_eq!(
        daemon.await.unwrap(),
        [Command::ApprovalApprove {
            request: 7,
            revision: 1,
            digest,
        }]
    );
}

#[tokio::test]
async fn the_owner_reads_a_request_with_its_warnings() {
    let home = Home::new();
    let _daemon = scripted_conversation(
        &home.owner_socket(),
        vec![run_reply(RunRequestView {
            shown_command: ShownText {
                text: "rm -rf /\\u{202e}".to_owned(),
                has_hidden_characters: true,
                non_ascii: vec!["а U+0430".to_owned()],
                warnings: vec![HiddenView::BidiControl, HiddenView::BlankLines],
            },
            ..request(RunStateView::Asking)
        })],
    );

    let output = home.cockpit(&["approval", "show", "7"], "").await;

    let shown = text(&output.stdout);
    assert!(shown.contains("command:\nrm -rf /\\u{202e}\n"), "{shown}");
    assert!(
        shown.contains(
            "! hidden characters shown escaped (direction controls, blank-line padding); \
             non-ASCII: а U+0430"
        ),
        "{shown}"
    );
    assert!(
        shown.contains("! the agent's project is unknown"),
        "{shown}"
    );
    assert!(shown.contains(&format!(
        "approve: cockpit approval approve 7 --revision 1 --digest {}",
        "d".repeat(64)
    )));
}

#[tokio::test]
async fn the_owner_reads_a_guides_directory_escaped_and_placed_against_the_project() {
    let home = Home::new();
    let _daemon = scripted_conversation(
        &home.owner_socket(),
        vec![guide_reply(GuideView {
            shown_cwd: ShownText {
                text: "/tmp/x\\u{202e}".to_owned(),
                has_hidden_characters: true,
                non_ascii: Vec::new(),
                warnings: vec![HiddenView::BidiControl],
            },
            project: Some("app".to_owned()),
            outside_project: true,
            ..guide_view(GuideStateView::Asking)
        })],
    );

    let output = home.cockpit(&["guide", "show", "3"], "").await;

    let shown = text(&output.stdout);
    assert!(shown.contains("directory: /tmp/x\\u{202e}\n"), "{shown}");
    assert!(
        shown.contains("! hidden characters shown escaped"),
        "{shown}"
    );
    assert!(
        shown.contains("! OUTSIDE the agent's project app"),
        "{shown}"
    );
}

#[tokio::test]
async fn the_owner_types_input_from_standard_input_not_the_command_line() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.owner_socket(),
        vec![run_reply(request(RunStateView::Running))],
    );

    let output = home.cockpit(&["approval", "input", "7"], "hunter2\n").await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        [Command::ApprovalInput {
            request: 7,
            text: "hunter2".to_owned(),
        }]
    );
}

#[tokio::test]
async fn empty_standard_input_is_refused_rather_than_pressing_enter() {
    let home = Home::new();

    let output = home.cockpit(&["approval", "input", "7"], "").await;

    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("no input on standard input"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn guide_steps_count_from_one_for_the_owner() {
    let home = Home::new();
    let daemon = scripted_conversation(&home.owner_socket(), vec![guide(GuideStateView::Done)]);

    let output = home
        .cockpit(&["guide", "answer", "3", "2", "1234"], "")
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        [Command::GuideAnswer {
            guide: 3,
            step: 1,
            answer: Some("1234".to_owned()),
        }]
    );
}

async fn shown_to_owner(request: RunRequestView) -> String {
    let home = Home::new();
    let _daemon = scripted_conversation(&home.owner_socket(), vec![run_reply(request)]);
    let output = home.cockpit(&["approval", "show", "7"], "").await;
    assert!(output.status.success(), "{}", text(&output.stderr));
    text(&output.stdout)
}

#[tokio::test]
async fn the_owner_is_warned_of_the_changed_build_files_a_command_runs() {
    let shown = shown_to_owner(RunRequestView {
        changed_build_files: Some(vec!["Makefile".to_owned(), "build.rs".to_owned()]),
        ..request(RunStateView::Asking)
    })
    .await;

    assert!(
        shown.contains(
            "! runs files changed in this checkout, not on the default branch: Makefile, build.rs"
        ),
        "{shown}"
    );
}

#[tokio::test]
async fn the_owner_is_told_when_changed_build_files_could_not_be_read() {
    let shown = shown_to_owner(RunRequestView {
        changed_build_files: None,
        ..request(RunStateView::Asking)
    })
    .await;

    assert!(
        shown.contains("! could not read which changed build files it runs"),
        "{shown}"
    );
}

#[tokio::test]
async fn the_owner_gets_the_timeout_hint_before_approving() {
    let shown = shown_to_owner(RunRequestView {
        timeout_without_foreground: true,
        ..request(RunStateView::Asking)
    })
    .await;

    assert!(shown.contains("! timeout without --foreground:"), "{shown}");
}

#[tokio::test]
async fn the_queue_folds_a_long_command_behind_an_explicit_more() {
    let home = Home::new();
    let _daemon = scripted_conversation(
        &home.owner_socket(),
        vec![Outcome::Ok(Reply::ApprovalQueue {
            runs: vec![RunRequestView {
                shown_command: shown("echo 1\necho 2\necho 3"),
                folded_command: Some(FoldedView {
                    preview: "echo 1".to_owned(),
                    more_lines: 2,
                    more_chars: 14,
                }),
                ..request(RunStateView::Asking)
            }],
            guides: Vec::new(),
        })],
    );

    let output = home.cockpit(&["approval", "list"], "").await;

    assert_eq!(
        text(&output.stdout),
        "run\t7\tasking\ts1\techo 1 [more: 2 lines; cockpit approval show 7]\n"
    );
}

#[tokio::test]
async fn ask_run_no_wait_files_the_request_and_returns_at_once_with_how_to_wait() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.agent_socket(),
        vec![
            run_reply(request(RunStateView::Asking)),
            run_reply(request(RunStateView::Asking)),
        ],
    );

    let output = home
        .cockpit(
            &["ask", "run", "--no-wait", "--reason", "tests", "make test"],
            "",
        )
        .await;

    assert_eq!(output.status.code(), Some(124), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("cockpit ask run --wait 7"),
        "{}",
        text(&output.stderr)
    );
    let received = daemon.await.unwrap();
    assert_eq!(
        received.len(),
        1,
        "no status poll after the ask: {received:?}"
    );
    assert!(matches!(received[0], Command::AskRun { .. }));
}

async fn queue_line(shown_command: ShownText, folded_command: Option<FoldedView>) -> String {
    let home = Home::new();
    let _daemon = scripted_conversation(
        &home.owner_socket(),
        vec![Outcome::Ok(Reply::ApprovalQueue {
            runs: vec![RunRequestView {
                shown_command,
                folded_command,
                ..request(RunStateView::Asking)
            }],
            guides: Vec::new(),
        })],
    );
    text(&home.cockpit(&["approval", "list"], "").await.stdout)
}

#[tokio::test]
async fn the_queue_counts_every_line_its_one_line_leaves_out() {
    let command: Vec<String> = (1..=10).map(|line| format!("echo {line}")).collect();
    let line = queue_line(
        shown(&command.join("\n")),
        Some(FoldedView {
            preview: command[..8].join("\n"),
            more_lines: 2,
            more_chars: 14,
        }),
    )
    .await;

    assert_eq!(
        line,
        "run\t7\tasking\ts1\techo 1 [more: 9 lines; cockpit approval show 7]\n"
    );
}

#[tokio::test]
async fn the_queue_folds_one_long_line_by_characters() {
    let line = queue_line(
        shown(&"x".repeat(700)),
        Some(FoldedView {
            preview: "x".repeat(600),
            more_lines: 0,
            more_chars: 100,
        }),
    )
    .await;

    assert_eq!(
        line,
        format!(
            "run\t7\tasking\ts1\t{} [more: 100 characters; cockpit approval show 7]\n",
            "x".repeat(600)
        )
    );
}

#[tokio::test]
async fn no_wait_given_twice_is_a_usage_error() {
    let home = Home::new();

    let output = home
        .cockpit(
            &["ask", "run", "--no-wait", "--no-wait", "--reason", "r"],
            "",
        )
        .await;

    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    assert!(text(&output.stderr).contains("--no-wait given twice"));
}
