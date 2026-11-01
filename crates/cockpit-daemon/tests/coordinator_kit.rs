//! Issue #410: a session acts on other sessions only through the owner's coordinator grants.
//! Driven through the real daemon, the real `cockpit` run by a launched agent on the agent
//! socket, and the owner's socket: without grants every verb is refused; with them a
//! coordinator sees a worker's question, messages it, and starts a new worker the owner
//! approves through the `ask run` queue; the audit names both sessions of each action.

mod real_daemon;

use std::path::Path;
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    Command, CoordinatorAuditView, ErrorCode, Reply, RunRequestView, RunStateView, SessionView,
};
use real_daemon::{
    ASKED_QUESTION, COMMANDED_PROJECT, GENEROUS, Home, QUESTION_PROJECT, cockpit_program, listed,
    new_session,
};

const COORDINATOR_GRANTS: [&str; 3] = ["observe-sessions", "message-sessions", "start-sessions"];

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

async fn grant(owner: &Connection, session: &str, permission: &str) {
    let answer = owner
        .request(
            Command::SessionGrant {
                session: session.to_owned(),
                permission: permission.to_owned(),
                without_confirmation: false,
                per_hour: None,
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(answer, Ok(Reply::CoordinatorGrants { .. })),
        "session-grant {permission}: {answer:?}"
    );
}

/// A coordinator session the test commands, and a worker that asked [`ASKED_QUESTION`].
struct Pair {
    coordinator: SessionView,
    coordinator_root: std::path::PathBuf,
    worker: SessionView,
    worker_root: std::path::PathBuf,
}

async fn coordinator_and_worker(home: &Home, owner: &Connection) -> Pair {
    let (coordinator_root, _) = home.project(owner, COMMANDED_PROJECT).await;
    let (worker_root, _) = home.project(owner, QUESTION_PROJECT).await;
    let coordinator = new_session(owner, &coordinator_root).await;
    let worker = new_session(owner, &worker_root).await;
    wait_until("the worker reports its question", || async {
        listed_report_question(owner, &worker.id).await.as_deref() == Some(ASKED_QUESTION)
    })
    .await;
    Pair {
        coordinator,
        coordinator_root,
        worker,
        worker_root,
    }
}

async fn listed_report_question(owner: &Connection, session: &str) -> Option<String> {
    match owner.request(Command::AgentReports, GENEROUS).await {
        Ok(Reply::AgentReports { reports }) => reports
            .into_iter()
            .find(|report| report.session == session)
            .and_then(|report| report.needs_input),
        other => panic!("agent-reports answered {other:?}"),
    }
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
async fn without_its_grant_each_coordinator_verb_is_refused_naming_the_grant() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let pair = coordinator_and_worker(&home, &owner).await;
    let worker_dir = pair.worker_root.display().to_string();
    let coordinator = pair.coordinator.id.as_str();

    for (arguments, permission) in [
        (vec!["agent", "sessions"], "observe-sessions"),
        (
            vec!["agent", "message", pair.worker.id.as_str(), "hello"],
            "message-sessions",
        ),
        (
            vec!["agent", "start", worker_dir.as_str()],
            "start-sessions",
        ),
    ] {
        let printed = as_coordinator(&home, &pair.coordinator_root, &arguments).await;

        assert!(printed.ends_with("exit=1\n"), "{arguments:?}: {printed}");
        assert!(
            printed.contains(&format!("holds no {permission} grant")),
            "{arguments:?}: {printed}"
        );
        assert!(
            printed.contains(&format!("cockpit session grant {coordinator} {permission}")),
            "{arguments:?}: {printed}"
        );
    }
    let (runs, _) = queue(&owner).await;
    assert!(runs.is_empty(), "a refused start asked nothing: {runs:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_coordinator_sees_a_question_messages_the_worker_and_starts_one_the_owner_approves() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let pair = coordinator_and_worker(&home, &owner).await;
    let (new_root, _) = home.project(&owner, "plain").await;
    for permission in COORDINATOR_GRANTS {
        grant(&owner, &pair.coordinator.id, permission).await;
    }

    let observed = as_coordinator(&home, &pair.coordinator_root, &["agent", "sessions"]).await;
    assert!(observed.ends_with("exit=0\n"), "{observed}");
    let worker_line = observed
        .lines()
        .find(|line| line.starts_with(&pair.worker.id))
        .unwrap_or_else(|| panic!("the worker is listed: {observed}"));
    assert!(
        worker_line.contains(&format!("needs-input={ASKED_QUESTION:?}")),
        "{worker_line}"
    );
    assert!(
        !observed.contains(&pair.coordinator.id),
        "a coordinator does not list itself: {observed}"
    );

    let messaged = as_coordinator(
        &home,
        &pair.coordinator_root,
        &["agent", "message", &pair.worker.id, "Base it on main."],
    )
    .await;
    assert!(messaged.ends_with("exit=0\n"), "{messaged}");
    let inbox = pair.worker_root.join("inbox.log");
    let expected = format!(
        "[Cockpit] Message from session {} (a collaborator, not the owner): Base it on main.",
        pair.coordinator.id
    );
    wait_until("the worker reads the message", || async {
        std::fs::read_to_string(&inbox).is_ok_and(|read| read.lines().any(|line| line == expected))
    })
    .await;

    let started = home.command_agent(
        &pair.coordinator_root,
        &cockpit_line(&[
            "agent",
            "start",
            &new_root.display().to_string(),
            "--prompt",
            "Review the fix",
        ]),
    );
    let request = asked_start(&owner, &pair.coordinator.id).await;
    assert_eq!(
        request.command,
        format!(
            "cockpit session new '{}' --prompt 'Review the fix'",
            new_root.display()
        )
    );
    approve(&owner, &request).await;
    let printed = home.agent_output(&pair.coordinator_root, started).await;
    assert!(printed.ends_with("exit=0\n"), "{printed}");
    let new_worker = printed
        .lines()
        .find_map(|line| line.strip_prefix("started session "))
        .unwrap_or_else(|| panic!("the start names its session: {printed}"))
        .to_owned();
    assert_eq!(
        listed(&owner, &new_worker).await.working_dir,
        new_root.display().to_string()
    );
    assert_eq!(
        listed(&owner, &new_worker).await.agent,
        listed(&owner, &pair.coordinator.id).await.agent,
        "a coordinator's start runs its own agent"
    );

    let audit = audit_of(&owner, &pair.coordinator.id).await;
    let actions: Vec<(&str, Option<&str>)> = audit
        .iter()
        .map(|entry| (entry.action.as_str(), entry.target.as_deref()))
        .collect();
    assert_eq!(
        actions,
        vec![
            ("started", Some(new_worker.as_str())),
            ("start", None),
            ("message", Some(pair.worker.id.as_str())),
        ]
    );
    assert!(
        audit.iter().all(|entry| entry.actor == pair.coordinator.id),
        "{audit:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unconfirmed_start_begins_at_once_until_the_hourly_cap() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let (new_root, _) = home.project(&owner, "plain").await;
    let coordinator = new_session(&owner, &coordinator_root).await;
    let granted = owner
        .request(
            Command::SessionGrant {
                session: coordinator.id.clone(),
                permission: "start-sessions".to_owned(),
                without_confirmation: true,
                per_hour: Some(1),
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(granted, Ok(Reply::CoordinatorGrants { .. })),
        "{granted:?}"
    );
    let new_dir = new_root.display().to_string();

    let first = as_coordinator(&home, &coordinator_root, &["agent", "start", &new_dir]).await;
    let second = as_coordinator(&home, &coordinator_root, &["agent", "start", &new_dir]).await;

    assert!(first.ends_with("exit=0\n"), "{first}");
    assert!(first.starts_with("started session "), "{first}");
    assert!(second.ends_with("exit=1\n"), "{second}");
    assert!(
        second.contains("has made its 1 starts of the past hour"),
        "{second}"
    );
    let (runs, _) = queue(&owner).await;
    assert!(
        runs.is_empty(),
        "an unconfirmed start asks nobody: {runs:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cards_show_a_coordinators_grants_and_the_message_it_sent() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let pair = coordinator_and_worker(&home, &owner).await;
    grant(&owner, &pair.coordinator.id, "message-sessions").await;

    as_coordinator(
        &home,
        &pair.coordinator_root,
        &["agent", "message", &pair.worker.id, "Base it on main."],
    )
    .await;

    assert_eq!(
        listed(&owner, &pair.coordinator.id)
            .await
            .coordinator_grants,
        vec!["message-sessions".to_owned()]
    );
    wait_until("the worker's card shows the message received", || async {
        listed(&owner, &pair.worker.id)
            .await
            .inbound_message
            .is_some_and(|message| {
                message.from == pair.coordinator.id
                    && message.text == "Base it on main."
                    && message.delivery == "received"
            })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_grant_refuses_again() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let pair = coordinator_and_worker(&home, &owner).await;
    grant(&owner, &pair.coordinator.id, "observe-sessions").await;
    let revoked = owner
        .request(
            Command::SessionRevoke {
                session: pair.coordinator.id.clone(),
                permission: "observe-sessions".to_owned(),
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(&revoked, Ok(Reply::CoordinatorGrants { grants, .. }) if grants.is_empty()),
        "{revoked:?}"
    );

    let printed = as_coordinator(&home, &pair.coordinator_root, &["agent", "sessions"]).await;

    assert!(printed.ends_with("exit=1\n"), "{printed}");
    assert!(
        printed.contains("holds no observe-sessions grant"),
        "{printed}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_still_queued_when_its_grant_is_revoked_is_never_typed() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let coordinator = new_session(&owner, &coordinator_root).await;
    // A second commanded agent (the fake agent acts as its folder's name says), which has
    // not reported yet, so nothing can be typed to it.
    let target_root = coordinator_root.join("target").join(COMMANDED_PROJECT);
    std::fs::create_dir_all(&target_root).unwrap();
    let target = new_session(&owner, &target_root).await;
    grant(&owner, &coordinator.id, "message-sessions").await;
    let printed = as_coordinator(
        &home,
        &coordinator_root,
        &["agent", "message", &target.id, "Base it on main."],
    )
    .await;
    assert!(printed.ends_with("exit=0\n"), "{printed}");
    wait_until("the message waits, not typed", || async {
        listed(&owner, &target.id)
            .await
            .inbound_message
            .is_some_and(|message| message.delivery == "queued")
    })
    .await;

    let revoked = owner
        .request(
            Command::SessionRevoke {
                session: coordinator.id.clone(),
                permission: "message-sessions".to_owned(),
            },
            GENEROUS,
        )
        .await;
    assert!(revoked.is_ok(), "{revoked:?}");
    let reported = as_coordinator(
        &home,
        &target_root,
        &["agent", "report", "--state", "waiting"],
    )
    .await;
    assert!(reported.ends_with("exit=0\n"), "{reported}");

    wait_until(
        "the revoked message fails instead of being typed",
        || async {
            listed(&owner, &target.id)
                .await
                .inbound_message
                .is_some_and(|message| message.delivery == "failed")
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_grant_is_given_from_the_agent_socket() {
    let home = Home::new();
    let _daemon = home.start();
    let agent = Connection::open(&home.agent_socket_path(), GENEROUS)
        .await
        .expect("the agent socket accepts the handshake");

    let answer = agent
        .request(
            Command::SessionGrant {
                session: "s0".to_owned(),
                permission: "start-sessions".to_owned(),
                without_confirmation: true,
                per_hour: Some(100),
            },
            GENEROUS,
        )
        .await;

    assert!(
        matches!(&answer, Err(ClientError::Daemon(error)) if error.code == ErrorCode::NotAuthorized),
        "{answer:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_coordinator_grant_is_given_with_session_grant() {
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (root, _) = home.project(&owner, "plain").await;
    let session = new_session(&owner, &root).await;

    let answer = owner
        .request(
            Command::SessionGrant {
                session: session.id,
                permission: "run-commands".to_owned(),
                without_confirmation: false,
                per_hour: None,
            },
            GENEROUS,
        )
        .await;

    assert!(
        matches!(&answer, Err(ClientError::Daemon(error)) if error.code == ErrorCode::InvalidArgument),
        "{answer:?}"
    );
}

async fn queue(owner: &Connection) -> (Vec<RunRequestView>, Vec<cockpit_protocol::GuideView>) {
    match owner.request(Command::ApprovalQueue, GENEROUS).await {
        Ok(Reply::ApprovalQueue { runs, guides }) => (runs, guides),
        other => panic!("approval-queue answered {other:?}"),
    }
}

/// The start request `coordinator` filed, once the owner's queue holds it.
async fn asked_start(owner: &Connection, coordinator: &str) -> RunRequestView {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (runs, _) = queue(owner).await;
        if let Some(run) = runs.into_iter().find(|run| run.session == coordinator) {
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

async fn audit_of(owner: &Connection, session: &str) -> Vec<CoordinatorAuditView> {
    match owner
        .request(
            Command::SessionGrants {
                session: session.to_owned(),
            },
            GENEROUS,
        )
        .await
    {
        Ok(Reply::CoordinatorGrants { audit, .. }) => audit,
        other => panic!("session-grants answered {other:?}"),
    }
}
