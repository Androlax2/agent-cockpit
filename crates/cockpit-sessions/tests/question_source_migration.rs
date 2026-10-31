//! `cockpit-sessions` migration 4 (#182) rebuilds `session_reports` with `question_source`:
//! every column of a version-3 row survives, and a question recorded before becomes the
//! agent's own.

use cockpit_authorization::SessionId;
use cockpit_provider_api::TurnOutcome;
use cockpit_sessions::{AgentState, QuestionSource, SessionReport, latest_reports};
use cockpit_turso::{Database, MigrationSet};

const UPGRADED: [MigrationSet; 3] = [
    cockpit_authorization::MIGRATIONS,
    cockpit_agent_channel::MIGRATIONS,
    cockpit_sessions::MIGRATIONS,
];

#[tokio::test]
async fn a_version_3_report_keeps_every_column_and_its_question_becomes_the_agents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let version_3 = [
        cockpit_authorization::MIGRATIONS,
        cockpit_agent_channel::MIGRATIONS,
        MigrationSet {
            owner: cockpit_sessions::MIGRATIONS.owner,
            migrations: &cockpit_sessions::MIGRATIONS.migrations[..3],
        },
    ];
    let database = Database::open(&path, &version_3).await.unwrap();
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO session_reports (session, state, label, doing, needs_input,
                 reported_at, launch_id, outcome, outcome_reason)
             VALUES ('s1', 'waiting', 'Ship it', 'writing the tests', 'Ship it?', 42, 7,
                 'failed', 'overloaded'),
                 ('s2', 'working', NULL, NULL, NULL, 43, 8, NULL, NULL)",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    drop(database);

    let database = Database::open(&path, &UPGRADED).await.unwrap();

    let reports = latest_reports(&database.read().await.unwrap())
        .await
        .unwrap();
    assert_eq!(
        reports,
        vec![
            SessionReport {
                session: SessionId::new("s1").unwrap(),
                state: Some(AgentState::Waiting),
                label: Some("Ship it".to_owned()),
                doing: Some("writing the tests".to_owned()),
                needs_input: Some("Ship it?".to_owned()),
                question_source: Some(QuestionSource::Agent),
                covered_question: None,
                outcome: Some(TurnOutcome::Failed {
                    reason: Some("overloaded".to_owned()),
                }),
                reported_at: 42,
                launch: Some(7),
            },
            SessionReport {
                session: SessionId::new("s2").unwrap(),
                state: Some(AgentState::Working),
                label: None,
                doing: None,
                needs_input: None,
                question_source: None,
                covered_question: None,
                outcome: None,
                reported_at: 43,
                launch: Some(8),
            },
        ]
    );
}

#[tokio::test]
async fn a_question_without_its_source_is_refused_by_the_schema() {
    let dir = tempfile::tempdir().unwrap();
    let database = Database::open(&dir.path().join("state.db"), &UPGRADED)
        .await
        .unwrap();
    let write = database.write().await.unwrap();

    let inserted = write
        .execute(
            "INSERT INTO session_reports (session, needs_input, reported_at)
             VALUES ('s1', 'Ship it?', 42)",
            (),
        )
        .await;

    assert!(inserted.is_err(), "{inserted:?}");
}
