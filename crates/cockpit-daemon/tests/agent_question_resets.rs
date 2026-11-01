//! What the owner sees of a session's question and turn outcome, through the daemon binary's
//! sockets: `/clear` or an in-session `/resume` starts afresh (#213, #225, decided in #311), a
//! subagent's doing-now leaves a dialog open (#216), and the agent's own question waiting
//! under a dialog reaches the owner (#250). This test process plays the launched agent and its
//! Claude Code hook bridge.
#![cfg(target_os = "linux")]

mod common;

use cockpit_agent_channel::{LaunchToken, identity_of};
use cockpit_authorization::Permission;
use cockpit_protocol::{
    AgentReport, AgentState, Command, QuestionSource, Reply, SessionReport, TurnOutcome,
};
use common::{GENEROUS, RunningDaemon, TestHome};

/// Session `s000000000001` in Claude Code conversation `c-1`, launched from this test
/// process with `change-state`; the daemon runs.
async fn session_in_conversation() -> (TestHome, RunningDaemon, LaunchToken) {
    let home = TestHome::new();
    let token = home
        .launch(
            "s000000000001",
            &identity_of(std::process::id()).unwrap(),
            Some(Permission::ChangeState),
        )
        .await;
    let database = home.open_state().await;
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO sessions (id, provider, checkout_id, holds_checkout, working_dir,
                 conversation, state, revision, created_at, changed_at)
             VALUES ('s000000000001', 'claude', 1, 0, '/work', 'c-1', 'asleep', 1, 0, 0)",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    drop(database);
    let daemon = home.start();
    (home, daemon, token)
}

async fn send(home: &TestHome, token: &LaunchToken, report: AgentReport) {
    let reply = home
        .connect_agent()
        .await
        .request(
            Command::AgentReport {
                launch_token: Some(token.as_str().to_owned()),
                report: report.clone(),
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(reply, Ok(Reply::AgentReported { .. })),
        "{report:?}: expected the recorded report, got {reply:?}"
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

/// What the bridge sends for a hook event of `conversation`.
fn from(conversation: &str, report: AgentReport) -> AgentReport {
    AgentReport {
        conversation: Some(conversation.to_owned()),
        ..report
    }
}

fn own_question(question: &str) -> AgentReport {
    AgentReport {
        needs_input: Some(question.to_owned()),
        ..AgentReport::default()
    }
}

fn dialog(question: &str) -> AgentReport {
    AgentReport {
        state: Some(AgentState::Waiting),
        dialog: Some(question.to_owned()),
        ..AgentReport::default()
    }
}

/// `SessionEnd` of `/clear` or `/resume` for `c-1`, then `SessionStart` of `c-2`.
async fn clear(home: &TestHome, token: &LaunchToken) {
    let ending = AgentReport {
        conversation: Some("c-1".to_owned()),
        is_conversation_ending: true,
        ..AgentReport::default()
    };
    send(home, token, ending).await;
    let started = AgentReport {
        state: Some(AgentState::Waiting),
        ..AgentReport::default()
    };
    send(home, token, from("c-2", started)).await;
}

#[tokio::test]
async fn clear_takes_away_the_agents_question_from_the_previous_conversation() {
    let (home, _daemon, token) = session_in_conversation().await;
    send(&home, &token, from("c-1", own_question("Which database?"))).await;

    clear(&home, &token).await;

    let view = owner_view(&home).await;
    assert_eq!((view.needs_input, view.question_source), (None, None));
}

#[tokio::test]
async fn clear_takes_away_the_previous_conversations_failed_turn() {
    let (home, _daemon, token) = session_in_conversation().await;
    let failed = AgentReport {
        state: Some(AgentState::Waiting),
        outcome: Some(TurnOutcome::Failed { reason: None }),
        ..AgentReport::default()
    };
    send(&home, &token, from("c-1", failed)).await;

    clear(&home, &token).await;

    assert_eq!(owner_view(&home).await.outcome, None);
}

#[tokio::test]
async fn a_subagents_doing_leaves_the_dialog_in_the_owners_view() {
    let (home, _daemon, token) = session_in_conversation().await;
    send(&home, &token, from("c-1", dialog("Allow Bash?"))).await;
    let doing = AgentReport {
        doing: Some("Reading the parser tests".to_owned()),
        ..AgentReport::default()
    };

    send(&home, &token, doing).await;

    let view = owner_view(&home).await;
    assert_eq!(
        view.needs_input.as_deref().zip(view.question_source),
        Some(("Allow Bash?", QuestionSource::Dialog))
    );
}

#[tokio::test]
async fn the_owner_sees_the_agents_question_waiting_under_a_dialog() {
    let (home, _daemon, token) = session_in_conversation().await;
    send(&home, &token, from("c-1", own_question("Which database?"))).await;

    send(&home, &token, from("c-1", dialog("Allow Bash?"))).await;

    let view = owner_view(&home).await;
    assert_eq!(
        (
            view.needs_input.as_deref(),
            view.covered_question.as_deref()
        ),
        (Some("Allow Bash?"), Some("Which database?"))
    );
}
