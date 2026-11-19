//! Issue #1387: `torlek setup root-run` installs the root-run helper on the install script's
//! path (Theo, 2026-11-19): the daemon names the root step and the `torlek-sandbox` it runs,
//! and the owner's terminal runs that step once with sudo, the owner typing the password. A
//! fake `sudo` stands in for root here; the step's own effects are `cockpit-sandbox`'s
//! tests/root_run_install.rs.
mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::TestHome;

/// What a launch puts in its agent's environment.
const SESSION_ENVIRONMENT: [(&str, &str); 2] = [
    ("TORLEK_SESSION", "s0123456789ab"),
    ("TORLEK_LAUNCH_TOKEN", "the-token"),
];

const SET_UP_HELPER: &str = "/usr/local/lib/torlek/root-run/torlek-sandbox";

/// A `sudo` that logs each argument, NUL-terminated, then exits `status`.
fn fake_sudo(home: &TestHome, status: u8) -> PathBuf {
    let fakes = home.home().join("fakes");
    std::fs::create_dir_all(&fakes).unwrap();
    let sudo = fakes.join("sudo");
    std::fs::write(
        &sudo,
        format!(
            "#!/bin/sh\nfor word in \"$@\"; do printf '%s\\0' \"$word\"; done >'{}'\nexit {status}\n",
            sudo_log(home).display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
    fakes
}

fn sudo_log(home: &TestHome) -> PathBuf {
    home.home().join("sudo.log")
}

/// What `sudo` was asked to run, or `None` when it never ran.
fn sudo_ran(home: &TestHome) -> Option<Vec<String>> {
    let log = std::fs::read(sudo_log(home)).ok()?;
    Some(
        log.split(|byte| *byte == 0)
            .filter(|word| !word.is_empty())
            .map(|word| String::from_utf8(word.to_vec()).unwrap())
            .collect(),
    )
}

fn torlek(
    home: &TestHome,
    fakes: &Path,
    arguments: &[&str],
    environment: &[(&str, &str)],
) -> Output {
    home.base_command(Path::new(env!("CARGO_BIN_EXE_torlek")))
        .args(arguments)
        .env("PATH", fakes)
        .envs(environment.iter().copied())
        .output()
        .expect("the torlek binary runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn setup_root_run_runs_the_daemons_root_step_once_with_sudo() {
    let home = TestHome::new();
    let _daemon = home.start();
    let fakes = fake_sudo(&home, 0);

    let output = torlek(&home, &fakes, &["setup", "root-run"], &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    let ran = sudo_ran(&home).expect("sudo ran");
    let [sh, dash_c, script, name, helper] = ran.as_slice() else {
        panic!("{ran:?}");
    };
    assert_eq!(
        (sh.as_str(), dash_c.as_str(), name.as_str()),
        ("sh", "-c", "torlek-setup")
    );
    assert!(
        script.contains(&format!(
            "install -o root -g root -m 0755 \"$helper\" {SET_UP_HELPER}"
        )),
        "{script}"
    );
    assert!(helper.ends_with("/torlek-sandbox"), "{helper}");
}

#[test]
fn setup_root_run_says_what_the_root_step_installs_before_sudo_asks() {
    let home = TestHome::new();
    let _daemon = home.start();
    let fakes = fake_sudo(&home, 0);

    let output = torlek(&home, &fakes, &["setup", "root-run"], &[]);

    let shown = text(&output.stdout);
    for named in [
        SET_UP_HELPER,
        "owned by root",
        "no sudoers rule",
        "each time",
    ] {
        assert!(shown.contains(named), "{named}: {shown}");
    }
}

#[test]
fn setup_root_run_uninstall_runs_the_removal_step() {
    let home = TestHome::new();
    let _daemon = home.start();
    let fakes = fake_sudo(&home, 0);

    let output = torlek(&home, &fakes, &["setup", "root-run", "--uninstall"], &[]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    let ran = sudo_ran(&home).expect("sudo ran");
    let script = &ran[2];
    assert!(
        script.contains(&format!("rm -f {SET_UP_HELPER}")),
        "{script}"
    );
    assert!(!script.contains("install -o root"), "{script}");
}

#[test]
fn inside_a_session_setup_root_run_is_refused_and_runs_no_sudo() {
    let home = TestHome::new();
    let _daemon = home.start();
    let fakes = fake_sudo(&home, 0);

    let output = torlek(&home, &fakes, &["setup", "root-run"], &SESSION_ENVIRONMENT);

    assert!(!output.status.success());
    assert_eq!(sudo_ran(&home), None);
}
