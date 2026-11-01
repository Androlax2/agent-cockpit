//! #254 through the daemon binary: stops a killed daemon left under way are finished before
//! the next one accepts requests, and agents that ignore `SIGTERM` each wait out the stop's
//! grace (5 s) before their kill. The start waits for them together, not one after another,
//! so several stalled agents delay the first accepted request by about one grace.
#![cfg(target_os = "linux")]

mod real_daemon;

use std::time::{Duration, Instant};

use cockpit_protocol::SessionStateView;
use real_daemon::{Home, STALL_PROJECT, is_alive, listed, new_session};

/// Stalled agents left stopping: one after another their stops would take at least
/// `STALLED * 5 s`.
const STALLED: usize = 3;
/// Two graces: one is what the stops take together, the other is the margin.
const STARTED_WITHIN: Duration = Duration::from_secs(10);

/// Rewrites every session's stored state while no daemon runs.
async fn leave_every_session_stopping(home: &Home) {
    let database = cockpit_turso::Database::open(
        &home.path().join("state/cockpit/state.db"),
        &cockpit_daemon::MIGRATION_SETS,
    )
    .await
    .unwrap();
    let write = database.write().await.unwrap();
    write
        .execute(
            "UPDATE sessions SET state = 'stopping', stopping_to = 'asleep'",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn stalled_stops_left_by_a_killed_daemon_are_finished_together_before_it_accepts() {
    let home = Home::new();
    let daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, STALL_PROJECT).await;
    let mut stalled = Vec::new();
    for _ in 0..STALLED {
        let session = new_session(&client, &root).await;
        let agent = home.wait_for_launches(&session.id, 1).await[0].clone();
        stalled.push((session.id, agent.pid));
    }
    drop(client);
    daemon.kill();
    leave_every_session_stopping(&home).await;

    let starting = Instant::now();
    let _daemon = home.start();
    let client = home.connect().await;
    let first = listed(&client, &stalled[0].0).await;
    let accepted_after = starting.elapsed();

    assert_eq!(first.state, SessionStateView::Asleep, "{first:?}");
    for (session, agent) in &stalled {
        assert_eq!(
            listed(&client, session).await.state,
            SessionStateView::Asleep
        );
        assert!(!is_alive(*agent), "session {session}'s agent was stopped");
    }
    assert!(
        accepted_after < STARTED_WITHIN,
        "the first request was answered {accepted_after:?} after the start"
    );
}
