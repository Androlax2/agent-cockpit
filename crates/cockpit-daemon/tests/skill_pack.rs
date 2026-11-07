//! Issue #786 through the daemon: the optional skill pack, written at every start into the
//! folders each adapter's launch points its harness at, holds only the skills the owner's
//! `[skill_pack]` turns on, an owner's or trusted project's skill of the same name winning.
//! Each adapter is wired with the folder `serve.rs` gives it (Claude Code `claude-skills`
//! through `--add-dir`, OpenCode `agent-context` in `skills.paths`, Codex none), and the
//! test reads the folder back from the launch itself. Issue #787: from that launch, each
//! harness's listing is built the way its source builds it, and a skill only the user invokes
//! has no description in it.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cockpit_client::Connection;
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes, StartError};
use cockpit_protocol::{Command, PackDeliveryView, PackSkillStateView, Reply};
use cockpit_provider_api::{Conversation, LaunchCommand, LaunchRequest, Provider};
use common::GENEROUS;

const STARTER_AND_PR: &str = "[skill_pack]\nenabled = true\nskills = { tdd = false, pr = true }\n";

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    fn state_dir(&self) -> PathBuf {
        self.path().join("state/cockpit")
    }

    /// The owner's home, as [`common::wiring_in`] names it to the daemon.
    fn owner_home(&self) -> PathBuf {
        self.path().join("home")
    }

    fn write_owner_config(&self, contents: &str) {
        let dir = self.owner_home().join(".config/torlek");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), contents).unwrap();
    }

    /// A skill of the owner's in Claude Code's folder.
    fn owner_skill(&self, name: &str) -> PathBuf {
        self.owner_skill_in(".claude/skills", name)
    }

    /// A skill of the owner's in `folder`, relative to the home.
    fn owner_skill_in(&self, folder: &str, name: &str) -> PathBuf {
        let dir = self.owner_home().join(folder).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
        dir
    }

    fn environment(&self) -> cockpit_config::Environment {
        cockpit_config::Environment {
            home: Some(self.owner_home().into()),
            ..cockpit_config::Environment::default()
        }
    }

    async fn start(&self) -> Result<Daemon, StartError> {
        let options = Options {
            runtime_dir: self.path().join("run/cockpit"),
            state_dir: self.state_dir(),
            limits: Limits::default(),
        };
        let environment = self.environment();
        Instance::acquire(options)
            .unwrap()
            .start(
                common::wiring_in(&self.path(), Vec::new()),
                move |capabilities| Arc::new(Routes::new(capabilities, environment)),
            )
            .await
    }

    /// [`Home::start`] after a daemon was dropped: it releases its database once its aborted
    /// tasks are gone, so the start is retried until then.
    async fn restart(&self) -> Daemon {
        let deadline = std::time::Instant::now() + GENEROUS;
        loop {
            match self.start().await {
                Err(StartError::Database(cockpit_turso::Error::AlreadyOpen { .. }))
                    if std::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                started => return started.unwrap(),
            }
        }
    }

    fn claude(&self) -> cockpit_claude::ClaudeCode {
        cockpit_claude::ClaudeCode::new("/usr/bin/claude".into(), None, None)
            .with_instruction_skills(self.state_dir().join("claude-skills"))
    }

    fn opencode(&self) -> cockpit_opencode::OpenCode {
        cockpit_opencode::OpenCode::new(
            "/usr/bin/opencode".into(),
            None,
            Some(cockpit_opencode::Bridge {
                plugin: "/opt/torlek/plugin.js".into(),
                bridge_program: "/opt/torlek/bin/bridge".into(),
                cockpit_program: "/opt/torlek/bin/torlek".into(),
            }),
        )
        .with_instruction_skills(self.state_dir().join("agent-context"))
        .with_hidden_skills(cockpit_daemon::opencode_hidden_skills(&self.environment()).unwrap())
    }

    fn codex(&self) -> cockpit_codex::Codex {
        cockpit_codex::Codex::new(
            "/usr/bin/codex".into(),
            None,
            Some(cockpit_codex::Bridge {
                session_program: "/opt/torlek/bin/codex-session".into(),
                hook_program: "/opt/torlek/bin/hook".into(),
                cockpit_program: "/opt/torlek/bin/torlek".into(),
            }),
        )
    }
}

fn launch(provider: &dyn Provider) -> LaunchCommand {
    let conversation = Conversation::New;
    provider
        .launch(&LaunchRequest {
            working_dir: Path::new("/work"),
            session_dir: Path::new("/run/torlek/sessions/s-1"),
            conversation: &conversation,
            first_message: None,
            effort: None,
            model: None,
            remote_control: false,
            compaction_threshold: None,
        })
        .unwrap()
}

