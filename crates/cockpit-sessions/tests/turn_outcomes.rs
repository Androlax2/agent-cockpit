//! A turn's outcome on a session's report (#183), on a real state database: it marks the
//! agent waiting, a new turn clears it, and the stored outcome reads back as reported.
#![cfg(target_os = "linux")]

#[expect(
    dead_code,
    reason = "the real-process fixtures serve tests/reports.rs; this file reports as itself only"
)]
mod common;

use cockpit_agent_channel::Refusal;
use cockpit_authorization::Permission;
use cockpit_provider_api::TurnOutcome;
use cockpit_sessions::{AgentState, Error, Report, SessionReport, latest_reports, report};
use cockpit_turso::Database;
use common::{grant, launch, open, session, this_process, this_process_identity};

/// A database with session `s1` bound to this test process and allowed to report.
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

fn ended(outcome: TurnOutcome) -> Report {
    Report {
        outcome: Some(outcome),
        ..Report::default()
    }
}

fn state(state: AgentState) -> Report {
    Report {
        state: Some(state),
        ..Report::default()
    }
}

#[tokio::test]
async fn each_outcome_reads_back_as_reported() {
    let outcomes = [
        TurnOutcome::Completed,
        TurnOutcome::QuotaStopped,
        TurnOutcome::Failed { reason: None },
        TurnOutcome::Failed {
            reason: Some("server_error".to_owned()),
        },
    ];
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    for outcome in outcomes {
        send(&database, ended(outcome.clone())).await.unwrap();

        let reports = latest_reports(&database.read().await.unwrap())
            .await
            .unwrap();

        assert_eq!(reports[0].outcome, Some(outcome));
    }
}

#[tokio::test]
async fn an_outcome_marks_a_working_session_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    send(&database, state(AgentState::Working)).await.unwrap();

    let recorded = send(&database, ended(TurnOutcome::Completed))
        .await
        .unwrap();

    assert_eq!(recorded.state, Some(AgentState::Waiting));
}

#[tokio::test]
async fn reporting_working_clears_the_last_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    send(&database, ended(TurnOutcome::QuotaStopped))
        .await
        .unwrap();

    let recorded = send(&database, state(AgentState::Working)).await.unwrap();

    assert_eq!(recorded.outcome, None);
}

#[tokio::test]
async fn waiting_without_an_outcome_keeps_the_last_one() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    send(&database, ended(TurnOutcome::Completed))
        .await
        .unwrap();

    let recorded = send(&database, state(AgentState::Waiting)).await.unwrap();

    assert_eq!(recorded.outcome, Some(TurnOutcome::Completed));
}

#[tokio::test]
async fn an_invalid_outcome_is_refused_naming_its_field() {
    let failed = |reason: &str| {
        ended(TurnOutcome::Failed {
            reason: Some(reason.to_owned()),
        })
    };
    let cases: [(Report, &str); 5] = [
        (
            Report {
                state: Some(AgentState::Working),
                outcome: Some(TurnOutcome::Completed),
                ..Report::default()
            },
            "state",
        ),
        (failed(""), "outcome-reason"),
        (failed(&"x".repeat(81)), "outcome-reason"),
        (failed("server\u{1b}[31m"), "outcome-reason"),
        (failed("#(rm -rf ~)"), "outcome-reason"),
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

#[tokio::test]
async fn a_report_recorded_before_outcomes_reads_back_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let before_outcomes = [
        cockpit_authorization::MIGRATIONS,
        cockpit_agent_channel::MIGRATIONS,
        cockpit_turso::MigrationSet {
            owner: cockpit_sessions::MIGRATIONS.owner,
            migrations: &cockpit_sessions::MIGRATIONS.migrations[..2],
        },
    ];
    let database = Database::open(&path, &before_outcomes).await.unwrap();
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO session_reports (session, state, reported_at) VALUES ('s1', 'waiting', 42)",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    drop(database);

    let database = open(dir.path()).await;

    let reports = latest_reports(&database.read().await.unwrap())
        .await
        .unwrap();
    assert_eq!(reports[0].state, Some(AgentState::Waiting));
    assert_eq!(reports[0].outcome, None);
}
