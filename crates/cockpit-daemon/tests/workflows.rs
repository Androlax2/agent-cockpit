//! Issue #39 through the daemon binary's sockets: the preset a project asks for, what stays
//! the owner's, requirements no request can claim, and a restart mid-step resuming at the
//! checkpoint. This test process plays the agent; its working directory says which registered
//! project it works in, as a launched session's does.
//!
//! This process has no session record, so it cannot begin a step through the daemon (the
//! daemon reads the session's provider to check the step's capabilities): the restart fixture
//! begins the step through `cockpit-workflows` before the daemon starts, as a launched
//! session's `workflow begin` would have. `workflow_begin.rs` begins one from a launched
//! session's agent (#329).
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};

use cockpit_agent_channel::{LaunchToken, identity_of};
use cockpit_authorization::{Permission, SessionId};
use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    Command, ErrorCode, Reply, TaskStatusView, UnblockedBy, WorkflowHandoffView, WorkflowNextView,
    WorkflowPacketView, WorkflowStatusView, WorkflowStepStatusView, WorkflowView,
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
        self.begun_preset_with_checkpoint(note, &cockpit_workflows::CODE_CHANGE)
            .await
    }

    async fn begun_preset_with_checkpoint(
        &self,
        note: &str,
        preset: &'static cockpit_workflows::Preset,
    ) -> i64 {
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
        let run =
            cockpit_workflows::start(&write, &cockpit_tasks::Actor::Owner, item.id, preset, NOW)
                .await
                .unwrap();
        if preset.name == "design-led" {
            cockpit_workflows::complete(
                &write,
                &cockpit_tasks::Actor::Owner,
                run.id,
                cockpit_workflows::Completion {
                    handoff: cockpit_workflows::Handoff {
                        changed: "Approved the design".into(),
                        checked: "design v1".into(),
                        outcome: "approved".into(),
                        needs_human: String::new(),
                        evidence: Vec::new(),
                    },
                    satisfied: Vec::new(),
                    waived: Vec::new(),
                },
                NOW,
            )
            .await
            .unwrap();
        }
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

/// #333: the item closes outside the workflow; the run stays running and its packet answers,
/// then says the item is gone once it is deleted.
#[tokio::test]
async fn the_packet_of_a_run_whose_item_was_finished_then_deleted_still_answers() {
    let workflows = started("").await;
    let item = workflows.owner_adds("Fix it").await;
    let run = workflow(workflows.owner_starts(item, None).await).id;
    workflows
        .as_owner(Command::TaskDone {
            launch_token: None,
            item,
            summary: Some("fixed by hand".to_owned()),
        })
        .await
        .unwrap();

    let finished = workflows.packet(run).await;
    workflows
        .as_owner(Command::TaskDelete {
            launch_token: None,
            item,
        })
        .await
        .unwrap();
    let deleted = workflows.packet(run).await;

    assert_eq!(
        finished.goal.map(|goal| goal.item_status),
        Some(TaskStatusView::Done)
    );
    assert_eq!(
        (deleted.item, deleted.goal, deleted.status),
        (item, None, WorkflowStatusView::Running)
    );
}

/// Passing prose cannot replace receipts; stale or failed receipts cannot advance a step.
#[tokio::test]
async fn fresh_receipts_complete_check_but_stale_failed_and_claimed_evidence_refuse() {
    evidence_scenario(&cockpit_workflows::CODE_CHANGE, "review").await;
}

#[tokio::test]
async fn design_match_receipts_complete_design_led_verification() {
    evidence_scenario(&cockpit_workflows::DESIGN_LED, "design-match").await;
}

async fn evidence_scenario(preset: &'static cockpit_workflows::Preset, second: &str) {
    let mut config = format!(
        "[[checks]]\nname = \"checks\"\ncommand = \"true\"\nartifacts = [\"artifact\"]\n\n[[checks]]\nname = \"{second}\"\ncommand = \"true\"\n"
    );
    config.push_str("\n[[checks]]\nname = \"unrelated\"\ncommand = \"true\"\n");
    let mut workflows = prepared(&config).await;
    let run = workflows
        .begun_preset_with_checkpoint("implemented", preset)
        .await;
    let root = std::env::current_dir().unwrap();
    std::fs::write(root.join(".gitignore"), "artifact\n").unwrap();
    std::fs::write(root.join("artifact"), "evidence").unwrap();
    for args in [
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "initial",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
    }
    workflows.daemon = Some(workflows.home.start());
    let token = workflows.token();
    let complete = || Command::WorkflowComplete {
        launch_token: token.clone(),
        workflow: run,
        handoff: handoff("Checked the implementation"),
        waive: Vec::new(),
    };
    workflow(workflows.as_agent(complete()).await);
    workflows.daemon.take().unwrap().kill();
    // Begin check directly, like the fixture's initial step: no live session is needed.
    {
        let database = workflows.home.open_state().await;
        let write = database.write().await.unwrap();
        let actor = cockpit_tasks::Actor::Session {
            session: SessionId::new("s0123456789ab").unwrap(),
            project: Some("app".into()),
        };
        let capabilities = Capabilities::none()
            .with(Capability::Events, Provenance::Discovered)
            .with(Capability::Outcome, Provenance::Discovered);
        cockpit_workflows::begin(
            &write,
            &actor,
            cockpit_workflows::RunId(run),
            &capabilities,
            NOW,
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
    }
    workflows.daemon = Some(workflows.home.start());
    let mut waiver = complete();
    if let Command::WorkflowComplete { waive, .. } = &mut waiver {
        *waive = vec!["checks".into()];
    }
    assert_eq!(
        refused(workflows.as_agent(waiver).await).code,
        ErrorCode::NotAuthorized
    );
    let missing = refused(workflows.as_agent(complete()).await);
    assert_eq!(missing.code, ErrorCode::Conflict);
    assert!(missing.message.contains("checks"));
    workflows
        .as_agent(Command::CheckClaim {
            launch_token: workflows.token(),
            check: "checks".into(),
            statement: "everything passed".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        refused(workflows.as_agent(complete()).await).code,
        ErrorCode::Conflict
    );
    let source = cockpit_projects::Git::from_path()
        .source_state(&root)
        .unwrap();
    let record = async |workflows: &mut Workflows, outcome| {
        workflows.daemon.take().unwrap().kill();
        let database = workflows.home.open_state().await;
        let write = database.write().await.unwrap();
        for name in ["checks", second, "unrelated"] {
            cockpit_verification::record_receipt(
                &write,
                cockpit_verification::NewReceipt {
                    project: "app".into(),
                    run_id: format!("{name}-{outcome:?}"),
                    check: cockpit_verification::CheckDefinition {
                        name: name.into(),
                        command: "true".into(),
                        working_directory: ".".into(),
                        environment: "native".into(),
                    },
                    source: cockpit_verification::Source {
                        head: source.head.clone(),
                        base: None,
                        dirty: source.dirty.clone(),
                    },
                    artifacts: if name == "checks" {
                        cockpit_verification::fingerprint(&root, &["artifact"]).unwrap()
                    } else {
                        Vec::new()
                    },
                    outcome,
                    exit_code: Some(if outcome == cockpit_verification::Outcome::Passed {
                        0
                    } else {
                        1
                    }),
                    output: String::new(),
                    output_references: Vec::new(),
                    started_at: NOW,
                    finished_at: NOW,
                },
                NOW,
            )
            .await
            .unwrap();
        }
        write.commit().await.unwrap();
        drop(database);
        workflows.daemon = Some(workflows.home.start());
    };
    for outcome in [
        cockpit_verification::Outcome::Failed,
        cockpit_verification::Outcome::Unknown,
        cockpit_verification::Outcome::Unavailable,
        cockpit_verification::Outcome::Blocked,
        cockpit_verification::Outcome::Cancelled,
    ] {
        record(&mut workflows, outcome).await;
        assert_eq!(
            refused(workflows.as_agent(complete()).await).code,
            ErrorCode::Conflict
        );
    }
    record(&mut workflows, cockpit_verification::Outcome::Passed).await;
    std::fs::write(root.join("artifact"), "changed evidence").unwrap();
    assert!(
        refused(workflows.as_agent(complete()).await)
            .message
            .contains("checks")
    );
    std::fs::write(root.join("artifact"), "evidence").unwrap();
    std::fs::write(root.join("changed"), "new result").unwrap();
    let stale = refused(workflows.as_agent(complete()).await);
    assert_eq!(stale.code, ErrorCode::Conflict);
    assert!(stale.message.contains("checks"));
    std::fs::remove_file(root.join("changed")).unwrap();
    let completed = workflow(workflows.as_agent(complete()).await);
    assert_eq!(
        completed.next,
        if preset.name == "design-led" {
            WorkflowNextView::Nothing
        } else {
            WorkflowNextView::AwaitPlacement { position: 3 }
        }
    );
}

/// Completion may hash a checkout: full bulk admission must refuse it before Git/artifact
/// work starts, while ordinary owner workflow reads retain their control capacity.
#[tokio::test]
async fn owner_workflow_completion_uses_bulk_admission_while_reads_stay_responsive() {
    use cockpit_daemon::{
        Handler, Instance, LaneLimits, Limits, Options, Peer, Route, Routes, WorkClass,
    };
    use cockpit_protocol::{FeedPosition, Outcome, Welcome};
    use std::sync::Arc;
    use tokio::sync::{Notify, Semaphore};

    struct HeldBulk {
        routes: Routes,
        started: Arc<Notify>,
        release: Arc<Semaphore>,
    }
    impl Handler for HeldBulk {
        fn route(&self, command: Command, welcome: &Welcome, peer: Peer) -> Route {
            if matches!(command, Command::Subscribe { .. }) {
                let started = Arc::clone(&self.started);
                let release = Arc::clone(&self.release);
                return Route::Run {
                    class: WorkClass::Bulk,
                    job: Box::pin(async move {
                        started.notify_one();
                        let _permit = release.acquire().await.unwrap();
                        Outcome::Ok(Reply::Pong)
                    }),
                };
            }
            self.routes.route(command, welcome, peer)
        }
    }
    let home = tempfile::tempdir().unwrap();
    let root = home.path().canonicalize().unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let daemon = Instance::acquire(Options {
        runtime_dir: root.join("run"),
        state_dir: root.join("state"),
        limits: Limits {
            bulk: LaneLimits {
                workers: 1,
                waiting: 0,
            },
            ..Limits::default()
        },
    })
    .unwrap()
    .start(common::wiring_in(&root, Vec::new()), |capabilities| {
        Arc::new(HeldBulk {
            routes: Routes::new(
                capabilities,
                cockpit_config::Environment {
                    home: Some(root.join("home").into()),
                    ..cockpit_config::Environment::default()
                },
            ),
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        })
    })
    .await
    .unwrap();
    let client = Arc::new(
        Connection::open(daemon.socket_path(), GENEROUS)
            .await
            .unwrap(),
    );
    let held_client = Arc::clone(&client);
    let held = tokio::spawn(async move {
        held_client
            .request(
                Command::Subscribe {
                    from: FeedPosition::Snapshot,
                },
                GENEROUS,
            )
            .await
    });
    tokio::time::timeout(GENEROUS, started.notified())
        .await
        .unwrap();
    let completion = client
        .request(
            Command::WorkflowComplete {
                launch_token: None,
                workflow: 1,
                handoff: handoff("Checked the implementation"),
                waive: Vec::new(),
            },
            GENEROUS,
        )
        .await;
    let reads = client
        .request(
            Command::WorkflowList {
                launch_token: None,
                project: None,
                all: false,
            },
            GENEROUS,
        )
        .await;
    // Release even on a regression, so the fixture leaves no held task behind.
    release.add_permits(1);
    held.await.unwrap().unwrap();
    let refusal = refused(completion);
    assert_eq!(refusal.code, ErrorCode::Busy);
    assert!(refusal.message.contains("bulk"));
    assert!(matches!(reads, Ok(Reply::Workflows { .. })));
    drop(daemon);
}