/// The skills Claude Code lists from the folder the launch adds with `--add-dir`.
fn claude_skills(launch: &LaunchCommand) -> BTreeSet<String> {
    let words: Vec<String> = launch
        .arguments
        .iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    let added = words.iter().position(|word| word == "--add-dir").unwrap();
    skills_under(&cockpit_claude::instruction_skills_dir(Path::new(
        &words[added + 1],
    )))
}

/// The skills OpenCode lists from the `skills.paths` the launch's inline configuration adds.
fn opencode_skills(launch: &LaunchCommand) -> BTreeSet<String> {
    let config = launch
        .environment
        .iter()
        .find(|(name, _)| name == "OPENCODE_CONFIG_CONTENT")
        .map(|(_, value)| value.to_str().unwrap())
        .unwrap();
    let config: serde_json::Value = serde_json::from_str(config).unwrap();
    config["skills"]["paths"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|path| skills_under(Path::new(path.as_str().unwrap())))
        .collect()
}

/// Every directory below `folder` that holds a `SKILL.md`, by name: what a harness scanning
/// it for `**/SKILL.md` lists.
fn skills_under(folder: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_dir() {
            continue;
        }
        if path.join("SKILL.md").is_file() {
            found.insert(path.file_name().unwrap().to_string_lossy().into_owned());
        }
        found.extend(skills_under(&path));
    }
    found
}

/// A skill's `SKILL.md` frontmatter, as the harnesses read it.
fn frontmatter_of(skill_dir: &Path) -> String {
    let text = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
    let (_, rest) = text.split_once("---\n").unwrap();
    rest.split_once("\n---\n").unwrap().0.to_owned()
}

/// Every directory below `folder` that holds a `SKILL.md`, with its frontmatter.
fn skill_frontmatters(folder: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_dir() {
            continue;
        }
        if path.join("SKILL.md").is_file() {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            found.push((name, frontmatter_of(&path)));
        }
        found.extend(skill_frontmatters(&path));
    }
    found
}

/// What Claude Code puts in the model's context for the skills of the folder the launch adds:
/// each frontmatter, but not that of a skill with `disable-model-invocation: true`
/// (code.claude.com/docs/en/skills, "Description not in context").
fn claude_listing(launch: &LaunchCommand) -> String {
    let words: Vec<String> = launch
        .arguments
        .iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    let added = words.iter().position(|word| word == "--add-dir").unwrap();
    let folder = cockpit_claude::instruction_skills_dir(Path::new(&words[added + 1]));
    skill_frontmatters(&folder)
        .into_iter()
        .filter(|(_, frontmatter)| {
            !frontmatter
                .lines()
                .any(|line| line == "disable-model-invocation: true")
        })
        .map(|(_, frontmatter)| frontmatter)
        .collect::<Vec<_>>()
        .join("\n")
}

/// What OpenCode v1.19.5 puts in the model's system prompt: `Skill.available` drops a skill
/// the agent's `permission.skill` denies, and `Skill.fmt` prints the description of the rest.
/// OpenCode reads no `disable-model-invocation`, so the launch's inline configuration is
/// all that can leave a description out.
fn opencode_listing(launch: &LaunchCommand) -> String {
    let config = launch
        .environment
        .iter()
        .find(|(name, _)| name == "OPENCODE_CONFIG_CONTENT")
        .map(|(_, value)| value.to_str().unwrap())
        .unwrap();
    let config: serde_json::Value = serde_json::from_str(config).unwrap();
    let is_denied = |name: &str| config["permission"]["skill"][name] == "deny";
    config["skills"]["paths"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|path| skill_frontmatters(Path::new(path.as_str().unwrap())))
        .filter(|(name, _)| !is_denied(name))
        .map(|(_, frontmatter)| frontmatter)
        .collect::<Vec<_>>()
        .join("\n")
}

/// What Codex 0.166.0 lists: the skills of folders its launch is given, none (#419).
fn codex_listing(launch: &LaunchCommand) -> String {
    let state_dir_arguments = launch
        .arguments
        .iter()
        .chain(launch.environment.iter().map(|(_, value)| value));
    assert!(
        !state_dir_arguments
            .map(|word| word.to_string_lossy())
            .any(|word| word.contains("claude-skills") || word.contains("agent-context")),
        "a Codex launch names a skill folder"
    );
    String::new()
}

/// The `description` line of a pack skill, quotes stripped.
fn description_of(name: &str) -> String {
    let skill = cockpit_agent_context::skill_pack()
        .skills
        .iter()
        .find(|skill| skill.name == name)
        .unwrap();
    let line = skill
        .frontmatter()
        .lines()
        .find_map(|line| line.strip_prefix("description: "))
        .unwrap();
    line.trim_matches('"').to_owned()
}

