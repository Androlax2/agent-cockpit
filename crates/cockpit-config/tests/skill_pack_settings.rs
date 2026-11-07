//! The owner's `[skill_pack]` settings (#786): off by default, each skill on or off by name,
//! and the pin the owner confirmed for skills that grant tools. Which names exist is the
//! pack's to check (`cockpit-agent-context`); this crate reads the values with their source.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use cockpit_config::{ConfigError, EffectiveConfig, Environment, Source};
use tempfile::TempDir;

fn write_user_config(home: &Path, contents: &str) {
    let config_dir = home.join(".config/torlek");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("config.toml"), contents).unwrap();
}

fn load(home: &Path, project_dir: Option<&Path>) -> Result<EffectiveConfig, ConfigError> {
    let environment = Environment {
        home: Some(home.into()),
        ..Environment::default()
    };
    cockpit_config::load(&environment, project_dir, &[])
}

#[test]
fn a_clean_home_leaves_the_skill_pack_off() {
    let home = TempDir::new().unwrap();

    let config = load(home.path(), None).unwrap();

    assert!(!config.skill_pack.enabled.value);
    assert_eq!(config.skill_pack.enabled.source, Source::ProductDefault);
    assert!(config.skill_pack.skills.value.is_empty());
    assert_eq!(config.skill_pack.grants_confirmed_at.value, None);
}

#[test]
fn the_owner_turns_the_pack_on_with_its_line() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[skill_pack]\nenabled = true\n");

    let config = load(home.path(), None).unwrap();

    assert!(config.skill_pack.enabled.value);
    assert_eq!(
        config.skill_pack.enabled.source,
        Source::UserFile {
            path: config.user_file.path.clone(),
            line: 2
        }
    );
}

#[test]
fn the_owner_turns_skills_on_or_off_by_name() {
    let home = TempDir::new().unwrap();
    write_user_config(
        home.path(),
        "[skill_pack]\nenabled = true\nskills = { pr = true, wait-what = false }\n",
    );

    let config = load(home.path(), None).unwrap();

    assert_eq!(
        config.skill_pack.skills.value,
        BTreeMap::from([("pr".to_owned(), true), ("wait-what".to_owned(), false)])
    );
}

#[test]
fn the_owner_confirms_a_pin_whose_skills_grant_tools() {
    let home = TempDir::new().unwrap();
    write_user_config(
        home.path(),
        "[skill_pack]\ngrants_confirmed_at = \"4ae2317ddb22b046e26455e4f9c9b3aaaf62212d\"\n",
    );

    let config = load(home.path(), None).unwrap();

    assert_eq!(
        config.skill_pack.grants_confirmed_at.value.as_deref(),
        Some("4ae2317ddb22b046e26455e4f9c9b3aaaf62212d")
    );
}

#[test]
fn an_unknown_skill_pack_key_fails_to_parse() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[skill_pack]\nturned_on = true\n");

    assert!(matches!(
        load(home.path(), None),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn a_project_file_cannot_turn_the_skill_pack_on() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    fs::create_dir_all(project.path().join(".torlek")).unwrap();
    fs::write(
        project.path().join(".torlek/config.toml"),
        "[skill_pack]\nenabled = true\n",
    )
    .unwrap();

    assert!(matches!(
        load(home.path(), Some(project.path())),
        Err(ConfigError::OwnerOnly {
            key: "skill_pack",
            ..
        })
    ));
}

#[test]
fn config_show_lists_the_skill_pack_settings_with_their_source() {
    let home = TempDir::new().unwrap();
    write_user_config(
        home.path(),
        "[skill_pack]\nenabled = true\nskills = { pr = true }\n",
    );

    let rendered = cockpit_config::render(&load(home.path(), None).unwrap());

    assert!(rendered.contains("skill_pack.enabled = true"), "{rendered}");
    assert!(
        rendered.contains("skill_pack.skills.pr = true"),
        "{rendered}"
    );
    assert!(
        rendered.contains("skill_pack.grants_confirmed_at = <unset>"),
        "{rendered}"
    );
}
