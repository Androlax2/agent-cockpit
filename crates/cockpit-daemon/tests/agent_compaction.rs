//! A context compaction through the daemon binary's sockets (#782): the harness's hook reports
//! its start and end on the agent socket, and the owner reads the latest one on the session's
//! report. This test process plays the agent.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use cockpit_agent_channel::identity_of;
use cockpit_authorization::Permission;
use cockpit_protocol::{
    AgentReport, AgentState, Command, CompactionPhase, CompactionReport, CompactionStatus,
    CompactionTrigger, Reply, SessionReport,
};
use common::{GENEROUS, TestHome};

async fn home_with_launched_session() -> (TestHome, String) {
    let home = TestHome::new();
    let token = home
        .launch(
            "s1",
            &identity_of(std::process::id()).unwrap(),
            Some(Permission::ChangeState),
        )
        .await;
    let database = home.open_state().await;
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO session_launches (session_id, kind, execution_id, launched_at)
             VALUES ('s1', 'wake', 1, 1)",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    drop(database);
    (home, token.as_str().to_owned())
}

/// The agent's own first report, as its session-start hook sends it: a compaction is shown on
/// the report of a session whose agent reported before.
async fn report_waiting(home: &TestHome, token: &str) {
    let reply = home
        .connect_agent()
        .await
        .request(
            Command::AgentReport {
                launch_token: Some(token.to_owned()),
                report: AgentReport {
                    state: Some(AgentState::Waiting),
                    ..AgentReport::default()
                },
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(reply, Ok(Reply::AgentReported { .. })),
        "{reply:?}"
    );
}

async fn report_compaction(home: &TestHome, token: &str, compaction: CompactionReport) -> Reply {
    home.connect_agent()
        .await
        .request(
            Command::AgentReport {
                launch_token: Some(token.to_owned()),
                report: AgentReport {
                    compaction: Some(compaction),
                    ..AgentReport::default()
                },
            },
            GENEROUS,
        )
        .await
        .expect("the agent socket answers")
}

async fn owner_reports(home: &TestHome) -> Vec<SessionReport> {
    match home
        .connect()
        .await
        .request(Command::AgentReports, GENEROUS)
        .await
    {
        Ok(Reply::AgentReports { reports }) => reports,
        other => panic!("expected the reports, got {other:?}"),
    }
}

#[tokio::test]
async fn a_compaction_reaches_the_owner_with_trigger_tokens_and_seconds() {
    let (home, token) = home_with_launched_session().await;
    let _daemon = home.start();
    report_waiting(&home, &token).await;

    let started = report_compaction(
        &home,
        &token,
        CompactionReport {
            phase: CompactionPhase::Started,
            trigger: Some(CompactionTrigger::Auto),
            tokens_before: Some(268_364),
            tokens_after: None,
        },
    )
    .await;
    let ended = report_compaction(
        &home,
        &token,
        CompactionReport {
            phase: CompactionPhase::Ended,
            trigger: None,
            tokens_before: None,
            tokens_after: Some(30_470),
        },
    )
    .await;

    assert!(
        matches!(started, Reply::AgentReported { .. }),
        "{started:?}"
    );
    assert!(matches!(ended, Reply::AgentReported { .. }), "{ended:?}");
    let reports = owner_reports(&home).await;
    let compaction = reports[0].compaction.as_ref().expect("a compaction");
    assert_eq!(compaction.status, CompactionStatus::Completed);
    assert_eq!(compaction.trigger, Some(CompactionTrigger::Auto));
    assert_eq!(compaction.tokens_before, Some(268_364));
    assert_eq!(compaction.tokens_after, Some(30_470));
    assert!(compaction.seconds.is_some(), "{compaction:?}");
}

#[tokio::test]
async fn a_compaction_does_not_make_the_session_look_active() {
    let (home, token) = home_with_launched_session().await;
    let _daemon = home.start();
    report_waiting(&home, &token).await;

    report_compaction(
        &home,
        &token,
        CompactionReport {
            phase: CompactionPhase::Started,
            trigger: None,
            tokens_before: None,
            tokens_after: None,
        },
    )
    .await;

    let reports = owner_reports(&home).await;
    assert_eq!(reports[0].state, Some(AgentState::Waiting));
    assert!(reports[0].compaction.is_some());
}

#[tokio::test]
async fn a_compaction_sent_with_a_state_is_refused() {
    let (home, token) = home_with_launched_session().await;
    let _daemon = home.start();
    report_waiting(&home, &token).await;

    let reply = home
        .connect_agent()
        .await
        .request(
            Command::AgentReport {
                launch_token: Some(token),
                report: AgentReport {
                    state: Some(AgentState::Working),
                    compaction: Some(CompactionReport {
                        phase: CompactionPhase::Started,
                        trigger: None,
                        tokens_before: None,
                        tokens_after: None,
                    }),
                    ..AgentReport::default()
                },
            },
            GENEROUS,
        )
        .await;

    assert!(
        !matches!(reply, Ok(Reply::AgentReported { .. })),
        "{reply:?}"
    );
    assert_eq!(owner_reports(&home).await[0].compaction, None);
}
