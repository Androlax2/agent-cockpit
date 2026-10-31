//! The owner's `[sessions]` settings (#144): the idle window and the asleep expiry, read
//! through `cockpit_config::load` from an isolated home.

use std::fs;
use std::path::Path;
use std::time::Duration;

use cockpit_config::{ConfigError, EffectiveConfig, Environment, Source};
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
fn a_clean_home_uses_the_product_idle_window_and_expiry() {
    let home = TempDir::new().unwrap();

    let config = load(home.path(), None).unwrap();

    assert_eq!(config.idle_sleep_after.value, Duration::from_secs(60 * 60));
    assert_eq!(config.idle_sleep_after.source, Source::ProductDefault);
    assert_eq!(config.asleep_expiry.value, Duration::from_secs(7 * 86_400));
    assert_eq!(config.asleep_expiry.source, Source::ProductDefault);
}

#[test]
fn the_owner_file_sets_the_idle_window_and_expiry_with_their_lines() {
    let home = TempDir::new().unwrap();
    write_user_config(
        home.path(),
        "[sessions]\nidle_sleep_after_minutes = 45\nasleep_expiry_days = 2\n",
    );

    let config = load(home.path(), None).unwrap();

    let user_file = config.user_file.path.clone();
    assert_eq!(config.idle_sleep_after.value, Duration::from_secs(45 * 60));
    assert_eq!(
        config.idle_sleep_after.source,
        Source::UserFile {
            path: user_file.clone(),
            line: 2
        }
    );
    assert_eq!(config.asleep_expiry.value, Duration::from_secs(2 * 86_400));
    assert_eq!(
        config.asleep_expiry.source,
        Source::UserFile {
            path: user_file,
            line: 3
        }
    );
}

#[test]
fn an_idle_window_below_thirty_minutes_fails_with_its_line() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[sessions]\nidle_sleep_after_minutes = 29\n");

    let error = load(home.path(), None).unwrap_err().to_string();

    assert!(error.contains("config.toml:2"), "{error}");
    assert!(
        error.contains("sessions.idle_sleep_after_minutes"),
        "{error}"
    );
    assert!(error.contains("at least 30"), "{error}");
}

#[test]
fn an_expiry_below_one_day_fails_with_its_line() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[sessions]\nasleep_expiry_days = 0\n");

    let error = load(home.path(), None).unwrap_err().to_string();

    assert!(error.contains("config.toml:2"), "{error}");
    assert!(error.contains("sessions.asleep_expiry_days"), "{error}");
    assert!(error.contains("at least 1"), "{error}");
}

#[test]
fn a_negative_idle_window_fails_instead_of_wrapping() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[sessions]\nidle_sleep_after_minutes = -60\n");

    assert!(matches!(
        load(home.path(), None),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn a_project_file_cannot_set_the_sessions_settings() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    fs::create_dir_all(project.path().join(".cockpit")).unwrap();
    fs::write(
        project.path().join(".cockpit/config.toml"),
        "[sessions]\nidle_sleep_after_minutes = 30\n",
    )
    .unwrap();

    let error = load(home.path(), Some(project.path())).unwrap_err();

    assert!(
        matches!(
            error,
            ConfigError::OwnerOnly {
                key: "sessions",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn config_show_prints_both_settings_with_their_source() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[sessions]\nidle_sleep_after_minutes = 45\n");

    let config = load(home.path(), None).unwrap();
    let shown = cockpit_config::render(&config);

    let user_file = &config.user_file.path;
    assert!(
        shown.contains(&format!(
            "sessions.idle_sleep_after_minutes = 45  # user config {}:2\n",
            user_file.display()
        )),
        "{shown}"
    );
    assert!(
        shown.contains("sessions.asleep_expiry_days = 7  # product default\n"),
        "{shown}"
    );
}
