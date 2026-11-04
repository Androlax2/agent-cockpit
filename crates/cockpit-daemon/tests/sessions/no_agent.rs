//! Issue #406: a session left running with no agent, because processes its agent started
//! outlive it, is reported to the owner with the command that ends it.

use super::*;

/// Settings whose no-agent window passes within a test.
fn short_agentless_window() -> cockpit_sessions::Settings {
    cockpit_sessions::Settings {
        agentless_after: Duration::from_secs(1),
        ..settings()
    }
}

async fn end_session(client: &Connection, id: &str) -> SessionView {
    session_reply(
        client,
        Command::SessionEnd {
            session: id.to_owned(),
        },
    )
    .await
}

/// The session's open question after the periodic pass ran several times.
async fn question_after_passes(client: &Connection, id: &str) -> Option<String> {
    tokio::time::sleep(Duration::from_secs(2)).await;
    agent_reports(client)
        .await
        .into_iter()
        .find(|report| report.session == id)
        .and_then(|report| report.needs_input)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_left_with_no_agent_waits_on_the_owner_with_the_command_that_ends_it() {
    let home = Home::new();
    let daemon = home
        .start(LEAVING_A_PROCESS, short_agentless_window())
        .await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;

    let started = new_session(&client, &root, None).await;

    let report = report_once(&client, &started.id, |report| report.needs_input.is_some()).await;
    assert_eq!(report.question_source, Some(QuestionSource::Dialog));
    assert_eq!(
        report.needs_input,
        Some(format!(
            "its agent exited, but processes it started still run; \
             `torlek session end {}` stops them",
            started.id
        ))
    );
    end_session(&client, &started.id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_reported_with_no_agent_keeps_running_until_the_owner_ends_it() {
    let home = Home::new();
    let daemon = home
        .start(LEAVING_A_PROCESS, short_agentless_window())
        .await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    report_once(&client, &started.id, |report| report.needs_input.is_some()).await;

    let reported = sessions(&client, false)
        .await
        .into_iter()
        .find(|session| session.id == started.id)
        .expect("the session is listed");
    let ended = end_session(&client, &started.id).await;

    assert_eq!(reported.state, SessionStateView::Running);
    assert_eq!(ended.state, SessionStateView::Ended);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_with_no_agent_is_not_reported_before_the_window_passed() {
    let home = Home::new();
    // The product's window: ten minutes.
    let daemon = home.start(LEAVING_A_PROCESS, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;

    let question = question_after_passes(&client, &started.id).await;

    assert_eq!(question, None);
    end_session(&client, &started.id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_whose_agent_runs_is_never_reported_as_having_none() {
    let home = Home::new();
    let daemon = home.start(BASELINE, short_agentless_window()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    home.launches(1).await;

    let question = question_after_passes(&client, &started.id).await;

    assert_eq!(question, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_whose_agent_waited_at_its_prompt_is_left_to_the_idle_sleep() {
    let home = Home::new();
    std::fs::write(home.leaving_agent_hold(), "").unwrap();
    let daemon = home
        .start(LEAVING_A_PROCESS, short_agentless_window())
        .await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    let launch = home.launches(1).await;
    home.report_as_agent(&launch[0], cockpit_sessions::AgentState::Waiting)
        .await;
    std::fs::remove_file(home.leaving_agent_hold()).unwrap();

    let question = question_after_passes(&client, &started.id).await;

    // The idle sleep, an hour away, stops what the agent left: no question before it.
    assert!(!is_alive(launch[0].pid), "the agent exited");
    assert_eq!(question, None);
    end_session(&client, &started.id).await;
}
