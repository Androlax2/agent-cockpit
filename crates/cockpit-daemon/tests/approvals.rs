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
    Command, ErrorCode, GuideStateView, GuideView, ProtocolError, Reply, RunRequestView,
    RunStateView, UnblockedBy,
};
use common::{GENEROUS, TestHome};

/// Launches session `s1` rooted at this test process, granted `run-commands` or not, while
/// no daemon runs.
async fn launch(home: &TestHome, granted: bool) -> LaunchToken {
    let session = SessionId::new("s1").unwrap();
    let database = home.open_state().await;
    let write = database.write().await.unwrap();
    let token = cockpit_agent_channel::mint(&write, &session).await.unwrap();
    cockpit_agent_channel::record_root(&write, &session, &identity_of(std::process::id()).unwrap())
        .await
        .unwrap();
    if granted {
        cockpit_authorization::grant(
            &write,
            &session,
            Permission::RunCommands,
            &Scope::Session(session.clone()),
        )
        .await
        .unwrap();
    }
    write.commit().await.unwrap();
    token
}

/// Fields drop in order: the daemon stops before its home is deleted.
struct Asking {
    _daemon: common::RunningDaemon,
    _home: TestHome,
    token: LaunchToken,
    agent: Connection,
    owner: Connection,
    /// Where the asked commands run.
    cwd: tempfile::TempDir,
}

async fn started(granted: bool) -> Asking {
    let home = TestHome::new();
    let token = launch(&home, granted).await;
    let daemon = home.start();
    let agent = home.connect_agent().await;
    let owner = home.connect().await;
    Asking {
        _daemon: daemon,
        _home: home,
        token,
        agent,
        owner,
        cwd: tempfile::tempdir().unwrap(),
    }
}

impl Asking {
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

    async fn approve(&self, request: &RunRequestView) -> Result<Reply, ClientError> {
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

fn refused(reply: Result<Reply, ClientError>) -> ProtocolError {
    match reply {
        Err(ClientError::Daemon(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
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
    let request = asking.asked("printf 'caf\u{e9}\\n' # \u{202e}").await;

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
