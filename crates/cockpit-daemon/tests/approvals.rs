//! Issue #34's acceptance through the daemon binary's sockets: an agent asks, the owner
//! decides on the revision and digest it read, and the approved command runs once in a
//! pseudo-terminal on the host, its output redacted. This test process plays the agent.
#![cfg(target_os = "linux")]

mod common;

use std::time::Instant;

use cockpit_agent_channel::{LaunchToken, identity_of};
use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    Command, ErrorCode, GuideStateView, GuideView, HiddenView, Reply, RunRequestView, RunStateView,
    UnblockedBy,
};
use common::{GENEROUS, TestHome, refused};

/// Fields drop in order: the daemon stops before its home is deleted.
struct Asking {
    daemon: Option<common::RunningDaemon>,
    home: TestHome,
    token: LaunchToken,
    agent: Connection,
    owner: Connection,
    /// Where the asked commands run.
    cwd: tempfile::TempDir,
}

async fn started(granted: bool) -> Asking {
    started_in(TestHome::new(), granted).await
}

async fn started_in(home: TestHome, granted: bool) -> Asking {
    let token = home
        .launch(
            "s1",
            &identity_of(std::process::id()).unwrap(),
            granted.then_some(Permission::RunCommands),
        )
        .await;
    let daemon = home.start();
    let agent = home.connect_agent().await;
    let owner = home.connect().await;
    Asking {
        daemon: Some(daemon),
        home,
        token,
        agent,
        owner,
        cwd: tempfile::tempdir().unwrap(),
    }
}

impl Asking {
    /// Kills the daemon as a crash would, starts another on the same home and reconnects.
    async fn restart(&mut self) {
        self.daemon.take().expect("a daemon runs").kill();
        self.daemon = Some(self.home.start());
        self.agent = self.home.connect_agent().await;
        self.owner = self.home.connect().await;
    }

