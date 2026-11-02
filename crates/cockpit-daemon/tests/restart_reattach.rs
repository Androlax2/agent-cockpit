//! Issue #36's restart fixtures through the daemon binary (report 03's restart contract):
//! a daemon killed while a session runs comes back to the same agent, launched once. The
//! in-process daemon's drop is covered in `sessions.rs`; these kill the real process, so no
//! destructor and no runtime shutdown runs, and Claude Code is wired as the binary wires it.
#![cfg(target_os = "linux")]

mod candidate;
mod real_daemon;
#[path = "latency/termination_evidence.rs"]
mod termination_evidence;

use std::io::Write as _;

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
    // Capture on unwind before Home removes the failed fixture's daemon logs.
    let mut evidence = std::env::var_os("COCKPIT_COMPATIBILITY_REPORT").map(|report| {
        let report = std::path::PathBuf::from(report);
        let revision = std::env::var("COCKPIT_CANDIDATE_REVISION")
            .expect("the compatibility candidate source revision");
        std::fs::write(
            &report,
            format!(
                "# Interrupted-restart compatibility fixture\n\nCandidate source revision: `{revision}`\n\nHarness incomplete until the final assertion verdict.\n",
            ),
        )
        .expect("the initial compatibility report is writable");
        let capture = termination_evidence::TerminationEvidence::new(
            home.state_dir().join("logs/daemon"),
            report.clone(),
        );
        (report, capture)
    });
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
    if let Some((report, capture)) = &mut evidence {
        capture
            .finish()
            .expect("complete compatibility termination evidence");
        let mut report = std::fs::OpenOptions::new()
            .append(true)
            .open(report)
            .expect("the compatibility report is writable");
        writeln!(
            report,
            "\nHarness phase: complete: all required assertions passed"
        )
        .unwrap();
    }
}

#[test]
fn release_compatibility_evidence_survives_a_restart_fixture_failure() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command as Process;

    let artifact = tempfile::tempdir().unwrap();
    let report = artifact.path().join("compatibility.md");
    let programs = artifact.path().join("bin");
    std::fs::create_dir(&programs).unwrap();
    let daemon = programs.join("torlek-daemon");
    std::fs::write(
        &daemon,
        r#"#!/bin/sh
set -eu
logs="$XDG_STATE_HOME/torlek-dev/logs/daemon"
mkdir -p "$logs"
printf '%s\n' '{"timestamp":"2026-11-02T16:10:42Z","fields":{"message":"a session'"'"'s agent termination is unconfirmed","session":"compatibility-fixture","execution":17,"diagnostics":"Unidentified; term: None; kill: None","environment":"must-not-export"},"provider_output":"must-not-export"}' > "$logs/cockpit.log"
printf 'deliberately not ready\n'
"#,
    )
    .unwrap();
    std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = Process::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_daemon_killed_while_it_restarts_a_stalled_agent_leaves_one_agent_running",
            "--nocapture",
        ])
        .env("COCKPIT_CANDIDATE_ROOT", artifact.path())
        .env("COCKPIT_CANDIDATE_REVISION", "a".repeat(40))
        .env("COCKPIT_COMPATIBILITY_REPORT", &report)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("the daemon did not report ready"),
        "the original fixture failure: {output:?}"
    );
    let retained =
        std::fs::read_to_string(&report).expect("failure evidence survives Home cleanup");
    assert!(retained.contains(&format!("Candidate source revision: `{}`", "a".repeat(40))));
    assert!(retained.contains("compatibility-fixture") && retained.contains("\"execution\":17"));
    assert!(retained.contains("Unidentified; term: None; kill: None"));
    assert!(!retained.contains("must-not-export"));
    assert!(!retained.contains("all required assertions passed"));
    assert_eq!(retained.matches("### Termination evidence").count(), 1);
}
