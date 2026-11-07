//! The optional skill pack (#786): vendored from one upstream commit with its licence, off
//! until the owner turns it on, each skill on or off by name, an owner's or project's skill
//! of the same name winning, and a skill that grants tools refused until the owner confirms
//! the pin.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use cockpit_agent_context::{
    InstalledSkill, Licence, PACK_DIR, Pack, PackChoice, PackError, PackFile, PackSkill, Scope,
    SkillSearch, SkillState, discover_skills, select_pack, skill_pack, write_pack,
};

const STARTER: [&str; 3] = ["diagnosing-bugs", "tdd", "writing-for-agents"];
const OPTIONAL: [&str; 3] = ["pr", "codebase-design", "wait-what"];

fn turned_on() -> PackChoice {
    PackChoice {
        enabled: true,
        ..PackChoice::default()
    }
}

fn with_skills(choice: PackChoice, skills: &[(&str, bool)]) -> PackChoice {
    PackChoice {
        skills: skills
            .iter()
            .map(|(name, is_on)| ((*name).to_owned(), *is_on))
            .collect(),
        ..choice
    }
}

fn delivered(choice: &PackChoice, installed: Vec<InstalledSkill>) -> BTreeSet<&'static str> {
    select_pack(skill_pack(), choice, installed)
        .unwrap()
        .delivered()
        .map(|skill| skill.name)
        .collect()
}

