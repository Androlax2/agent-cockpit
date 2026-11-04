//! Issue #710: a question from a session a coordinator started waits on that coordinator
//! before the owner. Driven through the real daemon, the real `cockpit` run by a launched
//! coordinator agent on the agent socket, and the owner's socket: the coordinator starts a
//! worker that asks, sees the question as its own, answers it or escalates it; the owner sees
//! it held, and waiting on them once escalated or once the coordinator lost its grant. The
//! question's time with the coordinator is driven with a given clock in `cockpit-sessions`'
//! `question_routing` tests, since a test does not wait fifteen minutes.

mod candidate;
mod real_daemon;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    Command, DecidedThroughView, ErrorCode, Reply, RunRequestView, RunStateView, SessionReport,
};
use real_daemon::{
    ASKED_QUESTION, COMMANDED_PROJECT, GENEROUS, Home, QUESTION_PROJECT, cockpit_program,
    new_session,
};

fn assert_the_cli_is_built() {
    let cockpit = cockpit_program();
    assert!(
        cockpit.is_file(),
        "{} is missing: run the workspace's tests (`make crates-test`), which build it",
        cockpit.display()
    );
}

/// `cockpit ARGUMENTS` as the commanded agent in `root` runs it, single-quoted.
fn cockpit_line(arguments: &[&str]) -> String {
    std::iter::once(cockpit_program().display().to_string())
        .chain(arguments.iter().map(|argument| (*argument).to_owned()))
        .map(|word| format!("'{}'", word.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the coordinator's `cockpit ARGUMENTS` printed, ending `exit=<code>`.
async fn as_coordinator(home: &Home, root: &Path, arguments: &[&str]) -> String {
    let number = home.command_agent(root, &cockpit_line(arguments));
    home.agent_output(root, number).await
}

async fn owner_request(owner: &Connection, command: Command) -> Reply {
    owner
        .request(command.clone(), GENEROUS)
        .await
        .unwrap_or_else(|error| panic!("{command:?} answered {error:?}"))
}

/// A coordinator holding every coordinator grant, starts without confirmation included, and
/// the worker it started in the question project, once the worker asked.
struct Started {
    home: Home,
    _daemon: real_daemon::Daemon,
    owner: Connection,
    coordinator: String,
    coordinator_root: PathBuf,
    worker: String,
}

async fn a_coordinator_starts_a_worker_that_asks() -> Started {
    assert_the_cli_is_built();
    let home = Home::new();
    let daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let (question_root, _) = home.project(&owner, QUESTION_PROJECT).await;
    let coordinator = new_session(&owner, &coordinator_root).await.id;
    for permission in ["observe-sessions", "message-sessions"] {
        owner_request(
            &owner,
            Command::SessionGrant {
                session: coordinator.clone(),
                permission: permission.to_owned(),
                without_confirmation: false,
                per_hour: None,
            },
        )
        .await;
    }
    owner_request(
        &owner,
        Command::SessionGrant {
            session: coordinator.clone(),
            permission: "start-sessions".to_owned(),
            without_confirmation: true,
            per_hour: Some(5),
        },
    )
    .await;
    let started = as_coordinator(
        &home,
        &coordinator_root,
        &[
            "agent",
            "start",
            &question_root.display().to_string(),
            "--agent",
            "claude",
        ],
    )
    .await;
    assert!(started.ends_with("exit=0\n"), "{started}");
    let worker = started
        .lines()
        .find_map(|line| line.strip_prefix("started session "))
        .unwrap_or_else(|| panic!("the start names its session: {started}"))
        .to_owned();
    wait_until("the worker reports its question", || async {
        report_of(&owner, &worker)
            .await
            .is_some_and(|report| report.needs_input.as_deref() == Some(ASKED_QUESTION))
    })
    .await;
    Started {
        home,
        _daemon: daemon,
        owner,
        coordinator,
        coordinator_root,
        worker,
    }
}

async fn report_of(owner: &Connection, session: &str) -> Option<SessionReport> {
    match owner_request(owner, Command::AgentReports).await {
        Reply::AgentReports { reports } => {
            reports.into_iter().find(|report| report.session == session)
        }
        other => panic!("agent-reports answered {other:?}"),
    }
}

async fn holder_of(owner: &Connection, session: &str) -> Option<String> {
    report_of(owner, session)
        .await
        .and_then(|report| report.held_by)
}

async fn wait_until<Check, Checked>(what: &str, check: Check)
where
    Check: Fn() -> Checked,
    Checked: Future<Output = bool>,
{
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check().await {
        assert!(Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_sees_a_started_workers_question_held_by_its_coordinator() {
    let started = a_coordinator_starts_a_worker_that_asks().await;

    let held_by = holder_of(&started.owner, &started.worker).await;

    assert_eq!(held_by.as_deref(), Some(started.coordinator.as_str()));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_coordinator_lists_the_question_as_held_by_it() {
    let started = a_coordinator_starts_a_worker_that_asks().await;

    let observed = as_coordinator(
        &started.home,
        &started.coordinator_root,
        &["agent", "sessions"],
    )
    .await;

    assert!(observed.ends_with("exit=0\n"), "{observed}");
    let worker_line = observed
        .lines()
        .find(|line| line.starts_with(&started.worker))
        .unwrap_or_else(|| panic!("the worker is listed: {observed}"));
    assert!(
        worker_line.contains(&format!("needs-input={ASKED_QUESTION:?}")),
        "{worker_line}"
    );
    assert!(
        worker_line.contains(&format!("held-by={:?}", started.coordinator)),
        "{worker_line}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_coordinators_answer_reaches_the_worker_it_holds_the_question_of() {
    let started = a_coordinator_starts_a_worker_that_asks().await;
    let worker_root = PathBuf::from(
        match owner_request(&started.owner, Command::SessionList { ended: false }).await {
            Reply::Sessions { sessions } => {
                sessions
                    .into_iter()
                    .find(|session| session.id == started.worker)
                    .expect("the worker is listed")
                    .working_dir
            }
            other => panic!("session-list answered {other:?}"),
        },
    );

    let messaged = as_coordinator(
        &started.home,
        &started.coordinator_root,
        &["agent", "message", &started.worker, "Base it on main."],
    )
    .await;

    assert!(messaged.ends_with("exit=0\n"), "{messaged}");
    let expected = format!(
        "[Torlek] Message from session {} (a collaborator, not the owner): Base it on main.",
        started.coordinator
    );
    let inbox = worker_root.join("inbox.log");
    wait_until("the worker reads the answer", || async {
        std::fs::read_to_string(&inbox).is_ok_and(|read| read.lines().any(|line| line == expected))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_escalated_question_waits_on_the_owner() {
    let started = a_coordinator_starts_a_worker_that_asks().await;

    let escalated = as_coordinator(
        &started.home,
        &started.coordinator_root,
        &["agent", "escalate", &started.worker],
    )
    .await;

    assert!(escalated.ends_with("exit=0\n"), "{escalated}");
    assert!(
        escalated.contains(&format!(
            "session {}'s question waits on the owner now",
            started.worker
        )),
        "{escalated}"
    );
    let report = report_of(&started.owner, &started.worker)
        .await
        .expect("the worker reported");
    assert_eq!(report.needs_input.as_deref(), Some(ASKED_QUESTION));
    assert_eq!(report.held_by, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_coordinator_cannot_escalate_a_session_it_did_not_start() {
    let started = a_coordinator_starts_a_worker_that_asks().await;
    let (plain_root, _) = started.home.project(&started.owner, "plain").await;
    let owners_session = new_session(&started.owner, &plain_root).await.id;

    let escalated = as_coordinator(
        &started.home,
        &started.coordinator_root,
        &["agent", "escalate", &owners_session],
    )
    .await;

    assert!(escalated.ends_with("exit=1\n"), "{escalated}");
    assert!(
        escalated.contains(&format!(
            "session {} did not start session {owners_session}",
            started.coordinator
        )),
        "{escalated}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_question_reaches_the_owner_once_the_coordinator_loses_message_sessions() {
    let started = a_coordinator_starts_a_worker_that_asks().await;

    owner_request(
        &started.owner,
        Command::SessionRevoke {
            session: started.coordinator.clone(),
            permission: "message-sessions".to_owned(),
        },
    )
    .await;

    assert_eq!(holder_of(&started.owner, &started.worker).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_question_in_a_session_the_owner_started_waits_on_the_owner() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (question_root, _) = home.project(&owner, QUESTION_PROJECT).await;
    let worker = new_session(&owner, &question_root).await.id;
    wait_until("the worker reports its question", || async {
        report_of(&owner, &worker)
            .await
            .is_some_and(|report| report.needs_input.is_some())
    })
    .await;

    assert_eq!(holder_of(&owner, &worker).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_the_owner_approved_starting_asks_its_coordinator_first() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let (question_root, _) = home.project(&owner, QUESTION_PROJECT).await;
    let coordinator = new_session(&owner, &coordinator_root).await.id;
    for permission in ["message-sessions", "start-sessions"] {
        owner_request(
            &owner,
            Command::SessionGrant {
                session: coordinator.clone(),
                permission: permission.to_owned(),
                without_confirmation: false,
                per_hour: None,
            },
        )
        .await;
    }
    let asked = home.command_agent(
        &coordinator_root,
        &cockpit_line(&[
            "agent",
            "start",
            &question_root.display().to_string(),
            "--agent",
            "claude",
        ]),
    );
    approve(&owner, &asked_start(&owner, &coordinator).await).await;
    let printed = home.agent_output(&coordinator_root, asked).await;
    assert!(printed.ends_with("exit=0\n"), "{printed}");
    let worker = printed
        .lines()
        .find_map(|line| line.strip_prefix("started session "))
        .unwrap_or_else(|| panic!("the start names its session: {printed}"))
        .to_owned();

    wait_until(
        "the worker's question is held by its coordinator",
        || async { holder_of(&owner, &worker).await.as_deref() == Some(coordinator.as_str()) },
    )
    .await;
}

/// The start request `coordinator` filed, once the owner's queue holds it.
async fn asked_start(owner: &Connection, coordinator: &str) -> RunRequestView {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Reply::ApprovalQueue { runs, .. } =
            owner_request(owner, Command::ApprovalQueue).await
            && let Some(run) = runs.into_iter().find(|run| run.session == coordinator)
        {
            return run;
        }
        assert!(
            Instant::now() < deadline,
            "the start never reached the queue"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Approves `request` once its grace has passed.
async fn approve(owner: &Connection, request: &RunRequestView) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let answer = owner
            .request(
                Command::ApprovalApprove {
                    request: request.id,
                    revision: request.revision,
                    digest: request.digest.clone(),
                    through: Some(DecidedThroughView::Tui),
                },
                GENEROUS,
            )
            .await;
        match answer {
            Ok(Reply::RunRequest { request }) => {
                assert_eq!(request.state, RunStateView::Approved);
                return;
            }
            Err(ClientError::Daemon(error)) if error.code == ErrorCode::Refused => {
                assert!(
                    Instant::now() < deadline,
                    "the grace never passed: {error:?}"
                );
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            other => panic!("approval-approve answered {other:?}"),
        }
    }
}
