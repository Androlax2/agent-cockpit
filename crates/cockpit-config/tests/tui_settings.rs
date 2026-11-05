//! The owner's `[tui]` settings, read through `cockpit_config::load` from an isolated home:
//! `attention_bell`, whether the TUI rings the terminal bell when a session needs the owner
//! (#682), and `phone_below_columns`, the width below which it takes its phone layout (#855).

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
fn a_clean_home_rings_the_attention_bell() {
    let home = TempDir::new().unwrap();

    let config = load(home.path(), None).unwrap();

    assert!(config.tui_attention_bell.value);
    assert_eq!(config.tui_attention_bell.source, Source::ProductDefault);
}

#[test]
fn the_owner_file_turns_the_attention_bell_off_with_its_line() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[tui]\nattention_bell = false\n");

    let config = load(home.path(), None).unwrap();

    assert!(!config.tui_attention_bell.value);
    assert_eq!(
        config.tui_attention_bell.source,
        Source::UserFile {
            path: config.user_file.path.clone(),
            line: 2
        }
    );
}

#[test]
fn a_bell_value_that_is_not_a_boolean_fails_to_parse() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[tui]\nattention_bell = \"off\"\n");

    assert!(matches!(
        load(home.path(), None),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn an_unknown_tui_key_fails_to_parse() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[tui]\nattention_sound = true\n");

    assert!(matches!(
        load(home.path(), None),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn a_project_file_cannot_set_the_tui_settings() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    fs::create_dir_all(project.path().join(".torlek")).unwrap();
    fs::write(
        project.path().join(".torlek/config.toml"),
        "[tui]\nattention_bell = false\n",
    )
    .unwrap();

    let error = load(home.path(), Some(project.path())).unwrap_err();

    assert!(
        matches!(error, ConfigError::OwnerOnly { key: "tui", .. }),
        "{error}"
    );
}

#[test]
fn config_show_prints_the_attention_bell_with_its_source() {
    let home = TempDir::new().unwrap();

    let shown = cockpit_config::render(&load(home.path(), None).unwrap());

    assert!(
        shown.contains("tui.attention_bell = true  # product default\n"),
        "{shown}"
    );
}

#[test]
fn a_clean_home_takes_the_phone_layout_below_sixty_columns() {
    let home = TempDir::new().unwrap();

    let config = load(home.path(), None).unwrap();

    assert_eq!(config.tui_phone_below_columns.value, 60);
    assert_eq!(
        config.tui_phone_below_columns.source,
        Source::ProductDefault
    );
}

#[test]
fn the_owner_file_sets_the_phone_width_with_its_line() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[tui]\nphone_below_columns = 0\n");

    let config = load(home.path(), None).unwrap();

    assert_eq!(config.tui_phone_below_columns.value, 0);
    assert_eq!(
        config.tui_phone_below_columns.source,
        Source::UserFile {
            path: config.user_file.path.clone(),
            line: 2
        }
    );
}

#[test]
fn a_negative_phone_width_fails_with_its_line() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[tui]\nphone_below_columns = -1\n");

    let error = load(home.path(), None).unwrap_err().to_string();

    assert!(error.contains("config.toml:2"), "{error}");
    assert!(error.contains("tui.phone_below_columns"), "{error}");
}

#[test]
fn config_show_prints_the_phone_width_with_its_source() {
    let home = TempDir::new().unwrap();

    let shown = cockpit_config::render(&load(home.path(), None).unwrap());

    assert!(
        shown
            .lines()
            .any(|line| line == "tui.phone_below_columns = 60  # product default"),
        "{shown}"
    );
}
