//! Issue #214 through the daemon's sockets: a dialog arriving while the agent's own question
//! is open shows to the owner, and the agent's question comes back once the dialog is
//! answered. This test process plays the launched agent and its provider bridge.
#![cfg(target_os = "linux")]

mod common;

use cockpit_agent_channel::{LaunchToken, ProcessIdentity, identity_of};
use cockpit_authorization::Permission;
use cockpit_protocol::{AgentReport, AgentState, Command, QuestionSource, Reply, SessionReport};
use common::{GENEROUS, TestHome};

fn this_process() -> ProcessIdentity {
    identity_of(std::process::id()).unwrap()
}

async fn send(home: &TestHome, token: &LaunchToken, report: AgentReport) {
    let reply = home
        .connect_agent()
        .await
        .request(
            Command::AgentReport {
                launch_token: Some(token.as_str().to_owned()),
                report,
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(reply, Ok(Reply::AgentReported { .. })),
        "expected the recorded report, got {reply:?}"
    );
}

async fn owner_view(home: &TestHome) -> SessionReport {
    match home
        .connect()
        .await
        .request(Command::AgentReports, GENEROUS)
        .await
    {
        Ok(Reply::AgentReports { mut reports }) if reports.len() == 1 => reports.remove(0),
        other => panic!("expected one report, got {other:?}"),
    }
}

#[tokio::test]
async fn an_answered_dialog_gives_the_owner_back_the_agents_own_question() {
    let home = TestHome::new();
    let token = home
        .launch("s1", &this_process(), Some(Permission::ChangeState))
        .await;
    let _daemon = home.start();
    let question = AgentReport {
        needs_input: Some("Which database?".to_owned()),
        ..AgentReport::default()
    };
    send(&home, &token, question).await;
    let dialog = AgentReport {
        state: Some(AgentState::Waiting),
        dialog: Some("Allow Bash?".to_owned()),
        ..AgentReport::default()
    };
    send(&home, &token, dialog).await;
    let during_dialog = owner_view(&home).await;
    assert_eq!(during_dialog.needs_input.as_deref(), Some("Allow Bash?"));
    assert_eq!(during_dialog.question_source, Some(QuestionSource::Dialog));
    let answered = AgentReport {
        state: Some(AgentState::Working),
        ..AgentReport::default()
    };

    send(&home, &token, answered).await;

    let after_dialog = owner_view(&home).await;
    assert_eq!(after_dialog.needs_input.as_deref(), Some("Which database?"));
    assert_eq!(after_dialog.question_source, Some(QuestionSource::Agent));
}
