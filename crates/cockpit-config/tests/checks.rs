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

/// A project directory whose `.torlek/config.toml` holds `contents`.
fn project(home: &TempDir, contents: &str) -> PathBuf {
    let project = home.path().join("project");
    fs::create_dir_all(project.join(".torlek")).unwrap();
    fs::write(project.join(".torlek/config.toml"), contents).unwrap();
    project
}

#[test]
fn a_project_declares_its_checks_with_their_lines() {
    let home = TempDir::new().unwrap();
    let project = project(
        &home,
        "[[checks]]\nname = \"test\"\ncommand = \"cargo test\"\nartifacts = [\"src/lib.rs\"]\n\n\
         [[checks]]\nname = \"lint\"\ncommand = \"make lint\"\nworking_directory = \"tools\"\n\
         environment = \"native\"\n",
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
                environment: "sandbox".to_owned(),
                artifacts: vec!["src/lib.rs".to_owned()],
                image: None,
            },
            &Check {
                name: "lint".to_owned(),
                command: "make lint".to_owned(),
                working_directory: "tools".to_owned(),
                environment: "native".to_owned(),
                artifacts: Vec::new(),
                image: None,
            },
        ]
    );
    assert_eq!(
        config.checks[1].source,
        Source::ProjectFile {
            path: project.join(".torlek/config.toml"),
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

/// #352: a docker check runs in the image it names.
#[test]
fn a_docker_check_declares_its_image() {
    let home = TempDir::new().unwrap();
    let project = project(
        &home,
        "[[checks]]\nname = \"test\"\ncommand = \"cargo test\"\nenvironment = \"docker\"\n\
         image = \"docker.io/library/rust:1.86@sha256:0123abcd\"\n",
    );

    let config = load(home.path(), Some(&project)).unwrap();

    assert_eq!(
        config.checks[0].value.image.as_deref(),
        Some("docker.io/library/rust:1.86@sha256:0123abcd")
    );
}

#[test]
fn the_shown_configuration_names_a_docker_checks_image() {
    let home = TempDir::new().unwrap();
    let project = project(
        &home,
        "[[checks]]\nname = \"test\"\ncommand = \"make\"\nenvironment = \"docker\"\n\
         image = \"rust:1.86\"\n",
    );

    let shown = cockpit_config::render(&load(home.path(), Some(&project)).unwrap());

    let line = shown
        .lines()
        .find(|line| line.starts_with("checks.\"test\" = "))
        .expect("the check is shown");
    assert!(line.contains("image = \"rust:1.86\""), "{line}");
}

#[test]
fn a_docker_check_without_an_image_fails() {
    let home = TempDir::new().unwrap();
    let project = project(
        &home,
        "[[checks]]\nname = \"test\"\ncommand = \"make\"\nenvironment = \"docker\"\n",
    );

    let error = load(home.path(), Some(&project)).unwrap_err().to_string();

    assert!(
        error.contains("\"test\" runs in docker and names no image"),
        "{error}"
    );
}

#[test]
fn an_image_on_a_check_that_runs_elsewhere_fails() {
    let home = TempDir::new().unwrap();
    for environment in [
        "",
        "environment = \"sandbox\"\n",
        "environment = \"native\"\n",
    ] {
        let project = project(
            &home,
            &format!(
                "[[checks]]\nname = \"test\"\ncommand = \"make\"\n{environment}image = \"rust\"\n"
            ),
        );

        let error = load(home.path(), Some(&project)).unwrap_err().to_string();

        assert!(
            error.contains("names an image but runs in"),
            "{environment}: {error}"
        );
    }
}

/// An image never reaches an engine's command line as an option or a second word.
#[test]
fn an_image_that_is_no_image_reference_fails() {
    let home = TempDir::new().unwrap();
    let too_long = "a".repeat(256);
    for image in [
        "",
        "-v/:/host",
        "rust latest",
        "rust;true",
        too_long.as_str(),
    ] {
        let project = project(
            &home,
            &format!(
                "[[checks]]\nname = \"test\"\ncommand = \"make\"\nenvironment = \"docker\"\n\
                 image = {image:?}\n"
            ),
        );

        let error = load(home.path(), Some(&project)).unwrap_err().to_string();

        assert!(error.contains("names the image"), "{image:?}: {error}");
    }
}

#[test]
fn the_owner_file_cannot_declare_checks_for_every_project() {
    let home = TempDir::new().unwrap();
    let config_dir = home.path().join(".config/torlek");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        "[[checks]]\nname = \"test\"\ncommand = \"make test\"\n",
    )
    .unwrap();

    let error = load(home.path(), None).unwrap_err().to_string();

    assert!(error.contains("describes a project"), "{error}");
}
