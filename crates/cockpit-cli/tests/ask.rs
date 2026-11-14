//! `cockpit ask run`, `cockpit ask guide`, `cockpit ask service`, `cockpit approval` and
//! `cockpit guide` driven as processes against a scripted daemon socket: what each sends, what
//! it prints and the exit codes agents rely on. The daemon's side is proven in
//! `cockpit-daemon/tests/approvals.rs` and, for a service, `cockpit-daemon/tests/sandbox.rs`.

use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Output, Stdio};

mod scripted;

use cockpit_protocol::{
    Command, DecidedRunView, DecidedThroughView, ErrorCode, FileNowView, FindingSeverityView,
    FindingView, FoldedView, GuideStateView, GuideStepView, GuideView, HiddenView,
    NetworkDecisionView, NetworkGrantView, NetworkRequestStateView, NetworkRequestView,
    NetworkSubjectView, NetworkTargetView, Outcome, ProtocolError, Reply, RootFileView,
    RootRunsView, RunDecisionView, RunRequestView, RunResultView, RunStateView, SecretText,
    ShownText, SudoAskView, Unblock, UnblockedBy,
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
        let path = self.runtime_dir().join("torlek-dev").join(relative);
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
    /// that waits at most `TORLEK_ASK_WAIT_SECONDS=2`.
    async fn cockpit(&self, arguments: &[&str], stdin: &str) -> Output {
        let mut command = self.command(arguments);
        command.env("TORLEK_LAUNCH_TOKEN", "the-token");
        Self::output(command, stdin).await
    }

    /// Runs `cockpit` with `arguments` and `stdin` from the owner's own shell, outside any
    /// session.
    async fn cockpit_as_owner(&self, arguments: &[&str], stdin: &str) -> Output {
        Self::output(self.command(arguments), stdin).await
    }

    fn command(&self, arguments: &[&str]) -> std::process::Command {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_torlek"));
        command
            .args(arguments)
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.runtime_dir())
            .env("TORLEK_ASK_WAIT_SECONDS", "2")
            .env("TZ", "UTC")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    async fn output(mut command: std::process::Command, stdin: &str) -> Output {
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
        unchecked_shell: None,
        changed_build_files: Some(Vec::new()),
        digest: "d".repeat(64),
        asked_at: 1,
        expires_at: 901,
        waiting_for_input: false,
        waiting_prompt: None,
        sudo_ask: None,
        sudo_ran_unasked: 0,
        typed_secret_redacted: 0,
        root_runs: None,
        pinned: None,
        findings: None,
        decision: None,
        result: None,
        note: None,
        already_asked: false,
        writes: Vec::new(),
        hosts: Vec::new(),
        limits: Vec::new(),
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
            output_retired: false,
            stopped_by_owner: false,
            sudo_ran_unasked: 0,
            typed_secret_redacted: 0,
            limit_hits: Vec::new(),
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
            writes: Vec::new(),
            hosts: Vec::new(),
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
        text(&output.stderr).contains("torlek ask run --wait 7"),
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
        .cockpit_as_owner(
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
            through: Some(DecidedThroughView::Cli),
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

    let output = home.cockpit_as_owner(&["approval", "show", "7"], "").await;

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
        "approve: torlek approval approve 7 --revision 1 --digest {}",
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

    let output = home.cockpit_as_owner(&["guide", "show", "3"], "").await;

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

    let output = home
        .cockpit_as_owner(&["approval", "input", "7"], "hunter2\n")
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        [Command::ApprovalInput {
            request: 7,
            text: "hunter2".to_owned(),
            shown: false,
        }]
    );
}

#[tokio::test]
async fn the_owner_answers_sudos_ask_from_standard_input_not_the_command_line() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.owner_socket(),
        vec![run_reply(request(RunStateView::Running))],
    );

    let output = home
        .cockpit_as_owner(&["approval", "sudo-password", "7", "3"], "hunter2\n")
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        !text(&output.stdout).contains("hunter2"),
        "{}",
        text(&output.stdout)
    );
    assert_eq!(
        daemon.await.unwrap(),
        [Command::ApprovalSudoPassword {
            request: 7,
            ask: 3,
            password: SecretText::new("hunter2".to_owned()),
        }]
    );
}