fn names(names: &[&'static str]) -> BTreeSet<&'static str> {
    names.iter().copied().collect()
}

fn installed(name: &str, scope: Scope) -> InstalledSkill {
    InstalledSkill {
        name: name.to_owned(),
        scope,
        path: PathBuf::from(format!("/owner/skills/{name}")),
    }
}

#[test]
fn the_pack_is_pinned_to_one_upstream_commit_and_tree() {
    let pack = skill_pack();

    assert_eq!(pack.source, "https://github.com/mattpocock/skills");
    assert_eq!(pack.commit, "4ae2317ddb22b046e26455e4f9c9b3aaaf62212d");
    assert_eq!(pack.tree, "a73b182b5f22565e829e0969eb0bfd284508a7b8");
}

#[test]
fn the_pack_carries_the_upstream_mit_licence() {
    let licence = &skill_pack().licence;

    assert_eq!(licence.spdx, "MIT");
    assert!(
        licence
            .text
            .starts_with("MIT License\n\nCopyright (c) 2026 Matt Pocock\n")
    );
}

#[test]
fn the_pack_holds_the_starter_and_the_optional_skills_only() {
    let pack = skill_pack();
    let starters: BTreeSet<&str> = pack
        .skills
        .iter()
        .filter(|skill| skill.is_starter)
        .map(|skill| skill.name)
        .collect();
    let all: BTreeSet<&str> = pack.skills.iter().map(|skill| skill.name).collect();

    assert_eq!(starters, names(&STARTER));
    assert_eq!(
        all,
        names(&STARTER).union(&names(&OPTIONAL)).copied().collect()
    );
}

#[test]
fn no_pack_skill_takes_the_name_of_torleks_own_skill_or_the_pack_folder() {
    for skill in &skill_pack().skills {
        assert_ne!(skill.name, "torlek");
        assert_ne!(skill.name, PACK_DIR);
    }
}

#[test]
fn no_skill_at_this_pin_grants_tools_or_hooks() {
    for skill in &skill_pack().skills {
        assert_eq!(skill.grants(), Vec::<&str>::new(), "{}", skill.name);
    }
}

#[test]
fn only_wait_what_is_invoked_by_the_user_alone() {
    let user_invoked: Vec<&str> = skill_pack()
        .skills
        .iter()
        .filter(|skill| skill.is_user_invoked())
        .map(|skill| skill.name)
        .collect();

    assert_eq!(user_invoked, ["wait-what"]);
}

/// Each skill's files are its upstream directory byte for byte: Git's tree id of them,
/// computed here with `git hash-object`, is the one recorded for the pin.
#[test]
fn each_vendored_skill_is_its_upstream_tree_byte_for_byte() {
    for skill in &skill_pack().skills {
        let files: Vec<(&str, &[u8])> = skill
            .files
            .iter()
            .map(|file| (file.path, file.text.as_bytes()))
            .collect();

        assert_eq!(git_tree_id(&files), skill.tree, "{}", skill.name);
    }
}

#[test]
fn the_vendored_licence_is_the_upstream_blob() {
    assert_eq!(
        git_hash(
            &["hash-object", "--stdin"],
            skill_pack().licence.text.as_bytes()
        ),
        "f1dd2c09108dde1a5f56097cee8461b3ea834499"
    );
}

#[test]
fn the_pack_is_off_until_the_owner_turns_it_on() {
    let choice = with_skills(PackChoice::default(), &[("tdd", true)]);

    assert_eq!(delivered(&choice, Vec::new()), BTreeSet::new());
}

#[test]
fn turning_the_pack_on_delivers_the_starter_skills() {
    assert_eq!(delivered(&turned_on(), Vec::new()), names(&STARTER));
}

#[test]
fn an_optional_skill_is_turned_on_by_name() {
    let choice = with_skills(turned_on(), &[("pr", true)]);

    assert_eq!(
        delivered(&choice, Vec::new()),
        names(&["diagnosing-bugs", "pr", "tdd", "writing-for-agents"])
    );
}

#[test]
fn a_starter_skill_is_turned_off_by_name() {
    let choice = with_skills(turned_on(), &[("tdd", false)]);

    assert_eq!(
        delivered(&choice, Vec::new()),
        names(&["diagnosing-bugs", "writing-for-agents"])
    );
}

#[test]
fn a_skill_name_the_pack_does_not_hold_is_refused() {
    let choice = with_skills(turned_on(), &[("triage", true)]);

    let refused = select_pack(skill_pack(), &choice, Vec::new()).unwrap_err();

    assert!(
        matches!(&refused, PackError::UnknownSkill { name, .. } if name == "triage"),
        "{refused:?}"
    );
}

#[test]
fn an_owners_skill_with_the_same_name_wins() {
    let owner_skill = installed("tdd", Scope::User);

    let listing = select_pack(skill_pack(), &turned_on(), vec![owner_skill.clone()]).unwrap();

    let tdd = listing
        .skills
        .iter()
        .find(|listed| listed.skill.name == "tdd")
        .unwrap();
    assert_eq!(
        tdd.state,
        SkillState::Shadowed {
            by: owner_skill.path
        }
    );
}

#[test]
fn a_projects_skill_with_the_same_name_wins() {
    let project_skill = installed("diagnosing-bugs", Scope::Project);

    assert_eq!(
        delivered(&turned_on(), vec![project_skill]),
        names(&["tdd", "writing-for-agents"])
    );
}

#[test]
fn the_listing_shows_the_installed_skills_beside_the_packs() {
    let installed_skills = vec![
        installed("tdd", Scope::User),
        installed("repo-workflow", Scope::Project),
    ];

    let listing = select_pack(skill_pack(), &turned_on(), installed_skills.clone()).unwrap();

    assert_eq!(listing.installed, installed_skills);
    assert!(listing.collides("tdd"));
    assert!(!listing.collides("repo-workflow"));
}

fn granting_pack() -> Pack {
    Pack {
        source: "https://example.test/skills",
        commit: "1111111111111111111111111111111111111111",
        tree: "2222222222222222222222222222222222222222",
        licence: Licence {
            spdx: "MIT",
            text: "MIT License\n",
        },
        skills: vec![PackSkill {
            name: "deploy",
            upstream_path: "skills/deploy",
            tree: "3333333333333333333333333333333333333333",
            is_starter: true,
            files: vec![PackFile {
                path: "SKILL.md",
                text: "---\nname: deploy\ndescription: Deploys.\nallowed-tools: Bash(git push:*)\n---\n\nPush.\n",
            }],
        }],
    }
}

#[test]
fn a_skill_that_grants_tools_is_refused_until_the_owner_confirms_the_pin() {
    let pack = granting_pack();

    let refused = select_pack(&pack, &turned_on(), Vec::new()).unwrap_err();

    assert!(
        matches!(&refused, PackError::UnconfirmedGrants { skills, commit }
            if skills == &[("deploy".to_owned(), vec!["allowed-tools"])] && *commit == pack.commit),
        "{refused:?}"
    );
}

#[test]
fn a_confirmation_of_another_pin_does_not_count() {
    let pack = granting_pack();
    let choice = PackChoice {
        grants_confirmed_at: Some("4ae2317ddb22b046e26455e4f9c9b3aaaf62212d".to_owned()),
        ..turned_on()
    };

    assert!(select_pack(&pack, &choice, Vec::new()).is_err());
}

#[test]
fn the_owners_confirmation_of_the_pin_delivers_a_skill_that_grants_tools() {
    let pack = granting_pack();
    let choice = PackChoice {
        grants_confirmed_at: Some(pack.commit.to_owned()),
        ..turned_on()
    };

    let listing = select_pack(&pack, &choice, Vec::new()).unwrap();

    assert_eq!(listing.delivered().count(), 1);
}

#[test]
fn a_skill_that_grants_tools_needs_no_confirmation_while_it_is_off() {
    let pack = granting_pack();

    assert!(select_pack(&pack, &PackChoice::default(), Vec::new()).is_ok());
}

#[test]
fn the_catalog_counts_each_delivered_skills_frontmatter() {
    let choice = with_skills(turned_on(), &[("wait-what", true)]);
    let listing = select_pack(skill_pack(), &choice, Vec::new()).unwrap();
    let frontmatter_bytes: usize = listing
        .delivered()
        .map(|skill| skill.frontmatter().len())
        .sum();

    assert_eq!(listing.catalog_bytes(false), frontmatter_bytes);
}

#[test]
fn a_harness_that_hides_user_invoked_skills_is_not_charged_for_them() {
    let choice = with_skills(turned_on(), &[("wait-what", true)]);
    let listing = select_pack(skill_pack(), &choice, Vec::new()).unwrap();
    let wait_what = skill_pack()
        .skills
        .iter()
        .find(|skill| skill.name == "wait-what")
        .unwrap();

    assert_eq!(
        listing.catalog_bytes(true),
        listing.catalog_bytes(false) - wait_what.frontmatter().len()
    );
}

#[test]
fn the_frontmatter_is_measured_without_its_fences() {
    let tdd = skill_pack()
        .skills
        .iter()
        .find(|skill| skill.name == "tdd")
        .unwrap();

    assert!(
        tdd.frontmatter().starts_with("name: tdd\n"),
        "{}",
        tdd.frontmatter()
    );
    assert!(!tdd.frontmatter().contains("---"));
}

fn skill_dir(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
    dir
}

#[test]
fn the_owners_and_the_projects_skills_are_found_in_every_harness_folder() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let claude = skill_dir(&home.path().join(".claude/skills"), "tdd");
    let agents = skill_dir(&home.path().join(".agents/skills"), "notes");
    let opencode = skill_dir(&home.path().join(".config/opencode/skills"), "review");
    let project_skill = skill_dir(&project.path().join(".agents/skills"), "repo-workflow");
    std::fs::create_dir_all(home.path().join(".claude/skills/not-a-skill")).unwrap();

    let found = discover_skills(&SkillSearch {
        home: Some(home.path()),
        project_roots: &[project.path().to_path_buf()],
    });

    let found: BTreeMap<String, (Scope, PathBuf)> = found
        .into_iter()
        .map(|skill| (skill.name, (skill.scope, skill.path)))
        .collect();
    assert_eq!(
        found,
        BTreeMap::from([
            ("notes".to_owned(), (Scope::User, agents)),
            ("repo-workflow".to_owned(), (Scope::Project, project_skill)),
            ("review".to_owned(), (Scope::User, opencode)),
            ("tdd".to_owned(), (Scope::User, claude)),
        ])
    );
}

fn listed_dirs(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn writing_the_pack_writes_the_delivered_skills_beside_the_licence_and_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let choice = with_skills(turned_on(), &[("tdd", false)]);
    let listing = select_pack(skill_pack(), &choice, Vec::new()).unwrap();

    write_pack(dir.path(), &listing).unwrap();

    assert_eq!(
        listed_dirs(dir.path()),
        BTreeSet::from(["diagnosing-bugs", "skill-pack", "writing-for-agents"].map(String::from))
    );
}

#[test]
fn a_delivered_skill_is_written_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let listing = select_pack(skill_pack(), &turned_on(), Vec::new()).unwrap();

    write_pack(dir.path(), &listing).unwrap();

    let tdd = skill_pack()
        .skills
        .iter()
        .find(|skill| skill.name == "tdd")
        .unwrap();
    for file in &tdd.files {
        assert_eq!(
            std::fs::read_to_string(dir.path().join("tdd").join(file.path)).unwrap(),
            file.text
        );
    }
}

