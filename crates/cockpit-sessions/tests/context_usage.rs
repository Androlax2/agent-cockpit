//! The harness's own context at a turn's end (#479), on a real state database: it comes with
//! the turn's outcome, the session keeps the latest one, and a new launch starts without it.
#![cfg(target_os = "linux")]

#[expect(
    dead_code,
    reason = "the real-process fixtures serve tests/reports.rs; this file reports as itself only"
)]
mod common;

use cockpit_agent_channel::Refusal;
use cockpit_authorization::Permission;
use cockpit_provider_api::{ContextUsage, TurnOutcome};
use cockpit_sessions::{
    AgentState, Error, Report, SessionReport, current_context, latest_report, report,
};
use cockpit_turso::Database;
use common::{grant, launch, open, session, this_process, this_process_identity};

/// A database with session `s1`, in launch 1, bound to this test process and allowed to
/// report.
async fn reporting_session(dir: &std::path::Path) -> Database {
    let database = open(dir).await;
    let token = launch(&database, &session("s1"), &this_process_identity()).await;
    let write = database.write().await.unwrap();
    cockpit_agent_channel::identify(&write, &this_process(), Some(token.as_str()))
        .await
        .unwrap();
    write.commit().await.unwrap();
    grant(&database, &session("s1"), Permission::ChangeState).await;
    relaunch(&database, 1).await;
    database
}

/// Records launch `id` of session `s1`, as a restart, wake or resume does.
async fn relaunch(database: &Database, id: i64) {
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO session_launches (id, session_id, kind, execution_id, launched_at)
             VALUES (?1, 's1', 'restart', ?1, 0)",
            [id],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
}

async fn send(database: &Database, sent: Report) -> Result<SessionReport, Error> {
    let write = database.write().await.unwrap();
    let recorded = report(&write, &this_process(), None, sent).await;
    write.commit().await.unwrap();
    recorded
}

fn usage(tokens: u64, window: Option<u64>) -> ContextUsage {
    ContextUsage { tokens, window }
}

/// The report a bridge sends when a turn ends with the harness's context.
fn turn_ended_with(context: ContextUsage) -> Report {
    Report {
        outcome: Some(TurnOutcome::Completed),
        context: Some(context),
        ..Report::default()
    }
}

async fn current(database: &Database) -> Option<ContextUsage> {
    current_context(&database.read().await.unwrap(), &session("s1"))
        .await
        .unwrap()
}

#[tokio::test]
async fn a_turns_end_records_the_harness_context_with_its_window() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;

    send(&database, turn_ended_with(usage(45_123, Some(258_400))))
        .await
        .unwrap();

    assert_eq!(current(&database).await, Some(usage(45_123, Some(258_400))));
}

#[tokio::test]
async fn a_context_without_a_window_reads_back_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;

    send(&database, turn_ended_with(usage(45_123, None)))
        .await
        .unwrap();

    assert_eq!(current(&database).await, Some(usage(45_123, None)));
}

#[tokio::test]
async fn the_next_turns_end_replaces_the_context() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    send(&database, turn_ended_with(usage(45_123, None)))
        .await
        .unwrap();

    send(&database, turn_ended_with(usage(61_000, None)))
        .await
        .unwrap();

    assert_eq!(current(&database).await, Some(usage(61_000, None)));
}

#[tokio::test]
async fn a_report_without_a_context_keeps_the_last_turns() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    send(&database, turn_ended_with(usage(45_123, None)))
        .await
        .unwrap();

    send(
        &database,
        Report {
            state: Some(AgentState::Working),
            is_new_prompt: true,
            ..Report::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(current(&database).await, Some(usage(45_123, None)));
}

#[tokio::test]
async fn a_session_whose_turn_never_ended_has_no_context() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    send(
        &database,
        Report {
            state: Some(AgentState::Waiting),
            ..Report::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(current(&database).await, None);
}

#[tokio::test]
async fn a_new_launch_has_no_context_before_its_first_turn_ends() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    send(&database, turn_ended_with(usage(45_123, None)))
        .await
        .unwrap();

    relaunch(&database, 2).await;
    let before_any_report = current(&database).await;
    send(
        &database,
        Report {
            state: Some(AgentState::Waiting),
            ..Report::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(before_any_report, None);
    assert_eq!(current(&database).await, None);
}

fn refused_field(recorded: Result<SessionReport, Error>) -> &'static str {
    match recorded {
        Err(Error::AgentChannel(cockpit_agent_channel::Error::Refused(
            Refusal::InvalidRequest { field, .. },
        ))) => field,
        other => panic!("expected an invalid request, got {other:?}"),
    }
}

#[tokio::test]
async fn a_malformed_context_is_refused_and_records_nothing() {
    let without_outcome = Report {
        state: Some(AgentState::Waiting),
        context: Some(usage(45_123, None)),
        ..Report::default()
    };
    for (sent, expected_field) in [
        (without_outcome, "context"),
        (
            Report {
                context: Some(usage(45_123, None)),
                ..Report::default()
            },
            "context",
        ),
        (turn_ended_with(usage(45_123, Some(0))), "context-window"),
        (turn_ended_with(usage(u64::MAX, None)), "context-tokens"),
        (turn_ended_with(usage(1, Some(u64::MAX))), "context-window"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let database = reporting_session(dir.path()).await;

        assert_eq!(
            refused_field(send(&database, sent.clone()).await),
            expected_field,
            "{sent:?}"
        );
        let stored = latest_report(&database.read().await.unwrap(), &session("s1"))
            .await
            .unwrap();
        assert_eq!(stored, None, "{sent:?}");
    }
}
