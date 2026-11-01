//! The project's `[[checks]]` (#313, report 09): the checks Cockpit runs and records, read
//! through `cockpit_config::load` from an isolated home and project.

use std::fs;
use std::path::{Path, PathBuf};

use cockpit_config::{Check, ConfigError, EffectiveConfig, Environment, Source};
use tempfile::TempDir;

fn load(home: &Path, project_dir: Option<&Path>) -> Result<EffectiveConfig, ConfigError> {
    let environment = Environment {
        home: Some(home.into()),
        ..Environment::default()
    };
    cockpit_config::load(&environment, project_dir, &[])
}

/// A project directory whose `.cockpit/config.toml` holds `contents`.
fn project(home: &TempDir, contents: &str) -> PathBuf {
    let project = home.path().join("project");
    fs::create_dir_all(project.join(".cockpit")).unwrap();
    fs::write(project.join(".cockpit/config.toml"), contents).unwrap();
    project
}

#[test]
fn a_project_declares_its_checks_with_their_lines() {
    let home = TempDir::new().unwrap();
    let project = project(
        &home,
        "[[checks]]\nname = \"test\"\ncommand = \"cargo test\"\nartifacts = [\"src/lib.rs\"]\n\n\
         [[checks]]\nname = \"lint\"\ncommand = \"make lint\"\nworking_directory = \"tools\"\n\
         environment = \"sandbox\"\n",
    );

    let config = load(home.path(), Some(&project)).unwrap();

    let checks: Vec<&Check> = config.checks.iter().map(|check| &check.value).collect();
    assert_eq!(
        checks,
        [
            &Check {
                name: "test".to_owned(),
                command: "cargo test".to_owned(),
                working_directory: ".".to_owned(),
                environment: "native".to_owned(),
                artifacts: vec!["src/lib.rs".to_owned()],
            },
            &Check {
                name: "lint".to_owned(),
                command: "make lint".to_owned(),
                working_directory: "tools".to_owned(),
                environment: "sandbox".to_owned(),
                artifacts: Vec::new(),
            },
        ]
    );
    assert_eq!(
        config.checks[1].source,
        Source::ProjectFile {
            path: project.join(".cockpit/config.toml"),
            line: 6,
        }
    );
}

#[test]
fn a_project_without_checks_declares_none() {
    let home = TempDir::new().unwrap();
    let project = project(&home, "");

    assert!(load(home.path(), Some(&project)).unwrap().checks.is_empty());
}

#[test]
fn a_check_named_twice_fails_with_its_line() {
    let home = TempDir::new().unwrap();
    let project = project(
        &home,
        "[[checks]]\nname = \"test\"\ncommand = \"a\"\n\n[[checks]]\nname = \"test\"\ncommand = \"b\"\n",
    );

    let error = load(home.path(), Some(&project)).unwrap_err().to_string();

    assert!(error.contains(":5: `checks`"), "{error}");
    assert!(error.contains("repeats the name \"test\""), "{error}");
}

#[test]
fn a_check_without_a_name_or_a_command_fails() {
    let home = TempDir::new().unwrap();
    for contents in [
        "[[checks]]\nname = \" \"\ncommand = \"make\"\n",
        "[[checks]]\nname = \"test\"\ncommand = \"\"\n",
    ] {
        let project = project(&home, contents);
        let error = load(home.path(), Some(&project)).unwrap_err();
        assert!(
            matches!(&error, ConfigError::Invalid { key, .. } if key == "checks"),
            "{contents}: {error}"
        );
    }
}

#[test]
fn the_owner_file_cannot_declare_checks_for_every_project() {
    let home = TempDir::new().unwrap();
    let config_dir = home.path().join(".config/cockpit");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        "[[checks]]\nname = \"test\"\ncommand = \"make test\"\n",
    )
    .unwrap();

    let error = load(home.path(), None).unwrap_err().to_string();

    assert!(error.contains("describes a project"), "{error}");
}
