//! Issue #329: `workflow begin` through the daemon from a launched session's own agent. The
//! daemon reads the provider from the session's record and checks the step's capabilities
//! against what that provider reports.
//!
//! The daemon runs in this process with fake providers (as in `sessions.rs`). Each fake agent
//! runs this test binary's `workflow_agent` from its own process tree, so the request reaches
//! the agent socket from the launched session's tree with its launch token, as `cockpit
//! workflow begin` would.
#![cfg(target_os = "linux")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{
    Command, ErrorCode, ProtocolError, Reply, SessionView, WorkflowNextView,
    WorkflowStepStatusView, WorkflowView,
};
use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{Capability, ProviderName};
use common::GENEROUS;
use serde::{Deserialize, Serialize};

/// The variables the fake agent's helper reads; outside this file's tests it has none and
/// does nothing.
const AGENT_SOCKET: &str = "WORKFLOW_AGENT_SOCKET";
const AGENT_WORKFLOW: &str = "WORKFLOW_AGENT_WORKFLOW";
const AGENT_ANSWER: &str = "WORKFLOW_AGENT_ANSWER";

/// What the helper got back, written for the test to read.
#[derive(Debug, Serialize, Deserialize)]
enum Answer {
    Reply(Reply),
    Refused(ProtocolError),
}

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    fn helper(&self) -> PathBuf {
        self.path().join("workflow-agent")
    }

    fn answer_path(&self) -> PathBuf {
        self.path().join("answer.json")
    }

    /// A provider named `name` reporting `capabilities` besides `resume`, whose agent runs
    /// the helper.
    fn provider(&self, name: &str, capabilities: &[Capability]) -> Arc<FakeProvider> {
        let provider = capabilities.iter().fold(
            FakeProvider::new(
                ProviderName::new(name).unwrap(),
                self.path().join("agent.log"),
            ),
            |provider, capability| provider.reporting(*capability),
        );
        Arc::new(provider.running(self.helper()))
    }

    /// A daemon offering `observed`, which reports what every agent step needs, and
    /// `unobserved`, whose turn-end events do not say how a turn ended.
    async fn start(&self) -> Daemon {
        let providers: Vec<Arc<dyn cockpit_provider_api::Provider>> = vec![
            self.provider("observed", &[Capability::Events, Capability::Outcome]),
            self.provider("unobserved", &[Capability::Events]),
        ];
        let options = Options {
            runtime_dir: self.path().join("run/cockpit"),
            state_dir: self.path().join("state/cockpit"),
            limits: Limits::default(),
        };
        let environment = cockpit_config::Environment {
            home: Some(self.path().join("home").into()),
            ..cockpit_config::Environment::default()
        };
        Instance::acquire(options)
            .unwrap()
            .start(
                common::wiring_in(&self.path(), providers),
                move |capabilities| Arc::new(Routes::new(capabilities, environment)),
            )
            .await
            .unwrap()
    }

    /// The helper the next launched agent runs: it begins `workflow` on `daemon`'s agent
    /// socket and writes what it got back.
    fn arm_helper(&self, daemon: &Daemon, workflow: i64) {
        let _ = std::fs::remove_file(self.answer_path());
        let script = format!(
            "#!/bin/sh\nexec /usr/bin/env {AGENT_SOCKET}='{}' {AGENT_WORKFLOW}={workflow} \
             {AGENT_ANSWER}='{}' '{}' --exact workflow_agent --nocapture \
             >'{}' 2>&1\n",
            daemon.agent_socket_path().display(),
            self.answer_path().display(),
            std::env::current_exe().unwrap().display(),
            self.path().join("workflow-agent.log").display(),
        );
        std::fs::write(self.helper(), script).unwrap();
        std::fs::set_permissions(self.helper(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// What the helper wrote, once it has.
    async fn answer(&self) -> Answer {
        let deadline = Instant::now() + GENEROUS;
        loop {
            if let Ok(written) = std::fs::read_to_string(self.answer_path()) {
                return serde_json::from_str(&written).unwrap();
            }
            assert!(
                Instant::now() < deadline,
                "the agent's helper wrote no answer; its log: {}",
                std::fs::read_to_string(self.path().join("workflow-agent.log")).unwrap_or_default()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// A Git project `app` registered through the daemon, returning its root.
    async fn project(&self, owner: &Connection) -> PathBuf {
        let root = self.path().join("projects/app");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        git(
            &root,
            &["commit", "--quiet", "--allow-empty", "-m", "initial"],
        );
        request(
            owner,
            Command::ProjectAdd {
                path: root.display().to_string(),
                name: None,
            },
        )
        .await
        .unwrap();
        root
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = Process::new("tmux")
            .arg("-S")
            .arg(self.path().join("tmux.sock"))
            .arg("kill-server")
            .env_remove("TMUX")
            .output();
    }
}

fn git(dir: &Path, arguments: &[&str]) {
    let output = Process::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.com"])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn request(client: &Connection, command: Command) -> Result<Reply, ClientError> {
    client.request(command, Duration::from_secs(60)).await
}

fn workflow(reply: Result<Reply, ClientError>) -> WorkflowView {
    match reply {
        Ok(Reply::Workflow { workflow }) => *workflow,
        other => panic!("expected a workflow, got {other:?}"),
    }
}

/// A `code-change` workflow the owner started on a new item of `app`.
async fn started_code_change(owner: &Connection) -> i64 {
    let added = request(
        owner,
        Command::TaskAdd {
            launch_token: None,
            project: Some("app".to_owned()),
            title: "Fix the login form".to_owned(),
            notes: String::new(),
            item_kind: None,
            priority: None,
            after: Vec::new(),
        },
    )
    .await;
    let Ok(Reply::Task { task }) = added else {
        panic!("task-add: {added:?}");
    };
    workflow(
        request(
            owner,
            Command::WorkflowStart {
                item: task.id,
                preset: Some("code-change".to_owned()),
            },
        )
        .await,
    )
    .id
}

/// Launches a session of `agent` in `root`; its agent's helper begins the armed workflow.
async fn launched(owner: &Connection, root: &Path, agent: &str) -> SessionView {
    match request(
        owner,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some(agent.to_owned()),
            prompt: None,
            clone: None,
        },
    )
    .await
    {
        Ok(Reply::Session { session }) => *session,
        other => panic!("session-new: {other:?}"),
    }
}

async fn shown(owner: &Connection, run: i64) -> WorkflowView {
    workflow(
        request(
            owner,
            Command::WorkflowShow {
                launch_token: None,
                workflow: run,
            },
        )
        .await,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launched_session_begins_a_code_change_step_through_its_agent_socket() {
    let home = Home::new();
    let daemon = home.start().await;
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&owner).await;
    let run = started_code_change(&owner).await;
    home.arm_helper(&daemon, run);

    let session = launched(&owner, &root, "observed").await;

    let Answer::Reply(Reply::Workflow { workflow: begun }) = home.answer().await else {
        panic!("the begin was not answered with the workflow");
    };
    assert_eq!(
        begun.next,
        WorkflowNextView::InProgress {
            position: 1,
            session: Some(session.id.clone())
        }
    );
    let step = &shown(&owner, run).await.steps[0];
    assert_eq!(
        (step.status, step.session.as_deref(), step.attempts),
        (WorkflowStepStatusView::Active, Some(session.id.as_str()), 1)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_whose_provider_lacks_outcome_is_refused_naming_it() {
    let home = Home::new();
    let daemon = home.start().await;
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&owner).await;
    let run = started_code_change(&owner).await;
    home.arm_helper(&daemon, run);

    launched(&owner, &root, "unobserved").await;

    let Answer::Refused(refusal) = home.answer().await else {
        panic!("the begin was not refused");
    };
    assert_eq!(refusal.code, ErrorCode::Refused);
    assert!(
        refusal.message.contains("outcome") && !refusal.message.contains("events"),
        "{}",
        refusal.message
    );
    assert_eq!(
        shown(&owner, run).await.steps[0].status,
        WorkflowStepStatusView::Ready
    );
}

/// The fake agent's helper: run by the tests above from inside a launched agent's process
/// tree, it begins the workflow they name on the agent socket.
#[tokio::test]
async fn workflow_agent() {
    let Some(socket) = std::env::var_os(AGENT_SOCKET) else {
        return;
    };
    let workflow = std::env::var(AGENT_WORKFLOW).unwrap().parse().unwrap();
    let answer_path = PathBuf::from(std::env::var_os(AGENT_ANSWER).unwrap());
    let agent = Connection::open(Path::new(&socket), GENEROUS)
        .await
        .unwrap();
    let answer = match agent
        .request(
            Command::WorkflowBegin {
                launch_token: std::env::var(cockpit_protocol::LAUNCH_TOKEN_VARIABLE).ok(),
                workflow,
            },
            GENEROUS,
        )
        .await
    {
        Ok(reply) => Answer::Reply(reply),
        Err(ClientError::Daemon(refusal)) => Answer::Refused(refusal),
        Err(error) => panic!("the begin failed: {error}"),
    };
    // Written whole, then renamed, so the test never reads half an answer.
    let partial = answer_path.with_extension("partial");
    std::fs::write(&partial, serde_json::to_string(&answer).unwrap()).unwrap();
    std::fs::rename(partial, answer_path).unwrap();
}
