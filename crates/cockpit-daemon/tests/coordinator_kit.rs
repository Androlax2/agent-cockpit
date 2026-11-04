//! Issue #410: a session acts on other sessions only through the owner's coordinator grants.
//! Driven through the real daemon, the real `cockpit` run by a launched agent on the agent
//! socket, and the owner's socket: without grants every verb is refused; with them a
//! coordinator sees a worker's question, messages it, and starts a new worker the owner
//! approves through the `ask run` queue; the audit names both sessions of each action.

mod candidate;
mod real_daemon;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    Command, CoordinatorAuditView, DecidedThroughView, ErrorCode, Reply, RunRequestView,
    RunStateView, SessionView,
};
use real_daemon::{
    ASKED_QUESTION, COMMANDED_PROJECT, Daemon, GENEROUS, Home, QUESTION_PROJECT, cockpit_program,
    listed, new_session, wait_for_state,
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
                max_active: None,
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
            printed.contains(&format!("torlek session grant {coordinator} {permission}")),
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
        "[Torlek] Message from session {} (a collaborator, not the owner): Base it on main.",
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
            "--agent",
            "claude",
            "--prompt",
            "Review the fix",
        ]),
    );
    let request = asked_start(&owner, &pair.coordinator.id).await;
    assert_eq!(
        request.command,
        format!(
            "torlek session new '{}' --agent 'claude' --prompt 'Review the fix'",
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

    let audit = audit_of(&owner, &pair.coordinator.id).await;
    let actions: Vec<(&str, Option<&str>)> = audit
        .iter()
        .map(|entry| (entry.action.as_str(), entry.target.as_deref()))
        .collect();
    assert_eq!(
        actions,
        vec![
            ("started", Some(new_worker.as_str())),
            ("approved", None),
            ("start", None),
            ("message", Some(pair.worker.id.as_str())),
        ]
    );
    assert!(
        audit.iter().all(|entry| entry.actor == pair.coordinator.id),
        "{audit:?}"
    );
    assert_eq!(
        audit[1].detail,
        Some(format!(
            "approval request {}, by the owner through the TUI",
            request.id
        ))
    );
}

/// A coordinator holding `start-sessions` (confirmed) whose `agent start` waits in the
/// owner's queue as `request`; `started` is the agent command to read its answer from.
struct AskedStart {
    home: Home,
    _daemon: Daemon,
    owner: Connection,
    coordinator: String,
    coordinator_root: PathBuf,
    started: usize,
    request: RunRequestView,
}

async fn asked_start_by_a_coordinator() -> AskedStart {
    assert_the_cli_is_built();
    let home = Home::new();
    let daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let (new_root, _) = home.project(&owner, "plain").await;
    let coordinator = new_session(&owner, &coordinator_root).await.id;
    grant(&owner, &coordinator, "start-sessions").await;
    let started = home.command_agent(
        &coordinator_root,
        &cockpit_line(&[
            "agent",
            "start",
            &new_root.display().to_string(),
            "--agent",
            "claude",
        ]),
    );
    let request = asked_start(&owner, &coordinator).await;
    AskedStart {
        home,
        _daemon: daemon,
        owner,
        coordinator,
        coordinator_root,
        started,
        request,
    }
}

/// #657: the owner auditing an agent-started session reads the decision that let it start:
/// approved, by the owner, when, and through which client.
#[tokio::test(flavor = "multi_thread")]
async fn an_approved_start_names_the_owners_decision() {
    let asked = asked_start_by_a_coordinator().await;
    approve(&asked.owner, &asked.request).await;
    let printed = asked
        .home
        .agent_output(&asked.coordinator_root, asked.started)
        .await;
    assert!(printed.ends_with("exit=0\n"), "{printed}");

    let shown = match asked
        .owner
        .request(
            Command::ApprovalShow {
                request: asked.request.id,
            },
            GENEROUS,
        )
        .await
    {
        Ok(Reply::RunRequest { request }) => request,
        other => panic!("approval-show answered {other:?}"),
    };

    let decision = shown.decision.expect("the decision is recorded");
    assert!(decision.approved, "{decision:?}");
    assert_eq!(decision.through, Some(DecidedThroughView::Tui));
    assert!(
        decision.at >= asked.request.asked_at,
        "{decision:?} after {}",
        asked.request.asked_at
    );
}

/// #657: a declined start leaves an audit entry, not just the ask.
#[tokio::test(flavor = "multi_thread")]
async fn a_declined_start_is_audited() {
    let asked = asked_start_by_a_coordinator().await;
    let declined = asked
        .owner
        .request(
            Command::ApprovalDecline {
                request: asked.request.id,
                through: Some(DecidedThroughView::Cli),
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(declined, Ok(Reply::RunRequest { .. })),
        "{declined:?}"
    );
    let printed = asked
        .home
        .agent_output(&asked.coordinator_root, asked.started)
        .await;
    assert!(printed.ends_with("exit=125\n"), "{printed}");

    let audit = audit_of(&asked.owner, &asked.coordinator).await;

    let actions: Vec<(&str, Option<&str>)> = audit
        .iter()
        .map(|entry| (entry.action.as_str(), entry.detail.as_deref()))
        .collect();
    let ask = format!("approval request {}", asked.request.id);
    let decided = format!(
        "approval request {}, by the owner through the CLI",
        asked.request.id
    );
    assert_eq!(
        actions,
        vec![
            ("declined", Some(decided.as_str())),
            ("start", Some(ask.as_str())),
        ]
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
                max_active: None,
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(granted, Ok(Reply::CoordinatorGrants { .. })),
        "{granted:?}"
    );
    let new_dir = new_root.display().to_string();

    let first = as_coordinator(
        &home,
        &coordinator_root,
        &["agent", "start", &new_dir, "--agent", "claude"],
    )
    .await;
    let second = as_coordinator(
        &home,
        &coordinator_root,
        &["agent", "start", &new_dir, "--agent", "claude"],
    )
    .await;

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
    let audit = audit_of(&owner, &coordinator.id).await;
    let start = audit
        .iter()
        .find(|entry| entry.action == "start")
        .unwrap_or_else(|| panic!("the start is audited: {audit:?}"));
    assert_eq!(start.detail.as_deref(), Some("without confirmation"));
}

/// #701: the owner reads the model and effort a start asks for, and the approved start's
/// `claude` receives them.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_choosing_its_model_and_effort_shows_them_and_launches_claude_with_them() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let (new_root, _) = home.project(&owner, "plain").await;
    let coordinator = new_session(&owner, &coordinator_root).await.id;
    grant(&owner, &coordinator, "start-sessions").await;
    let started = home.command_agent(
        &coordinator_root,
        &cockpit_line(&[
            "agent",
            "start",
            &new_root.display().to_string(),
            "--agent",
            "claude",
            "--model",
            "sonnet",
            "--effort",
            "low",
        ]),
    );
    let request = asked_start(&owner, &coordinator).await;
    assert!(
        request
            .command
            .ends_with(" --agent 'claude' --model 'sonnet' --effort 'low'"),
        "{}",
        request.command
    );

    approve(&owner, &request).await;
    let printed = home.agent_output(&coordinator_root, started).await;

    assert!(printed.ends_with("exit=0\n"), "{printed}");
    let session = printed
        .lines()
        .find_map(|line| line.strip_prefix("started session "))
        .unwrap_or_else(|| panic!("{printed}"))
        .to_owned();
    home.wait_for_launches(&session, 1).await;
    let words = home.arguments_of(&session);
    assert!(
        words[0].contains("--effort low") && words[0].contains("--model sonnet"),
        "{words:?}"
    );
}

/// #701: an effort Claude Code does not have is refused before the owner is asked.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_with_an_effort_claude_does_not_have_is_refused_before_the_owner_is_asked() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let (new_root, _) = home.project(&owner, "plain").await;
    let coordinator = new_session(&owner, &coordinator_root).await.id;
    grant(&owner, &coordinator, "start-sessions").await;

    let printed = as_coordinator(
        &home,
        &coordinator_root,
        &[
            "agent",
            "start",
            &new_root.display().to_string(),
            "--agent",
            "claude",
            "--effort",
            "extreme",
        ],
    )
    .await;

    assert!(!printed.ends_with("exit=0\n"), "{printed}");
    assert!(
        printed.contains("effort \"extreme\" is not one of low, medium, high, xhigh, max"),
        "{printed}"
    );
    let (runs, _) = queue(&owner).await;
    assert!(runs.is_empty(), "nothing reaches the owner: {runs:?}");
}

