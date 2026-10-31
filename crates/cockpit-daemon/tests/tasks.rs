//! Issue #38 through the daemon binary's sockets: what an agent may file and work on, what
//! stays the owner's, and the owner's `[tasks]` settings applied. This test process plays the
//! agent; its working directory says which registered project it works in, as a launched
//! session's does.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};

use cockpit_agent_channel::{LaunchToken, identity_of};
use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    Command, ErrorCode, HexBytes, ProtocolError, Reply, TaskDraftTargetView, TaskStatusView,
    TaskView, UnblockedBy,
};
use common::{GENEROUS, TestHome};

/// The working directory is the process's: tests that move it take turns. Under nextest each
/// test is its own process; the lock keeps `cargo test`'s threads apart too.
static WORKING_DIRECTORY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Fields drop in order: the daemon stops before its home is deleted.
struct Tasks {
    _daemon: common::RunningDaemon,
    home: TestHome,
    token: LaunchToken,
    agent: Connection,
    owner: Connection,
    _cwd: tokio::sync::MutexGuard<'static, ()>,
}

/// A daemon whose owner registered `projects` and wrote `config` as the owner config, with
/// session `s1` launched from this process and working in the first project.
async fn started(projects: &[&str], config: &str) -> Tasks {
    let cwd = WORKING_DIRECTORY.lock().await;
    let home = TestHome::new();
    let config_dir = home.home().join("config/cockpit");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), config).unwrap();
    let database = home.open_state().await;
    for project in projects {
        let root = repository(home.home(), project);
        cockpit_projects::register_project(
            &database,
            &cockpit_projects::Git::from_path(),
            &root,
            None,
        )
        .await
        .unwrap();
    }
    let token = launch(&database).await;
    drop(database);
    if let Some(first) = projects.first() {
        std::env::set_current_dir(home.home().join("projects").join(first)).unwrap();
    }
    let daemon = home.start();
    let agent = home.connect_agent().await;
    let owner = home.connect().await;
    Tasks {
        _daemon: daemon,
        home,
        token,
        agent,
        owner,
        _cwd: cwd,
    }
}

fn repository(home: &Path, name: &str) -> PathBuf {
    let root = home.join("projects").join(name);
    std::fs::create_dir_all(&root).unwrap();
    let initialised = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(initialised.success());
    root.canonicalize().unwrap()
}

