//! A dialog stacks on the agent's own open question instead of replacing it (#214): the
//! dialog shows while open, and the agent's question shows again once the dialog clears.
//! Each source keeps its clearing rule (#182, decided in #200). Driven through `report` on a
//! real state database.
#![cfg(target_os = "linux")]

#[allow(dead_code, reason = "each test file uses part of the shared fixtures")]
mod common;

use cockpit_authorization::Permission;
use cockpit_sessions::{AgentState, Error, QuestionSource, Report, SessionReport, report};
use cockpit_turso::Database;
use common::{grant, launch, open, session, this_process, this_process_identity};

async fn reporting_session(dir: &std::path::Path) -> Database {
    let database = open(dir).await;
    let token = launch(&database, &session("s1"), &this_process_identity()).await;
    let write = database.write().await.unwrap();
    cockpit_agent_channel::identify(&write, &this_process(), Some(token.as_str()))
        .await
        .unwrap();
    write.commit().await.unwrap();
    grant(&database, &session("s1"), Permission::ChangeState).await;
    database
}

async fn send(database: &Database, sent: Report) -> Result<SessionReport, Error> {
    let write = database.write().await.unwrap();
    let recorded = report(&write, &this_process(), None, sent).await;
    write.commit().await.unwrap();
    recorded
}

fn dialog(question: &str) -> Report {
    Report {
        state: Some(AgentState::Waiting),
        dialog: Some(question.to_owned()),
        ..Report::default()
    }
}

fn own_question(question: &str) -> Report {
    Report {
        needs_input: Some(question.to_owned()),
        ..Report::default()
    }
}

fn state(state: AgentState) -> Report {
    Report {
        state: Some(state),
        ..Report::default()
    }
}

fn shown(report: &SessionReport) -> Option<(&str, QuestionSource)> {
    report.needs_input.as_deref().zip(report.question_source)
}

/// A session whose agent asked "Which database?" and then hit a permission dialog.
async fn dialog_over_own_question(dir: &std::path::Path) -> Database {
    let database = reporting_session(dir).await;
    send(&database, own_question("Which database?"))
        .await
        .unwrap();
    send(&database, dialog("Allow Bash?")).await.unwrap();
    database
}

#[tokio::test]
async fn a_dialog_over_the_agents_question_shows_while_open() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    send(&database, own_question("Which database?"))
        .await
        .unwrap();

    let recorded = send(&database, dialog("Allow Bash?")).await.unwrap();

    assert_eq!(
        shown(&recorded),
        Some(("Allow Bash?", QuestionSource::Dialog))
    );
    assert_eq!(
        recorded.covered_question.as_deref(),
        Some("Which database?")
    );
}

#[tokio::test]
async fn the_agents_question_shows_again_once_the_dialog_clears() {
    let answered_reports = [state(AgentState::Working), state(AgentState::Waiting)];
    for answered in answered_reports {
        let dir = tempfile::tempdir().unwrap();
        let database = dialog_over_own_question(dir.path()).await;

        let recorded = send(&database, answered.clone()).await.unwrap();

        assert_eq!(
            shown(&recorded),
            Some(("Which database?", QuestionSource::Agent)),
            "after {answered:?}"
        );
        assert_eq!(recorded.covered_question, None, "after {answered:?}");
    }
}

#[tokio::test]
async fn the_agents_question_under_a_dialog_survives_a_daemon_reading_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let database = dialog_over_own_question(dir.path()).await;
    drop(database);
    let database = open(dir.path()).await;

    let recorded = send(&database, state(AgentState::Waiting)).await.unwrap();

    assert_eq!(
        shown(&recorded),
        Some(("Which database?", QuestionSource::Agent))
    );
}

#[tokio::test]
async fn a_second_dialog_keeps_the_agents_question_beneath() {
    let dir = tempfile::tempdir().unwrap();
    let database = dialog_over_own_question(dir.path()).await;

    let recorded = send(&database, dialog("Allow Edit?")).await.unwrap();

    assert_eq!(
        shown(&recorded),
        Some(("Allow Edit?", QuestionSource::Dialog))
    );
    assert_eq!(
        recorded.covered_question.as_deref(),
        Some("Which database?")
    );
}

/// A stateless report leaves the dialog open (#216), so the new question waits beneath it.
#[tokio::test]
async fn a_new_question_from_the_agent_replaces_its_own_beneath_the_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let database = dialog_over_own_question(dir.path()).await;

    let recorded = send(&database, own_question("Which schema?"))
        .await
        .unwrap();

    assert_eq!(
        shown(&recorded),
        Some(("Allow Bash?", QuestionSource::Dialog))
    );
    assert_eq!(recorded.covered_question.as_deref(), Some("Which schema?"));
}

#[tokio::test]
async fn a_new_prompt_during_a_dialog_clears_both_questions() {
    let dir = tempfile::tempdir().unwrap();
    let database = dialog_over_own_question(dir.path()).await;

    let recorded = send(
        &database,
        Report {
            state: Some(AgentState::Working),
            is_new_prompt: true,
            ..Report::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(shown(&recorded), None);
    assert_eq!(recorded.covered_question, None);
}

#[tokio::test]
async fn a_new_launch_clears_the_agents_question_under_a_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let database = dialog_over_own_question(dir.path()).await;
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO session_launches (session_id, kind, execution_id, launched_at)
             VALUES ('s1', 'restart', 1, 0)",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();

    let recorded = send(&database, state(AgentState::Waiting)).await.unwrap();

    assert_eq!(shown(&recorded), None);
    assert_eq!(recorded.covered_question, None);
}

#[tokio::test]
async fn a_dialog_alone_covers_no_question() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;

    let recorded = send(&database, dialog("Allow Bash?")).await.unwrap();

    assert_eq!(recorded.covered_question, None);
}

#[tokio::test]
async fn the_schema_refuses_a_covered_question_without_an_open_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let write = database.write().await.unwrap();

    let inserted = write
        .execute(
            "INSERT INTO session_reports
                 (session, needs_input, question_source, covered_question, reported_at)
             VALUES ('s1', 'Which database?', 'agent', 'Which schema?', 42)",
            (),
        )
        .await;

    assert!(inserted.is_err(), "{inserted:?}");
}
