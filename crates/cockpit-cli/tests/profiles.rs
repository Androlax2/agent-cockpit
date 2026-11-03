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
        fs::copy(env!("CARGO_BIN_EXE_torlek"), bin.join("torlek")).unwrap();
        for (name, text) in [
            ("torlek-daemon", "#!/bin/sh\n"),
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
        self.run(&self.bin().join("torlek"), arguments, &[])
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
            "paths.config_dir = \"{}/config/torlek-dev\"  # environment $XDG_CONFIG_HOME",
            home.path().display()
        )
    );
}

#[test]
fn a_default_profile_inherited_from_the_installed_cockpit_does_not_reach_it_from_a_dev_build() {
    let home = Home::new();

    let shown = succeeded(&home.run(
        &home.bin().join("torlek"),
        &["config", "show"],
        &[("TORLEK_PROFILE", "torlek")],
    ));

    assert_eq!(
        line_for(&shown, "profile"),
        "profile = \"dev\"  # development build default; $TORLEK_PROFILE ignored: a \
         development build reaches the default profile only with --profile torlek"
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
    std::os::unix::fs::symlink(home.bin().join("torlek"), home.bin().join("torlek-next")).unwrap();
    let cockpit = home.bin().join("torlek");
    let next = home.bin().join("torlek-next");
    let cases = [
        Choice {
            program: &cockpit,
            arguments: &["--profile", "torlek", "config", "show"],
            variables: &[],
            profile: "torlek",
            source: "command line --profile",
        },
        Choice {
            program: &cockpit,
            arguments: &["config", "show"],
            variables: &[("TORLEK_PROFILE", "review")],
            profile: "review",
            source: "environment $TORLEK_PROFILE",
        },
        Choice {
            program: &next,
            arguments: &["config", "show"],
            // What every agent of the installed Cockpit inherits.
            variables: &[("TORLEK_PROFILE", "torlek")],
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
        let directory = if profile == "torlek" {
            "torlek".to_owned()
        } else {
            format!("torlek-{profile}")
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

    let unit = fs::read_to_string(home.unit_of("torlek-dev")).unwrap();
    let daemon = home.bin().join("torlek-daemon");
    if cfg!(target_os = "macos") {
        assert!(
            unit.contains("<string>io.github.androlax2.torlek-dev</string>"),
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
        assert!(unit.contains("X-Torlek-Profile=dev\n"), "{unit}");
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
            "systemctl --user daemon-reload\nsystemctl --user enable torlek-dev.service\n\
             systemctl --user restart torlek-dev.service\n"
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

    let command = home.bin().join("torlek-next");
    assert_eq!(
        fs::canonicalize(&command).unwrap(),
        fs::canonicalize(home.bin().join("torlek")).unwrap()
    );
    assert!(home.unit_of("torlek-next").is_file());
    let shown = succeeded(&home.run(&command, &["config", "show"], &[]));
    assert_eq!(
        line_for(&shown, "profile"),
        "profile = \"next\"  # command name"
    );
}

/// A package install (#562): torlek in a directory only root writes, such as
/// /usr/lib/torlek/bin. The unit selects the profile by `--profile`, so the command link
/// beside the program is skipped with the way to select it instead.
#[test]
fn daemon_install_of_another_profile_beside_a_read_only_program_skips_its_command() {
    let home = Home::new();
    let package = home.path().join("package/bin");
    fs::create_dir_all(&package).unwrap();
    for name in ["torlek", "torlek-daemon"] {
        fs::copy(home.bin().join(name), package.join(name)).unwrap();
    }
    let _writable_again = ReadOnly::make(&package);
    if fs::write(package.join("probe"), "").is_ok() {
        // Root writes through the mode bits, so this run cannot model a package directory.
        eprintln!(
            "skipped: {} stays writable for this user",
            package.display()
        );
        return;
    }

    let output = home.run(
        &package.join("torlek"),
        &["--profile", "next", "daemon", "install"],
        &[],
    );

    succeeded(&output);
    assert!(!package.join("torlek-next").exists());
    assert!(home.unit_of("torlek-next").is_file());
    let warning = String::from_utf8_lossy(&output.stderr);
    assert!(warning.contains("not linked"), "{warning}");
    assert!(warning.contains("torlek --profile next"), "{warning}");
    assert!(warning.contains("TORLEK_PROFILE=next"), "{warning}");
}

/// A directory at 0555 until dropped, so a failing test still lets its TempDir be removed.
struct ReadOnly<'a>(&'a Path);

impl<'a> ReadOnly<'a> {
    fn make(directory: &'a Path) -> Self {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o555)).unwrap();
        Self(directory)
    }
}

impl Drop for ReadOnly<'_> {
    fn drop(&mut self) {
        let _ = fs::set_permissions(self.0, fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn daemon_install_refuses_a_command_name_another_file_holds() {
    let home = Home::new();
    fs::write(home.bin().join("torlek-next"), "someone else's\n").unwrap();

    let output = home.cockpit(&["--profile", "next", "daemon", "install"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("exists and is not this torlek"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!home.unit_of("torlek-next").exists());
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

    let output = home.cockpit(&["--profile", "torlek", "daemon", "install"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("a legacy Cockpit is installed (unit ")
            && stderr.contains("torlek --profile next daemon install"),
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
        &home.bin().join("torlek"),
        &["--profile", "torlek", "daemon", "install"],
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
    assert!(!home.unit_of("torlek").exists());
}

#[test]
fn daemon_install_accepts_the_default_profile_when_no_legacy_cockpit_is_found() {
    let home = Home::new();

    succeeded(&home.cockpit(&["--profile", "torlek", "daemon", "install"]));

    assert!(home.unit_of("torlek").is_file());
}

#[test]
fn doctor_names_the_active_profile_and_every_other_installation_it_sees() {
    let home = Home::new();
    succeeded(&home.cockpit(&["--profile", "next", "daemon", "install"]));
    succeeded(&home.cockpit(&["daemon", "install"]));
    let installed_socket = home.path().join("run/torlek/daemon.sock");
    fs::create_dir_all(installed_socket.parent().unwrap()).unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(&installed_socket).unwrap();
    let legacy_bin = home.path().join("legacy-bin");
    fs::create_dir_all(&legacy_bin).unwrap();
    fs::write(legacy_bin.join("cockpit"), "#!/bin/sh\n").unwrap();
    let path = format!("{}:{}", home.bin().display(), legacy_bin.display());

    let output = home.run(&home.bin().join("torlek"), &["doctor"], &[("PATH", &path)]);

    // Exit 1: this profile's daemon is not running, which the readiness report after the
    // installations names as the missing prerequisite (#50).
    assert_eq!(output.status.code(), Some(1));
    let printed = String::from_utf8(output.stdout).unwrap();

    let root = home.path();
    let root = root.display();
    let expected_head = [
        "profile dev (development build default)".to_owned(),
        "build development".to_owned(),
        format!("config dir {root}/config/torlek-dev"),
        format!("state dir {root}/state/torlek-dev"),
        format!("daemon {root}/run/torlek-dev/daemon.sock: not running"),
        "tmux server torlek-dev".to_owned(),
        format!("unit {}: installed", home.unit_of("torlek-dev").display()),
        "other installations:".to_owned(),
        format!(
            "- profile next: unit {}",
            home.unit_of("torlek-next").display()
        ),
        format!("- profile torlek: daemon {root}/run/torlek/daemon.sock: answering"),
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

    assert!(home.unit_of("torlek-next").is_file());
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
    let remote = "[Unit]\nDescription=someone else's remote access\n";
    fs::write(home.unit_dir().join("torlek-remote.service"), remote).unwrap();

    let output = home.cockpit(&["--profile", "remote", "daemon", "install"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("was not written by `torlek daemon install` for profile remote"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(home.unit_dir().join("torlek-remote.service")).unwrap(),
        remote
    );
    assert!(!home.bin().join("torlek-remote").exists());
    assert_eq!(home.service_manager_log(), "");
}

#[test]
fn daemon_install_again_replaces_its_own_unit_and_restarts_it() {
    let home = Home::new();
    succeeded(&home.cockpit(&["--profile", "next", "daemon", "install"]));

    succeeded(&home.cockpit(&["--profile", "next", "daemon", "install"]));

    assert!(home.unit_of("torlek-next").is_file());
}

#[cfg(target_os = "linux")]
#[test]
fn torlek_default_install_refuses_historical_cockpit_units_without_touching_state() {
    for legacy_unit in [LEGACY_UNIT, "[Unit]\nX-Cockpit-Profile=cockpit\n"] {
        let home = Home::new();
        fs::create_dir_all(home.unit_dir()).unwrap();
        let unit = home.unit_dir().join("cockpit.service");
        fs::write(&unit, legacy_unit).unwrap();
        let old_state = home.path().join("state/cockpit-next");
        fs::create_dir_all(&old_state).unwrap();
        fs::write(old_state.join("state.db"), b"historical state").unwrap();

        let output = home.cockpit(&["--profile", "torlek", "daemon", "install"]);

        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("a legacy Cockpit"));
        assert_eq!(fs::read_to_string(&unit).unwrap(), legacy_unit);
        assert_eq!(
            fs::read(old_state.join("state.db")).unwrap(),
            b"historical state"
        );
        assert!(!home.unit_of("torlek").exists());
        assert!(!home.path().join("state/torlek").exists());
        assert_eq!(home.service_manager_log(), "");
    }
}

#[test]
fn the_old_profile_variable_is_not_a_torlek_alias() {
    let home = Home::new();
    let output = home.run(
        &home.bin().join("torlek"),
        &["config", "show"],
        &[("COCKPIT_PROFILE", "next")],
    );

    assert_eq!(
        line_for(&succeeded(&output), "profile"),
        "profile = \"dev\"  # development build default"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn named_profile_installs_leave_legacy_remote_and_failsafe_units_untouched() {
    for profile in ["remote", "failsafe"] {
        let home = Home::new();
        fs::create_dir_all(home.unit_dir()).unwrap();
        let old_unit = home.unit_dir().join(format!("cockpit-{profile}.service"));
        fs::write(&old_unit, LEGACY_UNIT).unwrap();

        succeeded(&home.cockpit(&["--profile", profile, "daemon", "install"]));

        assert_eq!(fs::read_to_string(&old_unit).unwrap(), LEGACY_UNIT);
        assert!(home.unit_of(&format!("torlek-{profile}")).is_file());
        assert!(!home.service_manager_log().contains("cockpit-"));
    }
}

#[test]
fn daemon_install_never_adopts_an_old_product_marker_as_its_own() {
    let home = Home::new();
    fs::create_dir_all(home.unit_dir()).unwrap();
    let unit = home.unit_of("torlek-next");
    let old_marker = "[Unit]\nX-Cockpit-Profile=next\n";
    fs::write(&unit, old_marker).unwrap();

    let output = home.cockpit(&["--profile", "next", "daemon", "install"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read_to_string(unit).unwrap(), old_marker);
    assert_eq!(home.service_manager_log(), "");
}
