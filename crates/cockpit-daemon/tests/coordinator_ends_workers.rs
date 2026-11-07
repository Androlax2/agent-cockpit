//! Issue #713: a coordinator holding `end-sessions` ends a session it started once the task
//! list says its work is done, and nothing else. Driven through the real daemon, the real
//! `cockpit` run by a launched coordinator agent on the agent socket, and the owner's socket:
//! the coordinator starts a worker that files, starts and finishes an item; with the grant it
//! ends that worker, audited with both sessions and the item; it is refused for a worker
//! still working, for a session it did not start, and without the grant; and the owner's
//! lifecycle verbs stay refused on the agent socket.

mod candidate;
mod real_daemon;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_protocol::{Command, CoordinatorAuditView, Reply, SessionStateView};
use real_daemon::{
    COMMANDED_PROJECT, GENEROUS, Home, cockpit_program, listed, new_session, wait_for_running,
};

/// The second commanded agent's project: the same fake agent, under another name.
const WORKER_PROJECT: &str = "worker";

fn assert_the_cli_is_built() {
    let cockpit = cockpit_program();
    assert!(
        cockpit.is_file(),
        "{} is missing: run the workspace's tests, which build it",
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

/// What the commanded agent in `root` printed for `cockpit ARGUMENTS`, ending `exit=<code>`.
async fn as_agent(home: &Home, root: &Path, arguments: &[&str]) -> String {
    let number = home.command_agent(root, &cockpit_line(arguments));
    home.agent_output(root, number).await
}

async fn owner_request(owner: &Connection, command: Command) -> Reply {
    owner
        .request(command.clone(), GENEROUS)
        .await
        .unwrap_or_else(|error| panic!("{command:?} answered {error:?}"))
}

async fn grant(owner: &Connection, session: &str, permission: &str) {
    let answer = owner_request(
        owner,
        Command::SessionGrant {
            session: session.to_owned(),
            permission: permission.to_owned(),
            all_projects: true,
            without_confirmation: false,
            per_hour: None,
            max_active: None,
        },
    )
    .await;
    assert!(
        matches!(answer, Reply::CoordinatorGrants { .. }),
        "session-grant {permission}: {answer:?}"
    );
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

/// A coordinator holding `observe-sessions` and unconfirmed `start-sessions`, and the
/// commanded worker it started in the worker project. `end-sessions` is each test's to give.
struct Started {
    home: Home,
    _daemon: real_daemon::Daemon,
    owner: Connection,
    coordinator: String,
    coordinator_root: PathBuf,
    worker: String,
    worker_root: PathBuf,
}

async fn a_coordinator_starts_a_worker() -> Started {
    assert_the_cli_is_built();
    let home = Home::new();
    let daemon = home.start();
    let owner = home.connect().await;
    let (coordinator_root, _) = home.project(&owner, COMMANDED_PROJECT).await;
    let (worker_root, _) = home
        .project_at(
            &owner,
            &format!("{WORKER_PROJECT}/{COMMANDED_PROJECT}"),
            Some(WORKER_PROJECT),
        )
        .await;
    let coordinator = new_session(&owner, &coordinator_root).await.id;
    grant(&owner, &coordinator, "observe-sessions").await;
    owner_request(
        &owner,
        Command::SessionGrant {
            session: coordinator.clone(),
            permission: "start-sessions".to_owned(),
            all_projects: true,
            without_confirmation: true,
            per_hour: Some(5),
            max_active: None,
        },
    )
    .await;
    let started = as_agent(
        &home,
        &coordinator_root,
        &[
            "agent",
            "start",
            &worker_root.display().to_string(),
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
    wait_for_running(&owner, &worker).await;
    Started {
        home,
        _daemon: daemon,
        owner,
        coordinator,
        coordinator_root,
        worker,
        worker_root,
    }
}

impl Started {
    /// The worker files an item, starts it and marks it done, so the task list labels it
    /// finished (#157); returns the item's id.
    async fn worker_finishes_an_item(&self) -> String {
        let added = as_agent(
            &self.home,
            &self.worker_root,
            &["todo", "add", "Fix the thing", "--kind", "chore"],
        )
        .await;
        assert!(added.ends_with("exit=0\n"), "{added}");
        let item = added
            .lines()
            .find_map(|line| line.strip_prefix("item "))
            .and_then(|rest| rest.split(' ').next())
            .unwrap_or_else(|| panic!("the add names its item: {added}"))
            .to_owned();
        for verb in ["start", "done"] {
            let printed = as_agent(&self.home, &self.worker_root, &["todo", verb, &item]).await;
            assert!(printed.ends_with("exit=0\n"), "todo {verb}: {printed}");
        }
        wait_until("the worker is labelled finished", || async {
            listed(&self.owner, &self.worker).await.finished
        })
        .await;
        item
    }

    async fn audit_of(&self, session: &str) -> Vec<CoordinatorAuditView> {
        match owner_request(
            &self.owner,
            Command::SessionGrants {
                session: session.to_owned(),
            },
        )
        .await
        {
            Reply::CoordinatorGrants { audit, .. } => audit,
            other => panic!("session-grants answered {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_coordinator_ends_the_finished_worker_it_started_and_the_audit_names_the_item() {
    let started = a_coordinator_starts_a_worker().await;
    let item = started.worker_finishes_an_item().await;
    grant(&started.owner, &started.coordinator, "end-sessions").await;

    let ended = as_agent(
        &started.home,
        &started.coordinator_root,
        &["agent", "end", &started.worker],
    )
    .await;

    assert!(ended.ends_with("exit=0\n"), "{ended}");
    assert!(
        ended.contains(&format!("ended session {}", started.worker)),
        "{ended}"
    );
    let worker = listed(&started.owner, &started.worker).await;
    assert_eq!(worker.state, SessionStateView::Ended, "{worker:?}");
    assert_eq!(
        worker.reason.as_deref(),
        Some(format!("ended by session {}", started.coordinator).as_str()),
        "{worker:?}"
    );
    let audit = started.audit_of(&started.coordinator).await;
    let entry = audit
        .iter()
        .find(|entry| entry.action == "ended")
        .unwrap_or_else(|| panic!("the end is audited: {audit:?}"));
    assert_eq!(entry.actor, started.coordinator);
    assert_eq!(entry.permission, "end-sessions");
    assert_eq!(entry.target.as_deref(), Some(started.worker.as_str()));
    assert_eq!(
        entry.detail.as_deref(),
        Some(format!("item {item}").as_str())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_whose_item_is_not_done_is_not_ended() {
    let started = a_coordinator_starts_a_worker().await;
    grant(&started.owner, &started.coordinator, "end-sessions").await;

    let refused = as_agent(
        &started.home,
        &started.coordinator_root,
        &["agent", "end", &started.worker],
    )
    .await;

    assert!(refused.ends_with("exit=1\n"), "{refused}");
    assert!(
        refused.contains(&format!(
            "session {} has not finished its work: the task list records no item it finished since its launch",
            started.worker
        )),
        "{refused}"
    );
    assert_eq!(
        listed(&started.owner, &started.worker).await.state,
        SessionStateView::Running
    );
    assert!(
        started
            .audit_of(&started.coordinator)
            .await
            .iter()
            .all(|entry| entry.action != "ended"),
        "a refused end is not audited as one"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_coordinator_cannot_end_a_finished_session_it_did_not_start() {
    let started = a_coordinator_starts_a_worker().await;
    grant(&started.owner, &started.coordinator, "end-sessions").await;
    let (plain_root, _) = started.home.project(&started.owner, "plain").await;
    let owners_session = new_session(&started.owner, &plain_root).await.id;

    let refused = as_agent(
        &started.home,
        &started.coordinator_root,
        &["agent", "end", &owners_session],
    )
    .await;

    assert!(refused.ends_with("exit=1\n"), "{refused}");
    assert!(
        refused.contains(&format!(
            "session {} did not start session {owners_session}",
            started.coordinator
        )),
        "{refused}"
    );
    assert_ne!(
        listed(&started.owner, &owners_session).await.state,
        SessionStateView::Ended
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_end_sessions_the_end_is_refused_naming_the_grant() {
    let started = a_coordinator_starts_a_worker().await;
    started.worker_finishes_an_item().await;

    let refused = as_agent(
        &started.home,
        &started.coordinator_root,
        &["agent", "end", &started.worker],
    )
    .await;

    assert!(refused.ends_with("exit=1\n"), "{refused}");
    assert!(refused.contains("holds no end-sessions grant"), "{refused}");
    assert!(
        refused.contains(&format!(
            "torlek session grant {} end-sessions",
            started.coordinator
        )),
        "{refused}"
    );
    assert_eq!(
        listed(&started.owner, &started.worker).await.state,
        SessionStateView::Running
    );
}

/// The owner's lifecycle verbs reach the daemon on the agent socket from a session (#729),
/// where they stay the owner's alone, `end-sessions` or not.
#[tokio::test(flavor = "multi_thread")]
async fn the_owners_lifecycle_verbs_stay_refused_on_the_agent_socket() {
    let started = a_coordinator_starts_a_worker().await;
    started.worker_finishes_an_item().await;
    grant(&started.owner, &started.coordinator, "end-sessions").await;

    for verb in ["end", "restart", "sleep", "wake", "resume"] {
        let refused = as_agent(
            &started.home,
            &started.coordinator_root,
            &["session", verb, &started.worker],
        )
        .await;

        assert!(refused.ends_with("exit=1\n"), "session {verb}: {refused}");
        assert!(
            refused.contains("the owner's alone"),
            "session {verb}: {refused}"
        );
    }
    let refused = as_agent(
        &started.home,
        &started.coordinator_root,
        &["session", "end", "--finished"],
    )
    .await;
    assert!(refused.ends_with("exit=1\n"), "{refused}");
    assert!(refused.contains("the owner's alone"), "{refused}");
    assert_eq!(
        listed(&started.owner, &started.worker).await.state,
        SessionStateView::Running
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn end_sessions_takes_no_start_terms_and_is_revoked_when_the_coordinator_ends() {
    let started = a_coordinator_starts_a_worker().await;

    let with_terms = started
        .owner
        .request(
            Command::SessionGrant {
                session: started.coordinator.clone(),
                permission: "end-sessions".to_owned(),
                all_projects: true,
                without_confirmation: false,
                per_hour: Some(3),
                max_active: None,
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(
            &with_terms,
            Err(cockpit_client::ClientError::Daemon(error))
                if error.code == cockpit_protocol::ErrorCode::InvalidArgument
        ),
        "{with_terms:?}"
    );

    grant(&started.owner, &started.coordinator, "end-sessions").await;
    assert!(
        listed(&started.owner, &started.coordinator)
            .await
            .coordinator_grants
            .contains(&"end-sessions".to_owned())
    );
    owner_request(
        &started.owner,
        Command::SessionEnd {
            session: started.coordinator.clone(),
        },
    )
    .await;

    let grants = match owner_request(
        &started.owner,
        Command::SessionGrants {
            session: started.coordinator.clone(),
        },
    )
    .await
    {
        Reply::CoordinatorGrants { grants, .. } => grants,
        other => panic!("session-grants answered {other:?}"),
    };
    assert!(grants.is_empty(), "{grants:?}");
}