#[tokio::test]
async fn the_owner_is_told_how_to_answer_a_sudo_ask() {
    let shown = shown_to_owner(RunRequestView {
        waiting_for_input: true,
        sudo_ask: Some(SudoAskView {
            ask: 3,
            prompt: shown("[sudo] password for owner: "),
        }),
        ..request(RunStateView::Running)
    })
    .await;

    assert!(
        shown.contains("torlek approval sudo-password 7 3"),
        "{shown}"
    );
}

#[tokio::test]
async fn shown_input_answers_a_visible_prompt_on_purpose() {
    for arguments in [
        ["approval", "input", "7", "--shown"],
        ["approval", "input", "--shown", "7"],
    ] {
        let home = Home::new();
        let daemon = scripted_conversation(
            &home.owner_socket(),
            vec![run_reply(request(RunStateView::Running))],
        );

        let output = home.cockpit_as_owner(&arguments, "y\n").await;

        assert!(output.status.success(), "{}", text(&output.stderr));
        assert_eq!(
            daemon.await.unwrap(),
            [Command::ApprovalInput {
                request: 7,
                text: "y".to_owned(),
                shown: true,
            }],
            "{arguments:?}"
        );
    }
}

#[tokio::test]
async fn empty_standard_input_is_refused_rather_than_pressing_enter() {
    let home = Home::new();

    let output = home.cockpit_as_owner(&["approval", "input", "7"], "").await;

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
        .cockpit_as_owner(&["guide", "answer", "3", "2", "1234"], "")
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
    let output = home.cockpit_as_owner(&["approval", "show", "7"], "").await;
    assert!(output.status.success(), "{}", text(&output.stderr));
    text(&output.stdout)
}

/// #657: an owner auditing a request reads who decided it, when and through which client.
#[tokio::test]
async fn the_owner_reads_the_decision_that_let_a_request_run() {
    let shown = shown_to_owner(RunRequestView {
        decision: Some(RunDecisionView {
            approved: true,
            at: 1_800_000_000,
            through: Some(DecidedThroughView::Tui),
        }),
        ..request(RunStateView::Finished)
    })
    .await;

    assert!(
        shown.contains(
            "decided: approved by the owner through the TUI at 2027-01-15 08:00:00 +00:00\n"
        ),
        "{shown}"
    );
}

#[tokio::test]
async fn a_decline_from_a_client_that_did_not_name_itself_says_so() {
    let shown = shown_to_owner(RunRequestView {
        decision: Some(RunDecisionView {
            approved: false,
            at: 1_800_000_000,
            through: None,
        }),
        ..request(RunStateView::Declined)
    })
    .await;

    assert!(
        shown.contains(
            "decided: declined by the owner at 2027-01-15 08:00:00 +00:00 \
             (the client did not name itself)\n"
        ),
        "{shown}"
    );
}

#[tokio::test]
async fn the_cli_names_itself_when_the_owner_declines() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.owner_socket(),
        vec![run_reply(request(RunStateView::Declined))],
    );

    let output = home
        .cockpit_as_owner(&["approval", "decline", "7"], "")
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        [Command::ApprovalDecline {
            request: 7,
            through: Some(DecidedThroughView::Cli),
        }]
    );
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
async fn the_owner_reads_each_editable_file_sudo_runs_as_it_is_now() {
    let shown = shown_to_owner(RunRequestView {
        root_runs: Some(RootRunsView {
            editable: vec![
                RootFileView {
                    path: "/p/x.sh".to_owned(),
                    shown_path: shown("./x.sh"),
                    sha256: Some("a".repeat(64)),
                    now: Some(FileNowView {
                        sha256: Some("b".repeat(64)),
                        head: Some(shown("#!/bin/sh\necho hi")),
                        more_lines: 3,
                        unreadable: false,
                    }),
                },
                RootFileView {
                    path: "/p/later.sh".to_owned(),
                    shown_path: shown("./later.sh"),
                    sha256: None,
                    now: Some(FileNowView {
                        sha256: None,
                        head: None,
                        more_lines: 0,
                        unreadable: false,
                    }),
                },
            ],
            cannot_tell: false,
        }),
        ..request(RunStateView::Asking)
    })
    .await;

    let expected = format!(
        "  Runs as root: ./x.sh\n\
         \x20 ! ./x.sh changed since it was asked: approving it refuses the run\n\
         \x20   sha256 {}\n\
         \x20   | #!/bin/sh\n\
         \x20   | echo hi\n\
         \x20   ... 3 more lines\n\
         \x20 Runs as root: ./later.sh\n\
         \x20   sha256 none: the file is not there\n",
        "b".repeat(64)
    );
    assert!(shown.contains(&expected), "{shown}");
}

