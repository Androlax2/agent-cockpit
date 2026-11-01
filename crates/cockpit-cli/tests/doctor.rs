//! `cockpit doctor` without a daemon (#50): it still reads the configuration, names the
//! daemon as the missing prerequisite with how to start it, and exits 1.

use std::process::{Command, Output};

use tempfile::TempDir;

fn doctor(home: &TempDir, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cockpit"))
        .arg("doctor")
        .args(arguments)
        .env_clear()
        .env("HOME", home.path())
        .env("XDG_RUNTIME_DIR", home.path().join("run"))
        .output()
        .expect("cockpit runs")
}

fn line<'report>(report: &'report str, name: &str) -> &'report str {
    report
        .lines()
        .find(|line| line.contains(&format!("  {name}: ")))
        .unwrap_or_else(|| panic!("no {name} requirement in:\n{report}"))
}

#[test]
fn without_a_daemon_the_daemon_is_the_missing_prerequisite() {
    let home = TempDir::new().unwrap();

    let output = doctor(&home, &[]);

    let report = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert!(
        line(&report, "daemon").starts_with("missing-prerequisite"),
        "{report}"
    );
    assert!(report.contains("next: start `cockpit-daemon`"), "{report}");
    assert!(line(&report, "config").starts_with("available"), "{report}");
    assert!(line(&report, "host").starts_with("unknown"), "{report}");
}

#[test]
fn without_a_daemon_an_invalid_project_file_names_its_line() {
    let home = TempDir::new().unwrap();
    let project = home.path().join("app");
    std::fs::create_dir_all(project.join(".cockpit")).unwrap();
    std::fs::write(
        project.join(".cockpit/config.toml"),
        "[execution]\nenvironment = \"vm\"\n",
    )
    .unwrap();

    let output = doctor(&home, &["--project", project.to_str().unwrap()]);

    let report = String::from_utf8(output.stdout).unwrap();
    let config = line(&report, "config");
    assert!(config.starts_with("missing-prerequisite"), "{report}");
    assert!(config.contains("config.toml"), "{config}");
}

#[test]
fn doctor_takes_only_a_project_flag() {
    let home = TempDir::new().unwrap();

    let output = doctor(&home, &["--all"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_project_directory_that_does_not_exist_is_a_usage_error() {
    let home = TempDir::new().unwrap();
    let missing = home.path().join("typo");

    let output = doctor(&home, &["--project", missing.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not a directory"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
