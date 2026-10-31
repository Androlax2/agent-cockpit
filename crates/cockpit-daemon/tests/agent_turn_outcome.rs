//! A turn's outcome through the daemon binary's sockets (#183): an agent reports how its turn
//! ended on the agent socket, and the owner reads it back. This test process plays the agent.
#![cfg(target_os = "linux")]

mod common;

use cockpit_agent_channel::identity_of;
use cockpit_authorization::Permission;
use cockpit_protocol::{AgentReport, AgentState, Command, Reply, TurnOutcome};
use common::{GENEROUS, TestHome};

#[tokio::test]
async fn a_failed_turn_reaches_the_owner_with_its_reason() {
    let home = TestHome::new();
    let token = home
        .launch(
            "s1",
            &identity_of(std::process::id()).unwrap(),
            Some(Permission::ChangeState),
        )
        .await;
    let _daemon = home.start();
    let failed = TurnOutcome::Failed {
        reason: Some("server_error".to_owned()),
    };

    let reply = home
        .connect_agent()
        .await
        .request(
            Command::AgentReport {
                launch_token: Some(token.as_str().to_owned()),
                report: AgentReport {
                    outcome: Some(failed.clone()),
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
    assert_eq!(reports[0].state, Some(AgentState::Waiting));
    assert_eq!(reports[0].outcome, Some(failed));
}