/// #739: gives `coordinator` `start-sessions` with `max_active`, its other terms as given.
async fn grant_at_once(
    owner: &Connection,
    coordinator: &str,
    max_active: u32,
    without_confirmation: bool,
) {
    let granted = owner
        .request(
            Command::SessionGrant {
                session: coordinator.to_owned(),
                permission: "start-sessions".to_owned(),
                without_confirmation,
                per_hour: Some(100),
                max_active: Some(max_active),
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(granted, Ok(Reply::CoordinatorGrants { .. })),
        "{granted:?}"
    );
}

/// The session `printed` says `agent start` started.
fn started_session(printed: &str) -> String {
    printed
        .strip_prefix("started session ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("agent start started no session: {printed}"))
        .to_owned()
}

/// #739: a coordinator at its sessions at once is refused, naming the cap and the count, and
/// may start again as soon as one of the sessions it started ends.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_at_max_active_is_refused_until_a_started_session_ends() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let (new_root, _) = home.project(&owner, "plain").await;
    let coordinator = new_session(&owner, &coordinator_root).await;
    grant_at_once(&owner, &coordinator.id, 1, true).await;
    let start = [
        "agent",
        "start",
        &new_root.display().to_string(),
        "--agent",
        "claude",
    ];
    let first = as_coordinator(&home, &coordinator_root, &start).await;
    assert!(first.ends_with("exit=0\n"), "{first}");

    let refused = as_coordinator(&home, &coordinator_root, &start).await;

    assert!(refused.ends_with("exit=1\n"), "{refused}");
    assert!(
        refused.contains("is at its 1 sessions at once: 1 it started run or wait for the owner"),
        "{refused}"
    );
    let worker = started_session(&first);
    owner
        .request(
            Command::SessionEnd {
                session: worker.clone(),
            },
            GENEROUS,
        )
        .await
        .expect("the owner ends the worker");
    wait_for_state(&owner, &worker, |state| {
        *state == cockpit_protocol::SessionStateView::Ended
    })
    .await;
    let again = as_coordinator(&home, &coordinator_root, &start).await;
    assert!(again.ends_with("exit=0\n"), "{again}");
}

/// #739: two starts asked at once, without confirmation, by a coordinator with one place:
/// the second sees the first before its session exists, and only one session starts.
#[tokio::test(flavor = "multi_thread")]
async fn parallel_unconfirmed_starts_take_one_place_each() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let (new_root, _) = home.project(&owner, "plain").await;
    let coordinator = new_session(&owner, &coordinator_root).await;
    grant_at_once(&owner, &coordinator.id, 1, true).await;
    let start = cockpit_line(&[
        "agent",
        "start",
        &new_root.display().to_string(),
        "--agent",
        "claude",
    ]);

    let line = home.command_agent(
        &coordinator_root,
        &format!(
            "{start} > .agent/first 2>&1 & {start} > .agent/second 2>&1; wait; \
             cat .agent/first .agent/second"
        ),
    );
    let printed = home.agent_output(&coordinator_root, line).await;

    assert_eq!(printed.matches("started session ").count(), 1, "{printed}");
    assert_eq!(
        printed.matches("is at its 1 sessions at once").count(),
        1,
        "{printed}"
    );
}

