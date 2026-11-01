//! Issue #39 through the daemon binary's sockets: the preset a project asks for, what stays
//! the owner's, requirements no request can claim, and a restart mid-step resuming at the
//! checkpoint. This test process plays the agent; its working directory says which registered
//! project it works in, as a launched session's does.
//!
//! This process has no session record, so it cannot begin a step through the daemon (the
//! daemon reads the session's provider to check the step's capabilities): the restart fixture
//! begins the step through `cockpit-workflows` before the daemon starts, as a launched
//! session's `workflow begin` would have.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};

use cockpit_agent_channel::{LaunchToken, identity_of};
use cockpit_authorization::{Permission, SessionId};
use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    Command, ErrorCode, Reply, UnblockedBy, WorkflowHandoffView, WorkflowNextView,
    WorkflowPacketView, WorkflowStepStatusView, WorkflowView,
};
use cockpit_provider_api::{Capabilities, Capability, Provenance};
use common::{GENEROUS, TestHome, refused};

/// The working directory is the process's: tests that move it take turns.
static WORKING_DIRECTORY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const NOW: i64 = 1_800_000_000;

/// Fields drop in order: the daemon stops before its home is deleted.
struct Workflows {
    daemon: Option<common::RunningDaemon>,
    home: TestHome,
    token: LaunchToken,
    _cwd: tokio::sync::MutexGuard<'static, ()>,
}

/// A home whose owner registered project `app` with `project_config` as its
/// `.cockpit/config.toml`, with session `s0123456789ab` launched from this process and working in it.
/// The daemon is not started yet.
async fn prepared(project_config: &str) -> Workflows {
    let cwd = WORKING_DIRECTORY.lock().await;
    let home = TestHome::new();
    std::fs::create_dir_all(home.home().join("config/cockpit")).unwrap();
    let root = repository(home.home(), "app");
    std::fs::create_dir_all(root.join(".cockpit")).unwrap();
    std::fs::write(root.join(".cockpit/config.toml"), project_config).unwrap();
    let database = home.open_state().await;
    cockpit_projects::register_project(&database, &cockpit_projects::Git::from_path(), &root, None)
        .await
        .unwrap();
    drop(database);
    let token = home
        .launch(
            "s0123456789ab",
            &identity_of(std::process::id()).unwrap(),
            Some(Permission::ChangeState),
        )
        .await;
    std::env::set_current_dir(&root).unwrap();
    Workflows {
        daemon: None,
        home,
        token,
        _cwd: cwd,
    }
}

