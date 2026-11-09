//! A turn's outcome through the daemon binary's sockets (#183): an agent reports how its turn
//! ended on the agent socket, and the owner reads it back. This test process plays the agent.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use cockpit_agent_channel::identity_of;
use cockpit_authorization::Permission;
use cockpit_protocol::{AgentReport, AgentState, Command, Reply, TurnOutcome};
use common::{GENEROUS, TestHome};

/// Reports `outcome` as the agent of a launch on the agent socket; what the owner then reads
/// back of the session.
async fn reported_and_read_back(outcome: TurnOutcome) -> (Option<AgentState>, Option<TurnOutcome>) {
    let home = TestHome::new();
    let token = home
        .launch(
            "s1",
            &identity_of(std::process::id()).unwrap(),
            Some(Permission::ChangeState),
        )
        .await;
    let _daemon = home.start();

    let reply = home
        .connect_agent()
        .await
        .request(
            Command::AgentReport {
                launch_token: Some(token.as_str().to_owned()),
                report: AgentReport {
                    outcome: Some(outcome),
                    ..AgentReport::default()
                },
            },
            GENEROUS,
        )
        .await;

    assert!(
        matches!(reply, Ok(Reply::AgentReported { .. })),
        "{reply:?}"
    );
    let reports = match home
        .connect()
        .await
        .request(Command::AgentReports, GENEROUS)
        .await
    {
        Ok(Reply::AgentReports { reports }) => reports,
        other => panic!("expected the reports, got {other:?}"),
    };
    (reports[0].state, reports[0].outcome.clone())
}

#[tokio::test]
async fn a_failed_turn_reaches_the_owner_with_its_reason() {
    let failed = TurnOutcome::Failed {
        reason: Some("server_error".to_owned()),
    };

    assert_eq!(
        reported_and_read_back(failed.clone()).await,
        (Some(AgentState::Waiting), Some(failed))
    );
}

/// #718: a lost network is its own outcome, never shown as a failure.
#[tokio::test]
async fn a_network_lost_turn_reaches_the_owner_with_its_reason() {
    let network_lost = TurnOutcome::NetworkLost {
        reason: "connection_failed".to_owned(),
    };

    assert_eq!(
        reported_and_read_back(network_lost.clone()).await,
        (Some(AgentState::Waiting), Some(network_lost))
    );
}