/// Launches session `s1` rooted at this test process, holding change-state over itself as a
/// launched session does.
async fn launch(database: &cockpit_turso::Database) -> LaunchToken {
    let session = SessionId::new("s1").unwrap();
    let write = database.write().await.unwrap();
    let token = cockpit_agent_channel::mint(&write, &session).await.unwrap();
    cockpit_agent_channel::record_root(&write, &session, &identity_of(std::process::id()).unwrap())
        .await
        .unwrap();
    cockpit_authorization::grant(
        &write,
        &session,
        Permission::ChangeState,
        &Scope::Session(session.clone()),
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    token
}

impl Tasks {
    fn token(&self) -> Option<String> {
        Some(self.token.as_str().to_owned())
    }

    async fn as_agent(&self, command: Command) -> Result<Reply, ClientError> {
        self.agent.request(command, GENEROUS).await
    }

    async fn as_owner(&self, command: Command) -> Result<Reply, ClientError> {
        self.owner.request(command, GENEROUS).await
    }

    async fn agent_adds(&self, project: Option<&str>, title: &str) -> Result<Reply, ClientError> {
        self.as_agent(Command::TaskAdd {
            launch_token: self.token(),
            project: project.map(str::to_owned),
            title: title.to_owned(),
            notes: String::new(),
            item_kind: None,
            priority: None,
            after: Vec::new(),
        })
        .await
    }

    async fn owner_adds(&self, project: &str, title: &str, after: Vec<i64>) -> TaskView {
        task(
            self.as_owner(Command::TaskAdd {
                launch_token: None,
                project: Some(project.to_owned()),
                title: title.to_owned(),
                notes: String::new(),
                item_kind: None,
                priority: None,
                after,
            })
            .await,
        )
    }

    async fn reported(&self, title: &str) -> Result<Reply, ClientError> {
        self.as_agent(Command::TaskReportBlocker {
            launch_token: self.token(),
            title: title.to_owned(),
            why: "greywall hides the socket".to_owned(),
            notes: String::new(),
        })
        .await
    }
}

fn task(reply: Result<Reply, ClientError>) -> TaskView {
    match reply {
        Ok(Reply::Task { task }) => *task,
        other => panic!("expected a task, got {other:?}"),
    }
}

fn refused(reply: Result<Reply, ClientError>) -> ProtocolError {
    match reply {
        Err(ClientError::Daemon(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn an_agent_proposes_in_its_own_project_and_the_owner_approves_it() {
    let tasks = started(&["app"], "").await;

    let proposed = task(
        tasks
            .as_agent(Command::TaskPropose {
                launch_token: tasks.token(),
                project: None,
                title: "Cache the build".to_owned(),
                why: "every run rebuilds".to_owned(),
                notes: String::new(),
                item_kind: None,
            })
            .await,
    );
    let approved = task(
        tasks
            .as_owner(Command::TaskApprove { item: proposed.id })
            .await,
    );

    assert_eq!(
        (
            proposed.project.as_str(),
            proposed.status,
            proposed.filed_by.as_deref(),
            proposed.filed_from.as_deref(),
        ),
        ("app", TaskStatusView::Proposed, Some("s1"), Some("app"))
    );
    assert_eq!(approved.status, TaskStatusView::Todo);
}

#[tokio::test]
async fn an_agent_filing_in_another_project_is_queued_until_the_owners_cap() {
    let tasks = started(&["app", "web"], "[tasks]\ncross_project_open_max = 1\n").await;

    let handed = task(tasks.agent_adds(Some("web"), "Expose the port").await);
    let over_cap = refused(tasks.agent_adds(Some("web"), "Expose another port").await);

    assert_eq!((handed.project.as_str(), handed.is_queued), ("web", true));
    assert_eq!(
        (over_cap.code, over_cap.unblock.map(|unblock| unblock.by)),
        (ErrorCode::Conflict, Some(UnblockedBy::Owner))
    );
}

#[tokio::test]
async fn a_blocker_report_goes_queued_to_the_project_the_owner_configured() {
    let tasks = started(
        &["app", "dotfiles"],
        "[tasks]\nblocker_project = \"dotfiles\"\n",
    )
    .await;

    let report = task(tasks.reported("Reach the docker socket").await);

    assert_eq!(
        (
            report.project.as_str(),
            report.is_blocker_report,
            report.is_queued,
            report.why.as_deref(),
        ),
        ("dotfiles", true, true, Some("greywall hides the socket"))
    );
}

#[tokio::test]
async fn a_blocker_report_without_a_configured_project_is_refused_for_the_owner_to_fix() {
    let tasks = started(&["app"], "").await;

    let refusal = refused(tasks.reported("Reach the docker socket").await);

    assert_eq!(
        (refusal.code, refusal.unblock.map(|unblock| unblock.by)),
        (ErrorCode::Refused, Some(UnblockedBy::Owner))
    );
}

#[tokio::test]
async fn a_blocker_project_that_is_not_registered_is_refused() {
    let tasks = started(&["app"], "[tasks]\nblocker_project = \"gone\"\n").await;

    let refusal = refused(tasks.reported("Reach the docker socket").await);

    assert_eq!(refusal.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_broken_owner_config_refuses_a_filing_instead_of_using_defaults() {
    let tasks = started(&["app"], "[tasks]\ncross_project_open_max = -1\n").await;

    let refusal = refused(tasks.agent_adds(None, "Cache the build").await);

    assert_eq!(
        (refusal.code, refusal.unblock.map(|unblock| unblock.by)),
        (ErrorCode::Refused, Some(UnblockedBy::Owner))
    );
}

#[tokio::test]
async fn the_owner_cannot_file_in_a_project_that_is_not_registered() {
    let tasks = started(&["app"], "").await;

    let refusal = refused(
        tasks
            .as_owner(Command::TaskAdd {
                launch_token: None,
                project: Some("nope".to_owned()),
                title: "Anything".to_owned(),
                notes: String::new(),
                item_kind: None,
                priority: None,
                after: Vec::new(),
            })
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn an_agent_finishes_an_item_and_the_item_waiting_on_it_shows_its_summary() {
    let tasks = started(&["app"], "").await;
    let first = tasks
        .owner_adds("app", "Migrate the schema", Vec::new())
        .await;
    let second = tasks
        .owner_adds("app", "Backfill the column", vec![first.id])
        .await;

    let early = refused(
        tasks
            .as_agent(Command::TaskStart {
                launch_token: tasks.token(),
                item: second.id,
            })
            .await,
    );
    task(
        tasks
            .as_agent(Command::TaskStart {
                launch_token: tasks.token(),
                item: first.id,
            })
            .await,
    );
    let done = task(
        tasks
            .as_agent(Command::TaskDone {
                launch_token: tasks.token(),
                item: first.id,
                summary: Some("column added in 0042".to_owned()),
            })
            .await,
    );
    let waiting = task(
        tasks
            .as_agent(Command::TaskShow {
                launch_token: tasks.token(),
                item: second.id,
            })
            .await,
    );

    assert_eq!(early.code, ErrorCode::Conflict);
    assert_eq!(done.status, TaskStatusView::Done);
    assert_eq!(
        waiting.waits[0].summary.as_deref(),
        Some("column added in 0042")
    );
}

#[tokio::test]
async fn the_agent_socket_refuses_the_owners_task_commands() {
    let tasks = started(&["app"], "").await;
    let owner_commands = [
        Command::TaskEdit {
            item: 1,
            intent_revision: None,
            title: Some("x".to_owned()),
            notes: None,
            item_kind: None,
        },
        Command::TaskPriority {
            item: 1,
            priority: 3,
        },
        Command::TaskQueue {
            item: 1,
            queued: true,
        },
        Command::TaskApprove { item: 1 },
        Command::TaskDecline { item: 1 },
        Command::TaskReopen { item: 1 },
        Command::TaskWait {
            item: 1,
            on: vec![2],
        },
        Command::TaskUnwait {
            item: 1,
            on: vec![2],
        },
        Command::TaskImageAdd {
            item: 1,
            data: HexBytes(vec![0x89]),
        },
        Command::TaskImage { item: 1, number: 1 },
        Command::TaskDraftSave {
            target: TaskDraftTargetView::Item { item: 1 },
            title: String::new(),
            notes: String::new(),
            base_intent_revision: None,
        },
        Command::TaskDrafts,
        Command::TaskDraftDiscard {
            target: TaskDraftTargetView::Item { item: 1 },
        },
    ];

    for command in owner_commands {
        let refusal = refused(tasks.as_agent(command.clone()).await);
        assert_eq!(
            (refusal.code, refusal.unblock.map(|unblock| unblock.by)),
            (ErrorCode::NotAuthorized, Some(UnblockedBy::Owner)),
            "{command:?}"
        );
    }
}

#[tokio::test]
async fn the_owner_socket_refuses_proposals_and_blocker_reports() {
    let tasks = started(&["app"], "").await;
    let session_commands = [
        Command::TaskPropose {
            launch_token: None,
            project: Some("app".to_owned()),
            title: "x".to_owned(),
            why: "y".to_owned(),
            notes: String::new(),
            item_kind: None,
        },
        Command::TaskReportBlocker {
            launch_token: None,
            title: "x".to_owned(),
            why: "y".to_owned(),
            notes: String::new(),
        },
    ];

    for command in session_commands {
        let refusal = refused(tasks.as_owner(command.clone()).await);
        assert_eq!(refusal.code, ErrorCode::NotAuthorized, "{command:?}");
    }
}

#[tokio::test]
async fn an_agent_outside_every_registered_project_must_name_one() {
    let tasks = started(&[], "").await;
    std::env::set_current_dir(tasks.home.home()).unwrap();

    let refusal = refused(tasks.agent_adds(None, "Cache the build").await);

    assert_eq!(refusal.code, ErrorCode::Refused);
}
