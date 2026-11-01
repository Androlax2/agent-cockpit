//! Issue #49 through the daemon: the instruction bundle written at every start, the
//! `COCKPIT_INSTRUCTIONS` every agent gets, and a launched session's brief assembled from its
//! owners' facts (the session's workspace, the step it does, its grants).
//!
//! The daemon runs in this process with a fake provider; its agent runs this test binary's
//! `brief_agent` from its own process tree, so requests reach the agent socket as `cockpit
//! brief` would (as in `workflow_begin.rs`).
#![cfg(target_os = "linux")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{Command, ErrorCode, ProtocolError, Reply};
use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{Capability, ProviderName};
use common::GENEROUS;
use serde::{Deserialize, Serialize};

const AGENT_SOCKET: &str = "BRIEF_AGENT_SOCKET";
const AGENT_WORKFLOW: &str = "BRIEF_AGENT_WORKFLOW";
const AGENT_ANSWER: &str = "BRIEF_AGENT_ANSWER";
const AGENT_BRIEF_FROM: &str = "BRIEF_AGENT_FROM";

#[derive(Debug, Serialize, Deserialize)]
enum Answer {
    Reply(Reply),
    Refused(ProtocolError),
}

/// What the helper saw and got back.
#[derive(Debug, Serialize, Deserialize)]
struct Seen {
    instructions: Option<String>,
    begun: Option<Answer>,
    brief: Answer,
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
        self.path().join("brief-agent")
    }

    fn answer_path(&self) -> PathBuf {
        self.path().join("answer.json")
    }

    fn bundle_dir(&self) -> PathBuf {
        self.path().join("state/cockpit/agent-context")
    }

    async fn start(&self) -> Daemon {
        let provider = FakeProvider::new(
            ProviderName::new("observed").unwrap(),
            self.path().join("agent.log"),
        )
        .reporting(Capability::Events)
        .reporting(Capability::Outcome)
        .running(self.helper());
        let providers: Vec<Arc<dyn cockpit_provider_api::Provider>> = vec![Arc::new(provider)];
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

    /// The next launched agent begins `workflow` when given one, then asks for its brief.
    fn arm_helper(&self, daemon: &Daemon, workflow: Option<i64>) {
        self.arm_helper_asking_from(daemon, workflow, None);
    }

    /// As [`Home::arm_helper`], the brief asked from `directory` when given one.
    fn arm_helper_asking_from(
        &self,
        daemon: &Daemon,
        workflow: Option<i64>,
        directory: Option<&Path>,
    ) {
        let _ = std::fs::remove_file(self.answer_path());
        let workflow = workflow
            .map(|id| format!("{AGENT_WORKFLOW}={id} "))
            .unwrap_or_default();
        let directory = directory
            .map(|path| format!("{AGENT_BRIEF_FROM}='{}' ", path.display()))
            .unwrap_or_default();
        let script = format!(
            "#!/bin/sh\nexec /usr/bin/env {AGENT_SOCKET}='{}' {workflow}{directory}{AGENT_ANSWER}='{}' '{}' \
             --exact brief_agent --nocapture >'{}' 2>&1\n",
            daemon.agent_socket_path().display(),
            self.answer_path().display(),
            std::env::current_exe().unwrap().display(),
            self.path().join("brief-agent.log").display(),
        );
        std::fs::write(self.helper(), script).unwrap();
        std::fs::set_permissions(self.helper(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    async fn seen(&self) -> Seen {
        let deadline = Instant::now() + GENEROUS;
        loop {
            if let Ok(written) = std::fs::read_to_string(self.answer_path()) {
                return serde_json::from_str(&written).unwrap();
            }
            assert!(
                Instant::now() < deadline,
                "the agent's helper wrote nothing; its log: {}",
                std::fs::read_to_string(self.path().join("brief-agent.log")).unwrap_or_default()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

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
    assert!(output.status.success(), "git {arguments:?}");
}

async fn request(client: &Connection, command: Command) -> Result<Reply, ClientError> {
    client.request(command, Duration::from_secs(60)).await
}

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
    match request(
        owner,
        Command::WorkflowStart {
            item: task.id,
            preset: Some("code-change".to_owned()),
        },
    )
    .await
    {
        Ok(Reply::Workflow { workflow }) => workflow.id,
        other => panic!("workflow-start: {other:?}"),
    }
}

async fn launched(owner: &Connection, root: &Path) -> String {
    match request(
        owner,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("observed".to_owned()),
            prompt: None,
            clone: None,
        },
    )
    .await
    {
        Ok(Reply::Session { session }) => session.id,
        other => panic!("session-new: {other:?}"),
    }
}

fn brief_of(seen: &Seen) -> (&str, &str, &cockpit_protocol::ContextBudgetView) {
    match &seen.brief {
        Answer::Reply(Reply::AgentBrief {
            brief,
            manifest,
            budget,
        }) => (brief, manifest, budget),
        other => panic!("the brief was not answered: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_writes_the_bundle_at_start_and_names_it_to_agents() {
    let home = Home::new();
    let daemon = home.start().await;
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&owner).await;
    home.arm_helper(&daemon, None);

    launched(&owner, &root).await;
    let seen = home.seen().await;

    let manifest = std::fs::read_to_string(home.bundle_dir().join("manifest.json")).unwrap();
    assert!(
        manifest.contains(&cockpit_agent_context::manifest().id),
        "{manifest}"
    );
    assert_eq!(
        std::fs::read_to_string(home.bundle_dir().join("cockpit/SKILL.md")).unwrap(),
        cockpit_agent_context::documents()[0].file_text()
    );
    assert_eq!(
        seen.instructions.as_deref(),
        Some(home.bundle_dir().to_str().unwrap())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_doing_a_step_is_briefed_with_its_task_workspace_and_grants() {
    let home = Home::new();
    let daemon = home.start().await;
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&owner).await;
    let run = started_code_change(&owner).await;
    home.arm_helper(&daemon, Some(run));

    let session = launched(&owner, &root).await;
    let seen = home.seen().await;

    assert!(
        matches!(seen.begun, Some(Answer::Reply(Reply::Workflow { .. }))),
        "{:?}",
        seen.begun
    );
    let (brief, manifest, budget) = brief_of(&seen);
    for expected in [
        format!("Session {session} · project app"),
        format!("- Workspace: {}", root.display()),
        format!("workflow {run}, step implement (attempt 1, no checkpoint yet)"),
        format!("`cockpit workflow packet {run}`"),
        "change-state over this session".to_owned(),
        format!("instructions {manifest}"),
    ] {
        assert!(
            brief.contains(&expected),
            "missing {expected:?} in\n{brief}"
        );
    }
    assert_eq!(budget.brief_bytes, brief.len() as u64);
    assert!(
        budget.brief_bytes <= cockpit_agent_context::BRIEF_TARGET_BYTES as u64,
        "{budget:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_brief_asked_from_outside_the_checkout_still_names_the_sessions_project_and_step() {
    let home = Home::new();
    let daemon = home.start().await;
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&owner).await;
    let run = started_code_change(&owner).await;
    home.arm_helper_asking_from(&daemon, Some(run), Some(&home.path()));

    let session = launched(&owner, &root).await;
    let seen = home.seen().await;

    let (brief, _, _) = brief_of(&seen);
    assert!(
        brief.contains(&format!("Session {session} · project app"))
            && brief.contains(&format!("workflow {run}, step implement")),
        "{brief}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_without_a_step_is_told_where_to_find_one() {
    let home = Home::new();
    let daemon = home.start().await;
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&owner).await;
    started_code_change(&owner).await;
    home.arm_helper(&daemon, None);

    launched(&owner, &root).await;
    let seen = home.seen().await;

    let (brief, _, _) = brief_of(&seen);
    assert!(brief.contains("no workflow step yet"), "{brief}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_topic_is_not_found_naming_the_topics() {
    let home = Home::new();
    let daemon = home.start().await;
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    let refused = request(
        &owner,
        Command::InstructionsRead {
            topic: Some("teleport".to_owned()),
        },
    )
    .await;

    let Err(ClientError::Daemon(refusal)) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(refusal.code, ErrorCode::NotFound);
    assert!(
        refusal
            .unblock
            .unwrap()
            .action
            .contains("overview, workflow"),
        "the refusal names the topics"
    );
}

/// The fake agent's helper: run by the tests above from inside a launched agent's tree.
#[tokio::test]
async fn brief_agent() {
    let Some(socket) = std::env::var_os(AGENT_SOCKET) else {
        return;
    };
    let answer_path = PathBuf::from(std::env::var_os(AGENT_ANSWER).unwrap());
    let token = std::env::var(cockpit_protocol::LAUNCH_TOKEN_VARIABLE).ok();
    let agent = Connection::open(Path::new(&socket), GENEROUS)
        .await
        .unwrap();
    let answered = |outcome: Result<Reply, ClientError>| match outcome {
        Ok(reply) => Answer::Reply(reply),
        Err(ClientError::Daemon(refusal)) => Answer::Refused(refusal),
        Err(error) => panic!("the request failed: {error}"),
    };
    let begun = match std::env::var(AGENT_WORKFLOW) {
        Ok(workflow) => Some(answered(
            agent
                .request(
                    Command::WorkflowBegin {
                        launch_token: token.clone(),
                        workflow: workflow.parse().unwrap(),
                    },
                    GENEROUS,
                )
                .await,
        )),
        Err(_) => None,
    };
    if let Some(directory) = std::env::var_os(AGENT_BRIEF_FROM) {
        std::env::set_current_dir(directory).unwrap();
    }
    // Sent again, as `cockpit brief` does: a bound launch is known by its tree.
    let brief = answered(
        agent
            .request(
                Command::AgentBrief {
                    launch_token: token,
                },
                GENEROUS,
            )
            .await,
    );
    let seen = Seen {
        instructions: std::env::var("COCKPIT_INSTRUCTIONS").ok(),
        begun,
        brief,
    };
    let partial = answer_path.with_extension("partial");
    std::fs::write(&partial, serde_json::to_string(&seen).unwrap()).unwrap();
    std::fs::rename(partial, answer_path).unwrap();
}
