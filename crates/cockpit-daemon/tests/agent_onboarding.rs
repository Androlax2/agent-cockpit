//! The acceptance of #49: an agent in a clean home, with nothing but what Cockpit launched it
//! with, finds its workflow, the checks route and the approval route. Through the real daemon
//! binary, the real `cockpit` program and a stand-in Claude Code. And #419's: the launch puts
//! the brief in front of the agent as its context starts, without the agent asking.

mod candidate;
mod real_daemon;

use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_protocol::{Command, Reply, WorkflowHandoffView};

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

/// #781: after a compaction (Claude Code's `SessionStart` with `source: "compact"`), the
/// hook adds the brief and the step's task packet, the packet as `torlek workflow packet`
/// prints it: the goal, the step and its requirements, the checkpoint and the last handoff.
#[tokio::test(flavor = "multi_thread")]
async fn a_compacted_claude_context_gets_the_packet_torlek_workflow_packet_prints() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, project) = home.project(&client, COMMANDED_PROJECT).await;
    let session = new_session(&client, &root).await;
    let run = started_past_its_first_step(&client, &project).await;
    let torlek = format!("'{}'", cockpit_program().display());
    let begun = home.command_agent(
        &root,
        &format!(
            "{torlek} workflow begin {run} && \
             {torlek} workflow checkpoint {run} 'Branch greenfield/7-login at 0a1b2c3; review next'"
        ),
    );
    let begun = home.agent_output(&root, begun).await;
    assert!(begun.ends_with("exit=0\n"), "{begun}");

    let printed = home.start_claude_context_from(&root, "compact").await;
    let asked = home.command_agent(&root, &format!("{torlek} workflow packet {run}"));
    let asked = home.agent_output(&root, asked).await;

    let packet = asked
        .strip_suffix("exit=0\n")
        .unwrap_or_else(|| panic!("`torlek workflow packet` failed: {asked}"));
    for part in [
        "step 2 check (active",
        "requires: checks, review",
        "greenfield/7-login at 0a1b2c3",
        "handoff from step 1 implement",
    ] {
        assert!(packet.contains(part), "no {part:?} in\n{packet}");
    }
    let context = added_context(&printed);
    assert!(
        context.contains(&format!("Session {}", session.id)),
        "{context}"
    );
    assert!(
        context.ends_with(&cockpit_agent_context::task_packet(run, packet)),
        "the packet differs from `torlek workflow packet`'s:\n{context}\n---\n{packet}"
    );
}

/// A `code-change` workflow in `project` whose first step the owner completed with a handoff,
/// so its `check` step waits for a session.
async fn started_past_its_first_step(client: &Connection, project: &str) -> i64 {
    let request = |command| client.request(command, GENEROUS);
    let added = request(Command::TaskAdd {
        launch_token: None,
        project: Some(project.to_owned()),
        title: "Fix the login form".to_owned(),
        notes: "Users on #7 cannot sign in.".to_owned(),
        item_kind: None,
        priority: None,
        after: Vec::new(),
    })
    .await;
    let Ok(Reply::Task { task }) = added else {
        panic!("task-add: {added:?}");
    };
    let started = request(Command::WorkflowStart {
        item: task.id,
        preset: Some("code-change".to_owned()),
    })
    .await;
    let Ok(Reply::Workflow { workflow }) = started else {
        panic!("workflow-start: {started:?}");
    };
    let completed = request(Command::WorkflowComplete {
        launch_token: None,
        workflow: workflow.id,
        expected_step: None,
        expected_attempt: None,
        handoff: WorkflowHandoffView {
            changed: "Rewrote the form validation in PR #12".to_owned(),
            checked: "cargo nextest run -p app: 14 passed".to_owned(),
            outcome: "passed".to_owned(),
            ..WorkflowHandoffView::default()
        },
        waive: Vec::new(),
    })
    .await;
    assert!(
        matches!(completed, Ok(Reply::Workflow { .. })),
        "{completed:?}"
    );
    workflow.id
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