fn names(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_launch_lists_only_the_pack_skills_the_owner_turned_on() {
    let home = Home::new();
    home.write_owner_config(STARTER_AND_PR);
    let _daemon = home.start().await.unwrap();

    let listed = claude_skills(&launch(&home.claude()));

    assert_eq!(
        listed,
        names(&["diagnosing-bugs", "pr", "torlek", "writing-for-agents"])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_opencode_launch_lists_only_the_pack_skills_the_owner_turned_on() {
    let home = Home::new();
    home.write_owner_config(STARTER_AND_PR);
    let _daemon = home.start().await.unwrap();

    let listed = opencode_skills(&launch(&home.opencode()));

    assert_eq!(
        listed,
        names(&["diagnosing-bugs", "pr", "torlek", "writing-for-agents"])
    );
}

/// Codex gets the MCP server and the hooks, no skill folder (#419): no pack skill reaches it.
#[tokio::test(flavor = "multi_thread")]
async fn a_codex_launch_is_pointed_at_no_skill_folder() {
    let home = Home::new();
    home.write_owner_config(STARTER_AND_PR);
    let _daemon = home.start().await.unwrap();
    let state_dir = home.state_dir();

    let launch = launch(&home.codex());

    let words: Vec<OsString> = launch
        .arguments
        .iter()
        .cloned()
        .chain(launch.environment.iter().map(|(_, value)| value.clone()))
        .collect();
    for word in words {
        assert!(
            !word
                .to_string_lossy()
                .contains(&*state_dir.to_string_lossy()),
            "{word:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn without_the_owners_setting_a_launch_lists_torleks_skill_alone() {
    let home = Home::new();
    let _daemon = home.start().await.unwrap();

    assert_eq!(claude_skills(&launch(&home.claude())), names(&["torlek"]));
    assert_eq!(
        opencode_skills(&launch(&home.opencode())),
        names(&["torlek"])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owners_skill_with_the_same_name_wins_over_the_packs() {
    let home = Home::new();
    home.write_owner_config("[skill_pack]\nenabled = true\n");
    home.owner_skill("tdd");
    let _daemon = home.start().await.unwrap();

    assert_eq!(
        claude_skills(&launch(&home.claude())),
        names(&["diagnosing-bugs", "torlek", "writing-for-agents"])
    );
}

/// Claude Code reads only `.claude/skills`; OpenCode also reads `.agents/skills`. A skill of
/// the owner's there wins for OpenCode alone, so a Claude session still has the pack's.
#[tokio::test(flavor = "multi_thread")]
async fn an_owners_skill_wins_only_for_a_harness_that_reads_its_folder() {
    let home = Home::new();
    home.write_owner_config("[skill_pack]\nenabled = true\n");
    home.owner_skill_in(".agents/skills", "tdd");
    let _daemon = home.start().await.unwrap();

    assert_eq!(
        claude_skills(&launch(&home.claude())),
        names(&["diagnosing-bugs", "tdd", "torlek", "writing-for-agents"])
    );
    assert_eq!(
        opencode_skills(&launch(&home.opencode())),
        names(&["diagnosing-bugs", "torlek", "writing-for-agents"])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_trusted_projects_skill_with_the_same_name_wins_over_the_packs() {
    let home = Home::new();
    let project = home.path().join("app");
    let skill = project.join(".agents/skills/diagnosing-bugs");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "---\nname: diagnosing-bugs\n---\n").unwrap();
    home.write_owner_config(&format!(
        "[projects]\ntrusted = [\"{}\"]\n\n[skill_pack]\nenabled = true\n",
        project.display()
    ));
    let _daemon = home.start().await.unwrap();

    assert_eq!(
        opencode_skills(&launch(&home.opencode())),
        names(&["tdd", "torlek", "writing-for-agents"])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_the_pack_off_removes_its_skills_at_the_next_start() {
    let home = Home::new();
    home.write_owner_config("[skill_pack]\nenabled = true\n");
    drop(home.start().await.unwrap());
    home.write_owner_config("[skill_pack]\nenabled = false\n");

    let _daemon = home.restart().await;

    assert_eq!(claude_skills(&launch(&home.claude())), names(&["torlek"]));
    assert!(!home.state_dir().join("agent-context/skill-pack").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_skill_name_the_pack_does_not_hold_fails_the_start() {
    let home = Home::new();
    home.write_owner_config("[skill_pack]\nenabled = true\nskills = { triage = true }\n");

    let refused = home.start().await.err().expect("the start is refused");

    assert!(refused.to_string().contains("\"triage\""), "{refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_lists_the_pack_beside_the_installed_skills_with_collisions_marked() {
    let home = Home::new();
    home.write_owner_config("[skill_pack]\nenabled = true\n");
    let owner_tdd = home.owner_skill("tdd");
    home.owner_skill("notes");
    let daemon = home.start().await.unwrap();
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    let reply = owner.request(Command::SkillPack, GENEROUS).await.unwrap();

    let Reply::SkillPack { pack } = reply else {
        panic!("{reply:?}");
    };
    let tdd = pack
        .skills
        .iter()
        .find(|skill| skill.name == "tdd")
        .unwrap();
    let shadowed = PackSkillStateView::Shadowed {
        by: owner_tdd.display().to_string(),
    };
    assert_eq!(
        tdd.deliveries,
        vec![
            PackDeliveryView {
                harness: "claude".to_owned(),
                state: shadowed.clone()
            },
            PackDeliveryView {
                harness: "opencode".to_owned(),
                state: shadowed
            },
        ]
    );
    let collisions: BTreeSet<(String, bool)> = pack
        .installed
        .iter()
        .map(|skill| (skill.name.clone(), skill.collides))
        .collect();
    assert_eq!(
        collisions,
        BTreeSet::from([("notes".to_owned(), false), ("tdd".to_owned(), true)])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_may_not_list_the_owners_skills() {
    let home = Home::new();
    let daemon = home.start().await.unwrap();
    let agent = Connection::open(daemon.agent_socket_path(), GENEROUS)
        .await
        .unwrap();

    let refused = agent.request(Command::SkillPack, GENEROUS).await;

    assert!(refused.is_err(), "{refused:?}");
}

const WAIT_WHAT_AND_TDD: &str =
    "[skill_pack]\nenabled = true\nskills = { wait-what = true, tdd = true }\n";

/// A listing that names `tdd`'s description shows the fixture reads the delivered skills at
/// all, so the absence of `wait-what`'s is not an empty listing's.
fn assert_lists_tdd_but_not_wait_what(harness: &str, listing: &str) {
    assert!(
        listing.contains(&description_of("tdd")),
        "{harness} lists no description of a skill the model may invoke:\n{listing}"
    );
    assert!(
        !listing.contains(&description_of("wait-what")),
        "{harness} lists the description of a user-invoked skill:\n{listing}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_codes_listing_has_no_description_of_a_user_invoked_skill() {
    let home = Home::new();
    home.write_owner_config(WAIT_WHAT_AND_TDD);
    let _daemon = home.start().await.unwrap();

    assert_lists_tdd_but_not_wait_what("Claude Code", &claude_listing(&launch(&home.claude())));
}

#[tokio::test(flavor = "multi_thread")]
async fn opencodes_listing_has_no_description_of_a_user_invoked_skill() {
    let home = Home::new();
    home.write_owner_config(WAIT_WHAT_AND_TDD);
    let _daemon = home.start().await.unwrap();

    assert_lists_tdd_but_not_wait_what("OpenCode", &opencode_listing(&launch(&home.opencode())));
}

/// Without the adapter's denial OpenCode lists the description: the fixture is not vacuous.
#[tokio::test(flavor = "multi_thread")]
async fn opencode_would_list_a_user_invoked_skill_without_the_denial() {
    let home = Home::new();
    home.write_owner_config(WAIT_WHAT_AND_TDD);
    let _daemon = home.start().await.unwrap();
    let undenied = cockpit_opencode::OpenCode::new(
        "/usr/bin/opencode".into(),
        None,
        Some(cockpit_opencode::Bridge {
            plugin: "/opt/torlek/plugin.js".into(),
            bridge_program: "/opt/torlek/bin/bridge".into(),
            cockpit_program: "/opt/torlek/bin/torlek".into(),
        }),
    )
    .with_instruction_skills(home.state_dir().join("agent-context"));

    let listing = opencode_listing(&launch(&undenied));

    assert!(listing.contains(&description_of("wait-what")), "{listing}");
}

#[tokio::test(flavor = "multi_thread")]
async fn codexs_listing_has_no_description_of_a_user_invoked_skill() {
    let home = Home::new();
    home.write_owner_config(WAIT_WHAT_AND_TDD);
    let _daemon = home.start().await.unwrap();

    let listing = codex_listing(&launch(&home.codex()));

    assert!(!listing.contains(&description_of("wait-what")), "{listing}");
}

/// An owner's skill of the same name wins over the pack's for OpenCode, so the adapter has no
/// pack skill to deny: denying the name would hide the owner's own skill from the model.
#[tokio::test(flavor = "multi_thread")]
async fn opencode_does_not_deny_an_owners_skill_that_shadows_a_user_invoked_one() {
    let home = Home::new();
    home.write_owner_config(WAIT_WHAT_AND_TDD);
    home.owner_skill_in(".agents/skills", "wait-what");
    let _daemon = home.start().await.unwrap();

    let launch = launch(&home.opencode());

    let config = launch
        .environment
        .iter()
        .find(|(name, _)| name == "OPENCODE_CONFIG_CONTENT")
        .map(|(_, value)| value.to_str().unwrap())
        .unwrap();
    let config: serde_json::Value = serde_json::from_str(config).unwrap();
    assert_eq!(config.get("permission"), None);
}
