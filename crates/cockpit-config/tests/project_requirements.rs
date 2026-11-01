//! What a project describes about itself beyond its checks (#50, report 09): where its work
//! expects to run, what it needs reserved and the result files it produces. Read through
//! `cockpit_config::load` from an isolated home and project.

use std::fs;
use std::path::{Path, PathBuf};

use cockpit_config::{
    ConfigError, EffectiveConfig, Environment, ExecutionEnvironment, ProjectNeeds, Source,
};
use tempfile::TempDir;

fn load(home: &Path, project_dir: Option<&Path>) -> Result<EffectiveConfig, ConfigError> {
    let environment = Environment {
        home: Some(home.into()),
        ..Environment::default()
    };
    cockpit_config::load(&environment, project_dir, &[])
}

fn project(home: &TempDir, contents: &str) -> PathBuf {
    let project = home.path().join("project");
    fs::create_dir_all(project.join(".cockpit")).unwrap();
    fs::write(project.join(".cockpit/config.toml"), contents).unwrap();
    project
}

fn owner_file(home: &TempDir, contents: &str) {
    let dir = home.path().join(".config/cockpit");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), contents).unwrap();
}

#[test]
fn a_project_describes_its_environment_needs_and_artifacts_with_their_lines() {
    let home = TempDir::new().unwrap();
    let project = project(
        &home,
        "[execution]\nenvironment = \"docker\"\n\n[needs]\ncpu_millis = 2000\nmemory_mib = 4096\n\n\
         [results]\nartifacts = [\"dist/app.tar\"]\n",
    );

    let config = load(home.path(), Some(&project)).unwrap();

    let file = project.join(".cockpit/config.toml");
    assert_eq!(
        config.execution_environment.value,
        Some(ExecutionEnvironment::Docker)
    );
    assert_eq!(
        config.execution_environment.source,
        Source::ProjectFile {
            path: file.clone(),
            line: 2
        }
    );
    assert_eq!(
        config.needs.value,
        Some(ProjectNeeds {
            cpu_millis: 2000,
            memory_mib: 4096
        })
    );
    assert_eq!(config.result_artifacts.value, ["dist/app.tar"]);
    assert_eq!(
        config.result_artifacts.source,
        Source::ProjectFile {
            path: file,
            line: 9
        }
    );
}

#[test]
fn a_project_that_describes_nothing_asks_for_nothing() {
    let home = TempDir::new().unwrap();
    let project = project(&home, "");

    let config = load(home.path(), Some(&project)).unwrap();

    assert_eq!(config.execution_environment.value, None);
    assert_eq!(config.needs.value, None);
    assert!(config.result_artifacts.value.is_empty());
}

#[test]
fn needs_that_name_one_resource_ask_for_none_of_the_other() {
    let home = TempDir::new().unwrap();
    let project = project(&home, "[needs]\nmemory_mib = 512\n");

    let config = load(home.path(), Some(&project)).unwrap();

    assert_eq!(
        config.needs.value,
        Some(ProjectNeeds {
            cpu_millis: 0,
            memory_mib: 512
        })
    );
}

/// Each case is a project file and the words its refusal must contain.
#[test]
fn an_invalid_project_requirement_fails_with_its_file_and_line() {
    let cases = [
        (
            "[execution]\nenvironment = \"vm\"\n",
            "unknown variant `vm`",
        ),
        ("[needs]\n", "names neither cpu_millis nor memory_mib"),
        ("[needs]\ncpu_millis = -1\n", "config.toml"),
        (
            "[results]\nartifacts = [\"/etc/passwd\"]\n",
            "is absolute; name it relative to the checkout",
        ),
        ("[results]\nartifacts = [\"../up\"]\n", "contains `..`"),
        (
            "[execution]\nenvironment = \"docker\"\nimage = \"x\"\n",
            "unknown field `image`",
        ),
    ];
    for (contents, expected) in cases {
        let home = TempDir::new().unwrap();
        let project = project(&home, contents);

        let error = load(home.path(), Some(&project)).unwrap_err().to_string();

        assert!(error.contains(expected), "{contents:?}: {error}");
        assert!(
            error.contains(".cockpit/config.toml"),
            "{contents:?}: {error}"
        );
    }
}

/// The owner's file applies to every project, so a project's own description there would
/// describe all of them.
#[test]
fn the_owner_file_cannot_describe_a_project() {
    for (contents, key) in [
        ("[execution]\nenvironment = \"native\"\n", "execution"),
        ("[needs]\nmemory_mib = 1\n", "needs"),
        ("[results]\nartifacts = [\"a\"]\n", "results"),
    ] {
        let home = TempDir::new().unwrap();
        owner_file(&home, contents);

        let error = load(home.path(), None).unwrap_err().to_string();

        assert!(
            error.contains(&format!("`{key}` describes a project")),
            "{contents:?}: {error}"
        );
    }
}

#[test]
fn config_show_prints_the_project_requirements_with_their_sources() {
    let home = TempDir::new().unwrap();
    let project = project(
        &home,
        "[execution]\nenvironment = \"sandbox\"\n[needs]\ncpu_millis = 500\n",
    );

    let shown = cockpit_config::render(&load(home.path(), Some(&project)).unwrap());

    let file = project.join(".cockpit/config.toml");
    let file = file.display();
    assert!(
        shown.contains(&format!(
            "execution.environment = \"sandbox\"  # project config {file}:2\n"
        )),
        "{shown}"
    );
    assert!(
        shown.contains(&format!(
            "needs.cpu_millis = 500  # project config {file}:3\n"
        )),
        "{shown}"
    );
    assert!(
        shown.contains("results.artifacts = []  # product default\n"),
        "{shown}"
    );
}
