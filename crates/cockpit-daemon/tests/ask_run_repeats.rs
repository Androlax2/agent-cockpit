//! Issue #1335: an agent that repeats an `ask run` while an identical one is pending gets that
//! one back, so the owner sees one prompt; a different ask, or a rerun after a decision, files
//! a new one. Driven through the daemon's agent socket and the owner's queue.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use cockpit_agent_channel::identity_of;
use cockpit_authorization::Permission;
use cockpit_client::Connection;
use cockpit_protocol::{Command, Reply, RunRequestView};
use common::{GENEROUS, TestHome};

struct Agent {
    connection: Connection,
    token: String,
    cwd: String,
}

impl Agent {
    async fn ask(&self, command: &str, cwd: &str, reason: &str) -> RunRequestView {
        let reply = self
            .connection
            .request(
                Command::AskRun {
                    launch_token: Some(self.token.clone()),
                    command: command.to_owned(),
                    cwd: cwd.to_owned(),
                    reason: reason.to_owned(),
                },
                GENEROUS,
            )
            .await
            .unwrap();
        let Reply::RunRequest { request } = reply else {
            panic!("expected a run request, got {reply:?}");
        };
        *request
    }
}

async fn queued_ids(owner: &Connection) -> Vec<i64> {
    match owner.request(Command::ApprovalQueue, GENEROUS).await {
        Ok(Reply::ApprovalQueue { runs, .. }) => runs.into_iter().map(|run| run.id).collect(),
        other => panic!("approval-queue answered {other:?}"),
    }
}

async fn start() -> (TestHome, impl Drop, Agent, Connection) {
    let home = TestHome::new();
    let token = home
        .launch(
            "s1",
            &identity_of(std::process::id()).unwrap(),
            Some(Permission::RunCommands),
        )
        .await;
    let cwd = std::fs::canonicalize(home.home()).unwrap();
    let daemon = home.start();
    let connection = home.connect_agent().await;
    let owner = home.connect().await;
    let agent = Agent {
        connection,
        token: token.as_str().to_owned(),
        cwd: cwd.to_str().unwrap().to_owned(),
    };
    (home, daemon, agent, owner)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_identical_ask_returns_the_first_id_and_the_owner_sees_one_prompt() {
    let (_home, _daemon, agent, owner) = start().await;

    let first = agent.ask("true", &agent.cwd, "The first reason.").await;
    let second = agent.ask("true", &agent.cwd, "A different reason.").await;

    assert!(!first.already_asked);
    assert!(second.already_asked);
    assert_eq!(second.id, first.id);
    assert_eq!(second.reason, "The first reason.");
    assert_eq!(queued_ids(&owner).await, vec![first.id]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_different_command_or_directory_files_a_new_ask() {
    let (home, _daemon, agent, owner) = start().await;
    let elsewhere = home.home().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let elsewhere = std::fs::canonicalize(elsewhere).unwrap();
    let first = agent.ask("true", &agent.cwd, "Reason.").await;

    let other_command = agent.ask("false", &agent.cwd, "Reason.").await;
    let other_directory = agent
        .ask("true", elsewhere.to_str().unwrap(), "Reason.")
        .await;

    assert!(!other_command.already_asked && !other_directory.already_asked);
    let ids = [first.id, other_command.id, other_directory.id];
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        3
    );
    assert_eq!(queued_ids(&owner).await.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_repeated_after_the_owner_decided_asks_again() {
    let (_home, _daemon, agent, owner) = start().await;
    let first = agent.ask("true", &agent.cwd, "Reason.").await;
    owner
        .request(
            Command::ApprovalDecline {
                request: first.id,
                through: None,
            },
            GENEROUS,
        )
        .await
        .unwrap();

    let rerun = agent.ask("true", &agent.cwd, "Reason.").await;

    assert!(!rerun.already_asked);
    assert_ne!(rerun.id, first.id);
    assert_eq!(queued_ids(&owner).await, vec![rerun.id]);
}