async fn started(project_config: &str) -> Workflows {
    let mut workflows = prepared(project_config).await;
    workflows.daemon = Some(workflows.home.start());
    workflows
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

impl Workflows {
    fn token(&self) -> Option<String> {
        Some(self.token.as_str().to_owned())
    }

    async fn agent(&self) -> Connection {
        self.home.connect_agent().await
    }

    async fn as_agent(&self, command: Command) -> Result<Reply, ClientError> {
        self.agent().await.request(command, GENEROUS).await
    }

    async fn as_owner(&self, command: Command) -> Result<Reply, ClientError> {
        self.home.connect().await.request(command, GENEROUS).await
    }

    async fn owner_adds(&self, title: &str) -> i64 {
        match self
            .as_owner(Command::TaskAdd {
                launch_token: None,
                project: Some("app".to_owned()),
                title: title.to_owned(),
                notes: String::new(),
                item_kind: None,
                priority: None,
                after: Vec::new(),
            })
            .await
        {
            Ok(Reply::Task { task }) => task.id,
            other => panic!("expected a task, got {other:?}"),
        }
    }

    async fn owner_starts(&self, item: i64, preset: Option<&str>) -> Result<Reply, ClientError> {
        self.as_owner(Command::WorkflowStart {
            item,
            preset: preset.map(str::to_owned),
        })
        .await
    }

    async fn packet(&self, workflow: i64) -> WorkflowPacketView {
        match self
            .as_agent(Command::WorkflowPacket {
                launch_token: self.token(),
                workflow,
            })
            .await
        {
            Ok(Reply::WorkflowPacket { packet }) => *packet,
            other => panic!("expected a packet, got {other:?}"),
        }
    }

    /// Stops the daemon as a crash would and starts it again on the same state.
    fn crash_and_restart(&mut self) {
        self.daemon.take().expect("the daemon runs").kill();
        self.daemon = Some(self.home.start());
    }

    /// A `code-change` run on a new item whose first step `s0123456789ab` began and checkpointed, as
    /// its agent would have, before the daemon starts.
    async fn begun_with_checkpoint(&self, note: &str) -> i64 {
        let database = self.home.open_state().await;
        let write = database.write().await.unwrap();
        let settings = cockpit_tasks::Settings {
            blocker_project: None,
            cross_project_open_max: 3,
        };
        let item = cockpit_tasks::add(
            &write,
            &cockpit_tasks::Actor::Owner,
            &settings,
            cockpit_tasks::NewItem {
                project: Some("app".to_owned()),
                title: "Fix the login form".to_owned(),
                notes: String::new(),
                kind: cockpit_tasks::Kind::Bug,
                priority: cockpit_tasks::Priority::NORMAL,
                after: Vec::new(),
            },
            NOW,
        )
        .await
        .unwrap();
        let run = cockpit_workflows::start(
            &write,
            &cockpit_tasks::Actor::Owner,
            item.id,
            &cockpit_workflows::CODE_CHANGE,
            NOW,
        )
        .await
        .unwrap();
        let session = cockpit_tasks::Actor::Session {
            session: SessionId::new("s0123456789ab").unwrap(),
            project: Some("app".to_owned()),
        };
        let observed = Capabilities::none()
            .with(Capability::Events, Provenance::Discovered)
            .with(Capability::Outcome, Provenance::Discovered);
        cockpit_workflows::begin(&write, &session, run.id, &observed, NOW)
            .await
            .unwrap();
        cockpit_workflows::checkpoint(&write, &session, run.id, note, NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();
        run.id.0
    }
}

fn workflow(reply: Result<Reply, ClientError>) -> WorkflowView {
    match reply {
        Ok(Reply::Workflow { workflow }) => *workflow,
        other => panic!("expected a workflow, got {other:?}"),
    }
}

fn handoff(changed: &str) -> WorkflowHandoffView {
    WorkflowHandoffView {
        changed: changed.to_owned(),
        checked: "commits abc123..def456".to_owned(),
        outcome: "cargo nextest: 12 passed".to_owned(),
        needs_human: String::new(),
        evidence: vec!["receipt 7".to_owned()],
    }
}

#[tokio::test]
async fn a_restart_mid_step_resumes_at_the_checkpoint_through_the_agent_socket() {
    let mut workflows = prepared("").await;
    let run = workflows
        .begun_with_checkpoint("Validation rewritten; tests next")
        .await;
    workflows.daemon = Some(workflows.home.start());
    let before = workflows.packet(run).await;

    workflows.crash_and_restart();

    let after = workflows.packet(run).await;
    assert_eq!(after, before);
    let step = after.step.expect("a current step");
    assert_eq!(
        (step.position, step.status, step.session.as_deref()),
        (1, WorkflowStepStatusView::Active, Some("s0123456789ab"))
    );
    assert_eq!(
        step.checkpoint.map(|checkpoint| checkpoint.note),
        Some("Validation rewritten; tests next".to_owned())
    );
    let completed = workflow(
        workflows
            .as_agent(Command::WorkflowComplete {
                launch_token: workflows.token(),
                workflow: run,
                handoff: handoff("Rewrote the form validation"),
                waive: Vec::new(),
            })
            .await,
    );
    assert_eq!(
        completed.next,
        WorkflowNextView::AwaitPlacement { position: 2 }
    );
}

#[tokio::test]
async fn the_owner_starts_a_workflow_with_the_preset_the_project_asks_for() {
    let workflows = started("[workflow]\npreset = \"design-led\"\n").await;
    let item = workflows.owner_adds("New settings screen").await;

    let started = workflow(workflows.owner_starts(item, None).await);

    assert_eq!(
        (started.preset.as_str(), started.next),
        (
            "design-led",
            WorkflowNextView::OwnerCompletes { position: 1 }
        )
    );
}

#[tokio::test]
async fn a_preset_the_owner_names_wins_over_the_projects() {
    let workflows = started("[workflow]\npreset = \"design-led\"\n").await;
    let item = workflows.owner_adds("Compare queues").await;

    let started = workflow(workflows.owner_starts(item, Some("research")).await);

    assert_eq!(started.preset, "research");
}

#[tokio::test]
async fn a_project_preset_no_preset_has_is_refused_for_the_owner_to_fix() {
    let workflows = started("[workflow]\npreset = \"solo\"\n").await;
    let item = workflows.owner_adds("Fix it").await;

    let refusal = refused(workflows.owner_starts(item, None).await);

    assert_eq!(
        (refusal.code, refusal.unblock.map(|unblock| unblock.by)),
        (ErrorCode::InvalidArgument, Some(UnblockedBy::Owner))
    );
}

#[tokio::test]
async fn an_agent_cannot_start_or_cancel_a_workflow() {
    let workflows = started("").await;
    let item = workflows.owner_adds("Fix it").await;
    let run = workflow(workflows.owner_starts(item, None).await).id;

    let start = refused(
        workflows
            .as_agent(Command::WorkflowStart { item, preset: None })
            .await,
    );
    let cancel = refused(
        workflows
            .as_agent(Command::WorkflowCancel { workflow: run })
            .await,
    );

    assert_eq!(
        (start.code, cancel.code),
        (ErrorCode::NotAuthorized, ErrorCode::NotAuthorized)
    );
}

#[tokio::test]
async fn the_owner_cannot_begin_a_step() {
    let workflows = started("").await;
    let item = workflows.owner_adds("Fix it").await;
    let run = workflow(workflows.owner_starts(item, None).await).id;

    let refusal = refused(
        workflows
            .as_owner(Command::WorkflowBegin {
                launch_token: None,
                workflow: run,
            })
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::NotAuthorized);
}

#[tokio::test]
async fn a_sender_without_a_session_record_cannot_begin_a_step() {
    let workflows = started("").await;
    let item = workflows.owner_adds("Fix it").await;
    let run = workflow(workflows.owner_starts(item, None).await).id;

    let refusal = refused(
        workflows
            .as_agent(Command::WorkflowBegin {
                launch_token: workflows.token(),
                workflow: run,
            })
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::NotFound);
    let shown = workflow(
        workflows
            .as_owner(Command::WorkflowShow {
                launch_token: None,
                workflow: run,
            })
            .await,
    );
    assert_eq!(shown.steps[0].status, WorkflowStepStatusView::Ready);
}

#[tokio::test]
async fn an_agent_sees_its_projects_workflows() {
    let workflows = started("").await;
    let item = workflows.owner_adds("Fix it").await;
    let run = workflow(workflows.owner_starts(item, None).await).id;

    let listed = match workflows
        .as_agent(Command::WorkflowList {
            launch_token: workflows.token(),
            project: None,
            all: false,
        })
        .await
    {
        Ok(Reply::Workflows { workflows }) => workflows,
        other => panic!("expected workflows, got {other:?}"),
    };

    assert_eq!(
        listed.iter().map(|listed| listed.id).collect::<Vec<_>>(),
        [run]
    );
}

#[tokio::test]
async fn a_step_with_requirements_completes_only_with_the_owners_waiver() {
    let workflows = started("").await;
    let item = workflows.owner_adds("Fix it").await;
    let run = workflow(workflows.owner_starts(item, Some("code-change")).await).id;
    let complete = |waive: Vec<String>| Command::WorkflowComplete {
        launch_token: None,
        workflow: run,
        handoff: handoff("Rewrote the form validation"),
        waive,
    };
    workflow(workflows.as_owner(complete(Vec::new())).await);

    let unmet = refused(workflows.as_owner(complete(Vec::new())).await);
    let waived = workflow(
        workflows
            .as_owner(complete(vec!["checks".to_owned(), "review".to_owned()]))
            .await,
    );

    assert_eq!(unmet.code, ErrorCode::Conflict);
    assert_eq!(
        waived.next,
        WorkflowNextView::AwaitPlacement { position: 3 }
    );
}

#[tokio::test]
async fn the_presets_list_names_each_step_and_its_requirements() {
    let workflows = started("").await;

    let presets = match workflows.as_agent(Command::WorkflowPresets).await {
        Ok(Reply::WorkflowPresets { presets }) => presets,
        other => panic!("expected presets, got {other:?}"),
    };

    let check = &presets
        .iter()
        .find(|preset| preset.name == "code-change")
        .expect("code-change is offered")
        .steps[1];
    assert_eq!(
        (check.name.as_str(), check.requirements.clone()),
        ("check", vec!["checks".to_owned(), "review".to_owned()])
    );
}