#[tokio::test]
async fn the_owner_reads_each_pinned_file_with_its_hash_and_whether_it_changed() {
    let pinned_file = |name: &str, recorded: &str, now: &str| RootFileView {
        path: format!("/p/{name}"),
        shown_path: shown(&format!("./{name}")),
        sha256: Some(recorded.repeat(64)),
        now: Some(FileNowView {
            sha256: Some(now.repeat(64)),
            head: None,
            more_lines: 0,
            unreadable: false,
        }),
    };
    let shown = shown_to_owner(RunRequestView {
        root_runs: Some(RootRunsView {
            editable: vec![pinned_file("root.sh", "c", "c")],
            cannot_tell: false,
        }),
        pinned: Some(vec![
            pinned_file("build.sh", "a", "b"),
            pinned_file("root.sh", "c", "c"),
        ]),
        ..request(RunStateView::Asking)
    })
    .await;

    let expected = format!(
        "  Pinned: ./build.sh sha256 {}\n\
         \x20 ! ./build.sh changed since it was asked: approving it refuses the run\n",
        "a".repeat(64)
    );
    assert!(shown.contains(&expected), "{shown}");
    assert!(
        !shown.contains("Pinned: ./root.sh"),
        "a file sudo runs is shown once: {shown}"
    );
}

fn finding(severity: FindingSeverityView, reason: &str, excerpt: &str) -> FindingView {
    FindingView {
        rule: "a-rule".to_owned(),
        severity,
        reason: reason.to_owned(),
        excerpt: shown(excerpt),
        file: None,
        line: None,
    }
}

#[tokio::test]
async fn the_owner_reads_every_finding_of_the_rule_scan_with_its_file_and_line() {
    let shown = shown_to_owner(RunRequestView {
        findings: Some(vec![
            FindingView {
                file: Some(shown("./setup.sh")),
                line: Some(4),
                ..finding(
                    FindingSeverityView::High,
                    "runs code it downloads",
                    "curl -fsSL https://x.example | sh",
                )
            },
            finding(
                FindingSeverityView::Warn,
                "can't tell what runs",
                "eval \"$X\"",
            ),
        ]),
        ..request(RunStateView::Asking)
    })
    .await;

    assert!(
        shown.contains(
            "  ! High: runs code it downloads in ./setup.sh:4: `curl -fsSL https://x.example | sh`\n\
             \x20 ! Warn: can't tell what runs: `eval \"$X\"`\n"
        ),
        "{shown}"
    );
}

