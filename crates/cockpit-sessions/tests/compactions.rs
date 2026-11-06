//! A context compaction a harness announced (#782), on a real state database: one record per
//! compaction with its trigger, sizes and seconds; it changes nothing of the agent's report,
//! and a compaction of an earlier launch is not shown.
#![cfg(target_os = "linux")]

#[expect(
    dead_code,
    reason = "the real-process fixtures serve tests/reports.rs; this file reports as itself only"
)]
mod common;

use cockpit_authorization::Permission;
use cockpit_provider_api::{CompactionMoment, CompactionPhase, CompactionTrigger, TurnOutcome};
use cockpit_sessions::{
    AgentState, Compaction, CompactionStatus, Error, Report, SessionReport, latest_compaction,
    latest_report, report,
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

fn moment(phase: CompactionPhase) -> CompactionMoment {
    CompactionMoment {
        phase,
        trigger: None,
        tokens_before: None,
        tokens_after: None,
    }
}

async fn compact(database: &Database, moment: CompactionMoment) -> Result<SessionReport, Error> {
    send(
        database,
        Report {
            compaction: Some(moment),
            ..Report::default()
        },
    )
    .await
}

async fn latest(database: &Database) -> Option<Compaction> {
    latest_compaction(&database.read().await.unwrap(), &session("s1"))
        .await
        .unwrap()
}

#[tokio::test]
async fn a_started_and_ended_compaction_is_one_record_with_trigger_tokens_and_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;

    compact(
        &database,
        CompactionMoment {
            trigger: Some(CompactionTrigger::Auto),
            tokens_before: Some(268_364),
            ..moment(CompactionPhase::Started)
        },
    )
    .await
    .unwrap();
    assert_eq!(
        latest(&database).await.unwrap().status,
        CompactionStatus::Running
    );
    compact(
        &database,
        CompactionMoment {
            tokens_after: Some(30_470),
            ..moment(CompactionPhase::Ended)
        },
    )
    .await
    .unwrap();

    let recorded = latest(&database).await.unwrap();
    assert_eq!(recorded.status, CompactionStatus::Completed);
    assert_eq!(recorded.trigger, Some(CompactionTrigger::Auto));
    assert_eq!(recorded.tokens_before, Some(268_364));
    assert_eq!(recorded.tokens_after, Some(30_470));
    assert!(recorded.seconds.is_some(), "{recorded:?}");
}

#[tokio::test]
async fn a_compaction_changes_nothing_of_the_agents_report() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    let working = send(
        &database,
        Report {
            state: Some(AgentState::Working),
            doing: Some("Editing the parser".to_owned()),
            ..Report::default()
        },
    )
    .await
    .unwrap();

    compact(&database, moment(CompactionPhase::Started))
        .await
        .unwrap();
    compact(&database, moment(CompactionPhase::Ended))
        .await
        .unwrap();

    let after = latest_report(&database.read().await.unwrap(), &session("s1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, working);
}

#[tokio::test]
async fn a_compaction_keeps_the_turn_outcome_it_finds() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    send(
        &database,
        Report {
            outcome: Some(TurnOutcome::Completed),
            ..Report::default()
        },
    )
    .await
    .unwrap();

    let recorded = compact(&database, moment(CompactionPhase::Started))
        .await
        .unwrap();

    assert_eq!(recorded.outcome, Some(TurnOutcome::Completed));
    assert_eq!(recorded.state, Some(AgentState::Waiting));
}

#[tokio::test]
async fn a_failed_compaction_is_recorded_as_failed() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    compact(&database, moment(CompactionPhase::Started))
        .await
        .unwrap();

    compact(&database, moment(CompactionPhase::Failed))
        .await
        .unwrap();

    assert_eq!(
        latest(&database).await.unwrap().status,
        CompactionStatus::Failed
    );
}

#[tokio::test]
async fn a_start_while_another_compaction_runs_fails_the_unfinished_one() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    compact(&database, moment(CompactionPhase::Started))
        .await
        .unwrap();
    compact(&database, moment(CompactionPhase::Started))
        .await
        .unwrap();
    compact(&database, moment(CompactionPhase::Ended))
        .await
        .unwrap();

    let write = database.write().await.unwrap();
    let mut rows = write
        .query("SELECT status FROM session_compactions ORDER BY id", ())
        .await
        .unwrap();
    let mut statuses = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        statuses.push(row.get::<String>(0).unwrap());
    }
    drop(rows);
    write.commit().await.unwrap();

    assert_eq!(statuses, ["failed", "completed"]);
}

#[tokio::test]
async fn an_end_whose_start_was_lost_is_recorded_without_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;

    compact(
        &database,
        CompactionMoment {
            tokens_after: Some(30_470),
            ..moment(CompactionPhase::Ended)
        },
    )
    .await
    .unwrap();

    let recorded = latest(&database).await.unwrap();
    assert_eq!(recorded.status, CompactionStatus::Completed);
    assert_eq!(recorded.tokens_after, Some(30_470));
    assert_eq!(recorded.seconds, None);
}

#[tokio::test]
async fn a_new_launch_does_not_show_the_last_launchs_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;
    compact(&database, moment(CompactionPhase::Started))
        .await
        .unwrap();
    relaunch(&database, 2).await;

    assert_eq!(latest(&database).await, None);
}

#[tokio::test]
async fn a_compaction_reported_with_other_fields_is_refused_naming_its_field() {
    let dir = tempfile::tempdir().unwrap();
    let database = reporting_session(dir.path()).await;

    let refused = send(
        &database,
        Report {
            state: Some(AgentState::Working),
            compaction: Some(moment(CompactionPhase::Started)),
            ..Report::default()
        },
    )
    .await
    .unwrap_err();

    assert!(refused.to_string().contains("compaction"), "{refused}");
    assert_eq!(latest(&database).await, None);
}
