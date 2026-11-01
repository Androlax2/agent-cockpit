//! The owner's `[agents]` settings: `desktop_session`, the one opt-out that keeps launched
//! agents off the owner's display and session bus (#305), read through `cockpit_config::load`
//! from an isolated home.

use std::fs;
use std::path::Path;

use cockpit_config::{ConfigError, DesktopSession, EffectiveConfig, Environment, Source};
use tempfile::TempDir;

fn write_user_config(home: &Path, contents: &str) {
    let config_dir = home.join(".config/cockpit");
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
fn a_clean_home_shares_the_desktop_session_with_agents() {
    let home = TempDir::new().unwrap();

    let config = load(home.path(), None).unwrap();

    assert_eq!(
        config.agents_desktop_session,
        cockpit_config::Sourced::new(DesktopSession::Shared, Source::ProductDefault)
    );
}

#[test]
fn the_owner_file_withholds_the_desktop_session_with_its_line() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[agents]\ndesktop_session = \"withheld\"\n");

    let config = load(home.path(), None).unwrap();

    assert_eq!(
        config.agents_desktop_session.value,
        DesktopSession::Withheld
    );
    assert_eq!(
        config.agents_desktop_session.source,
        Source::UserFile {
            path: config.user_file.path.clone(),
            line: 2
        }
    );
}

#[test]
fn an_unknown_desktop_session_value_fails_to_parse() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[agents]\ndesktop_session = \"none\"\n");

    assert!(matches!(
        load(home.path(), None),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn a_project_file_cannot_set_the_agents_settings() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    fs::create_dir_all(project.path().join(".cockpit")).unwrap();
    fs::write(
        project.path().join(".cockpit/config.toml"),
        "[agents]\ndesktop_session = \"shared\"\n",
    )
    .unwrap();

    let error = load(home.path(), Some(project.path())).unwrap_err();

    assert!(
        matches!(error, ConfigError::OwnerOnly { key: "agents", .. }),
        "{error}"
    );
}

#[test]
fn config_show_prints_the_desktop_session_with_its_source() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[agents]\ndesktop_session = \"withheld\"\n");

    let config = load(home.path(), None).unwrap();
    let shown = cockpit_config::render(&config);

    assert!(
        shown.contains(&format!(
            "agents.desktop_session = \"withheld\"  # user config {}:2\n",
            config.user_file.path.display()
        )),
        "{shown}"
    );
}