#[test]
fn the_written_licence_is_the_upstreams() {
    let dir = tempfile::tempdir().unwrap();
    let listing = select_pack(skill_pack(), &turned_on(), Vec::new()).unwrap();

    write_pack(dir.path(), &listing).unwrap();

    assert_eq!(
        std::fs::read_to_string(dir.path().join(PACK_DIR).join("LICENSE")).unwrap(),
        skill_pack().licence.text
    );
}

#[test]
fn the_written_manifest_names_the_source_commit_tree_licence_and_each_skills_state() {
    let dir = tempfile::tempdir().unwrap();
    let listing = select_pack(
        skill_pack(),
        &turned_on(),
        vec![installed("tdd", Scope::User)],
    )
    .unwrap();

    write_pack(dir.path(), &listing).unwrap();

    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(PACK_DIR).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["source"], "https://github.com/mattpocock/skills");
    assert_eq!(manifest["commit"], skill_pack().commit);
    assert_eq!(manifest["tree"], skill_pack().tree);
    assert_eq!(manifest["licence"]["spdx"], "MIT");
    assert_eq!(manifest["licence"]["file"], "LICENSE");
    let states: BTreeMap<String, String> = manifest["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|skill| {
            (
                skill["name"].as_str().unwrap().to_owned(),
                skill["state"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(states["diagnosing-bugs"], "delivered");
    assert_eq!(states["tdd"], "shadowed");
    assert_eq!(states["pr"], "off");
}

#[test]
fn a_skill_turned_off_is_removed_from_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let on = select_pack(skill_pack(), &turned_on(), Vec::new()).unwrap();
    write_pack(dir.path(), &on).unwrap();
    let tdd_off = with_skills(turned_on(), &[("tdd", false)]);

    write_pack(
        dir.path(),
        &select_pack(skill_pack(), &tdd_off, Vec::new()).unwrap(),
    )
    .unwrap();

    assert!(!dir.path().join("tdd").exists());
}

#[test]
fn turning_the_pack_off_leaves_nothing_of_it_and_keeps_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("torlek")).unwrap();
    std::fs::write(dir.path().join("torlek/SKILL.md"), "torlek\n").unwrap();
    let on = select_pack(skill_pack(), &turned_on(), Vec::new()).unwrap();
    write_pack(dir.path(), &on).unwrap();

    write_pack(
        dir.path(),
        &select_pack(skill_pack(), &PackChoice::default(), Vec::new()).unwrap(),
    )
    .unwrap();

    assert_eq!(
        listed_dirs(dir.path()),
        BTreeSet::from(["torlek".to_owned()])
    );
}

