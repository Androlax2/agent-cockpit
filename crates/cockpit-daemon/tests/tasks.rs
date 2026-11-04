//! Issue #38 through the daemon binary's sockets: what an agent may file and work on, what
//! stays the owner's, and the owner's `[tasks]` settings applied. This test process plays the
//! agent; its working directory says which registered project it works in, as a launched
//! session's does.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use std::path::{Path, PathBuf};

use cockpit_agent_channel::{LaunchToken, identity_of};
use cockpit_authorization::Permission;
use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    Command, ErrorCode, HexBytes, Reply, TaskDraftTargetView, TaskKindView, TaskStatusView,
    TaskView, UnblockedBy,
};
use common::{GENEROUS, TestHome, refused};

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
    let config_dir = home.home().join("config").join(common::BUILD_PROFILE_DIR);
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
    drop(database);
    let token = home
        .launch(
            "s1",
            &identity_of(std::process::id()).unwrap(),
            Some(Permission::ChangeState),
        )
        .await;
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

impl Tasks {
    /// Replaces the owner config of the running daemon; the daemon refuses to start with a
    /// broken one, but tasks read it again on each filing.
    fn break_owner_config(&self) {
        std::fs::write(
            self.home
                .home()
                .join("config")
                .join(common::BUILD_PROFILE_DIR)
                .join("config.toml"),
            "[tasks]\ncross_project_open_max = -1\n",
        )
        .unwrap();
    }

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

    /// The agent fixes `item`'s title and notes, as `torlek todo edit` sends it.
    async fn agent_edits(
        &self,
        item: i64,
        title: Option<&str>,
        notes: Option<&str>,
        item_kind: Option<TaskKindView>,
    ) -> Result<Reply, ClientError> {
        self.as_agent(Command::TaskEdit {
            launch_token: self.token(),
            item,
            intent_revision: None,
            title: title.map(str::to_owned),
            notes: notes.map(str::to_owned),
            item_kind,
        })
        .await
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
    let tasks = started(&["app"], "").await;
    tasks.break_owner_config();

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

#[tokio::test]
async fn the_owner_files_even_when_the_owner_config_does_not_load() {
    let tasks = started(&["app"], "").await;
    tasks.break_owner_config();

    let added = tasks.owner_adds("app", "Fix the config", Vec::new()).await;

    assert_eq!(added.status, TaskStatusView::Todo);
}

#[tokio::test]
async fn a_draft_of_a_project_that_is_not_registered_can_still_be_discarded() {
    let tasks = started(&["app"], "").await;

    let discarded = tasks
        .as_owner(Command::TaskDraftDiscard {
            target: TaskDraftTargetView::New {
                project: "gone".to_owned(),
            },
        })
        .await;

    assert_eq!(discarded.unwrap(), Reply::TaskDraftDiscarded);
}

/// #714: an agent fixes the title and notes of an item it filed itself while nobody started
/// it; the owner reads each edit in the session's audit.
#[tokio::test]
async fn an_agent_fixes_the_title_and_notes_of_an_item_it_filed_and_the_edit_is_audited() {
    let tasks = started(&["app"], "").await;
    let filed = task(
        tasks
            .agent_adds(None, "#711? coordinator can end the workers it started")
            .await,
    );

    let edited = task(
        tasks
            .agent_edits(
                filed.id,
                Some("#713 coordinator can end the workers it started"),
                Some("Filed before the issue number existed."),
                None,
            )
            .await,
    );
    let audit = match tasks
        .as_owner(Command::SessionGrants {
            session: "s1".to_owned(),
        })
        .await
    {
        Ok(Reply::CoordinatorGrants { audit, .. }) => audit,
        other => panic!("session-grants answered {other:?}"),
    };

    assert_eq!(
        (
            edited.title.as_str(),
            edited.notes.as_str(),
            edited.intent_revision
        ),
        (
            "#713 coordinator can end the workers it started",
            "Filed before the issue number existed.",
            2
        )
    );
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(
        (
            audit[0].actor.as_str(),
            audit[0].permission.as_str(),
            audit[0].action.as_str(),
            audit[0].target.as_deref(),
        ),
        ("s1", "change-state", "edited", None)
    );
    assert_eq!(
        audit[0].detail.as_deref(),
        Some(&*format!(
            "item {}: title \"#711? coordinator can end the workers it started\" -> \
             \"#713 coordinator can end the workers it started\"; \
             notes \"\" -> \"Filed before the issue number existed.\"",
            filed.id
        ))
    );
}

#[tokio::test]
async fn an_agent_cannot_edit_an_item_it_did_not_file() {
    let tasks = started(&["app"], "").await;
    let owners = tasks
        .owner_adds("app", "Migrate the schema", Vec::new())
        .await;

    let refusal = refused(
        tasks
            .agent_edits(owners.id, Some("Migrate the schema v2"), None, None)
            .await,
    );

    assert_eq!(
        (refusal.code, refusal.unblock.map(|unblock| unblock.by)),
        (ErrorCode::NotAuthorized, Some(UnblockedBy::Owner))
    );
}

#[tokio::test]
async fn an_agent_cannot_edit_its_item_once_it_started() {
    let tasks = started(&["app"], "").await;
    let filed = task(tasks.agent_adds(None, "Migrate the schema").await);
    task(
        tasks
            .as_agent(Command::TaskStart {
                launch_token: tasks.token(),
                item: filed.id,
            })
            .await,
    );

    let refusal = refused(
        tasks
            .agent_edits(filed.id, Some("Migrate the schema v2"), None, None)
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::Conflict);
    assert!(
        refusal.message.contains("is doing"),
        "names the status: {}",
        refusal.message
    );
}

#[tokio::test]
async fn an_agents_change_of_its_items_kind_stays_the_owners() {
    let tasks = started(&["app"], "").await;
    let filed = task(tasks.agent_adds(None, "Migrate the schema").await);

    let refusal = refused(
        tasks
            .agent_edits(filed.id, None, None, Some(TaskKindView::Bug))
            .await,
    );

    assert_eq!(
        (refusal.code, refusal.unblock.map(|unblock| unblock.by)),
        (ErrorCode::NotAuthorized, Some(UnblockedBy::Owner))
    );
}
