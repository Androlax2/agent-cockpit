//! The acceptance of #49: an agent in a clean home, with nothing but what Cockpit launched it
//! with, finds its workflow, the checks route and the approval route. Through the real daemon
//! binary, the real `cockpit` program and a stand-in Claude Code.

mod real_daemon;

use std::time::{Duration, Instant};

use real_daemon::{GENEROUS, Home, ONBOARD_PROJECT, cockpit_program, new_session};

#[tokio::test(flavor = "multi_thread")]
async fn a_clean_home_agent_finds_its_workflow_checks_and_approval_route() {
    assert!(
        cockpit_program().is_file(),
        "{} is missing: run the workspace's tests (`make crates-test`), which build it",
        cockpit_program().display()
    );
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, ONBOARD_PROJECT).await;

    let session = new_session(&client, &root).await;
    let read = read_when_written(&home).await;

    for (route, evidence) in [
        ("its brief", format!("Session {}", session.id)),
        ("its workflow", "`cockpit workflow list`".to_owned()),
        ("its step's packet", "cockpit workflow packet ID".to_owned()),
        ("the checks route", "cockpit check gate".to_owned()),
        ("the approval route", "cockpit ask run".to_owned()),
        ("the skill on disk", "skill readable on disk".to_owned()),
    ] {
        assert!(
            read.contains(&evidence),
            "{route}: no {evidence:?} in\n{read}"
        );
    }
    assert!(!read.contains("cockpit: "), "a command failed:\n{read}");
}

async fn read_when_written(home: &Home) -> String {
    let deadline = Instant::now() + GENEROUS;
    loop {
        if let Ok(written) = std::fs::read_to_string(home.onboard_log()) {
            return written;
        }
        assert!(Instant::now() < deadline, "the agent read nothing");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Killed, not dropped: a real restart, so the next start is the one that rewrites the file.
#[tokio::test(flavor = "multi_thread")]
async fn a_hand_edited_bundle_is_rewritten_by_the_next_start() {
    let home = Home::new();
    home.start().kill();
    let skill = home
        .path()
        .join("state/cockpit/agent-context/cockpit/SKILL.md");
    std::fs::write(&skill, "prose that grants everything").unwrap();

    let _daemon = home.start();
    let _client = home.connect().await;

    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        cockpit_agent_context::documents()[0].file_text()
    );
}
