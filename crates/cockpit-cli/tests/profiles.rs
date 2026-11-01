//! Profiles through the `cockpit` command (#196, #134): what this test build selects, what
//! `version --all` and `doctor` show, and what `daemon install` writes and refuses, in a
//! fixture home with a fake service manager first on `PATH`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// A service manager that appends its arguments to `service-manager.log` beside it.
const FAKE_SERVICE_MANAGER: &str =
    "#!/bin/sh\nprintf '%s %s\\n' \"${0##*/}\" \"$*\" >> \"${0%/*}/service-manager.log\"\n";

/// A home and an installation: `bin/cockpit` (a copy of the binary under test), a stand-in
/// `bin/cockpit-daemon` beside it, and the fake service managers in `bin`.
struct Home {
    dir: TempDir,
}

impl Home {
    fn new() -> Home {
        let home = Home {
            dir: tempfile::tempdir().unwrap(),
        };
        let bin = home.bin();
        fs::create_dir_all(&bin).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_cockpit"), bin.join("cockpit")).unwrap();
        for (name, text) in [
            ("cockpit-daemon", "#!/bin/sh\n"),
            ("systemctl", FAKE_SERVICE_MANAGER),
            ("launchctl", FAKE_SERVICE_MANAGER),
        ] {
            fs::write(bin.join(name), text).unwrap();
            fs::set_permissions(bin.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        home
    }

    /// Canonical: macOS's temporary directory is behind a symbolic link.
    fn path(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    fn bin(&self) -> PathBuf {
        self.path().join("bin")
    }

    fn unit_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.path().join("Library/LaunchAgents")
        } else {
            self.path().join("config/systemd/user")
        }
    }

    fn unit_of(&self, qualified_name: &str) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.unit_dir()
                .join(format!("io.github.androlax2.{qualified_name}.plist"))
        } else {
            self.unit_dir().join(format!("{qualified_name}.service"))
        }
    }

    fn service_manager_log(&self) -> String {
        fs::read_to_string(self.bin().join("service-manager.log")).unwrap_or_default()
    }

    /// `program arguments` with only this home's variables and `extra`.
    fn run(&self, program: &Path, arguments: &[&str], extra: &[(&str, &str)]) -> Output {
        Command::new(program)
            .args(arguments)
            .env_clear()
            .env("HOME", self.path())
            .env("XDG_CONFIG_HOME", self.path().join("config"))
            .env("XDG_STATE_HOME", self.path().join("state"))
            .env("XDG_RUNTIME_DIR", self.path().join("run"))
            .env("PATH", self.bin())
            .envs(extra.iter().copied())
            .output()
            .expect("cockpit runs")
    }

    fn cockpit(&self, arguments: &[&str]) -> Output {
        self.run(&self.bin().join("cockpit"), arguments, &[])
    }
}

fn succeeded(output: &Output) -> String {
    assert!(
        output.status.success(),
        "exit {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn line_for(shown: &str, key: &str) -> String {
    shown
        .lines()
        .find(|line| line.starts_with(&format!("{key} = ")))
        .unwrap_or_else(|| panic!("no line for {key} in:\n{shown}"))
        .to_owned()
}

#[test]
fn a_build_outside_a_release_bundle_runs_profile_dev_in_its_own_directories() {
    let home = Home::new();

    let shown = succeeded(&home.cockpit(&["config", "show"]));

    assert_eq!(
        line_for(&shown, "profile"),
        "profile = \"dev\"  # development build default"
    );
    assert_eq!(
        line_for(&shown, "paths.config_dir"),
        format!(
            "paths.config_dir = \"{}/config/cockpit-dev\"  # environment $XDG_CONFIG_HOME",
            home.path().display()
        )
    );
}

#[test]
fn a_default_profile_inherited_from_the_installed_cockpit_does_not_reach_it_from_a_dev_build() {
    let home = Home::new();

    let shown = succeeded(&home.run(
        &home.bin().join("cockpit"),
        &["config", "show"],
        &[("COCKPIT_PROFILE", "cockpit")],
    ));

    assert_eq!(
        line_for(&shown, "profile"),
        "profile = \"dev\"  # development build default; $COCKPIT_PROFILE ignored: a \
         development build reaches the default profile only with --profile cockpit"
    );
}

/// One way of choosing a profile, and the profile and source `config show` must name.
struct Choice<'a> {
    program: &'a Path,
    arguments: &'a [&'a str],
    variables: &'a [(&'a str, &'a str)],
    profile: &'a str,
    source: &'a str,
}