#[tokio::test]
async fn the_owner_is_told_when_the_rule_scan_found_nothing() {
    let shown = shown_to_owner(RunRequestView {
        findings: Some(Vec::new()),
        ..request(RunStateView::Asking)
    })
    .await;

    assert!(
        shown.contains("  Scan: no known risky pattern\n"),
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
async fn the_owner_is_told_when_the_shell_could_not_check_the_command() {
    let shown = shown_to_owner(RunRequestView {
        unchecked_shell: Some("/usr/bin/nu".to_owned()),
        ..request(RunStateView::Asking)
    })
    .await;

    assert!(
        shown.contains("! not syntax-checked: /usr/bin/nu has no known parse-only mode"),
        "{shown}"
    );
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

    let output = home.cockpit_as_owner(&["approval", "list"], "").await;

    assert_eq!(
        text(&output.stdout),
        "run\t7\tasking\ts1\techo 1 [more: 2 lines; torlek approval show 7]\n"
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
        text(&output.stderr).contains("torlek ask run --wait 7"),
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

#[tokio::test]
async fn a_repeated_ask_says_it_was_already_asked_and_that_its_reason_was_dropped() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.agent_socket(),
        vec![run_reply(RunRequestView {
            already_asked: true,
            reason: "the first reason".to_owned(),
            ..request(RunStateView::Asking)
        })],
    );

    let output = home
        .cockpit(
            &[
                "ask",
                "run",
                "--no-wait",
                "--reason",
                "another reason",
                "make test",
            ],
            "",
        )
        .await;

    assert_eq!(output.status.code(), Some(124), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stderr),
        "torlek: already asked (request 7); this reason was dropped, the first one stands; \
         wait with: torlek ask run --wait 7\n"
    );
    assert_eq!(daemon.await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_repeated_ask_with_the_same_reason_does_not_claim_to_drop_one() {
    let home = Home::new();
    let _daemon = scripted_conversation(
        &home.agent_socket(),
        vec![run_reply(RunRequestView {
            already_asked: true,
            reason: "tests".to_owned(),
            ..request(RunStateView::Asking)
        })],
    );

    let output = home
        .cockpit(
            &["ask", "run", "--no-wait", "--reason", "tests", "make test"],
            "",
        )
        .await;

    assert!(
        !text(&output.stderr).contains("dropped"),
        "{}",
        text(&output.stderr)
    );
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
    text(
        &home
            .cockpit_as_owner(&["approval", "list"], "")
            .await
            .stdout,
    )
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
        "run\t7\tasking\ts1\techo 1 [more: 9 lines; torlek approval show 7]\n"
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
            "run\t7\tasking\ts1\t{} [more: 100 characters; torlek approval show 7]\n",
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

/// A run retention cleared: its output is gone, the counters that described it remain.
fn retired(exit_code: i32) -> Outcome {
    run_reply(RunRequestView {
        result: Some(RunResultView {
            exit_code,
            output: String::new(),
            omitted_lines: 500,
            truncated_bytes: 9,
            redacted: 3,
            output_retired: true,
            stopped_by_owner: false,
            sudo_ran_unasked: 0,
            typed_secret_redacted: 0,
            limit_hits: Vec::new(),
        }),
        ..request(RunStateView::Finished)
    })
}

#[tokio::test]
async fn waiting_on_a_run_whose_output_retention_removed_says_so_and_keeps_its_code() {
    let home = Home::new();
    let _daemon = scripted_conversation(&home.agent_socket(), vec![retired(4)]);

    let output = home.cockpit(&["ask", "run", "--wait", "7"], "").await;

    assert_eq!(output.status.code(), Some(4), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "");
    assert_eq!(
        text(&output.stderr),
        "torlek: database retention removed this run's output\n"
    );
}

#[tokio::test]
async fn the_owner_reads_that_retention_removed_a_runs_output() {
    let home = Home::new();
    let _daemon = scripted_conversation(&home.owner_socket(), vec![retired(0)]);

    let output = home.cockpit_as_owner(&["approval", "show", "7"], "").await;

    let shown = text(&output.stdout);
    assert!(
        shown.contains("exit 0 (output removed by database retention or purge-output)"),
        "{shown}"
    );
    assert!(!shown.contains("secrets redacted"), "{shown}");
}

/// A run the owner stopped at its prompt (#1301): SIGTERM's exit status, what it printed.
fn stopped_by_the_owner() -> Outcome {
    run_reply(RunRequestView {
        result: Some(RunResultView {
            exit_code: 143,
            output: "Username for 'https://github.com':".to_owned(),
            omitted_lines: 0,
            truncated_bytes: 0,
            redacted: 0,
            output_retired: false,
            stopped_by_owner: true,
            sudo_ran_unasked: 0,
            typed_secret_redacted: 0,
            limit_hits: Vec::new(),
        }),
        ..request(RunStateView::Finished)
    })
}

#[tokio::test]
async fn the_owner_stops_a_running_command_by_its_id() {
    let home = Home::new();
    let daemon = scripted_conversation(&home.owner_socket(), vec![stopped_by_the_owner()]);

    let output = home.cockpit_as_owner(&["approval", "stop", "7"], "").await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        [Command::ApprovalStop { request: 7 }]
    );
    assert!(
        text(&output.stdout).contains("stopped by the owner"),
        "{}",
        text(&output.stdout)
    );
}

#[tokio::test]
async fn waiting_on_a_run_the_owner_stopped_says_so_with_its_exit_status() {
    let home = Home::new();
    let _daemon = scripted_conversation(&home.agent_socket(), vec![stopped_by_the_owner()]);

    let output = home.cockpit(&["ask", "run", "--wait", "7"], "").await;

    assert_eq!(output.status.code(), Some(143), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("torlek: the owner stopped request 7 (exit status 143)"),
        "{}",
        text(&output.stderr)
    );
    assert!(text(&output.stdout).contains("Username for"));
}

/// A run in which one sudo ended having asked the owner nothing (#1374).
fn sudo_ran_unasked() -> Outcome {
    run_reply(RunRequestView {
        result: Some(RunResultView {
            exit_code: 0,
            output: "as-root".to_owned(),
            omitted_lines: 0,
            truncated_bytes: 0,
            redacted: 0,
            output_retired: false,
            stopped_by_owner: false,
            sudo_ran_unasked: 1,
            typed_secret_redacted: 0,
            limit_hits: Vec::new(),
        }),
        ..request(RunStateView::Finished)
    })
}

#[tokio::test]
async fn the_agent_is_told_a_sudo_ran_without_asking_the_owner() {
    let home = Home::new();
    let _daemon = scripted_conversation(&home.agent_socket(), vec![sudo_ran_unasked()]);

    let output = home.cockpit(&["ask", "run", "--wait", "7"], "").await;

    assert!(
        text(&output.stderr).contains("torlek: sudo ran without asking you: 1 sudo"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn the_owner_reads_that_a_sudo_ran_without_asking() {
    let home = Home::new();
    let _daemon = scripted_conversation(&home.owner_socket(), vec![sudo_ran_unasked()]);

    let output = home.cockpit_as_owner(&["approval", "show", "7"], "").await;

    assert!(
        text(&output.stdout).contains("sudo ran without asking you"),
        "{}",
        text(&output.stdout)
    );
}

#[tokio::test]
async fn the_owner_reads_that_a_sudo_ran_without_asking_while_the_command_still_runs() {
    let home = Home::new();
    let running = run_reply(RunRequestView {
        sudo_ran_unasked: 2,
        ..request(RunStateView::Running)
    });
    let _daemon = scripted_conversation(&home.owner_socket(), vec![running]);

    let output = home.cockpit_as_owner(&["approval", "show", "7"], "").await;

    assert!(
        text(&output.stdout).contains("sudo ran without asking you: 2 sudo"),
        "{}",
        text(&output.stdout)
    );
}

/// A run that printed a secret the owner typed for it once (#1386).
fn typed_secret_redacted() -> Outcome {
    run_reply(RunRequestView {
        result: Some(RunResultView {
            exit_code: 0,
            output: "[redacted]".to_owned(),
            omitted_lines: 0,
            truncated_bytes: 0,
            redacted: 0,
            output_retired: false,
            stopped_by_owner: false,
            sudo_ran_unasked: 0,
            typed_secret_redacted: 1,
            limit_hits: Vec::new(),
        }),
        ..request(RunStateView::Finished)
    })
}

#[tokio::test]
async fn the_agent_is_told_the_owners_typed_secret_was_redacted_from_the_output() {
    let home = Home::new();
    let _daemon = scripted_conversation(&home.agent_socket(), vec![typed_secret_redacted()]);

    let output = home.cockpit(&["ask", "run", "--wait", "7"], "").await;

    assert!(
        text(&output.stderr)
            .contains("torlek: your typed secret appeared in the output 1 time and was redacted"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn the_owner_reads_that_the_typed_secret_was_redacted_while_the_command_still_runs() {
    let home = Home::new();
    let running = run_reply(RunRequestView {
        typed_secret_redacted: 3,
        ..request(RunStateView::Running)
    });
    let _daemon = scripted_conversation(&home.owner_socket(), vec![running]);

    let output = home.cockpit_as_owner(&["approval", "show", "7"], "").await;

    assert!(
        text(&output.stdout).contains("your typed secret appeared in the output 3 times"),
        "{}",
        text(&output.stdout)
    );
}

const FIGCRAFT_RECORD: &str = "service figcraft listen 127.0.0.1:3845 target 127.0.0.1:3845";

fn figcraft() -> NetworkTargetView {
    NetworkTargetView::Service {
        service: "figcraft".to_owned(),
    }
}

/// The session's request 9 for the `figcraft` service, in `state`.
fn service_request(state: NetworkRequestStateView) -> Outcome {
    Outcome::Ok(Reply::NetworkRequest {
        request: Box::new(NetworkRequestView {
            id: 9,
            subject: NetworkSubjectView::Session {
                id: "s00000000000a".to_owned(),
            },
            project: "app".to_owned(),
            asks: figcraft(),
            target: FIGCRAFT_RECORD.to_owned(),
            state,
            asked_at: 1,
            decision: None,
            grant: None,
            shown_reason: None,
        }),
    })
}

#[tokio::test]
async fn ask_service_files_the_request_with_the_token_and_says_who_allows_it() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.agent_socket(),
        vec![service_request(NetworkRequestStateView::Asking)],
    );

    let output = home
        .cockpit(
            &["ask", "service", "figcraft", "--reason", "read the design"],
            "",
        )
        .await;

    assert_eq!(output.status.code(), Some(124), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("network request 9")
            && text(&output.stderr).contains("torlek network allow 9"),
        "{}",
        text(&output.stderr)
    );
    assert_eq!(
        daemon.await.unwrap(),
        [Command::AskService {
            launch_token: Some("the-token".to_owned()),
            service: "figcraft".to_owned(),
            port: None,
            reason: "read the design".to_owned(),
        }]
    );
}

#[tokio::test]
async fn each_service_answer_keeps_its_exit_code() {
    let granted = Outcome::Ok(Reply::NetworkGrant {
        grant: Box::new(NetworkGrantView {
            id: 4,
            project: "app".to_owned(),
            allows: figcraft(),
            target: FIGCRAFT_RECORD.to_owned(),
            request: Some(2),
            granted: NetworkDecisionView {
                at: 1,
                through: Some(DecidedThroughView::Tui),
            },
            revoked: None,
        }),
    });
    for (outcome, expected) in [
        (service_request(NetworkRequestStateView::AllowedOnce), 0),
        (granted, 0),
        (service_request(NetworkRequestStateView::Denied), 125),
    ] {
        let home = Home::new();
        let daemon = scripted_conversation(&home.agent_socket(), vec![outcome]);

        let output = home
            .cockpit(&["ask", "service", "figcraft", "--reason", "design"], "")
            .await;

        assert_eq!(
            output.status.code(),
            Some(expected),
            "{}",
            text(&output.stderr)
        );
        daemon.await.unwrap();
    }
}

#[tokio::test]
async fn an_ask_for_a_service_without_its_reason_is_a_usage_error() {
    let home = Home::new();

    let output = home.cockpit(&["ask", "service", "figcraft"], "").await;

    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).contains("--reason is required")
            && text(&output.stderr).contains("torlek ask service NAME --reason TEXT"),
        "{}",
        text(&output.stderr)
    );
}

/// #390: a container port is asked for by its number.
#[tokio::test]
async fn ask_service_containers_sends_the_port() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.agent_socket(),
        vec![service_request(NetworkRequestStateView::Asking)],
    );

    let output = home
        .cockpit(
            &[
                "ask",
                "service",
                "containers",
                "--port",
                "8080",
                "--reason",
                "open the web app",
            ],
            "",
        )
        .await;

    assert_eq!(output.status.code(), Some(124), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        [Command::AskService {
            launch_token: Some("the-token".to_owned()),
            service: "containers".to_owned(),
            port: Some(8080),
            reason: "open the web app".to_owned(),
        }]
    );
}

#[tokio::test]
async fn a_port_that_is_not_one_is_a_usage_error() {
    let home = Home::new();

    for port in ["0", "65536", "http", "-1"] {
        let output = home
            .cockpit(
                &[
                    "ask",
                    "service",
                    "containers",
                    "--port",
                    port,
                    "--reason",
                    "r",
                ],
                "",
            )
            .await;

        assert_eq!(output.status.code(), Some(2), "{port}");
        assert!(
            text(&output.stderr).contains(&format!("--port {port}: not a port from 1 to 65535")),
            "{port}: {}",
            text(&output.stderr)
        );
    }
}

#[tokio::test]
async fn an_option_in_place_of_the_service_is_a_usage_error() {
    let home = Home::new();

    let output = home
        .cockpit(&["ask", "service", "--wait", "3", "--reason", "r"], "")
        .await;

    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).contains("unknown ask service option --wait"),
        "{}",
        text(&output.stderr)
    );
}

fn decided_run(id: i64, state: RunStateView, exit_code: Option<i32>) -> DecidedRunView {
    DecidedRunView {
        id,
        session: "s1".to_owned(),
        state,
        digest: "d".repeat(64),
        shown_command: shown("make test"),
        asked_at: 100,
        decision: RunDecisionView {
            approved: state != RunStateView::Declined,
            at: 112,
            through: Some(DecidedThroughView::Cli),
        },
        exit_code,
    }
}

#[tokio::test]
async fn the_decided_list_shows_state_digest_wait_and_exit_status_per_request() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.owner_socket(),
        vec![Outcome::Ok(Reply::ApprovalDecided {
            runs: vec![
                decided_run(7, RunStateView::Finished, Some(2)),
                decided_run(8, RunStateView::Declined, None),
                DecidedRunView {
                    shown_command: shown("echo 1\necho 2\necho 3"),
                    ..decided_run(9, RunStateView::NotRun, None)
                },
            ],
        })],
    );

    let output = home
        .cockpit_as_owner(&["approval", "list", "--all"], "")
        .await;

    let digest = "d".repeat(64);
    assert_eq!(
        text(&output.stdout),
        format!(
            "run\t7\tfinished\ts1\t{digest}\t12s\t2\tmake test\n\
             run\t8\tdeclined\ts1\t{digest}\t12s\t-\tmake test\n\
             run\t9\tnot run\ts1\t{digest}\t12s\t-\techo 1 [more: 2 lines; torlek approval show 9]\n"
        )
    );
    assert_eq!(
        daemon.await.unwrap(),
        [Command::ApprovalDecided { since: None }]
    );
}

