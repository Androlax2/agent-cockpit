//! A report that names no state leaves an open dialog open (#216): with parallel tool calls
//! or a subagent, a label, a doing-now or the agent's own question can arrive while the
//! permission dialog still blocks the agent. Only a report that says what the agent does (a
//! state, a turn's outcome) or a new dialog answers it (#200, #214). Driven through `report`
//! on a real state database.
#![cfg(target_os = "linux")]

#[allow(dead_code, reason = "each test file uses part of the shared fixtures")]
mod common;

use cockpit_authorization::Permission;
use cockpit_provider_api::TurnOutcome;
use cockpit_sessions::{AgentState, Error, QuestionSource, Report, SessionReport, report};
use cockpit_turso::Database;
use common::{grant, launch, open, session, this_process, this_process_identity};

/// Session `s1`, bound to this process, granted `change-state`, with "Allow Bash?" open.
async fn dialog_open(dir: &std::path::Path) -> Database {
    let database = open(dir).await;
    let token = launch(&database, &session("s1"), &this_process_identity()).await;
    let write = database.write().await.unwrap();
    cockpit_agent_channel::identify(&write, &this_process(), Some(token.as_str()))
        .await
        .unwrap();
    write.commit().await.unwrap();
    grant(&database, &session("s1"), Permission::ChangeState).await;
    let dialog = Report {
        state: Some(AgentState::Waiting),
        dialog: Some("Allow Bash?".to_owned()),
        ..Report::default()
    };
    send(&database, dialog).await.unwrap();
    database
}

async fn send(database: &Database, sent: Report) -> Result<SessionReport, Error> {
    let write = database.write().await.unwrap();
    let recorded = report(&write, &this_process(), None, sent).await;
    write.commit().await.unwrap();
    recorded
}

fn shown(report: &SessionReport) -> Option<(&str, QuestionSource)> {
    report.needs_input.as_deref().zip(report.question_source)
}

#[tokio::test]
async fn a_subagents_doing_leaves_the_dialog_open() {
    let dir = tempfile::tempdir().unwrap();
    let database = dialog_open(dir.path()).await;
    let doing = Report {
        doing: Some("Reading the parser tests".to_owned()),
        ..Report::default()
    };

    let recorded = send(&database, doing).await.unwrap();

    assert_eq!(
        (shown(&recorded), recorded.state),
        (
            Some(("Allow Bash?", QuestionSource::Dialog)),
            Some(AgentState::Waiting)
        )
    );
    assert_eq!(recorded.doing.as_deref(), Some("Reading the parser tests"));
}

#[tokio::test]
async fn a_label_leaves_the_dialog_open() {
    let dir = tempfile::tempdir().unwrap();
    let database = dialog_open(dir.path()).await;
    let label = Report {
        label: Some("Ship the parser".to_owned()),
        ..Report::default()
    };

    let recorded = send(&database, label).await.unwrap();

    assert_eq!(
        shown(&recorded),
        Some(("Allow Bash?", QuestionSource::Dialog))
    );
}

#[tokio::test]
async fn the_agents_question_during_a_dialog_waits_beneath_it() {
    let dir = tempfile::tempdir().unwrap();
    let database = dialog_open(dir.path()).await;
    let question = Report {
        needs_input: Some("Which database?".to_owned()),
        ..Report::default()
    };

    let recorded = send(&database, question).await.unwrap();

    assert_eq!(
        (shown(&recorded), recorded.covered_question.as_deref()),
        (
            Some(("Allow Bash?", QuestionSource::Dialog)),
            Some("Which database?")
        )
    );
}

#[tokio::test]
async fn a_report_with_a_state_answers_the_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let database = dialog_open(dir.path()).await;
    let working = Report {
        state: Some(AgentState::Working),
        ..Report::default()
    };

    let recorded = send(&database, working).await.unwrap();

    assert_eq!(shown(&recorded), None);
}

#[tokio::test]
async fn a_turns_outcome_answers_the_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let database = dialog_open(dir.path()).await;
    let ended = Report {
        outcome: Some(TurnOutcome::Completed),
        ..Report::default()
    };

    let recorded = send(&database, ended).await.unwrap();

    assert_eq!(shown(&recorded), None);
}
