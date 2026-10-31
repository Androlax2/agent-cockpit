//! Issue #182 through the daemon's sockets: a dialog's question does not outlive the dialog,
//! and the agent's own question waits for the owner's next prompt (decided in #200). This
//! test process plays the launched agent and its provider bridge.
#![cfg(target_os = "linux")]

mod common;

use cockpit_agent_channel::{LaunchToken, ProcessIdentity, identity_of};
use cockpit_authorization::Permission;
use cockpit_protocol::{AgentReport, AgentState, Command, QuestionSource, Reply, SessionReport};
use common::{GENEROUS, TestHome};

fn this_process() -> ProcessIdentity {
    identity_of(std::process::id()).unwrap()
}

async fn send(home: &TestHome, token: &LaunchToken, report: AgentReport) -> SessionReport {
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
    match reply {
        Ok(Reply::AgentReported { report }) => *report,
        other => panic!("expected the recorded report, got {other:?}"),
    }
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

fn waiting() -> AgentReport {
    AgentReport {
        state: Some(AgentState::Waiting),
        ..AgentReport::default()
    }
}

#[tokio::test]
async fn an_answered_dialog_followed_by_a_turn_end_leaves_no_question() {
    let home = TestHome::new();
    let token = home
        .launch("s1", &this_process(), Some(Permission::ChangeState))
        .await;
    let _daemon = home.start();
    let dialog = AgentReport {
        state: Some(AgentState::Waiting),
        dialog: Some("Allow Bash?".to_owned()),
        ..AgentReport::default()
    };
    let asked = send(&home, &token, dialog).await;
    assert_eq!(asked.question_source, Some(QuestionSource::Dialog));

    send(&home, &token, waiting()).await;

    let report = owner_view(&home).await;
    assert_eq!(report.needs_input, None);
    assert_eq!(report.question_source, None);
}

#[tokio::test]
async fn the_agents_own_question_stays_open_until_the_next_prompt() {
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
    let tool_used = AgentReport {
        state: Some(AgentState::Working),
        ..AgentReport::default()
    };
    send(&home, &token, tool_used).await;
    send(&home, &token, waiting()).await;
    let after_turn_end = owner_view(&home).await;
    assert_eq!(
        after_turn_end.needs_input.as_deref(),
        Some("Which database?")
    );
    assert_eq!(after_turn_end.question_source, Some(QuestionSource::Agent));
    let prompt = AgentReport {
        state: Some(AgentState::Working),
        is_new_prompt: true,
        ..AgentReport::default()
    };

    send(&home, &token, prompt).await;

    assert_eq!(owner_view(&home).await.needs_input, None);
}