#[tokio::test]
async fn the_decided_list_since_a_time_asks_for_the_decisions_from_then_on() {
    for arguments in [
        [
            "approval",
            "list",
            "--all",
            "--since",
            "2026-01-02T03:04:05Z",
        ],
        [
            "approval",
            "list",
            "--since",
            "2026-01-02T03:04:05Z",
            "--all",
        ],
    ] {
        let home = Home::new();
        let daemon = scripted_conversation(
            &home.owner_socket(),
            vec![Outcome::Ok(Reply::ApprovalDecided { runs: Vec::new() })],
        );

        let output = home.cockpit_as_owner(&arguments, "").await;

        assert!(output.status.success(), "{}", text(&output.stderr));
        assert_eq!(
            daemon.await.unwrap(),
            [Command::ApprovalDecided {
                since: Some(1_767_323_045)
            }],
            "{arguments:?}"
        );
    }
}

#[tokio::test]
async fn since_without_all_is_a_usage_error() {
    let home = Home::new();

    let output = home
        .cockpit_as_owner(&["approval", "list", "--since", "2h"], "")
        .await;

    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
}

fn decided_network_request(id: i64, state: NetworkRequestStateView) -> NetworkRequestView {
    NetworkRequestView {
        id,
        subject: NetworkSubjectView::Session {
            id: "s1".to_owned(),
        },
        project: "app".to_owned(),
        asks: NetworkTargetView::Host {
            host: "crates.io".to_owned(),
            port: 443,
        },
        target: "crates.io:443".to_owned(),
        state,
        asked_at: 100,
        decision: Some(NetworkDecisionView {
            at: 145,
            through: Some(DecidedThroughView::Cli),
        }),
        grant: None,
        shown_reason: None,
    }
}

