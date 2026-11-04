//! Issue #710: the agent's own question in a session a coordinator started waits on that
//! coordinator first, and on the owner once it escalates, once its time is over, or once the
//! coordinator ended or lost `message-sessions`; the coordinator's message answers it. Driven
//! through `report`, `question_holder`, `escalate_question` and `answer_question` on a real
//! state database, the clock given.
#![cfg(target_os = "linux")]

#[allow(dead_code, reason = "each test file uses part of the shared fixtures")]
mod common;

use cockpit_authorization::{Permission, Scope};
use cockpit_sessions::{
    AgentState, QUESTION_HOLD_SECONDS, Report, SessionReport, answer_question, escalate_question,
    latest_report, question_holder, report,
};
use cockpit_turso::Database;
use common::{grant, launch, open, session, this_process, this_process_identity};

const WORKER: &str = "s0000000000a1";
const COORDINATOR: &str = "s0000000000c1";
const QUESTION: &str = "Which branch should I base the fix on?";

/// A worker `COORDINATOR` started, launched once, bound to this test process and granted
/// `change-state`;
/// the coordinator runs and holds `message-sessions` over every session, as the owner's
/// `session grant` gives it.
async fn started_worker(dir: &std::path::Path, started_by: Option<&str>) -> Database {
    let database = open(dir).await;
    let write = database.write().await.unwrap();
    for (id, starter) in [(COORDINATOR, None), (WORKER, started_by)] {
        write
            .execute(
                "INSERT INTO sessions (id, provider, checkout_id, holds_checkout, working_dir,
                     state, revision, created_at, changed_at, started_by)
                 VALUES (?1, 'claude', 1, 0, '/work', 'running', 1, 0, 0, ?2)",
                (id, starter),
            )
            .await
            .unwrap();
    }
    write
        .execute(
            "INSERT INTO session_launches (session_id, kind, execution_id, launched_at)
             VALUES (?1, 'start', 1, 0)",
            [WORKER],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    let token = launch(&database, &session(WORKER), &this_process_identity()).await;
    let write = database.write().await.unwrap();
    cockpit_agent_channel::identify(&write, &this_process(), Some(token.as_str()))
        .await
        .unwrap();
    cockpit_authorization::grant(
        &write,
        &session(COORDINATOR),
        Permission::MessageSessions,
        &Scope::Owner,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    grant(&database, &session(WORKER), Permission::ChangeState).await;
    database
}

async fn send(database: &Database, sent: Report) -> SessionReport {
    let write = database.write().await.unwrap();
    let recorded = report(&write, &this_process(), None, sent).await.unwrap();
    write.commit().await.unwrap();
    recorded
}

fn asking(question: &str) -> Report {
    Report {
        needs_input: Some(question.to_owned()),
        ..Report::default()
    }
}

async fn holder_at(database: &Database, report: &SessionReport, now: i64) -> Option<String> {
    let read = database.read().await.unwrap();
    question_holder(&read, report, now)
        .await
        .unwrap()
        .map(|coordinator| coordinator.as_str().to_owned())
}

#[tokio::test]
async fn the_coordinator_that_started_a_session_holds_its_question() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), Some(COORDINATOR)).await;

    let asked = send(&database, asking(QUESTION)).await;

    let asked_at = asked
        .question_asked_at
        .expect("the question's time is recorded");
    assert_eq!(
        holder_at(&database, &asked, asked_at).await.as_deref(),
        Some(COORDINATOR)
    );
}

#[tokio::test]
async fn a_question_in_a_session_the_owner_started_waits_on_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), None).await;

    let asked = send(&database, asking(QUESTION)).await;

    assert_eq!(holder_at(&database, &asked, asked.reported_at).await, None);
}

#[tokio::test]
async fn a_question_reaches_the_owner_once_its_time_with_the_coordinator_is_over() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), Some(COORDINATOR)).await;
    let asked = send(&database, asking(QUESTION)).await;
    let asked_at = asked.question_asked_at.unwrap();

    let just_before = holder_at(&database, &asked, asked_at + QUESTION_HOLD_SECONDS - 1).await;
    let at_the_end = holder_at(&database, &asked, asked_at + QUESTION_HOLD_SECONDS).await;

    assert_eq!(just_before.as_deref(), Some(COORDINATOR));
    assert_eq!(at_the_end, None);
}

#[tokio::test]
async fn an_escalated_question_waits_on_the_owner_and_stays_there_when_asked_again() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), Some(COORDINATOR)).await;
    let asked = send(&database, asking(QUESTION)).await;
    let write = database.write().await.unwrap();
    let has_question = escalate_question(&write, &session(WORKER), asked.reported_at)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let repeated = send(&database, asking(QUESTION)).await;

    assert!(has_question);
    assert!(repeated.question_escalated_at.is_some(), "{repeated:?}");
    assert_eq!(
        holder_at(&database, &repeated, repeated.reported_at).await,
        None
    );
}

