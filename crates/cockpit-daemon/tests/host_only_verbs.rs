//! The host-only verb list (#685, acceptance box 4): every command `routes.rs` answers with
//! `owner_only()` on the agent socket, proved against the real daemon from an agent session's
//! own connection, never from a payload claiming to be the owner.
//!
//! `routes.rs` groups its commands by one `match` arm per routing decision, so every command
//! inside one `(Command::A | Command::B | ..., peer) => match peer { Peer::Agent { .. } =>
//! Route::Answer(owner_only()), ... }` arm takes the identical branch: one representative
//! command proves the whole group, because the branch does not look at which variant it
//! matched. `docs/redteam-harness.md` lists every command of every group this file covers (one
//! test per group, named after it); a command added to an existing group needs no new test
//! here, one added to a new group does.
//!
//! This also demonstrates the "forged launch token or environment" and "from its child
//! process" parts of #685's acceptance: `Peer` comes from which socket file the kernel accepted
//! the connection on (`routes::Peer`'s own doc: "as the socket it came through and the kernel
//! say; never what a payload claims"), never from a command's fields or the connecting
//! process's environment. A command here carries no `launch_token`, `through` or similar field
//! that claims ownership, and still refuses: the routing match never reads one.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use cockpit_client::ClientError;
use cockpit_protocol::{Command, ErrorCode, ProfileView};
use common::{GENEROUS, TestHome};

/// One command per `routes.rs` group that answers `owner_only()` for `Peer::Agent`, named
/// after the group's own comment there so a failure names the file to look at.
fn owner_only_groups() -> Vec<(&'static str, Command)> {
    vec![
        ("agent-reports (routes.rs:249)", Command::AgentReports),
        (
            "bug-report (routes.rs:273)",
            Command::BugReportNew {
                description: "redteam".to_owned(),
                title: None,
                steps: String::new(),
                since: None,
                cli_version: None,
            },
        ),
        ("version-report (routes.rs:285)", Command::VersionReport),
        ("usage-quota (routes.rs:294)", Command::UsageQuota),
        ("usage-cost (routes.rs:296)", Command::UsageCost { days: 7 }),
        ("skill-pack (routes.rs:314)", Command::SkillPack),
        ("storage (routes.rs:330)", Command::StorageReport),
        (
            "project/checkout (routes.rs:347)",
            Command::ProjectAdd {
                path: "/nonexistent".to_owned(),
                name: None,
            },
        ),
        (
            "approval/guide (routes.rs:389)",
            Command::ApprovalApprove {
                request: 0,
                revision: 0,
                digest: String::new(),
                through: None,
            },
        ),
        (
            "network (routes.rs:406)",
            Command::NetworkAllow {
                request: 0,
                always: false,
                through: None,
            },
        ),
        (
            "account (routes.rs:415)",
            Command::AccountSignIn {
                provider: "codex".to_owned(),
                account: "default".to_owned(),
            },
        ),
        (
            "session-grant (routes.rs:441)",
            Command::SessionGrant {
                session: "s0".to_owned(),
                permission: "change-state".to_owned(),
                all_projects: false,
                without_confirmation: false,
                per_hour: None,
                max_active: None,
            },
        ),
        (
            "session lifecycle (routes.rs:465)",
            Command::SessionNew {
                dir: "/nonexistent".to_owned(),
                agent: None,
                prompt: None,
                clone: None,
                model: None,
                effort: None,
                compaction_threshold: None,
            },
        ),
        (
            "task decision (routes.rs:518)",
            Command::TaskPriority {
                item: 0,
                priority: 1,
            },
        ),
        (
            "workflow start/cancel (routes.rs:556)",
            Command::WorkflowStart {
                item: 0,
                preset: None,
            },
        ),
        (
            "resources (routes.rs:571)",
            Command::ResourceProfile {
                profile: ProfileView::Foreground,
            },
        ),
        ("queue/handoff (routes.rs:582)", Command::QueueDispatch),
        (
            "manager (routes.rs:597)",
            Command::ManagerFit {
                session_open: false,
            },
        ),
        ("provider-list (routes.rs:612)", Command::ProviderList),
        (
            "readiness (routes.rs:622)",
            Command::Readiness {
                project: None,
                is_one_sign_in_enough: false,
            },
        ),
        (
            "desktop-session import (routes.rs:628)",
            Command::DesktopSessionImport {
                variables: std::collections::BTreeMap::new(),
            },
        ),
        (
            "check run/receipt (routes.rs:640)",
            Command::CheckRun {
                project: "/nonexistent".to_owned(),
                check: "redteam".to_owned(),
                checkout: None,
            },
        ),
        ("logs level (routes.rs:662)", Command::LogsLevelStatus),
        ("config-reload (routes.rs:676)", Command::ConfigReload),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn every_host_only_verb_refuses_an_agent_caller() {
    let home = TestHome::new();
    let _daemon = home.start();
    let agent = home.connect_agent().await;

    let mut unexpected = Vec::new();
    for (group, command) in owner_only_groups() {
        let reply = agent.request(command, GENEROUS).await;
        let is_not_authorized = matches!(&reply, Err(ClientError::Daemon(error)) if error.code == ErrorCode::NotAuthorized);
        if !is_not_authorized {
            unexpected.push(format!("{group}: expected NotAuthorized, got {reply:?}"));
        }
    }

    assert!(unexpected.is_empty(), "{}", unexpected.join("\n"));
}

/// The owner's own socket still answers these: the refusal above is the agent socket's alone,
/// not a general breakage of the command.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_socket_is_unaffected() {
    let home = TestHome::new();
    let _daemon = home.start();
    let owner = home.connect().await;

    let reply = owner.request(Command::ProviderList, GENEROUS).await;

    assert!(reply.is_ok(), "{reply:?}");
}
