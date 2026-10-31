//! What clears a session's open question, by where it came from (#182, decided in #200): a
//! dialog the provider bridge saw clears on the next report of any kind; the agent's own
//! question only on a new prompt or a new launch. Driven through `report` on a real state
//! database.
#![cfg(target_os = "linux")]

#[allow(dead_code, reason = "each test file uses part of the shared fixtures")]
mod common;

use cockpit_agent_channel::Refusal;
use cockpit_authorization::Permission;
use cockpit_sessions::{AgentState, Error, QuestionSource, Report, SessionReport, report};
use cockpit_turso::Database;
use common::{grant, launch, open, session, this_process, this_process_identity};

/// A database with session `s1` bound to this test process and granted `change-state`.
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

fn new_prompt() -> Report {
    Report {
        state: Some(AgentState::Working),
        is_new_prompt: true,
        ..Report::default()
    }
}

fn label(text: &str) -> Report {
    Report {
        label: Some(text.to_owned()),
        ..Report::default()
    }
}

/// The reports the bridge and the agent send after a question, other than a new prompt.
fn later_reports() -> [Report; 3] {
    [
        state(AgentState::Working),
        state(AgentState::Waiting),
        label("Agent channel issue"),
    ]
}

#[tokio::test]
async fn a_question_records_where_it_came_from() {
    for (sent, source) in [
        (dialog("Allow Bash?"), QuestionSource::Dialog),
        (own_question("Which database?"), QuestionSource::Agent),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let database = reporting_session(dir.path()).await;

        let recorded = send(&database, sent).await.unwrap();

        assert_eq!(recorded.question_source, Some(source));
        assert_eq!(recorded.state, Some(AgentState::Waiting));
    }
}

#[tokio::test]
async fn a_dialog_question_clears_on_the_next_report() {
    for later in later_reports() {
        let dir = tempfile::tempdir().unwrap();
        let database = reporting_session(dir.path()).await;
        send(&database, dialog("Allow Bash?")).await.unwrap();

        let recorded = send(&database, later.clone()).await.unwrap();

        assert_eq!(recorded.needs_input, None, "after {later:?}");
        assert_eq!(recorded.question_source, None, "after {later:?}");
    }
}

#[tokio::test]
async fn the_agents_own_question_survives_tool_use_turn_end_and_other_reports() {
    for later in later_reports() {
        let dir = tempfile::tempdir().unwrap();
        let database = reporting_session(dir.path()).await;
        send(&database, own_question("Which database?"))
            .await
            .unwrap();

        let recorded = send(&database, later.clone()).await.unwrap();

        assert_eq!(
            recorded.needs_input.as_deref(),
            Some("Which database?"),
            "after {later:?}"
        );
        assert_eq!(
            recorded.question_source,
            Some(QuestionSource::Agent),
            "after {later:?}"
        );
    }
}

#[tokio::test]
async fn a_new_prompt_clears_the_agents_own_question() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    send(&database, own_question("Which database?"))
        .await
        .unwrap();

    let recorded = send(&database, new_prompt()).await.unwrap();

    assert_eq!(recorded.needs_input, None);
    assert_eq!(recorded.question_source, None);
    assert_eq!(recorded.state, Some(AgentState::Working));
}

#[tokio::test]
async fn a_new_launch_clears_the_agents_own_question() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    send(&database, own_question("Which database?"))
        .await
        .unwrap();
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

    assert_eq!(recorded.needs_input, None);
    assert_eq!(recorded.question_source, None);
}

#[tokio::test]
async fn a_dialog_replaces_the_agents_own_question() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    send(&database, own_question("Which database?"))
        .await
        .unwrap();

    let recorded = send(&database, dialog("Allow Bash?")).await.unwrap();

    assert_eq!(recorded.needs_input.as_deref(), Some("Allow Bash?"));
    assert_eq!(recorded.question_source, Some(QuestionSource::Dialog));
}

#[tokio::test]
async fn a_report_mixing_questions_or_a_prompt_is_refused_naming_its_field() {
    let cases: [(Report, &str); 4] = [
        (
            Report {
                needs_input: Some("Which database?".to_owned()),
                dialog: Some("Allow Bash?".to_owned()),
                ..Report::default()
            },
            "dialog",
        ),
        (
            Report {
                state: Some(AgentState::Waiting),
                is_new_prompt: true,
                ..Report::default()
            },
            "state",
        ),
        (
            Report {
                is_new_prompt: true,
                needs_input: Some("Which database?".to_owned()),
                ..Report::default()
            },
            "prompt",
        ),
        (
            Report {
                dialog: Some("?".repeat(301)),
                ..Report::default()
            },
            "dialog",
        ),
    ];
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    for (sent, expected_field) in cases {
        let refused = send(&database, sent.clone()).await;

        match refused {
            Err(Error::AgentChannel(cockpit_agent_channel::Error::Refused(
                Refusal::InvalidRequest { field, .. },
            ))) => assert_eq!(field, expected_field, "{sent:?}"),
            other => panic!("{sent:?}: expected an invalid request, got {other:?}"),
        }
    }
}