/// #739: a start waiting for the owner's approval holds one of the sessions at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_waiting_for_the_owner_counts_against_max_active() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let (new_root, _) = home.project(&owner, "plain").await;
    let coordinator = new_session(&owner, &coordinator_root).await;
    grant_at_once(&owner, &coordinator.id, 1, false).await;
    let start = [
        "agent",
        "start",
        &new_root.display().to_string(),
        "--agent",
        "claude",
    ];
    // One line: the agent runs its lines one at a time, and the first start waits for the
    // owner. The second waits for `.agent/go`, which the test drops once the first is queued;
    // both stay children of the agent, which identifies them.
    let first = cockpit_line(&start);
    let line = home.command_agent(
        &coordinator_root,
        &format!(
            "{first} >/dev/null 2>&1 & \
             while [ ! -e .agent/go ]; do sleep 0.1; done; \
             {first}; status=$?; kill $!; exit $status"
        ),
    );
    asked_start(&owner, &coordinator.id).await;
    std::fs::write(coordinator_root.join(".agent").join("go"), "").unwrap();

    let refused = home.agent_output(&coordinator_root, line).await;

    assert!(refused.ends_with("exit=1\n"), "{refused}");
    assert!(
        refused.contains("is at its 1 sessions at once"),
        "{refused}"
    );
}

