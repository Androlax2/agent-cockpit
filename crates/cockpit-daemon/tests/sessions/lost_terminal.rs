//! Issue #844: a running session whose terminal (its tmux session) is gone is failed with
//! the reason, so it can be resumed, instead of staying `running` for a terminal nobody can
//! attach.

use super::*;

/// Closes `session`'s tmux session behind the daemon's back.
fn kill_terminal_of(home: &Home, session: &str) {
    let output = Process::new("tmux")
        .arg("-S")
        .arg(home.path().join("tmux.sock"))
        .args(["kill-session", "-t", &format!("={session}")])
        .env_remove("TMUX")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failed_for_its_terminal(failed: &SessionView) {
    let reason = failed
        .reason
        .as_deref()
        .expect("a failed session has a reason");
    assert!(
        reason.contains("terminal")
            && reason.contains(&format!("torlek session resume {}", failed.id)),
        "{reason}"
    );
}

async fn assert_resumes(client: &Connection, id: &str) {
    let resumed = session_reply(
        client,
        Command::SessionResume {
            session: id.to_owned(),
        },
    )
    .await;
    assert_eq!(resumed.state, SessionStateView::Running);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_session_whose_tmux_session_is_gone_is_failed_and_can_be_resumed() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    home.launches(1).await;

    kill_terminal_of(&home, &started.id);

    let failed = wait_for_state(&client, &started.id, SessionStateView::Failed).await;
    assert_failed_for_its_terminal(&failed);
    assert_resumes(&client, &started.id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_session_whose_agent_outlived_its_tmux_session_is_stopped_and_failed() {
    let home = Home::new();
    let daemon = home.start(LEAVING_A_PROCESS, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    home.launches(1).await;

    kill_terminal_of(&home, &started.id);

    let failed = wait_for_state(&client, &started.id, SessionStateView::Failed).await;
    assert_failed_for_its_terminal(&failed);
    assert_resumes(&client, &started.id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn every_running_session_is_failed_when_the_whole_tmux_server_is_gone() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    home.launches(1).await;

    home.lose_every_agent();

    let failed = wait_for_state(&client, &started.id, SessionStateView::Failed).await;
    assert_failed_for_its_terminal(&failed);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_whose_tmux_session_is_gone_is_failed_between_ticks_as_well() {
    let home = Home::new();
    let daemon = home
        .start_wired(|| {
            let mut wiring = home.wiring(BASELINE, settings());
            wiring.session_tick = Duration::from_secs(600);
            wiring
        })
        .await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root, None).await;
    home.launches(1).await;

    kill_terminal_of(&home, &started.id);

    let failed = wait_for_state(&client, &started.id, SessionStateView::Failed).await;
    assert_failed_for_its_terminal(&failed);
}
