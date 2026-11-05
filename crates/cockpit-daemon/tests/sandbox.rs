//! This release has no sandbox backend (#828), through the real daemon binary: by default
//! every launch runs unsandboxed, the evidence says so and the risk is logged at the start
//! and at each launch; an explicit `agents.sandbox = "required"` refuses every launch naming
//! the missing backend and never runs an agent. `cockpit session sandbox` and
//! `cockpit sandbox show` print the evidence and the command.

mod candidate;
mod real_daemon;

use std::time::Duration;

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, ErrorCode};
use real_daemon::{GENEROUS, Home, SETTLE, new_session, session_new_command};

const REQUIRED: &str = "[agents]\nsandbox = \"required\"\n";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

async fn sandbox_show(home: &Home, root: &std::path::Path) -> std::process::Output {
    home.cockpit(
        &[
            "sandbox",
            "show",
            root.to_str().unwrap(),
            "--agent",
            "claude",
        ],
        &[],
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn by_default_a_launch_runs_unsandboxed_and_the_evidence_says_no_sandbox() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;

    let shown = home
        .cockpit(&["session", "sandbox", &session.id], &[])
        .await;

    assert!(shown.status.success(), "{}", text(&shown.stderr));
    let first = text(&shown.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    assert!(
        first.starts_with("sandbox: NONE on ")
            && first.ends_with("unsandboxed: no sandbox: agents.sandbox = \"off\""),
        "{first}"
    );
}

/// The security rule of #828: `required` never degrades to an unsandboxed launch.
#[tokio::test(flavor = "multi_thread")]
async fn an_explicit_required_setting_refuses_the_launch_naming_the_missing_backend() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let refused = client
        .request(session_new_command(&root), Duration::from_secs(60))
        .await;

    assert!(
        matches!(&refused, Err(ClientError::Daemon(error))
            if error.code == ErrorCode::Refused
                && error.message.contains(cockpit_config::NO_SANDBOX_BACKEND)),
        "{refused:?}"
    );
    tokio::time::sleep(SETTLE).await;
    assert!(home.launches().is_empty(), "{:?}", home.launches());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_required_setting_logs_at_start_that_every_launch_is_refused() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start();

    let log = home
        .log_once_it_contains("every agent launch is refused")
        .await;

    assert!(
        log.lines()
            .any(|line| line.contains("every agent launch is refused")
                && line.contains(cockpit_config::NO_SANDBOX_BACKEND)),
        "{log}"
    );
}

/// The daemon's start and each unsandboxed launch warn that grants no longer hold (#729).
#[tokio::test(flavor = "multi_thread")]
async fn the_default_warns_at_start_and_at_each_launch_that_an_agent_can_act_as_the_owner() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;

    let log = home
        .log_once_it_contains("an agent launches UNSANDBOXED")
        .await;

    for warning in ["agents run UNSANDBOXED", "an agent launches UNSANDBOXED"] {
        assert!(
            log.lines()
                .any(|line| line.contains(warning)
                    && line.contains(cockpit_config::UNSANDBOXED_RISK)),
            "{warning:?} missing from {log}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cockpit_sandbox_show_prints_no_sandbox_and_the_bare_command_without_launching() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let shown = sandbox_show(&home, &root).await;

    assert!(shown.status.success(), "{}", text(&shown.stderr));
    let printed = text(&shown.stdout);
    assert!(printed.starts_with("sandbox: NONE on "), "{printed}");
    assert!(printed.contains("\ncommand: <agent>\n"), "{printed}");
    assert!(home.launches().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn cockpit_sandbox_show_with_a_required_setting_fails_naming_the_missing_backend() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let shown = sandbox_show(&home, &root).await;

    assert!(!shown.status.success());
    assert!(
        text(&shown.stderr).contains(cockpit_config::NO_SANDBOX_BACKEND),
        "{}",
        text(&shown.stderr)
    );
}

/// #419: Claude Code lists the bundle's `torlek` skill because the launch adds the folder
/// whose `.claude/skills` holds it, written at the daemon's start.
#[tokio::test(flavor = "multi_thread")]
async fn a_claude_launch_adds_the_instruction_skills() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;
    let skills_root = home.state_dir().join("claude-skills");

    let arguments = home.arguments_of(&session.id);

    assert!(
        arguments[0].contains(&format!("--add-dir {}", skills_root.display())),
        "{arguments:?}"
    );
    let skill =
        std::fs::read_to_string(skills_root.join(".claude/skills/torlek/SKILL.md")).unwrap();
    assert!(skill.starts_with("---\nname: torlek\n"), "{skill}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_may_not_read_a_sessions_sandbox_or_the_profile() {
    let home = Home::new();
    let _daemon = home.start();
    let agent = Connection::open(&home.agent_socket_path(), GENEROUS)
        .await
        .unwrap();

    for command in [
        Command::SessionSandbox {
            session: "s1".to_owned(),
        },
        Command::SandboxShow {
            dir: home.path().display().to_string(),
            agent: None,
        },
    ] {
        let refused = agent.request(command, GENEROUS).await;

        assert!(
            matches!(&refused, Err(ClientError::Daemon(error)) if error.code == ErrorCode::NotAuthorized),
            "{refused:?}"
        );
    }
}