#[test]
fn each_way_of_choosing_a_profile_places_the_directories_under_it() {
    let home = Home::new();
    std::os::unix::fs::symlink(home.bin().join("cockpit"), home.bin().join("cockpit-next"))
        .unwrap();
    let cockpit = home.bin().join("cockpit");
    let next = home.bin().join("cockpit-next");
    let cases = [
        Choice {
            program: &cockpit,
            arguments: &["--profile", "cockpit", "config", "show"],
            variables: &[],
            profile: "cockpit",
            source: "command line --profile",
        },
        Choice {
            program: &cockpit,
            arguments: &["config", "show"],
            variables: &[("COCKPIT_PROFILE", "review")],
            profile: "review",
            source: "environment $COCKPIT_PROFILE",
        },
        Choice {
            program: &next,
            arguments: &["config", "show"],
            // What every agent of the installed Cockpit inherits.
            variables: &[("COCKPIT_PROFILE", "cockpit")],
            profile: "next",
            source: "command name",
        },
    ];
    for Choice {
        program,
        arguments,
        variables,
        profile,
        source,
    } in cases
    {
        let shown = succeeded(&home.run(program, arguments, variables));

        assert_eq!(
            line_for(&shown, "profile"),
            format!("profile = \"{profile}\"  # {source}")
        );
        let directory = if profile == "cockpit" {
            "cockpit".to_owned()
        } else {
            format!("cockpit-{profile}")
        };
        assert_eq!(
            line_for(&shown, "paths.state_dir"),
            format!(
                "paths.state_dir = \"{}/state/{directory}\"  # environment $XDG_STATE_HOME",
                home.path().display()
            )
        );
    }
}

#[test]
fn a_profile_that_is_not_a_name_exits_2_naming_it() {
    let home = Home::new();
    for arguments in [
        &["--profile", "Bad Name", "config", "show"][..],
        &["--profile"][..],
    ] {
        let output = home.cockpit(arguments);

        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("profile \"Bad Name\" is not a name")
                || stderr.contains("--profile needs a name"),
            "{stderr}"
        );
    }
}

#[test]
fn version_all_names_the_profile_and_the_build() {
    let home = Home::new();

    let printed = succeeded(&home.cockpit(&["version", "--all"]));

    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(
        lines[1..3],
        [
            "profile dev (development build default)",
            "build development"
        ],
        "{printed}"
    );
}

#[test]
fn daemon_install_writes_the_profiles_unit_and_starts_it() {
    let home = Home::new();

    let printed = succeeded(&home.cockpit(&["daemon", "install"]));

    let unit = fs::read_to_string(home.unit_of("cockpit-dev")).unwrap();
    let daemon = home.bin().join("cockpit-daemon");
    if cfg!(target_os = "macos") {
        assert!(
            unit.contains("<string>io.github.androlax2.cockpit-dev</string>"),
            "{unit}"
        );
        assert!(
            unit.contains(&format!(
                "<string>{}</string>\n\t\t<string>--profile</string>\n\t\t<string>dev</string>",
                daemon.display()
            )),
            "{unit}"
        );
        assert!(
            home.service_manager_log().contains(&format!(
                "launchctl bootstrap gui/{}",
                std::os::unix::fs::MetadataExt::uid(&fs::metadata(home.path()).unwrap())
            )),
            "{}",
            home.service_manager_log()
        );
    } else {
        assert!(unit.contains("X-Cockpit-Profile=dev\n"), "{unit}");
        assert!(
            unit.contains(&format!(
                "ExecStart=\"{}\" --profile dev\n",
                daemon.display()
            )),
            "{unit}"
        );
        assert!(
            unit.contains(&format!("Environment=\"PATH={}\"\n", home.bin().display())),
            "{unit}"
        );
        assert!(
            unit.contains(&format!(
                "Environment=\"XDG_STATE_HOME={}/state\"\n",
                home.path().display()
            )),
            "{unit}"
        );
        assert_eq!(
            home.service_manager_log(),
            "systemctl --user daemon-reload\nsystemctl --user enable cockpit-dev.service\n\
             systemctl --user restart cockpit-dev.service\n"
        );
    }
    assert!(
        printed.contains("the daemon of profile dev runs as"),
        "{printed}"
    );
}