#[tokio::test]
async fn a_new_question_after_an_escalation_goes_to_the_coordinator_again() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), Some(COORDINATOR)).await;
    send(&database, asking(QUESTION)).await;
    let write = database.write().await.unwrap();
    escalate_question(&write, &session(WORKER), 0)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let asked = send(&database, asking("Squash or merge?")).await;

    assert_eq!(
        holder_at(&database, &asked, asked.reported_at)
            .await
            .as_deref(),
        Some(COORDINATOR)
    );
}

#[tokio::test]
async fn escalating_without_a_question_of_the_agents_own_reports_none() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), Some(COORDINATOR)).await;
    send(
        &database,
        Report {
            state: Some(AgentState::Working),
            ..Report::default()
        },
    )
    .await;

    let write = database.write().await.unwrap();
    let has_question = escalate_question(&write, &session(WORKER), 0)
        .await
        .unwrap();

    assert!(!has_question);
}

#[tokio::test]
async fn a_dialog_waits_on_the_owner_even_in_a_session_a_coordinator_started() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), Some(COORDINATOR)).await;
    send(&database, asking(QUESTION)).await;

    let dialog = send(
        &database,
        Report {
            dialog: Some("Allow Bash?".to_owned()),
            ..Report::default()
        },
    )
    .await;

    assert_eq!(
        holder_at(&database, &dialog, dialog.reported_at).await,
        None
    );
}

#[tokio::test]
async fn a_question_reaches_the_owner_once_the_coordinator_ended() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), Some(COORDINATOR)).await;
    let asked = send(&database, asking(QUESTION)).await;
    let write = database.write().await.unwrap();
    write
        .execute(
            "UPDATE sessions SET state = 'ended' WHERE id = ?1",
            [COORDINATOR],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(holder_at(&database, &asked, asked.reported_at).await, None);
}

#[tokio::test]
async fn a_question_reaches_the_owner_once_the_coordinator_lost_message_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), Some(COORDINATOR)).await;
    let asked = send(&database, asking(QUESTION)).await;
    let write = database.write().await.unwrap();
    let held = cockpit_authorization::grant(
        &write,
        &session(COORDINATOR),
        Permission::MessageSessions,
        &Scope::Owner,
    )
    .await
    .unwrap();
    cockpit_authorization::revoke(&write, held.id)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(holder_at(&database, &asked, asked.reported_at).await, None);
}

async fn answered_by(database: &Database, from: &str) -> (bool, Option<SessionReport>) {
    let write = database.write().await.unwrap();
    let cleared = answer_question(&write, &session(WORKER), &session(from))
        .await
        .unwrap();
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();
    (
        cleared,
        latest_report(&read, &session(WORKER)).await.unwrap(),
    )
}

#[tokio::test]
async fn the_starting_coordinators_message_clears_the_question() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), Some(COORDINATOR)).await;
    send(&database, asking(QUESTION)).await;

    let (cleared, after) = answered_by(&database, COORDINATOR).await;

    assert!(cleared);
    let after = after.expect("the report stays");
    assert_eq!(after.needs_input, None);
    assert_eq!(after.question_asked_at, None);
}

#[tokio::test]
async fn another_sessions_message_leaves_the_question_open() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), None).await;
    send(&database, asking(QUESTION)).await;

    let (cleared, after) = answered_by(&database, COORDINATOR).await;

    assert!(!cleared);
    assert_eq!(after.unwrap().needs_input.as_deref(), Some(QUESTION));
}

#[tokio::test]
async fn the_coordinators_message_clears_the_question_under_a_dialog_and_keeps_the_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), Some(COORDINATOR)).await;
    send(&database, asking(QUESTION)).await;
    send(
        &database,
        Report {
            dialog: Some("Allow Bash?".to_owned()),
            ..Report::default()
        },
    )
    .await;

    let (cleared, after) = answered_by(&database, COORDINATOR).await;

    assert!(cleared);
    let after = after.unwrap();
    assert_eq!(after.needs_input.as_deref(), Some("Allow Bash?"));
    assert_eq!(after.covered_question, None);
}

#[tokio::test]
async fn a_question_from_an_earlier_launch_waits_on_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), Some(COORDINATOR)).await;
    let asked = send(&database, asking(QUESTION)).await;
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO session_launches (session_id, kind, execution_id, launched_at)
             VALUES (?1, 'restart', 2, 0)",
            [WORKER],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(holder_at(&database, &asked, asked.reported_at).await, None);
}

#[tokio::test]
async fn a_question_reaches_the_owner_once_the_coordinator_sleeps() {
    let dir = tempfile::tempdir().unwrap();
    let database = started_worker(dir.path(), Some(COORDINATOR)).await;
    let asked = send(&database, asking(QUESTION)).await;
    let write = database.write().await.unwrap();
    write
        .execute(
            "UPDATE sessions SET state = 'asleep' WHERE id = ?1",
            [COORDINATOR],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(holder_at(&database, &asked, asked.reported_at).await, None);
}