#[tokio::test]
async fn the_network_list_names_a_check_runs_request_by_its_check_and_run() {
    let home = Home::new();
    let request = NetworkRequestView {
        subject: NetworkSubjectView::Check {
            run: "execution-e-3".to_owned(),
            check: "test".to_owned(),
        },
        ..decided_network_request(5, NetworkRequestStateView::Denied)
    };
    let daemon = scripted_conversation(
        &home.owner_socket(),
        vec![Outcome::Ok(Reply::NetworkRequests {
            requests: vec![request],
        })],
    );

    let output = home
        .cockpit_as_owner(&["network", "list", "--all"], "")
        .await;

    assert_eq!(
        text(&output.stdout),
        "network\t5\tdenied\tcheck test (run execution-e-3)\tapp\tcrates.io:443\t45s\n"
    );
    assert_eq!(daemon.await.unwrap(), [Command::NetworkDecided]);
}

/// #390: a project's containers ask through its engine's proxy, which no session runs.
#[tokio::test]
async fn the_network_list_names_a_containers_request_by_its_project() {
    let home = Home::new();
    let request = NetworkRequestView {
        subject: NetworkSubjectView::Containers {
            project: "app".to_owned(),
        },
        ..decided_network_request(6, NetworkRequestStateView::Denied)
    };
    let daemon = scripted_conversation(
        &home.owner_socket(),
        vec![Outcome::Ok(Reply::NetworkRequests {
            requests: vec![request],
        })],
    );

    let output = home
        .cockpit_as_owner(&["network", "list", "--all"], "")
        .await;

    assert_eq!(
        text(&output.stdout),
        "network\t6\tdenied\tcontainers of app\tapp\tcrates.io:443\t45s\n"
    );
    assert_eq!(daemon.await.unwrap(), [Command::NetworkDecided]);
}

