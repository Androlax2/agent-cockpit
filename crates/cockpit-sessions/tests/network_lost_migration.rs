//! `cockpit-sessions` migration 23 (#718) rebuilds `session_reports` to admit the
//! `network-lost` outcome: every column of a version-22 row survives, and a lost network
//! without the harness's word for it is refused by the schema.

use cockpit_authorization::SessionId;
use cockpit_provider_api::{ContextUsage, TurnOutcome};
use cockpit_sessions::{AgentState, QuestionSource, SessionReport, latest_reports};
use cockpit_turso::{Database, MigrationSet};

const UPGRADED: [MigrationSet; 3] = [
    cockpit_authorization::MIGRATIONS,
    cockpit_agent_channel::MIGRATIONS,
    cockpit_sessions::MIGRATIONS,
];

#[tokio::test]
async fn a_version_22_report_keeps_every_column() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let version_22 = [
        cockpit_authorization::MIGRATIONS,
        cockpit_agent_channel::MIGRATIONS,
        MigrationSet {
            owner: cockpit_sessions::MIGRATIONS.owner,
            migrations: &cockpit_sessions::MIGRATIONS.migrations[..22],
        },
    ];
    let database = Database::open(&path, &version_22).await.unwrap();
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO session_reports (session, state, label, doing, needs_input,
                 reported_at, launch_id, outcome, outcome_reason, question_source,
                 covered_question, question_asked_at, question_escalated_at, context_tokens,
                 context_window)
             VALUES ('s1', 'waiting', 'Ship it', 'writing the tests', 'Allow Bash?', 42, 7,
                 'failed', 'overloaded', 'dialog', 'Ship it?', 40, 41, 45123, 200000)",
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
        vec![SessionReport {
            session: SessionId::new("s1").unwrap(),
            state: Some(AgentState::Waiting),
            label: Some("Ship it".to_owned()),
            doing: Some("writing the tests".to_owned()),
            needs_input: Some("Allow Bash?".to_owned()),
            question_source: Some(QuestionSource::Dialog),
            covered_question: Some("Ship it?".to_owned()),
            outcome: Some(TurnOutcome::Failed {
                reason: Some("overloaded".to_owned()),
            }),
            reported_at: 42,
            launch: Some(7),
            question_asked_at: Some(40),
            question_escalated_at: Some(41),
            context: Some(ContextUsage {
                tokens: 45123,
                window: Some(200000),
            }),
        }]
    );
}

#[tokio::test]
async fn a_lost_network_without_its_reason_is_refused_by_the_schema() {
    let dir = tempfile::tempdir().unwrap();
    let database = Database::open(&dir.path().join("state.db"), &UPGRADED)
        .await
        .unwrap();
    let write = database.write().await.unwrap();

    let inserted = write
        .execute(
            "INSERT INTO session_reports (session, state, reported_at, outcome)
             VALUES ('s1', 'waiting', 42, 'network-lost')",
            (),
        )
        .await;

    assert!(inserted.is_err(), "{inserted:?}");
}
