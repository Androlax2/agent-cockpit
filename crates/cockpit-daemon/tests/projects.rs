//! Issue #28 at the daemon's sockets: project and checkout commands are the owner's alone, so
//! a sandboxed agent cannot register a project, take a checkout, sweep or push.
#![cfg(target_os = "linux")]

mod common;

use cockpit_client::ClientError;
use cockpit_protocol::{Command, ErrorCode, UnblockedBy};
use common::{GENEROUS, TestHome};

fn project_commands() -> Vec<Command> {
    vec![
        Command::ProjectAdd {
            path: "/tmp/app".to_owned(),
            name: None,
        },
        Command::ProjectList,
        Command::CheckoutList { project: None },
        Command::CheckoutClone {
            project: "app".to_owned(),
            branch: "feature".to_owned(),
        },
        Command::CheckoutReview {
            project: "app".to_owned(),
            pull_request: 1,
        },
        Command::CheckoutClaim {
            checkout: 1,
            holder: "agent".to_owned(),
        },
        Command::CheckoutRelease {
            checkout: 1,
            holder: "agent".to_owned(),
        },
        Command::CheckoutSweep { dry_run: true },
        Command::CheckoutPush { checkout: 1 },
    ]
}

#[tokio::test]
async fn the_agent_socket_refuses_every_project_command() {
    let home = TestHome::new();
    let _daemon = home.start();
    let agent = home.connect_agent().await;

    for command in project_commands() {
        let refusal = match agent.request(command.clone(), GENEROUS).await {
            Err(ClientError::Daemon(error)) => error,
            other => panic!("{command:?}: expected a refusal, got {other:?}"),
        };
        assert_eq!(refusal.code, ErrorCode::NotAuthorized, "{command:?}");
        assert_eq!(
            refusal.unblock.map(|unblock| unblock.by),
            Some(UnblockedBy::Owner),
            "{command:?}"
        );
    }
}