/// #739: the owner's `torlek session grant --max-active` reaches the grant, and
/// `torlek session grants` shows it with the other terms.
#[tokio::test(flavor = "multi_thread")]
async fn max_active_round_trips_through_session_grant_and_session_grants() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (root, _) = home.project(&owner, "plain").await;
    let session = new_session(&owner, &root).await;

    let granted = home
        .cockpit(
            &[
                "session",
                "grant",
                &session.id,
                "start-sessions",
                "--max-active",
                "3",
            ],
            &[],
        )
        .await;
    let shown = home.cockpit(&["session", "grants", &session.id], &[]).await;

    assert!(
        granted.status.success(),
        "{}",
        String::from_utf8_lossy(&granted.stderr)
    );
    let shown = String::from_utf8_lossy(&shown.stdout);
    assert!(
        shown.contains("\tstart-sessions\t")
            && shown.contains("\tper-hour=20 max-active=3 confirmed\n"),
        "{shown}"
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
async fn a_message_still_queued_when_its_grant_is_revoked_is_never_handed_over() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let coordinator = new_session(&owner, &coordinator_root).await;
    // A second commanded agent (the fake agent acts as its folder's name says), which does
    // not collect its messages until told to.
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
    wait_until("the message waits, not collected", || async {
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
    let collected = home.command_agent(
        &target_root,
        &format!("timeout 5 {}", cockpit_line(&["agent", "inbox"])),
    );
    let printed = home.agent_output(&target_root, collected).await;

    assert_eq!(printed, "exit=124\n", "nothing is handed over");
    let message = listed(&owner, &target.id).await.inbound_message.unwrap();
    assert_eq!(message.delivery, "failed", "{message:?}");
}

/// #656, #703: a message whose target has not reported since its launch is collected once its
/// agent waits at its prompt; until then the sender and the target's card say why it waits,
/// instead of a silent `queued`.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_that_cannot_be_collected_yet_says_why_to_the_sender_and_on_the_card() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let coordinator = new_session(&owner, &coordinator_root).await;
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
    assert!(
        printed.contains(&format!(
            "held: session {}'s agent has not reported through its hooks yet",
            target.id
        )),
        "{printed}"
    );
    wait_until("the card says why the message waits", || async {
        listed(&owner, &target.id)
            .await
            .inbound_message
            .is_some_and(|message| message.delivery == "queued" && message.note.is_some())
    })
    .await;
}

/// #656: when the target's state already rules delivery out, the sender is told why at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_to_an_asleep_session_tells_the_sender_why_it_waits() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let pair = coordinator_and_worker(&home, &owner).await;
    grant(&owner, &pair.coordinator.id, "message-sessions").await;
    let slept = owner
        .request(
            Command::SessionSleep {
                session: pair.worker.id.clone(),
            },
            GENEROUS,
        )
        .await;
    assert!(slept.is_ok(), "{slept:?}");

    let printed = as_coordinator(
        &home,
        &pair.coordinator_root,
        &["agent", "message", &pair.worker.id, "Base it on main."],
    )
    .await;

    assert!(printed.ends_with("exit=0\n"), "{printed}");
    assert!(
        printed.contains(&format!("held: session {} is asleep", pair.worker.id)),
        "{printed}"
    );
}

/// A coordinator and a second commanded agent it holds `observe-sessions` and
/// `message-sessions` over, which collects its messages only when told to.
struct CommandedPair {
    home: Home,
    _daemon: Daemon,
    owner: Connection,
    coordinator_root: PathBuf,
    target: SessionView,
    target_root: PathBuf,
}

async fn commanded_pair() -> CommandedPair {
    assert_the_cli_is_built();
    let home = Home::new();
    let daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let coordinator = new_session(&owner, &coordinator_root).await;
    let target_root = coordinator_root.join("target").join(COMMANDED_PROJECT);
    std::fs::create_dir_all(&target_root).unwrap();
    let target = new_session(&owner, &target_root).await;
    for permission in ["observe-sessions", "message-sessions"] {
        grant(&owner, &coordinator.id, permission).await;
    }
    CommandedPair {
        home,
        _daemon: daemon,
        owner,
        coordinator_root,
        target,
        target_root,
    }
}

impl CommandedPair {
    async fn as_target(&self, arguments: &[&str]) -> String {
        as_coordinator(&self.home, &self.target_root, arguments).await
    }

    async fn as_coordinator(&self, arguments: &[&str]) -> String {
        as_coordinator(&self.home, &self.coordinator_root, arguments).await
    }

    /// The coordinator's `agent sessions` line about the target.
    async fn observed_target(&self) -> String {
        let observed = self.as_coordinator(&["agent", "sessions"]).await;
        observed
            .lines()
            .find(|line| line.starts_with(&self.target.id))
            .unwrap_or_else(|| panic!("the target is listed: {observed}"))
            .to_owned()
    }
}

