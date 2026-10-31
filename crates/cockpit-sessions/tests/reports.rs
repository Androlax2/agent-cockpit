//! The `report` verb's rules on a real state database: validation, merging, and
//! `cockpit-authorization` deciding for the session the agent channel identified. Moved with
//! the reports from `cockpit-agent-channel` (#146).
#![cfg(target_os = "linux")]

mod common;

use cockpit_agent_channel::{PeerAncestry, Refusal, UnblockedBy};
use cockpit_authorization::{Permission, Scope};
use cockpit_sessions::{AgentState, Error, Report, SessionReport, latest_reports, report};
use cockpit_turso::Database;
use common::{Process, grant, launch, open, session, this_process, this_process_identity};

/// A database with session `s1` bound to this test process.
async fn bound_session(dir: &std::path::Path) -> Database {
    let database = open(dir).await;
    let token = launch(&database, &session("s1"), &this_process_identity()).await;
    let write = database.write().await.unwrap();
    cockpit_agent_channel::identify(&write, &this_process(), Some(token.as_str()))
        .await
        .unwrap();
    write.commit().await.unwrap();
    database
}

async fn send(
    database: &Database,
    peer: &PeerAncestry,
    sent: Report,
) -> Result<SessionReport, Error> {
    let write = database.write().await.unwrap();
    let recorded = report(&write, peer, None, sent).await;
    write.commit().await.unwrap();
    recorded
}

fn doing(text: &str) -> Report {
    Report {
        doing: Some(text.to_owned()),
        ..Report::default()
    }
}

#[tokio::test]
async fn a_session_without_a_grant_is_refused_with_the_grant_that_unblocks_it() {
    let dir = tempfile::tempdir().unwrap();
    let database = bound_session(dir.path()).await;

    let refused = send(&database, &this_process(), doing("writing the tests")).await;

    let Err(Error::AgentChannel(cockpit_agent_channel::Error::Refused(refusal))) = refused else {
        panic!("expected a refusal, got {refused:?}");
    };
    assert_eq!(
        refusal,
        Refusal::NotAuthorized(cockpit_authorization::Refusal::NoGrant {
            permission: Permission::ChangeState,
            scope: Scope::Session(session("s1")),
        })
    );
    let unblock = refusal.unblock();
    assert_eq!(unblock.by, UnblockedBy::Owner);
    assert_eq!(
        unblock.action,
        "the owner grants change-state over session s1"
    );
}

#[tokio::test]
async fn a_granted_report_is_recorded_for_the_identified_session() {
    let dir = tempfile::tempdir().unwrap();
    let database = bound_session(dir.path()).await;
    grant(&database, &session("s1"), Permission::ChangeState).await;

    send(&database, &this_process(), doing("writing   the tests"))
        .await
        .unwrap();

    let reports = latest_reports(&database.read().await.unwrap())
        .await
        .unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].session, session("s1"));
    assert_eq!(reports[0].doing.as_deref(), Some("writing the tests"));
}

#[tokio::test]
async fn a_nested_process_reports_with_its_parent_sessions_grants() {
    let dir = tempfile::tempdir().unwrap();
    let database = bound_session(dir.path()).await;
    grant(&database, &session("s1"), Permission::ChangeState).await;
    let subagent = Process::sleeper();

    let recorded = send(&database, &subagent.ancestry(), doing("reviewing the diff")).await;

    assert_eq!(recorded.unwrap().session, session("s1"));
}

#[tokio::test]
async fn revoking_the_grant_refuses_the_nested_process_too() {
    let dir = tempfile::tempdir().unwrap();
    let database = bound_session(dir.path()).await;
    let write = database.write().await.unwrap();
    let granted = cockpit_authorization::grant(
        &write,
        &session("s1"),
        Permission::ChangeState,
        &Scope::Session(session("s1")),
    )
    .await
    .unwrap();
    cockpit_authorization::revoke(&write, granted.id)
        .await
        .unwrap();
    write.commit().await.unwrap();
    let subagent = Process::sleeper();

    let refused = send(&database, &subagent.ancestry(), doing("reviewing the diff")).await;

    assert!(
        matches!(
            refused,
            Err(Error::AgentChannel(cockpit_agent_channel::Error::Refused(
                Refusal::NotAuthorized(_)
            )))
        ),
        "{refused:?}"
    );
}

#[tokio::test]
async fn fields_left_out_keep_their_previous_values() {
    let dir = tempfile::tempdir().unwrap();
    let database = bound_session(dir.path()).await;
    grant(&database, &session("s1"), Permission::ChangeState).await;
    let label = Report {
        label: Some("Agent channel issue".to_owned()),
        ..Report::default()
    };
    send(&database, &this_process(), label).await.unwrap();

    let recorded = send(&database, &this_process(), doing("writing the tests")).await;

    let recorded = recorded.unwrap();
    assert_eq!(recorded.label.as_deref(), Some("Agent channel issue"));
    assert_eq!(recorded.doing.as_deref(), Some("writing the tests"));
}

