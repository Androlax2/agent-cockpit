//! `cockpit doctor` without a daemon (#50): it still reads the configuration, names the
//! daemon as the missing prerequisite with how to start it, and exits 1.

use std::process::{Command, Output};

use tempfile::TempDir;

fn doctor(home: &TempDir, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_torlek"))
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
    assert!(report.contains("next: start `torlek-daemon`"), "{report}");
    assert!(line(&report, "config").starts_with("available"), "{report}");
    assert!(line(&report, "host").starts_with("unknown"), "{report}");
}

#[test]
fn without_a_daemon_an_invalid_project_file_names_its_line() {
    let home = TempDir::new().unwrap();
    let project = home.path().join("app");
    std::fs::create_dir_all(project.join(".torlek")).unwrap();
    std::fs::write(
        project.join(".torlek/config.toml"),
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

// #828: what the removed Greywall and Greyproxy engines left. `doctor` names it and changes
// nothing; only `--remove-old-sandbox`, the owner's confirmation, stops and deletes it.

const OLD_FOLDERS: [&str; 3] = ["greyproxy", "engines", "sandbox"];

/// A home holding the old proxy's unit (`unit_text`) and state folders, with a `systemctl`
/// that logs its arguments and exits `systemctl_status`.
struct OldSandbox {
    home: TempDir,
    unit: std::path::PathBuf,
    folders: Vec<std::path::PathBuf>,
}

impl OldSandbox {
    fn new(systemctl_status: u8) -> OldSandbox {
        Self::with_unit(systemctl_status, |profile| {
            format!("# Torlek managed Greyproxy, profile {profile}\n[Service]\n")
        })
    }

    fn with_unit(systemctl_status: u8, unit_text: impl Fn(&str) -> String) -> OldSandbox {
        let home = TempDir::new().unwrap();
        let report = String::from_utf8(doctor(&home, &[]).stdout).unwrap();
        let field = |prefix: &str| {
            report
                .lines()
                .find_map(|line| line.strip_prefix(prefix))
                .unwrap_or_else(|| panic!("no {prefix:?} in:\n{report}"))
                .to_owned()
        };
        let profile = field("profile ")
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned();
        let state_dir = std::path::PathBuf::from(field("state dir "));
        let unit = home.path().join(format!(
            ".config/systemd/user/torlek-greyproxy-{profile}.service"
        ));
        std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
        std::fs::write(&unit, unit_text(&profile)).unwrap();
        let folders: Vec<_> = OLD_FOLDERS
            .iter()
            .map(|name| state_dir.join(name))
            .collect();
        for folder in &folders {
            std::fs::create_dir_all(folder.join("inner")).unwrap();
            std::fs::write(folder.join("inner/file"), "x").unwrap();
        }
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let systemctl = bin.join("systemctl");
        std::fs::write(
            &systemctl,
            format!(
                "#!/bin/sh\necho \"$*\" >> {}\nexit {systemctl_status}\n",
                home.path().join("systemctl.log").display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        OldSandbox {
            home,
            unit,
            folders,
        }
    }

    fn doctor(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_torlek"))
            .arg("doctor")
            .args(arguments)
            .env_clear()
            .env("HOME", self.home.path())
            .env("XDG_RUNTIME_DIR", self.home.path().join("run"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.home.path().join("bin").display()),
            )
            .output()
            .expect("torlek runs")
    }

    fn systemctl_calls(&self) -> String {
        std::fs::read_to_string(self.home.path().join("systemctl.log")).unwrap_or_default()
    }

    fn everything_is_left(&self) -> bool {
        self.unit.exists()
            && self
                .folders
                .iter()
                .all(|folder| folder.join("inner/file").exists())
    }
}

#[test]
fn doctor_names_the_old_sandbox_leftovers_and_how_to_remove_them_without_touching_them() {
    let old = OldSandbox::new(0);

    let output = old.doctor(&[]);

    let report = String::from_utf8(output.stdout).unwrap();
    let leftovers = line(&report, "old sandbox");
    assert!(leftovers.starts_with("available"), "{report}");
    for path in std::iter::once(&old.unit).chain(&old.folders) {
        assert!(
            leftovers.contains(&path.display().to_string()),
            "{} missing from {leftovers}",
            path.display()
        );
    }
    assert!(report.contains("doctor --remove-old-sandbox`"), "{report}");
    assert!(old.everything_is_left());
    assert_eq!(old.systemctl_calls(), "");
}

#[test]
fn remove_old_sandbox_stops_the_service_and_deletes_its_unit_and_state() {
    let old = OldSandbox::new(0);
    let unit_name = old.unit.file_name().unwrap().to_str().unwrap().to_owned();

    let output = old.doctor(&["--remove-old-sandbox"]);

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        old.systemctl_calls(),
        format!("--user disable --now {unit_name}\n--user daemon-reload\n")
    );
    assert!(!old.unit.exists());
    for path in std::iter::once(&old.unit).chain(&old.folders) {
        assert!(!path.exists(), "{} is still there", path.display());
        assert!(
            stdout.contains(&format!("removed {}", path.display())),
            "{stdout}"
        );
    }
    let again = old.doctor(&[]);
    assert!(
        !String::from_utf8(again.stdout)
            .unwrap()
            .contains("old sandbox"),
        "the leftovers are still reported"
    );
}

#[test]
fn a_service_that_does_not_stop_leaves_every_leftover_in_place() {
    let old = OldSandbox::new(1);

    let output = old.doctor(&["--remove-old-sandbox"]);

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("systemctl --user disable --now"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "stop the service yourself and delete {}",
            old.unit.display()
        )),
        "{stderr}"
    );
    assert!(old.everything_is_left());
}

/// Only the unit the old release wrote, with its marker, is Torlek's to stop and delete.
#[test]
fn a_unit_of_that_name_without_the_old_marker_is_never_reported_or_touched() {
    let old = OldSandbox::with_unit(0, |_| "[Service]\nExecStart=/usr/bin/mine\n".to_owned());

    let report = String::from_utf8(old.doctor(&[]).stdout).unwrap();
    let removed = old.doctor(&["--remove-old-sandbox"]);

    assert!(
        !line(&report, "old sandbox").contains(&old.unit.display().to_string()),
        "{report}"
    );
    assert!(removed.status.success());
    assert!(old.unit.exists());
    assert_eq!(old.systemctl_calls(), "");
}

#[test]
fn remove_old_sandbox_with_nothing_left_says_so() {
    let home = TempDir::new().unwrap();

    let output = doctor(&home, &["--remove-old-sandbox"]);

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains("no old sandbox service or state to remove"),
        "{stdout}"
    );
}
