//! The acceptance of #49: an agent in a clean home, with nothing but what Cockpit launched it
//! with, finds its workflow, the checks route and the approval route. Through the real daemon
//! binary, the real `cockpit` program and a stand-in Claude Code. And #419's: the launch puts
//! the brief in front of the agent as its context starts, without the agent asking.

mod candidate;
mod real_daemon;

use std::time::{Duration, Instant};

use real_daemon::{
    COMMANDED_PROJECT, GENEROUS, Home, ONBOARD_PROJECT, added_context, cockpit_program,
    mcp_tool_text, new_session,
};

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
        ("its workflow", "`torlek workflow list`".to_owned()),
        ("its step's packet", "torlek workflow packet ID".to_owned()),
        ("the checks route", "torlek check gate".to_owned()),
        ("the approval route", "torlek ask run".to_owned()),
        ("the skill on disk", "skill readable on disk".to_owned()),
    ] {
        assert!(
            read.contains(&evidence),
            "{route}: no {evidence:?} in\n{read}"
        );
    }
    assert!(!read.contains("torlek: "), "a command failed:\n{read}");
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
    let skill = home.state_dir().join("agent-context/torlek/SKILL.md");
    std::fs::write(&skill, "prose that grants everything").unwrap();

    let _daemon = home.start();
    let _client = home.connect().await;

    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        cockpit_agent_context::documents()[0].file_text()
    );
}

/// #419: Claude Code runs the launch's `SessionStart` hook as a context starts, and its answer
/// adds this session's brief to the model's context.
#[tokio::test(flavor = "multi_thread")]
async fn a_claude_launch_adds_the_sessions_brief_as_its_context_starts() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    let session = new_session(&client, &root).await;

    let printed = home.start_claude_context(&root).await;

    let context = added_context(&printed);
    assert!(
        context.contains(&format!("Session {}", session.id)),
        "{context}"
    );
}

/// #419: the MCP server a Claude launch registers answers the agent with its own brief.
#[tokio::test(flavor = "multi_thread")]
async fn the_mcp_server_a_claude_launch_registers_answers_the_sessions_brief() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    let session = new_session(&client, &root).await;

    let printed = home.call_claude_mcp_tool(&root, "torlek_brief").await;

    let brief = mcp_tool_text(&printed);
    assert!(
        brief.contains(&format!("Session {}", session.id)),
        "{brief}"
    );
}