    /// Asks `command`, approves it as asked and waits until it created `marker`.
    async fn running(&self, command: &str, marker: &std::path::Path) -> RunRequestView {
        let request = self.asked(command).await;
        run(self.approve(&request).await);
        let deadline = Instant::now() + GENEROUS;
        while !marker.exists() {
            assert!(Instant::now() < deadline, "{command:?} never started");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        request
    }

    fn cwd_text(&self) -> String {
        std::fs::canonicalize(self.cwd.path())
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    }

    async fn ask(&self, command: &str) -> Result<Reply, ClientError> {
        self.agent
            .request(
                Command::AskRun {
                    launch_token: Some(self.token.as_str().to_owned()),
                    command: command.to_owned(),
                    cwd: self.cwd_text(),
                    reason: "Proves the approvals path end to end.".to_owned(),
                },
                GENEROUS,
            )
            .await
    }

    async fn asked(&self, command: &str) -> RunRequestView {
        run(self.ask(command).await)
    }

    async fn approve_once(&self, request: &RunRequestView) -> Result<Reply, ClientError> {
        self.owner
            .request(
                Command::ApprovalApprove {
                    request: request.id,
                    revision: request.revision,
                    digest: request.digest.clone(),
                },
                GENEROUS,
            )
            .await
    }

    /// Approves `request` as soon as the daemon's grace after its ask or edit allows.
    async fn approve(&self, request: &RunRequestView) -> Result<Reply, ClientError> {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let reply = self.approve_once(request).await;
            let too_soon = matches!(
                &reply,
                Err(ClientError::Daemon(error)) if error.code == ErrorCode::Refused
            );
            if !too_soon || Instant::now() > deadline {
                return reply;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    async fn status(&self, request: i64) -> RunRequestView {
        run(self
            .agent
            .request(
                Command::AskRunStatus {
                    launch_token: None,
                    request,
                },
                GENEROUS,
            )
            .await)
    }

    /// The request once `done` holds for it, as the agent polls it.
    async fn status_once(
        &self,
        request: i64,
        done: impl Fn(&RunRequestView) -> bool,
    ) -> RunRequestView {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let seen = self.status(request).await;
            if done(&seen) || Instant::now() > deadline {
                return seen;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    async fn ended(&self, request: i64) -> RunRequestView {
        self.status_once(request, |run| {
            !matches!(
                run.state,
                RunStateView::Asking | RunStateView::Approved | RunStateView::Running
            )
        })
        .await
    }

    /// Asks `command`, approves it as asked and waits for its end.
    async fn ran(&self, command: &str) -> RunRequestView {
        let request = self.asked(command).await;
        run(self.approve(&request).await);
        self.ended(request.id).await
    }
}

fn run(reply: Result<Reply, ClientError>) -> RunRequestView {
    match reply {
        Ok(Reply::RunRequest { request }) => *request,
        other => panic!("expected a run request, got {other:?}"),
    }
}

fn guide(reply: Result<Reply, ClientError>) -> GuideView {
    match reply {
        Ok(Reply::Guide { guide }) => *guide,
        other => panic!("expected a guide, got {other:?}"),
    }
}

#[tokio::test]
async fn an_approved_command_runs_in_a_terminal_and_returns_its_exit_code() {
    let asking = started(true).await;

    let ended = asking
        .ran("test -t 0 && test -t 1 && printf 'in a terminal\\n'; exit 3")
        .await;

    assert_eq!(ended.state, RunStateView::Finished, "{ended:?}");
    let result = ended.result.unwrap();
    assert_eq!(result.exit_code, 3);
    assert_eq!(result.output, "in a terminal");
}

#[tokio::test]
async fn the_owner_sees_the_request_in_the_queue_with_the_display_rules_applied() {
    let asking = started(true).await;
    let request = asking.asked("printf 'café\\n' # \u{202e}").await;

    let Ok(Reply::ApprovalQueue { runs, guides }) =
        asking.owner.request(Command::ApprovalQueue, GENEROUS).await
    else {
        panic!("expected the queue");
    };

    assert_eq!(runs, std::slice::from_ref(&request));
    assert!(guides.is_empty());
    assert!(request.shown_command.has_hidden_characters);
    assert!(request.shown_command.text.contains("\\u{202e}"));
    assert_eq!(
        request.shown_command.non_ascii,
        ["é U+00E9"],
        "escaped characters are not listed again"
    );
    assert_eq!(request.shown_command.warnings, [HiddenView::BidiControl]);
    assert!(
        request.outside_project,
        "no registered checkout holds the agent, so every directory is outside"
    );
}

#[tokio::test]
async fn secrets_in_the_output_are_redacted_before_they_reach_the_agent() {
    let asking = started(true).await;

    let ended = asking
        .ran("printf 'token ghp_abcdefghijklmnopqrstuvwxyz0123456789\\n'")
        .await;

    let result = ended.result.unwrap();
    assert!(!result.output.contains("ghp_abcdef"), "{}", result.output);
    assert_eq!(result.redacted, 1);
}

#[tokio::test]
async fn git_in_an_approved_run_has_hooks_off() {
    let asking = started(true).await;

    let ended = asking.ran("git config --get core.hooksPath").await;

    let result = ended.result.unwrap();
    assert_eq!((result.exit_code, result.output.as_str()), (0, "/dev/null"));
}

#[tokio::test]
async fn git_in_an_approved_run_has_the_repository_drivers_reset() {
    let asking = started(true).await;
    let git = |arguments: &[&str]| {
        let status = std::process::Command::new("git")
            .args(arguments)
            .current_dir(asking.cwd.path())
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?}");
    };
    git(&["init", "--quiet"]);
    git(&["config", "diff.planted.textconv", "touch planted"]);

    let ended = asking
        .ran("test -z \"$(git config --get diff.planted.textconv)\" && echo reset")
        .await;

    let result = ended.result.unwrap();
    assert_eq!((result.exit_code, result.output.as_str()), (0, "reset"));
}

/// Report 04, walkthrough 3, across the sockets: the owner approves what it read before the
/// edit; the daemon refuses the stale revision, and the approval of what it reads again runs
/// the edited command, once.
#[tokio::test]
async fn an_approval_of_a_stale_revision_is_refused_and_the_replay_runs_the_edit() {
    let asking = started(true).await;
    let read_before_the_edit = asking.asked("printf 'asked\\n'").await;
    let edited = run(asking
        .owner
        .request(
            Command::ApprovalEdit {
                request: read_before_the_edit.id,
                revision: 1,
                command: "printf 'edited\\n'".to_owned(),
            },
            GENEROUS,
        )
        .await);
    assert_eq!(edited.revision, 2);
    assert_eq!(edited.asked_command.as_deref(), Some("printf 'asked\\n'"));

    let stale = refused(asking.approve(&read_before_the_edit).await);
    assert_eq!(stale.code, ErrorCode::StaleRevision);
    assert_eq!(stale.unblock.unwrap().by, UnblockedBy::Owner);

    let Ok(Reply::ApprovalQueue { runs, .. }) =
        asking.owner.request(Command::ApprovalQueue, GENEROUS).await
    else {
        panic!("expected the queue");
    };
    run(asking.approve(&runs[0]).await);
    let ended = asking.ended(edited.id).await;
    assert_eq!(ended.result.unwrap().output, "edited");
    assert_eq!(
        refused(asking.approve(&runs[0]).await).code,
        ErrorCode::Conflict,
        "an approval is consumed once"
    );
}

#[tokio::test]
async fn a_declined_request_never_runs_and_the_agent_learns_it() {
    let asking = started(true).await;
    let marker = asking.cwd.path().join("ran");
    let request = asking.asked(&format!("touch {}", marker.display())).await;

    run(asking
        .owner
        .request(
            Command::ApprovalDecline {
                request: request.id,
            },
            GENEROUS,
        )
        .await);

    assert_eq!(
        asking.status(request.id).await.state,
        RunStateView::Declined
    );
    assert!(!marker.exists());
}

#[tokio::test]
async fn the_owner_answers_a_command_waiting_for_input() {
    let asking = started(true).await;
    let request = asking
        .asked("printf 'Continue? '; read answer; printf 'got %s\\n' \"$answer\"")
        .await;
    run(asking.approve(&request).await);
    let waiting = asking
        .status_once(request.id, |run| run.waiting_for_input)
        .await;
    assert!(waiting.waiting_for_input, "{waiting:?}");

    run(asking
        .owner
        .request(
            Command::ApprovalInput {
                request: request.id,
                text: "yes".to_owned(),
            },
            GENEROUS,
        )
        .await);

    let ended = asking.ended(request.id).await;
    assert!(
        ended.result.unwrap().output.ends_with("got yes"),
        "the answer reached the command"
    );
}

#[tokio::test]
async fn a_command_the_shell_cannot_parse_is_refused_before_the_owner_is_asked() {
    let asking = started(true).await;

    let refusal = refused(asking.ask("if then fi (").await);

    assert_eq!(refusal.code, ErrorCode::InvalidArgument);
    assert_eq!(refusal.unblock.unwrap().by, UnblockedBy::Agent);
}

/// An approval shell at `<dir>/nu` that appends its arguments to `<dir>/executed` and
/// succeeds, as nushell does with `-n` (`--no-config-file`): it runs the command.
fn recording_nushell(dir: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let shell = dir.join("nu");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\n",
        dir.join("executed").display()
    );
    std::fs::write(&shell, script).unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    shell
}

#[tokio::test]
async fn an_unknown_shell_receives_the_command_only_after_owner_approval() {
    let shells = tempfile::tempdir().unwrap();
    let mut home = TestHome::new();
    home.shell = Some(recording_nushell(shells.path()));
    let asking = started_in(home, true).await;

    let request = asking.asked("echo unapproved").await;
    let record = shells.path().join("executed");
    assert!(!record.exists(), "the shell ran before owner approval");

    run(asking.approve(&request).await);
    let ended = asking.ended(request.id).await;

    assert_eq!(ended.state, RunStateView::Finished, "{ended:?}");
    assert_eq!(
        std::fs::read_to_string(record).unwrap(),
        "-c\necho unapproved\n"
    );
}

#[tokio::test]
async fn editing_a_request_does_not_run_an_unknown_shell() {
    let shells = tempfile::tempdir().unwrap();
    let mut home = TestHome::new();
    home.shell = Some(recording_nushell(shells.path()));
    let asking = started_in(home, true).await;
    let request = asking.asked("first command").await;

    let edited = run(asking
        .owner
        .request(
            Command::ApprovalEdit {
                request: request.id,
                revision: request.revision,
                command: "edited command".to_owned(),
            },
            GENEROUS,
        )
        .await);

    assert_eq!(edited.command, "edited command");
    assert!(!shells.path().join("executed").exists());
}

#[tokio::test]
async fn a_guide_does_not_send_its_commands_to_an_unknown_shell() {
    let shells = tempfile::tempdir().unwrap();
    let mut home = TestHome::new();
    home.shell = Some(recording_nushell(shells.path()));
    let asking = started_in(home, true).await;

    let asked = guide(
        asking
            .agent
            .request(
                Command::AskGuide {
                    launch_token: Some(asking.token.as_str().to_owned()),
                    guide: "# Check\nDo the check\n  run: unapproved command".to_owned(),
                    cwd: asking.cwd_text(),
                },
                GENEROUS,
            )
            .await,
    );

    assert_eq!(asked.state, GuideStateView::Asking);
    assert!(!shells.path().join("executed").exists());
}

#[tokio::test]
async fn the_owner_is_told_the_command_was_not_checked_by_a_shell_without_a_parse_only_mode() {
    let shells = tempfile::tempdir().unwrap();
    let shell = recording_nushell(shells.path());
    let mut home = TestHome::new();
    home.shell = Some(shell.clone());
    let asking = started_in(home, true).await;
    let request = asking.asked("echo unapproved").await;

    let shown = shown_to_owner(&asking, request.id).await;

    assert_eq!(shown.unchecked_shell, Some(shell.display().to_string()));
}

#[tokio::test]
async fn a_command_a_parse_only_shell_checked_carries_no_unchecked_shell() {
    let asking = started(true).await;

    let request = asking.asked("true").await;

    assert_eq!(request.unchecked_shell, None);
}

#[tokio::test]
async fn a_session_without_the_grant_is_refused_with_what_unblocks_it() {
    let asking = started(false).await;

    let refusal = refused(asking.ask("true").await);

    assert_eq!(refusal.code, ErrorCode::NotAuthorized);
    let unblock = refusal.unblock.unwrap();
    assert_eq!(unblock.by, UnblockedBy::Owner);
    assert!(
        unblock.action.contains("run-commands"),
        "{}",
        unblock.action
    );
}

#[tokio::test]
async fn ask_check_says_whether_an_ask_would_reach_the_owner() {
    for (granted, expected) in [(true, None), (false, Some(ErrorCode::NotAuthorized))] {
        let asking = started(granted).await;
        let reply = asking
            .agent
            .request(
                Command::AskCheck {
                    launch_token: Some(asking.token.as_str().to_owned()),
                },
                GENEROUS,
            )
            .await;
        match (reply, expected) {
            (Ok(Reply::AskReady { session }), None) => assert_eq!(session, "s1"),
            (Err(ClientError::Daemon(error)), Some(code)) => assert_eq!(error.code, code),
            (other, _) => panic!("granted {granted}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn only_the_owner_decides_and_only_agents_ask() {
    let asking = started(true).await;
    let request = asking.asked("true").await;

    let from_agent = refused(
        asking
            .agent
            .request(
                Command::ApprovalApprove {
                    request: request.id,
                    revision: request.revision,
                    digest: request.digest.clone(),
                },
                GENEROUS,
            )
            .await,
    );
    let from_owner = refused(
        asking
            .owner
            .request(Command::AskCheck { launch_token: None }, GENEROUS)
            .await,
    );

    assert_eq!(from_agent.code, ErrorCode::NotAuthorized);
    assert_eq!(from_owner.code, ErrorCode::NotAuthorized);
    assert_eq!(asking.status(request.id).await.state, RunStateView::Asking);
}

#[tokio::test]
async fn a_guide_is_walked_one_step_at_a_time_and_the_agent_reads_the_answers() {
    let asking = started(true).await;
    let asked = guide(
        asking
            .agent
            .request(
                Command::AskGuide {
                    launch_token: Some(asking.token.as_str().to_owned()),
                    guide: "# Connect the app\nOpen the settings\nCopy the id\n  ask: Which id?\n"
                        .to_owned(),
                    cwd: asking.cwd_text(),
                },
                GENEROUS,
            )
            .await,
    );
    let answer = |step: usize, answer: Option<&str>| Command::GuideAnswer {
        guide: asked.id,
        step,
        answer: answer.map(str::to_owned),
    };

    guide(asking.owner.request(answer(0, None), GENEROUS).await);
    let stale = refused(asking.owner.request(answer(0, None), GENEROUS).await);
    assert_eq!(stale.code, ErrorCode::StaleRevision);
    guide(asking.owner.request(answer(1, Some("42")), GENEROUS).await);

    let seen = guide(
        asking
            .agent
            .request(
                Command::AskGuideStatus {
                    launch_token: None,
                    guide: asked.id,
                },
                GENEROUS,
            )
            .await,
    );
    assert_eq!(seen.state, GuideStateView::Done);
    assert_eq!(seen.steps[1].answer.as_deref(), Some("42"));
}

#[tokio::test]
async fn the_owner_reads_numbered_and_bulleted_steps_without_their_marker() {
    let asking = started(true).await;
    let asked = guide(
        asking
            .agent
            .request(
                Command::AskGuide {
                    launch_token: Some(asking.token.as_str().to_owned()),
                    guide:
                        "# Look up\n1. Look at the sky\n2) Count the stars\n- Name one\n* Go in\n"
                            .to_owned(),
                    cwd: asking.cwd_text(),
                },
                GENEROUS,
            )
            .await,
    );

    let shown = guide(
        asking
            .owner
            .request(Command::GuideShow { guide: asked.id }, GENEROUS)
            .await,
    );

    let texts: Vec<&str> = shown
        .steps
        .iter()
        .map(|step| step.text.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["Look at the sky", "Count the stars", "Name one", "Go in"]
    );
}

#[tokio::test]
async fn the_owner_sees_a_guides_directory_with_the_display_rules_applied() {
    let asking = started(true).await;

    let asked = guide(
        asking
            .agent
            .request(
                Command::AskGuide {
                    launch_token: Some(asking.token.as_str().to_owned()),
                    guide: "# Connect the app\nOpen the settings\n".to_owned(),
                    cwd: asking.cwd_text(),
                },
                GENEROUS,
            )
            .await,
    );

    assert_eq!(asked.shown_cwd.text, asking.cwd_text());
    assert!(
        asked.outside_project,
        "no registered checkout holds the agent, so every directory is outside"
    );
}

/// Registers a fresh Git repository as a project and makes it this test process's working
/// directory, so the daemon finds the asking agent in it. nextest runs each test in its own
/// process, so no other test sees the directory change.
async fn in_a_registered_project(asking: &Asking) -> std::path::PathBuf {
    let root = std::fs::canonicalize(asking.cwd.path()).unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(status.success());
    let added = asking
        .owner
        .request(
            Command::ProjectAdd {
                path: root.to_str().unwrap().to_owned(),
                name: Some("app".to_owned()),
            },
            GENEROUS,
        )
        .await;
    assert!(added.is_ok(), "{added:?}");
    std::env::set_current_dir(&root).unwrap();
    root
}

async fn shown_to_owner(asking: &Asking, request: i64) -> RunRequestView {
    run(asking
        .owner
        .request(Command::ApprovalShow { request }, GENEROUS)
        .await)
}

#[tokio::test]
async fn the_owner_is_warned_of_changed_build_files_the_command_runs_as_they_are_now() {
    let asking = started(true).await;
    let root = in_a_registered_project(&asking).await;
    let request = asking.asked("make test").await;
    assert_eq!(request.project.as_deref(), Some("app"));

    let before = shown_to_owner(&asking, request.id).await;
    std::fs::write(root.join("Makefile"), "test:\n\techo hi\n").unwrap();
    let after = shown_to_owner(&asking, request.id).await;

    assert_eq!(before.changed_build_files, Some(Vec::new()));
    assert_eq!(after.changed_build_files, Some(vec!["Makefile".to_owned()]));
}

#[tokio::test]
async fn the_owner_is_told_when_changed_build_files_cannot_be_read() {
    let asking = started(true).await;
    let request = asking.asked("make test").await;

    let shown = shown_to_owner(&asking, request.id).await;

    assert_eq!(
        shown.changed_build_files, None,
        "no project is known for the agent, so no checkout to read"
    );
}

#[tokio::test]
async fn a_timeout_without_foreground_carries_the_hint() {
    let asking = started(true).await;

    let hinted = asking.asked("timeout 5 sh -c 'read -r answer'").await;
    let plain = asking.asked("timeout --foreground 5 true").await;

    assert!(hinted.timeout_without_foreground);
    assert!(!plain.timeout_without_foreground);
}

#[tokio::test]
async fn a_long_command_reaches_the_owner_folded_with_what_is_left_out() {
    let asking = started(true).await;
    let command: Vec<String> = (1..=30).map(|line| format!("echo {line}")).collect();

    let request = asking.asked(&command.join("\n")).await;

    let folded = request.folded_command.expect("thirty lines are folded");
    assert!(request.shown_command.text.starts_with(&folded.preview));
    assert_eq!(folded.more_lines, 30 - folded.preview.lines().count());
}

impl Asking {
    /// Approves `command` and returns it once the runner sees it waiting at a prompt.
    async fn waiting_at_a_prompt(&self, command: &str) -> RunRequestView {
        let request = self.asked(command).await;
        run(self.approve(&request).await);
        let waiting = self
            .status_once(request.id, |run| run.waiting_for_input)
            .await;
        assert!(waiting.waiting_for_input, "{waiting:?}");
        waiting
    }

    async fn type_line(&self, request: i64, text: &str) {
        run(self
            .owner
            .request(
                Command::ApprovalInput {
                    request,
                    text: text.to_owned(),
                },
                GENEROUS,
            )
            .await);
    }

    async fn queue(&self) -> Vec<RunRequestView> {
        match self.owner.request(Command::ApprovalQueue, GENEROUS).await {
            Ok(Reply::ApprovalQueue { runs, .. }) => runs,
            other => panic!("expected the queue, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_password_prompt_reads_the_typed_line_with_echo_off() {
    let asking = started(true).await;
    let request = asking
        .waiting_at_a_prompt(
            "printf 'Type it: '; stty -echo; read -r secret; stty echo; \
             printf '\\nlength %s\\n' \"${#secret}\"",
        )
        .await;

    asking.type_line(request.id, "hunter22").await;

    let output = asking.ended(request.id).await.result.unwrap().output;
    assert!(output.ends_with("length 8"), "{output}");
    assert!(
        !output.contains("hunter22"),
        "a terminal with echo off shows nothing typed"
    );
}

#[tokio::test]
async fn a_prompt_in_a_pipeline_stage_reads_the_terminal_in_the_foreground_group() {
    let asking = started(true).await;
    let request = asking
        .waiting_at_a_prompt(
            "sh -c 'printf \"Name? \" >/dev/tty; read -r name </dev/tty; echo \"hello $name\"' | cat",
        )
        .await;

    asking.type_line(request.id, "theo").await;

    let ended = asking.ended(request.id).await;
    let result = ended.result.unwrap();
    assert_eq!(result.exit_code, 0);
    assert!(result.output.ends_with("hello theo"), "{}", result.output);
}

#[tokio::test]
async fn a_new_request_queues_behind_a_command_waiting_for_input() {
    let asking = started(true).await;
    let waiting = asking
        .waiting_at_a_prompt("printf 'sudo password: '; read -r answer")
        .await;

    let newer = asking.asked("echo newer").await;
    let queue = asking.queue().await;

    let order: Vec<(i64, bool)> = queue
        .iter()
        .map(|run| (run.id, run.waiting_for_input))
        .collect();
    assert_eq!(order, [(waiting.id, true), (newer.id, false)]);
    asking.type_line(waiting.id, "done").await;
}

#[tokio::test]
async fn an_agent_waiting_on_an_approval_keeps_working_on_the_channel() {
    let home = TestHome::new();
    let token = home
        .launch(
            "s1",
            &identity_of(std::process::id()).unwrap(),
            Some(Permission::RunCommands),
        )
        .await;
    {
        let database = home.open_state().await;
        let write = database.write().await.unwrap();
        let session = SessionId::new("s1").unwrap();
        cockpit_authorization::grant(
            &write,
            &session,
            Permission::ChangeState,
            &Scope::Session(session.clone()),
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
    }
    let daemon = home.start();
    let asking = Asking {
        agent: home.connect_agent().await,
        owner: home.connect().await,
        daemon: Some(daemon),
        home,
        token,
        cwd: tempfile::tempdir().unwrap(),
    };
    let request = asking.asked("echo later").await;

    let reported = asking
        .agent
        .request(
            Command::AgentReport {
                launch_token: None,
                report: cockpit_protocol::AgentReport {
                    doing: Some("other work while the owner decides".to_owned()),
                    ..cockpit_protocol::AgentReport::default()
                },
            },
            GENEROUS,
        )
        .await;

    assert!(reported.is_ok(), "{reported:?}");
    let queue = asking.queue().await;
    assert_eq!(queue.len(), 1);
    assert_eq!(
        (queue[0].id, queue[0].state),
        (request.id, RunStateView::Asking)
    );
}

#[tokio::test]
async fn an_approval_at_once_after_the_ask_is_refused_with_when_to_approve() {
    let asking = started(true).await;
    let request = asking.asked("echo approved").await;

    let refusal = refused(asking.approve_once(&request).await);

    assert_eq!(refusal.code, ErrorCode::Refused);
    let unblock = refusal.unblock.unwrap();
    assert_eq!(unblock.by, UnblockedBy::Owner);
    assert!(
        unblock.action.contains("approve it again in"),
        "{unblock:?}"
    );
    assert_eq!(asking.status(request.id).await.state, RunStateView::Asking);
    run(asking.approve(&request).await);
    assert_eq!(asking.ended(request.id).await.state, RunStateView::Finished);
}

/// #163: the run is an execution, so a restart reports what became of it. Killing the daemon
/// closes the run's terminal, which hangs the command up.
#[tokio::test]
async fn a_run_a_daemon_crash_hung_up_is_reported_ended_and_never_repeated() {
    let mut asking = started(true).await;
    let marker = asking.cwd.path().join("started");
    let request = asking
        .running(
            &format!("echo once >> {}; exec sleep 300", marker.display()),
            &marker,
        )
        .await;

    asking.restart().await;

    let ended = asking.ended(request.id).await;
    assert_eq!(ended.state, RunStateView::OutcomeUnknown, "{ended:?}");
    assert!(
        ended
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("not observed"),
        "{ended:?}"
    );
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "once\n");
}

#[tokio::test]
async fn a_run_that_outlives_a_daemon_restart_is_reattached_until_it_ends() {
    let mut asking = started(true).await;
    let marker = asking.cwd.path().join("started");
    let request = asking
        .running(
            &format!("trap '' HUP; touch {}; sleep 5", marker.display()),
            &marker,
        )
        .await;

    asking.restart().await;

    let reattached = asking.status(request.id).await;
    assert_eq!(reattached.state, RunStateView::Running, "{reattached:?}");
    let ended = asking.ended(request.id).await;
    assert_eq!(ended.state, RunStateView::OutcomeUnknown, "{ended:?}");
    assert!(
        ended
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("restarted while the command ran; it ended later (exited)"),
        "{ended:?}"
    );
}