/// #703: a message to an agent at work is held, not typed into its prompt; its hook hands it
/// over once the turn ends, and the sender sees each step from `agent sessions`.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_to_a_working_agent_is_held_then_handed_over_when_its_turn_ends() {
    let pair = commanded_pair().await;
    let working = pair
        .as_target(&["agent", "report", "--state", "working"])
        .await;
    assert!(working.ends_with("exit=0\n"), "{working}");

    let sent = pair
        .as_coordinator(&["agent", "message", &pair.target.id, "Base it on main."])
        .await;
    assert!(sent.ends_with("exit=0\n"), "{sent}");
    assert!(
        sent.contains(&format!(
            "held: session {}'s agent is working; its hook hands it over when the turn ends",
            pair.target.id
        )),
        "{sent}"
    );
    let held = pair.home.command_agent(
        &pair.target_root,
        &format!("timeout 5 {}", cockpit_line(&["agent", "inbox"])),
    );
    assert_eq!(
        pair.home.agent_output(&pair.target_root, held).await,
        "exit=124\n",
        "nothing is handed over during the turn"
    );
    let observed = pair.observed_target().await;
    assert!(observed.contains("\tmessage=queued"), "{observed}");
    assert!(
        observed.contains("message-note=\"held until session"),
        "{observed}"
    );

    let waiting = pair
        .as_target(&["agent", "report", "--state", "waiting"])
        .await;
    assert!(waiting.ends_with("exit=0\n"), "{waiting}");
    let collected = pair.as_target(&["agent", "inbox"]).await;

    assert!(
        collected.ends_with(" (a collaborator, not the owner): Base it on main.\nexit=0\n"),
        "{collected}"
    );
    let observed = pair.observed_target().await;
    assert!(observed.contains("\tmessage=received"), "{observed}");
    assert!(!observed.contains("message-note="), "{observed}");
    let card = listed(&pair.owner, &pair.target.id).await;
    assert_eq!(
        card.inbound_message.map(|message| message.delivery),
        Some("received".to_owned())
    );
}

/// #703: each turn's end starts a waiter; the newer one takes over, so the older stops
/// instead of piling up, and only the newer hands the message over.
#[tokio::test(flavor = "multi_thread")]
async fn a_newer_inbox_waiter_supersedes_the_older_one() {
    let pair = commanded_pair().await;
    let inbox = cockpit_line(&["agent", "inbox"]);
    let waiters = pair.home.command_agent(
        &pair.target_root,
        &format!(
            "{inbox} > older.out & older=$!; sleep 1; {inbox} > newer.out & newer=$!; \
             wait $older; echo older=$?; wait $newer; echo newer=$?; cat older.out newer.out"
        ),
    );
    // The older waiter asks again within two seconds and learns it was superseded.
    tokio::time::sleep(Duration::from_secs(4)).await;

    let sent = pair
        .as_coordinator(&["agent", "message", &pair.target.id, "Base it on main."])
        .await;
    assert!(sent.ends_with("exit=0\n"), "{sent}");
    let printed = pair.home.agent_output(&pair.target_root, waiters).await;

    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines[..2], ["older=0", "newer=0"], "{printed}");
    assert_eq!(lines.len(), 4, "only the newer waiter printed: {printed}");
    assert!(
        lines[2].ends_with("(a collaborator, not the owner): Base it on main."),
        "{printed}"
    );
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
                max_active: None,
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
                max_active: None,
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

/// #703: `agent sessions` shows a coordinator its own messages only; what another coordinator
/// wrote to a session is not its to read.
#[tokio::test(flavor = "multi_thread")]
async fn a_coordinator_never_sees_the_message_another_coordinator_sent() {
    let pair = commanded_pair().await;
    let working = pair
        .as_target(&["agent", "report", "--state", "working"])
        .await;
    assert!(working.ends_with("exit=0\n"), "{working}");
    let sent = pair
        .as_coordinator(&["agent", "message", &pair.target.id, "Base it on main."])
        .await;
    assert!(sent.ends_with("exit=0\n"), "{sent}");
    let other_root = pair.coordinator_root.join("other").join(COMMANDED_PROJECT);
    std::fs::create_dir_all(&other_root).unwrap();
    let other = new_session(&pair.owner, &other_root).await;
    grant(&pair.owner, &other.id, "observe-sessions").await;

    let observed = as_coordinator(&pair.home, &other_root, &["agent", "sessions"]).await;

    let target = observed
        .lines()
        .find(|line| line.starts_with(&pair.target.id))
        .unwrap_or_else(|| panic!("the target is listed: {observed}"));
    assert!(!target.contains("message-from="), "{observed}");
    assert!(pair.observed_target().await.contains("message-from="));
}