#[tokio::test]
async fn the_decided_network_list_shows_host_decision_and_wait() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.owner_socket(),
        vec![Outcome::Ok(Reply::NetworkRequests {
            requests: vec![
                decided_network_request(3, NetworkRequestStateView::AllowedAlways),
                decided_network_request(4, NetworkRequestStateView::Denied),
            ],
        })],
    );

    let output = home
        .cockpit_as_owner(&["network", "list", "--all"], "")
        .await;

    assert_eq!(
        text(&output.stdout),
        "network\t3\tallowed-always\ts1\tapp\tcrates.io:443\t45s\n\
         network\t4\tdenied\ts1\tapp\tcrates.io:443\t45s\n"
    );
    assert_eq!(daemon.await.unwrap(), [Command::NetworkDecided]);
}

/// #1387: a confined run's card names the folders it may also write and the hosts it may
/// reach; the daemon resolves the folders.
#[tokio::test]
async fn ask_run_sends_the_folders_and_hosts_its_card_names() {
    let home = Home::new();
    let daemon = scripted_conversation(
        &home.agent_socket(),
        vec![run_reply(request(RunStateView::Asking))],
    );

    let output = home
        .cockpit(
            &[
                "ask",
                "run",
                "--no-wait",
                "--write",
                "out",
                "--write",
                "/srv/cache",
                "--host",
                "example.com",
                "--host",
                "127.0.0.2:8080",
                "--reason",
                "fetch",
                "make fetch",
            ],
            "",
        )
        .await;

    assert_eq!(output.status.code(), Some(124), "{}", text(&output.stderr));
    let received = daemon.await.unwrap();
    assert_eq!(
        received[0],
        Command::AskRun {
            launch_token: Some("the-token".to_owned()),
            command: "make fetch".to_owned(),
            cwd: home.cwd(),
            reason: "fetch".to_owned(),
            writes: vec![format!("{}/out", home.cwd()), "/srv/cache".to_owned()],
            hosts: vec!["example.com".to_owned(), "127.0.0.2:8080".to_owned()],
        }
    );
}

