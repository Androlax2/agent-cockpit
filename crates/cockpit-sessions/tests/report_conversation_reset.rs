//! A new conversation in the same launch starts afresh (#213, #225): once the session follows
//! the conversation that replaced an ended one (`/clear`, an in-session `/resume`, #219), the
//! previous conversation's question, dialog and turn outcome are gone, as after a new launch.
//! Driven through `report` on a real state database.
#![cfg(target_os = "linux")]

#[allow(dead_code, reason = "each test file uses part of the shared fixtures")]
mod common;

use cockpit_authorization::Permission;
use cockpit_provider_api::TurnOutcome;
use cockpit_sessions::{AgentState, Error, QuestionSource, Report, SessionReport, report};
use cockpit_turso::Database;
use common::{grant, launch, open, session, this_process, this_process_identity};

/// Session `s000000000001`, bound to this process, granted `change-state`, holding conversation
/// `conversation` (`None`: the provider never named one).
async fn session_in(dir: &std::path::Path, conversation: Option<&str>) -> Database {
    let database = open(dir).await;
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO sessions (id, provider, checkout_id, holds_checkout, working_dir,
                 conversation, state, revision, created_at, changed_at)
             VALUES ('s000000000001', 'claude', 1, 0, '/work', ?1, 'asleep', 1, 0, 0)",
            [conversation],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    let token = launch(
        &database,
        &session("s000000000001"),
        &this_process_identity(),
    )
    .await;
    let write = database.write().await.unwrap();
    cockpit_agent_channel::identify(&write, &this_process(), Some(token.as_str()))
        .await
        .unwrap();
    write.commit().await.unwrap();
    grant(
        &database,
        &session("s000000000001"),
        Permission::ChangeState,
    )
    .await;
    database
}

async fn send(database: &Database, sent: Report) -> Result<SessionReport, Error> {
    let write = database.write().await.unwrap();
    let recorded = report(&write, &this_process(), None, sent).await;
    write.commit().await.unwrap();
    recorded
}

/// What the bridge sends for `report` from Claude Code conversation `conversation`.
fn from(conversation: &str, report: Report) -> Report {
    Report {
        conversation: Some(conversation.to_owned()),
        ..report
    }
}

/// What the bridge sends on `SessionEnd` of `/clear` or an in-session `/resume`.
fn ending(conversation: &str) -> Report {
    Report {
        conversation: Some(conversation.to_owned()),
        is_conversation_ending: true,
        ..Report::default()
    }
}

/// What the bridge sends on `SessionStart` and on the idle notification.
fn waiting() -> Report {
    Report {
        state: Some(AgentState::Waiting),
        ..Report::default()
    }
}

fn own_question(question: &str) -> Report {
    Report {
        needs_input: Some(question.to_owned()),
        ..Report::default()
    }
}

fn dialog(question: &str) -> Report {
    Report {
        state: Some(AgentState::Waiting),
        dialog: Some(question.to_owned()),
        ..Report::default()
    }
}

fn failed_turn() -> Report {
    Report {
        state: Some(AgentState::Waiting),
        outcome: Some(TurnOutcome::Failed {
            reason: Some("overloaded".to_owned()),
        }),
        ..Report::default()
    }
}

/// `before` from conversation `c-1`, then `/clear`: `c-1` ends and `c-2` starts.
async fn cleared_after(dir: &std::path::Path, before: Report) -> SessionReport {
    let database = session_in(dir, Some("c-1")).await;
    send(&database, from("c-1", before)).await.unwrap();
    send(&database, ending("c-1")).await.unwrap();
    send(&database, from("c-2", waiting())).await.unwrap()
}

#[tokio::test]
async fn a_new_conversation_clears_the_agents_question() {
    let dir = tempfile::tempdir().unwrap();

    let started = cleared_after(dir.path(), own_question("Which database?")).await;

    assert_eq!(started.needs_input, None);
    assert_eq!(started.question_source, None);
}

#[tokio::test]
async fn a_new_conversation_clears_the_last_turns_outcome() {
    let dir = tempfile::tempdir().unwrap();

    let started = cleared_after(dir.path(), failed_turn()).await;

    assert_eq!(started.outcome, None);
    assert_eq!(started.state, Some(AgentState::Waiting));
}

#[tokio::test]
async fn a_new_conversation_clears_a_dialog_and_the_question_beneath_it() {
    let dir = tempfile::tempdir().unwrap();
    let database = session_in(dir.path(), Some("c-1")).await;
    send(&database, from("c-1", own_question("Which database?")))
        .await
        .unwrap();
    send(&database, from("c-1", dialog("Allow Bash?")))
        .await
        .unwrap();
    send(&database, ending("c-1")).await.unwrap();

    let started = send(&database, from("c-2", waiting())).await.unwrap();

    assert_eq!(
        (started.needs_input, started.covered_question),
        (None, None)
    );
}

#[tokio::test]
async fn a_new_conversation_keeps_the_sessions_label() {
    let dir = tempfile::tempdir().unwrap();
    let labelled = Report {
        label: Some("Ship the parser".to_owned()),
        ..own_question("Which database?")
    };

    let started = cleared_after(dir.path(), labelled).await;

    assert_eq!(started.label.as_deref(), Some("Ship the parser"));
}

#[tokio::test]
async fn the_new_conversation_keeps_a_question_it_asks_itself() {
    let dir = tempfile::tempdir().unwrap();
    let database = session_in(dir.path(), Some("c-1")).await;
    send(&database, ending("c-1")).await.unwrap();
    send(&database, from("c-2", own_question("Which branch?")))
        .await
        .unwrap();

    let idle = send(&database, from("c-2", waiting())).await.unwrap();

    assert_eq!(
        idle.needs_input.as_deref().zip(idle.question_source),
        Some(("Which branch?", QuestionSource::Agent))
    );
}

#[tokio::test]
async fn the_same_conversation_after_an_announced_end_keeps_the_question() {
    let dir = tempfile::tempdir().unwrap();
    let database = session_in(dir.path(), Some("c-1")).await;
    send(&database, from("c-1", own_question("Which database?")))
        .await
        .unwrap();
    send(&database, ending("c-1")).await.unwrap();

    let same = send(&database, from("c-1", waiting())).await.unwrap();

    assert_eq!(same.needs_input.as_deref(), Some("Which database?"));
}

#[tokio::test]
async fn a_conversation_named_for_the_first_time_keeps_the_question() {
    let dir = tempfile::tempdir().unwrap();
    let database = session_in(dir.path(), None).await;
    send(&database, own_question("Which database?"))
        .await
        .unwrap();

    let named = send(&database, from("c-1", waiting())).await.unwrap();

    assert_eq!(named.needs_input.as_deref(), Some("Which database?"));
}