#[test]
fn a_file_an_earlier_pin_left_in_a_delivered_skill_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tdd")).unwrap();
    std::fs::write(dir.path().join("tdd/old.md"), "old\n").unwrap();

    write_pack(
        dir.path(),
        &select_pack(skill_pack(), &turned_on(), Vec::new()).unwrap(),
    )
    .unwrap();

    assert!(!dir.path().join("tdd/old.md").exists());
}

#[cfg(unix)]
#[test]
fn a_link_where_a_skill_goes_is_replaced_never_followed() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), dir.path().join("tdd")).unwrap();

    write_pack(
        dir.path(),
        &select_pack(skill_pack(), &PackChoice::default(), Vec::new()).unwrap(),
    )
    .unwrap();

    assert!(!dir.path().join("tdd").exists());
    assert!(elsewhere.path().exists());
}

/// Git's id of the tree holding `files` (paths relative to it, `/`-separated), each a
/// regular, non-executable file, as `git hash-object` and Git's tree format compute it.
fn git_tree_id(files: &[(&str, &[u8])]) -> String {
    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut subtrees: BTreeMap<&str, Vec<(&str, &[u8])>> = BTreeMap::new();
    for (path, bytes) in files {
        match path.split_once('/') {
            Some((directory, rest)) => subtrees.entry(directory).or_default().push((rest, bytes)),
            None => {
                let blob = git_hash(&["hash-object", "--stdin"], bytes);
                entries.insert((*path).to_owned(), tree_entry("100644", path, &blob));
            }
        }
    }
    for (directory, inside) in subtrees {
        let tree = git_tree_id(&inside);
        // Git sorts a tree's entries by name, a directory's name as if it ended in `/`.
        entries.insert(
            format!("{directory}/"),
            tree_entry("40000", directory, &tree),
        );
    }
    let object: Vec<u8> = entries.into_values().flatten().collect();
    git_hash(
        &["hash-object", "-t", "tree", "--stdin", "--literally"],
        &object,
    )
}

fn tree_entry(mode: &str, name: &str, id: &str) -> Vec<u8> {
    let mut entry = format!("{mode} {name}\0").into_bytes();
    entry.extend(
        (0..id.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&id[at..at + 2], 16).unwrap()),
    );
    entry
}

fn git_hash(arguments: &[&str], input: &[u8]) -> String {
    use std::io::Write;
    let mut git = Command::new("git")
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("git runs");
    git.stdin.take().unwrap().write_all(input).unwrap();
    let output = git.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Every file under `pack/` is the licence or a file of a listed skill, so a file added there
/// cannot ship unpinned.
#[test]
fn the_pack_folder_holds_nothing_the_pin_does_not_list() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("pack");
    let mut listed: BTreeSet<PathBuf> = skill_pack()
        .skills
        .iter()
        .flat_map(|skill| {
            skill
                .files
                .iter()
                .map(|file| Path::new(skill.name).join(file.path))
        })
        .collect();
    listed.insert(PathBuf::from("LICENSE"));

    assert_eq!(files_under(&root, Path::new("")), listed);
}

fn files_under(root: &Path, relative: &Path) -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::new();
    for entry in std::fs::read_dir(root.join(relative)).unwrap() {
        let entry = entry.unwrap();
        let path = relative.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            files.extend(files_under(root, &path));
        } else {
            files.insert(path);
        }
    }
    files
}
