//! The optional skill pack from the command line (#786): `torlek skills` lists the pack the
//! daemon delivered beside the skills already installed, and `torlek brief --measure` counts
//! the pack's bytes apart from Torlek's own. Driven as a process against a scripted daemon.

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    Command, ContextBudgetView, InstalledSkillView, Outcome, PackSkillStateView, PackSkillView,
    Reply, SkillPackView,
};
use scripted::{scripted_daemon, text};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn daemon_socket(&self) -> PathBuf {
        let path = self.dir.path().join("run/torlek-dev/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    async fn torlek(&self, arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_torlek"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"));
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

fn pack_skill(name: &str, state: PackSkillStateView) -> PackSkillView {
    PackSkillView {
        name: name.to_owned(),
        state,
        is_starter: false,
        is_user_invoked: false,
        grants: Vec::new(),
    }
}

fn listing() -> SkillPackView {
    SkillPackView {
        source: "https://github.com/mattpocock/skills".to_owned(),
        commit: "4ae2317".to_owned(),
        tree: "a73b182".to_owned(),
        licence: "MIT".to_owned(),
        enabled: true,
        skills: vec![
            PackSkillView {
                is_starter: true,
                ..pack_skill("diagnosing-bugs", PackSkillStateView::Delivered)
            },
            PackSkillView {
                is_starter: true,
                ..pack_skill(
                    "tdd",
                    PackSkillStateView::Shadowed {
                        by: "/home/owner/.claude/skills/tdd".to_owned(),
                    },
                )
            },
            PackSkillView {
                is_user_invoked: true,
                grants: vec!["allowed-tools".to_owned()],
                ..pack_skill("pr", PackSkillStateView::Off)
            },
        ],
        installed: vec![
            InstalledSkillView {
                name: "notes".to_owned(),
                scope: "user".to_owned(),
                path: "/home/owner/.claude/skills/notes".to_owned(),
                collides: false,
            },
            InstalledSkillView {
                name: "tdd".to_owned(),
                scope: "user".to_owned(),
                path: "/home/owner/.claude/skills/tdd".to_owned(),
                collides: true,
            },
        ],
    }
}

#[tokio::test]
async fn skills_lists_the_pack_beside_the_installed_skills_with_collisions_marked() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::SkillPack {
            pack: Box::new(listing()),
        }),
    );

    let output = home.torlek(&["skills"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "skill pack: on, https://github.com/mattpocock/skills at 4ae2317 (tree a73b182), MIT\n\
         diagnosing-bugs\tdelivered\tstarter\n\
         tdd\tshadowed by /home/owner/.claude/skills/tdd\tstarter\n\
         pr\toff\tuser-invoked\tgrants allowed-tools\n\
         installed:\n\
         notes\tuser\t/home/owner/.claude/skills/notes\n\
         tdd\tuser\t/home/owner/.claude/skills/tdd\tsame name as a pack skill\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::SkillPack);
}

#[tokio::test]
async fn skills_says_so_when_the_pack_is_off() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::SkillPack {
            pack: Box::new(SkillPackView {
                enabled: false,
                skills: vec![pack_skill("tdd", PackSkillStateView::Off)],
                installed: Vec::new(),
                ..listing()
            }),
        }),
    );

    let output = home.torlek(&["skills"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stdout).starts_with("skill pack: off, "),
        "{}",
        text(&output.stdout)
    );
    daemon.await.unwrap();
}

#[tokio::test]
async fn skills_takes_no_argument() {
    let home = Home::new();

    let output = home.torlek(&["skills", "--all"]).await;

    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
}

#[tokio::test]
async fn brief_measure_counts_the_skill_packs_bytes_apart() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::AgentBrief {
            brief: "brief".to_owned(),
            packet: None,
            manifest: "m-1".to_owned(),
            budget: ContextBudgetView {
                brief_bytes: 100,
                packet_bytes: 20,
                catalog_bytes: 3,
                mcp_instructions_bytes: 4,
                pack_catalog_bytes: 500,
                on_demand_bytes: 9000,
            },
        }),
    );

    let output = home.torlek(&["brief", "--measure"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stdout).starts_with(
            "always loaded: 627 bytes (brief 100, task packet 20, MCP instructions 4, \
             skill catalog entry 3, skill pack 500)\n"
        ),
        "{}",
        text(&output.stdout)
    );
    daemon.await.unwrap();
}