fn hit_a_limit() -> Outcome {
    run_reply(RunRequestView {
        result: Some(RunResultView {
            exit_code: 1,
            output: "cat: /home/owner/.ssh/id_ed25519: Permission denied".to_owned(),
            omitted_lines: 0,
            truncated_bytes: 0,
            redacted: 0,
            output_retired: false,
            stopped_by_owner: false,
            sudo_ran_unasked: 0,
            typed_secret_redacted: 0,
            limit_hits: vec![
                "tried to read /home/owner/.ssh/id_ed25519, which confined runs cannot see"
                    .to_owned(),
            ],
        }),
        ..request(RunStateView::Finished)
    })
}

#[tokio::test]
async fn the_agent_is_told_which_limit_its_run_hit() {
    let home = Home::new();
    let _daemon = scripted_conversation(&home.agent_socket(), vec![hit_a_limit()]);

    let output = home.cockpit(&["ask", "run", "--wait", "7"], "").await;

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains(
            "torlek: the run hit a limit: tried to read /home/owner/.ssh/id_ed25519, which \
             confined runs cannot see"
        ),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn the_owner_reads_which_limit_a_run_hit() {
    let Outcome::Ok(Reply::RunRequest { request }) = hit_a_limit() else {
        unreachable!()
    };

    let shown = shown_to_owner(*request).await;

    assert!(
        shown.contains(
            "hit a limit: tried to read /home/owner/.ssh/id_ed25519, which confined runs \
             cannot see"
        ),
        "{shown}"
    );
}

#[tokio::test]
async fn the_owner_reads_what_a_run_may_do_before_approving() {
    let shown = shown_to_owner(RunRequestView {
        limits: vec![
            shown("writes only /p and a /tmp of its own"),
            shown("no network"),
        ],
        ..request(RunStateView::Asking)
    })
    .await;

    assert!(
        shown.contains("limits:\n  writes only /p and a /tmp of its own\n  no network\n"),
        "{shown}"
    );
}
