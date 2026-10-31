//! Issue #36's restart fixtures through the daemon binary (report 03's restart contract):
//! a daemon killed while a session runs comes back to the same agent, launched once. The
//! in-process daemon's drop is covered in `sessions.rs`; these kill the real process, so no
//! destructor and no runtime shutdown runs, and Claude Code is wired as the binary wires it.
#![cfg(target_os = "linux")]

mod real_daemon;

use cockpit_protocol::{Command, LaunchKindView, SessionStateView};
use real_daemon::{
    GENEROUS, Home, SETTLE, STALL_PROJECT, is_alive, listed, new_session, wait_for_running,
    wait_for_state,
};

#[tokio::test(flavor = "multi_thread")]
async fn a_killed_daemon_reattaches_the_running_session_without_launching_it_again() {
    let home = Home::new();
    let daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let started = new_session(&client, &root).await;
    let agent = home.wait_for_launches(&started.id, 1).await[0].clone();
    drop(client);
    daemon.kill();

    let _daemon = home.start();
    let client = home.connect().await;
    tokio::time::sleep(SETTLE).await;

    let kept = listed(&client, &started.id).await;
    assert_eq!(kept.state, SessionStateView::Running);
    assert_eq!(
        kept.launch, started.launch,
        "the same launch, not a new one"
    );
    assert_eq!(home.launches_of(&started.id), vec![agent.clone()]);
    assert!(is_alive(agent.pid), "the agent kept running");
}

/// The next daemon finishes the restart the killed one left under way (#205): the stalled
/// agent is stopped and the conversation launched again, once.
#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_killed_while_it_restarts_a_stalled_agent_leaves_one_agent_running() {
    let home = Home::new();
    let daemon = home.start();
    let client = home.connect().await;
    // The stalled agent ignores `SIGTERM`, so the restart waits out its grace before it
    // kills the agent; the daemon is killed inside that wait.
    let (root, _) = home.project(&client, STALL_PROJECT).await;
    let started = new_session(&client, &root).await;
    let stalled = home.wait_for_launches(&started.id, 1).await[0].clone();
    let restarting = tokio::spawn({
        let client = home.connect().await;
        let id = started.id.clone();
        async move {
            client
                .request(Command::SessionRestart { session: id }, GENEROUS)
                .await
        }
    });
    wait_for_state(&client, &started.id, |state| {
        matches!(state, SessionStateView::Stopping { .. })
    })
    .await;
    drop(client);
    daemon.kill();
    let _ = restarting.await;

    let _daemon = home.start();
    let client = home.connect().await;

    let session = wait_for_running(&client, &started.id).await;
    tokio::time::sleep(SETTLE).await;
    let launches = home.launches_of(&started.id);
    let running: Vec<_> = launches
        .iter()
        .filter(|launch| is_alive(launch.pid))
        .collect();
    assert_eq!(session.state, SessionStateView::Running, "{session:?}");
    assert_eq!(
        session.launch.as_ref().map(|launch| launch.kind),
        Some(LaunchKindView::Restart),
        "the restart was finished: {session:?}"
    );
    assert!(!is_alive(stalled.pid), "the stalled agent is gone");
    assert_eq!(running.len(), 1, "one agent runs: {launches:?}");
    assert_eq!(launches.len(), 2, "launched once more: {launches:?}");
}
