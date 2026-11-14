//! Issue #1131: a coordinator grant covers the coordinator's own project unless the owner
//! names every project. Driven through the real daemon, the real `torlek` run by a launched
//! coordinator agent on the agent socket, and the owner's socket: a coordinator granted for
//! project alpha lists and messages alpha's sessions only, and each verb aimed at project
//! beta is refused naming its grant's scope, never beta; `--all-projects` gives the behaviour before #1131.

mod candidate;
mod real_daemon;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    Command, DecidedThroughView, ErrorCode, Reply, RunRequestView, RunStateView,
};
use real_daemon::{
    COMMANDED_PROJECT, Daemon, GENEROUS, Home, QUESTION_PROJECT, cockpit_program, new_session,
};

/// A well-formed session id no session has.
const UNKNOWN_SESSION: &str = "s0000000000ff";

const COORDINATOR_GRANTS: [&str; 4] = [
    "observe-sessions",
    "message-sessions",
    "start-sessions",
    "end-sessions",
];

fn assert_the_cli_is_built() {
    let cockpit = cockpit_program();
    assert!(
        cockpit.is_file(),
        "{} is missing: run the workspace's tests, which build it",
        cockpit.display()
    );
}

/// `torlek ARGUMENTS` as the commanded agent in `root` runs it, single-quoted.
fn cockpit_line(arguments: &[&str]) -> String {
    std::iter::once(cockpit_program().display().to_string())
        .chain(arguments.iter().map(|argument| (*argument).to_owned()))
        .map(|word| format!("'{}'", word.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A coordinator in project alpha holding the four coordinator grants, a worker beside it in
/// alpha, and a worker in project beta.
struct Scoped {
    home: Home,
    _daemon: Daemon,
    owner: Connection,
    coordinator: String,
    coordinator_root: PathBuf,
    alpha_root: PathBuf,
    alpha_worker: String,
    beta_root: PathBuf,
    beta_worker: String,
}

impl Scoped {
    async fn new(all_projects: bool) -> Scoped {
        assert_the_cli_is_built();
        let home = Home::new();
        let daemon = home.start();
        let owner = home.connect().await;
        let (coordinator_root, _) = home
            .project_at(&owner, &format!("alpha/{COMMANDED_PROJECT}"), Some("alpha"))
            .await;
        let alpha_worker_dir = coordinator_root.join(QUESTION_PROJECT);
        std::fs::create_dir_all(&alpha_worker_dir).unwrap();
        let (beta_root, _) = home
            .project_at(&owner, &format!("beta/{QUESTION_PROJECT}"), Some("beta"))
            .await;
        let coordinator = new_session(&owner, &coordinator_root).await.id;
        let alpha_worker = new_session(&owner, &alpha_worker_dir).await.id;
        let beta_worker = new_session(&owner, &beta_root).await.id;
        for permission in COORDINATOR_GRANTS {
            grant(&owner, &coordinator, permission, all_projects).await;
        }
        Scoped {
            home,
            _daemon: daemon,
            owner,
            coordinator,
            alpha_root: coordinator_root.clone(),
            coordinator_root,
            alpha_worker,
            beta_root,
            beta_worker,
        }
    }

    /// What the coordinator's `torlek ARGUMENTS` printed, ending `exit=<code>`.
    async fn run(&self, arguments: &[&str]) -> String {
        let number = self
            .home
            .command_agent(&self.coordinator_root, &cockpit_line(arguments));
        self.home.agent_output(&self.coordinator_root, number).await
    }
}

async fn grant(owner: &Connection, session: &str, permission: &str, all_projects: bool) {
    let answer = owner
        .request(
            Command::SessionGrant {
                session: session.to_owned(),
                permission: permission.to_owned(),
                all_projects,
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

#[tokio::test(flavor = "multi_thread")]
async fn a_project_coordinator_lists_and_messages_only_its_projects_sessions() {
    let scoped = Scoped::new(false).await;

    let observed = scoped.run(&["agent", "sessions"]).await;
    let messaged = scoped
        .run(&["agent", "message", &scoped.alpha_worker, "Rebase on main."])
        .await;

    assert!(observed.ends_with("exit=0\n"), "{observed}");
    assert!(observed.contains(&scoped.alpha_worker), "{observed}");
    assert!(
        !observed.contains(&scoped.beta_worker),
        "another project's session is not listed: {observed}"
    );
    assert!(messaged.ends_with("exit=0\n"), "{messaged}");
}

#[tokio::test(flavor = "multi_thread")]
async fn each_verb_aimed_at_another_project_is_refused_naming_the_grants_scope() {
    let scoped = Scoped::new(false).await;
    let beta_dir = scoped.beta_root.display().to_string();

    for (arguments, permission) in [
        (
            vec!["agent", "message", scoped.beta_worker.as_str(), "hello"],
            "message-sessions",
        ),
        (
            vec!["agent", "start", beta_dir.as_str(), "--agent", "claude"],
            "start-sessions",
        ),
        // An unknown session reads as one of another project: probing ids tells nothing.
        (
            vec!["agent", "message", UNKNOWN_SESSION, "hello"],
            "message-sessions",
        ),
    ] {
        let printed = scoped.run(&arguments).await;

        assert!(printed.ends_with("exit=1\n"), "{arguments:?}: {printed}");
        assert!(
            printed.contains(&format!("{permission} grant covers project alpha only")),
            "{arguments:?}: {printed}"
        );
        assert!(
            printed.contains("--all-projects"),
            "{arguments:?}: {printed}"
        );
    }
}

/// The end is checked against the project of a session the coordinator started: one it
/// started in beta while its start grant covered every project.
#[tokio::test(flavor = "multi_thread")]
async fn an_end_in_another_project_is_refused_naming_the_grants_scope() {
    let scoped = Scoped::new(false).await;
    let regranted = scoped
        .owner
        .request(
            Command::SessionGrant {
                session: scoped.coordinator.clone(),
                permission: "start-sessions".to_owned(),
                all_projects: true,
                without_confirmation: true,
                per_hour: Some(5),
                max_active: None,
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(regranted, Ok(Reply::CoordinatorGrants { .. })),
        "{regranted:?}"
    );
    let beta_dir = scoped.beta_root.display().to_string();
    let started = scoped
        .run(&["agent", "start", &beta_dir, "--agent", "claude"])
        .await;
    let worker = started
        .lines()
        .find_map(|line| line.strip_prefix("started session "))
        .unwrap_or_else(|| panic!("the start names its session: {started}"))
        .to_owned();

    let printed = scoped.run(&["agent", "end", &worker]).await;

    assert!(printed.ends_with("exit=1\n"), "{printed}");
    assert!(
        printed.contains("end-sessions grant covers project alpha only"),
        "{printed}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_without_a_scope_flag_covers_the_sessions_own_project() {
    let scoped = Scoped::new(false).await;

    let shown = scoped
        .owner
        .request(
            Command::SessionGrants {
                session: scoped.coordinator.clone(),
            },
            GENEROUS,
        )
        .await;
    let brief = scoped.run(&["brief"]).await;

    let Ok(Reply::CoordinatorGrants { grants, .. }) = shown else {
        panic!("session-grants answered {shown:?}");
    };
    let scopes: Vec<(&str, &str)> = grants
        .iter()
        .map(|grant| (grant.permission.as_str(), grant.scope.as_str()))
        .collect();
    assert_eq!(
        scopes,
        COORDINATOR_GRANTS.map(|permission| (permission, "project alpha"))
    );
    assert!(
        brief.contains("observe-sessions over project alpha"),
        "{brief}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_over_all_projects_lists_and_messages_every_projects_sessions() {
    let scoped = Scoped::new(true).await;

    let observed = scoped.run(&["agent", "sessions"]).await;
    let messaged = scoped
        .run(&["agent", "message", &scoped.beta_worker, "Rebase on main."])
        .await;

    assert!(observed.ends_with("exit=0\n"), "{observed}");
    assert!(observed.contains(&scoped.alpha_worker), "{observed}");
    assert!(observed.contains(&scoped.beta_worker), "{observed}");
    assert!(messaged.ends_with("exit=0\n"), "{messaged}");
}

/// The owner's approval re-checks the very grant a start was asked under, which covers the
/// start's project, not everything.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_in_its_own_project_begins_once_the_owner_approves_it() {
    let scoped = Scoped::new(false).await;
    let new_dir = scoped.alpha_root.join("plain");
    std::fs::create_dir_all(&new_dir).unwrap();

    let started = scoped.home.command_agent(
        &scoped.coordinator_root,
        &cockpit_line(&[
            "agent",
            "start",
            &new_dir.display().to_string(),
            "--agent",
            "claude",
        ]),
    );
    let request = asked_start(&scoped.owner, &scoped.coordinator).await;
    approve(&scoped.owner, &request).await;
    let printed = scoped
        .home
        .agent_output(&scoped.coordinator_root, started)
        .await;

    assert!(printed.ends_with("exit=0\n"), "{printed}");
    assert!(printed.contains("started session "), "{printed}");
}

/// The start request `coordinator` filed, once the owner's queue holds it.
async fn asked_start(owner: &Connection, coordinator: &str) -> RunRequestView {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let runs = match owner.request(Command::ApprovalQueue, GENEROUS).await {
            Ok(Reply::ApprovalQueue { runs, .. }) => runs,
            other => panic!("approval-queue answered {other:?}"),
        };
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
                    always: false,
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