#[test]
fn daemon_install_of_another_profile_installs_the_command_whose_name_selects_it() {
    let home = Home::new();

    succeeded(&home.cockpit(&["--profile", "next", "daemon", "install"]));

    let command = home.bin().join("cockpit-next");
    assert_eq!(
        fs::canonicalize(&command).unwrap(),
        fs::canonicalize(home.bin().join("cockpit")).unwrap()
    );
    assert!(home.unit_of("cockpit-next").is_file());
    let shown = succeeded(&home.run(&command, &["config", "show"], &[]));
    assert_eq!(
        line_for(&shown, "profile"),
        "profile = \"next\"  # command name"
    );
}

#[test]
fn daemon_install_refuses_a_command_name_another_file_holds() {
    let home = Home::new();
    fs::write(home.bin().join("cockpit-next"), "someone else's\n").unwrap();

    let output = home.cockpit(&["--profile", "next", "daemon", "install"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("exists and is not this cockpit"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!home.unit_of("cockpit-next").exists());
    assert_eq!(home.service_manager_log(), "");
}

/// The legacy Cockpit's own unit, as its dotfiles install it.
const LEGACY_UNIT: &str = "[Unit]\nDescription=Claude session manager\n\n[Service]\nType=oneshot\nExecStart=%h/.local/bin/cockpit prepare\n";

#[cfg(target_os = "linux")]
#[test]
fn daemon_install_refuses_the_default_profile_beside_a_legacy_unit_and_proposes_next() {
    let home = Home::new();
    fs::create_dir_all(home.unit_dir()).unwrap();
    fs::write(home.unit_dir().join("cockpit.service"), LEGACY_UNIT).unwrap();

    let output = home.cockpit(&["--profile", "cockpit", "daemon", "install"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("a legacy Cockpit is installed (unit ")
            && stderr.contains("cockpit --profile next daemon install"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(home.unit_dir().join("cockpit.service")).unwrap(),
        LEGACY_UNIT
    );
    assert_eq!(home.service_manager_log(), "");
}

#[test]
fn daemon_install_refuses_the_default_profile_beside_another_cockpit_on_path() {
    let home = Home::new();
    let legacy_bin = home.path().join("legacy-bin");
    fs::create_dir_all(&legacy_bin).unwrap();
    fs::write(legacy_bin.join("cockpit"), "#!/bin/sh\n").unwrap();
    let path = format!("{}:{}", legacy_bin.display(), home.bin().display());

    let output = home.run(
        &home.bin().join("cockpit"),
        &["--profile", "cockpit", "daemon", "install"],
        &[("PATH", &path)],
    );

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "command {} on PATH",
            legacy_bin.join("cockpit").display()
        )),
        "{stderr}"
    );
    assert!(!home.unit_of("cockpit").exists());
}

#[test]
fn daemon_install_accepts_the_default_profile_when_no_legacy_cockpit_is_found() {
    let home = Home::new();

    succeeded(&home.cockpit(&["--profile", "cockpit", "daemon", "install"]));

    assert!(home.unit_of("cockpit").is_file());
}

#[test]
fn doctor_names_the_active_profile_and_every_other_installation_it_sees() {
    let home = Home::new();
    succeeded(&home.cockpit(&["--profile", "next", "daemon", "install"]));
    succeeded(&home.cockpit(&["daemon", "install"]));
    let installed_socket = home.path().join("run/cockpit/daemon.sock");
    fs::create_dir_all(installed_socket.parent().unwrap()).unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(&installed_socket).unwrap();
    let legacy_bin = home.path().join("legacy-bin");
    fs::create_dir_all(&legacy_bin).unwrap();
    fs::write(legacy_bin.join("cockpit"), "#!/bin/sh\n").unwrap();
    let path = format!("{}:{}", home.bin().display(), legacy_bin.display());

    let output = home.run(&home.bin().join("cockpit"), &["doctor"], &[("PATH", &path)]);

    // Exit 1: this profile's daemon is not running, which the readiness report after the
    // installations names as the missing prerequisite (#50).
    assert_eq!(output.status.code(), Some(1));
    let printed = String::from_utf8(output.stdout).unwrap();

    let root = home.path();
    let root = root.display();
    let expected_head = [
        "profile dev (development build default)".to_owned(),
        "build development".to_owned(),
        format!("config dir {root}/config/cockpit-dev"),
        format!("state dir {root}/state/cockpit-dev"),
        format!("daemon {root}/run/cockpit-dev/daemon.sock: not running"),
        "tmux server cockpit-dev".to_owned(),
        format!("unit {}: installed", home.unit_of("cockpit-dev").display()),
        "other installations:".to_owned(),
        format!(
            "- profile next: unit {}",
            home.unit_of("cockpit-next").display()
        ),
        format!("- profile cockpit: daemon {root}/run/cockpit/daemon.sock: answering"),
    ];
    let lines: Vec<String> = printed.lines().map(str::to_owned).collect();
    assert_eq!(lines[..expected_head.len()], expected_head, "{printed}");
    assert_eq!(
        lines[expected_head.len()],
        format!(
            "- legacy cockpit: command {}/cockpit on PATH",
            legacy_bin.display()
        ),
        "{printed}"
    );
    assert!(
        lines[expected_head.len() + 1].contains("  config: "),
        "the readiness report follows the installations: {printed}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn daemon_install_of_profile_next_beside_a_legacy_unit_leaves_that_unit_alone() {
    let home = Home::new();
    fs::create_dir_all(home.unit_dir()).unwrap();
    fs::write(home.unit_dir().join("cockpit.service"), LEGACY_UNIT).unwrap();

    succeeded(&home.cockpit(&["--profile", "next", "daemon", "install"]));

    assert!(home.unit_of("cockpit-next").is_file());
    assert_eq!(
        fs::read_to_string(home.unit_dir().join("cockpit.service")).unwrap(),
        LEGACY_UNIT
    );
    assert!(!home.service_manager_log().contains("cockpit.service"));
}

#[cfg(target_os = "linux")]
#[test]
fn daemon_install_refuses_to_replace_a_unit_it_did_not_write() {
    let home = Home::new();
    fs::create_dir_all(home.unit_dir()).unwrap();
    let remote = "[Unit]\nDescription=the legacy Cockpit's remote access\n";
    fs::write(home.unit_dir().join("cockpit-remote.service"), remote).unwrap();

    let output = home.cockpit(&["--profile", "remote", "daemon", "install"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("was not written by `cockpit daemon install` for profile remote"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(home.unit_dir().join("cockpit-remote.service")).unwrap(),
        remote
    );
    assert!(!home.bin().join("cockpit-remote").exists());
    assert_eq!(home.service_manager_log(), "");
}

#[test]
fn daemon_install_again_replaces_its_own_unit_and_restarts_it() {
    let home = Home::new();
    succeeded(&home.cockpit(&["--profile", "next", "daemon", "install"]));

    succeeded(&home.cockpit(&["--profile", "next", "daemon", "install"]));

    assert!(home.unit_of("cockpit-next").is_file());
}