#[tokio::test]
async fn reporting_working_clears_the_open_question() {
    let dir = tempfile::tempdir().unwrap();
    let database = bound_session(dir.path()).await;
    grant(&database, &session("s1"), Permission::ChangeState).await;
    let question = Report {
        state: Some(AgentState::Waiting),
        needs_input: Some("Start #32 now?".to_owned()),
        ..Report::default()
    };
    send(&database, &this_process(), question).await.unwrap();
    let working = Report {
        state: Some(AgentState::Working),
        ..Report::default()
    };

    let recorded = send(&database, &this_process(), working).await.unwrap();

    assert_eq!(recorded.state, Some(AgentState::Working));
    assert_eq!(recorded.needs_input, None);
}

#[tokio::test]
async fn a_question_marks_a_working_session_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let database = bound_session(dir.path()).await;
    grant(&database, &session("s1"), Permission::ChangeState).await;
    send(&database, &this_process(), doing("writing the tests"))
        .await
        .unwrap();
    let question = Report {
        needs_input: Some("Which database?".to_owned()),
        ..Report::default()
    };

    let recorded = send(&database, &this_process(), question).await.unwrap();

    assert_eq!(recorded.state, Some(AgentState::Waiting));
}

#[tokio::test]
async fn a_plain_hash_sign_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let database = bound_session(dir.path()).await;
    grant(&database, &session("s1"), Permission::ChangeState).await;
    let label = Report {
        label: Some("Agent channel issue #32".to_owned()),
        ..Report::default()
    };

    let recorded = send(&database, &this_process(), label).await.unwrap();

    assert_eq!(recorded.label.as_deref(), Some("Agent channel issue #32"));
}

#[tokio::test]
async fn an_invalid_report_is_refused_naming_its_field() {
    let cases: [(Report, &str); 10] = [
        (Report::default(), "report"),
        (doing("one"), "doing"),
        (doing(&"word ".repeat(13)), "doing"),
        (doing("two\u{7}words"), "doing"),
        (
            Report {
                label: Some("x".repeat(61)),
                ..Report::default()
            },
            "label",
        ),
        (
            Report {
                label: Some("#(rm -rf ~)".to_owned()),
                ..Report::default()
            },
            "label",
        ),
        (
            Report {
                needs_input: Some("?".repeat(301)),
                ..Report::default()
            },
            "needs-input",
        ),
        (
            Report {
                label: Some("build #[fg=red,blink]".to_owned()),
                ..Report::default()
            },
            "label",
        ),
        (doing("reversed \u{202E}txet"), "doing"),
        (
            Report {
                state: Some(AgentState::Working),
                needs_input: Some("Which database?".to_owned()),
                ..Report::default()
            },
            "state",
        ),
    ];
    let dir = tempfile::tempdir().unwrap();
    let database = bound_session(dir.path()).await;
    grant(&database, &session("s1"), Permission::ChangeState).await;
    for (sent, expected_field) in cases {
        let refused = send(&database, &this_process(), sent.clone()).await;

        match refused {
            Err(Error::AgentChannel(cockpit_agent_channel::Error::Refused(
                Refusal::InvalidRequest { field, .. },
            ))) => {
                assert_eq!(field, expected_field, "{sent:?}");
            }
            other => panic!("{sent:?}: expected an invalid request, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn an_unidentified_sender_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;

    let refused = send(&database, &this_process(), doing("writing the tests")).await;

    assert!(
        matches!(
            refused,
            Err(Error::AgentChannel(cockpit_agent_channel::Error::Refused(
                Refusal::Unidentified
            )))
        ),
        "{refused:?}"
    );
    let reports = latest_reports(&database.read().await.unwrap())
        .await
        .unwrap();
    assert_eq!(reports, Vec::new());
}

#[tokio::test]
async fn a_report_recorded_before_the_move_survives_the_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let before_the_move = [
        cockpit_authorization::MIGRATIONS,
        cockpit_agent_channel::MIGRATIONS,
        cockpit_turso::MigrationSet {
            owner: cockpit_sessions::MIGRATIONS.owner,
            migrations: &cockpit_sessions::MIGRATIONS.migrations[..1],
        },
    ];
    let database = Database::open(&path, &before_the_move).await.unwrap();
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO agent_channel_reports
                 (session, state, label, doing, needs_input, reported_at)
             VALUES ('s1', 'waiting', 'Reports move', 'writing the tests', 'Ship it?', 42)",
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
    assert_eq!(
        reports,
        vec![SessionReport {
            session: session("s1"),
            state: Some(AgentState::Waiting),
            label: Some("Reports move".to_owned()),
            doing: Some("writing the tests".to_owned()),
            needs_input: Some("Ship it?".to_owned()),
            reported_at: 42,
            launch: None,
        }]
    );
    let channel_table = database
        .read()
        .await
        .unwrap()
        .query(
            "SELECT name FROM sqlite_master WHERE name = 'agent_channel_reports'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap();
    assert!(channel_table.is_none(), "the channel's table is dropped");
}
