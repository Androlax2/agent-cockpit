//! Reports bound to their conversation (#219), through the daemon binary's agent socket: a
//! report naming a conversation that is not its session's is refused, unless the session's
//! conversation announced its end in the same launch (`/clear`, an in-session `/resume`),
//! after which the session follows the next one. This test process plays the agent.
#![cfg(target_os = "linux")]

mod common;

use cockpit_agent_channel::identity_of;
use cockpit_authorization::Permission;
use cockpit_protocol::{AgentReport, AgentState, Command, ErrorCode, Reply};
use common::{GENEROUS, RunningDaemon, TestHome, refused};

/// Session `s000000000001`, holding `conversation`, with a launch rooted at this test process and
/// granted `change-state`; the daemon runs and the launch is bound.
async fn session_holding(conversation: Option<&str>) -> (TestHome, RunningDaemon) {
    let home = TestHome::new();
    let token = home
        .launch(
            "s000000000001",
            &identity_of(std::process::id()).unwrap(),
            Some(Permission::ChangeState),
        )
        .await;
    let database = home.open_state().await;
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO sessions (id, provider, checkout_id, holds_checkout, working_dir,
                 conversation, state, revision, created_at, changed_at)
             VALUES ('s000000000001', 'claude', 1, 0, '/work', ?1, 'asleep', 1, 0, 0)",
            [conversation],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    drop(database);
    let daemon = home.start();
    let bound = send_with_token(&home, Some(token.as_str()), label("bound")).await;
    assert!(
        matches!(bound, Ok(Reply::AgentReported { .. })),
        "{bound:?}"
    );
    (home, daemon)
}

fn label(text: &str) -> AgentReport {
    AgentReport {
        label: Some(text.to_owned()),
        ..AgentReport::default()
    }
}

/// What the hook bridge sends for an event of `conversation`.
fn from(conversation: &str) -> AgentReport {
    AgentReport {
        state: Some(AgentState::Working),
        conversation: Some(conversation.to_owned()),
        ..AgentReport::default()
    }
}

/// What the hook bridge sends when `conversation` ends for another (`/clear`, `/resume`).
fn ending(conversation: &str) -> AgentReport {
    AgentReport {
        conversation: Some(conversation.to_owned()),
        is_conversation_ending: true,
        ..AgentReport::default()
    }
}

async fn send_with_token(
    home: &TestHome,
    launch_token: Option<&str>,
    report: AgentReport,
) -> Result<Reply, cockpit_client::ClientError> {
    home.connect_agent()
        .await
        .request(
            Command::AgentReport {
                launch_token: launch_token.map(str::to_owned),
                report,
            },
            GENEROUS,
        )
        .await
}

async fn send(home: &TestHome, report: AgentReport) -> Result<Reply, cockpit_client::ClientError> {
    send_with_token(home, None, report).await
}

async fn assert_recorded(home: &TestHome, report: AgentReport) {
    let reply = send(home, report.clone()).await;
    assert!(
        matches!(reply, Ok(Reply::AgentReported { .. })),
        "{report:?}: {reply:?}"
    );
}

async fn assert_refused(home: &TestHome, report: AgentReport) {
    let refusal = refused(send(home, report).await);
    assert_eq!(refusal.code, ErrorCode::InvalidArgument, "{refusal:?}");
    assert!(
        refusal.message.starts_with("conversation: "),
        "{}",
        refusal.message
    );
}

/// The session's conversation as stored, read once the daemon is stopped.
async fn stored_conversation(home: &TestHome, daemon: RunningDaemon) -> Option<String> {
    daemon.kill();
    let database = home.open_state().await;
    let read = database.read().await.unwrap();
    let mut rows = read
        .query(
            "SELECT conversation FROM sessions WHERE id = 's000000000001'",
            (),
        )
        .await
        .unwrap();
    rows.next().await.unwrap().unwrap().get(0).unwrap()
}

#[tokio::test]
async fn a_report_from_the_sessions_conversation_is_recorded() {
    let (home, _daemon) = session_holding(Some("c-1")).await;

    assert_recorded(&home, from("c-1")).await;
}

#[tokio::test]
async fn a_report_from_another_conversation_is_refused_and_changes_nothing() {
    let (home, _daemon) = session_holding(Some("c-1")).await;

    assert_refused(&home, from("c-2")).await;

    let reports = match home
        .connect()
        .await
        .request(Command::AgentReports, GENEROUS)
        .await
    {
        Ok(Reply::AgentReports { reports }) => reports,
        other => panic!("expected the reports, got {other:?}"),
    };
    assert_eq!(reports[0].state, None, "{reports:?}");
}

#[tokio::test]
async fn a_report_without_a_conversation_is_not_checked() {
    let (home, _daemon) = session_holding(Some("c-1")).await;

    assert_recorded(&home, label("a nested agent's label")).await;
}

#[tokio::test]
async fn after_its_conversation_ends_the_session_follows_the_next_one() {
    let (home, daemon) = session_holding(Some("c-1")).await;

    assert_recorded(&home, ending("c-1")).await;
    assert_recorded(&home, from("c-2")).await;
    assert_recorded(&home, from("c-2")).await;
    assert_refused(&home, from("c-1")).await;

    assert_eq!(
        stored_conversation(&home, daemon).await.as_deref(),
        Some("c-2")
    );
}

#[tokio::test]
async fn an_end_lets_only_one_new_conversation_in() {
    let (home, _daemon) = session_holding(Some("c-1")).await;

    assert_recorded(&home, ending("c-1")).await;
    assert_recorded(&home, from("c-2")).await;

    assert_refused(&home, from("c-3")).await;
}

#[tokio::test]
async fn an_end_announced_by_another_conversation_is_refused_and_opens_nothing() {
    let (home, _daemon) = session_holding(Some("c-1")).await;

    assert_refused(&home, ending("c-9")).await;

    assert_refused(&home, from("c-2")).await;
}

#[tokio::test]
async fn a_report_of_the_same_conversation_after_its_end_closes_the_change() {
    let (home, _daemon) = session_holding(Some("c-1")).await;

    assert_recorded(&home, ending("c-1")).await;
    assert_recorded(&home, from("c-1")).await;

    assert_refused(&home, from("c-2")).await;
}

#[tokio::test]
async fn an_end_does_not_outlive_its_launch() {
    let (home, daemon) = session_holding(Some("c-1")).await;
    assert_recorded(&home, ending("c-1")).await;
    daemon.kill();
    let database = home.open_state().await;
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO session_launches (session_id, kind, execution_id, launched_at)
             VALUES ('s000000000001', 'wake', 1, 1)",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
    drop(database);
    let _daemon = home.start();

    assert_refused(&home, from("c-2")).await;
}

#[tokio::test]
async fn a_session_without_a_conversation_takes_the_reported_one() {
    let (home, daemon) = session_holding(None).await;

    assert_recorded(&home, from("c-1")).await;
    assert_refused(&home, from("c-2")).await;

    assert_eq!(
        stored_conversation(&home, daemon).await.as_deref(),
        Some("c-1")
    );
}
