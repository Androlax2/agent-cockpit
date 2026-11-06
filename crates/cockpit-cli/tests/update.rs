//! `torlek update` and the update notice driven as processes from a copy laid out the way an
//! unpacked release archive is (#617). The release list is a fixture a stand-in `curl` prints,
//! never the network; stand-ins for `tar`, `sha256sum`, `ln` and `systemctl` prove that no
//! check downloads, unpacks, links or restarts anything. Notices need a terminal, so those
//! runs go through util-linux `script`, which gives the process a pseudo-terminal.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tempfile::TempDir;

const PLATFORM: &str = "linux-x86_64";
const INSTALLED: &str = "0.1.0-alpha.3";
const DAY: i64 = 24 * 60 * 60;

/// Programs a check must never run: they would download, unpack, link or restart.
const FORBIDDEN_EFFECTS: [&str; 5] = ["tar", "sha256sum", "ln", "systemctl", "launchctl"];

struct Setup {
    home: TempDir,
    prefix: PathBuf,
}

impl Setup {
    /// An installation of `INSTALLED` unpacked where the README's archive route puts it.
    fn archive_install() -> Self {
        Self::installed_at(
            ".local/lib/torlek",
            &format!("torlek-v{INSTALLED}-{PLATFORM}"),
            true,
        )
    }

    /// `directory_name` unpacked in `parent_below_home`, a path relative to the home.
    fn installed_at(parent_below_home: &str, directory_name: &str, has_manifest: bool) -> Self {
        let home = tempfile::tempdir().expect("temporary home");
        let prefix = home.path().join(parent_below_home).join(directory_name);
        fs::create_dir_all(prefix.join("bin")).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_torlek"), prefix.join("bin/torlek")).unwrap();
        if has_manifest {
            fs::create_dir_all(prefix.join("share/torlek")).unwrap();
            let manifest = json!({"name": "torlek", "version": INSTALLED, "platform": PLATFORM});
            fs::write(
                prefix.join("share/torlek/manifest.json"),
                manifest.to_string(),
            )
            .unwrap();
        }
        let setup = Self { home, prefix };
        for program in FORBIDDEN_EFFECTS {
            setup.stand_in(program, "exit 0");
        }
        setup
    }

    fn path_dir(&self) -> PathBuf {
        self.home.path().join("stand-ins")
    }

    /// A stand-in for `program` on `PATH`: logs one line per run with its arguments, then runs
    /// `then` (shell).
    fn stand_in(&self, program: &str, then: &str) {
        fs::create_dir_all(self.path_dir()).unwrap();
        let path = self.path_dir().join(program);
        let log = self.home.path().join(format!("{program}.runs"));
        fs::write(
            &path,
            format!("#!/bin/sh\necho \"$*\" >>'{}'\n{then}\n", log.display()),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The release list `curl` prints.
    fn publish(&self, releases: &Value) {
        let fixture = self.home.path().join("releases.json");
        fs::write(&fixture, releases.to_string()).unwrap();
        self.stand_in("curl", &format!("cat '{}'", fixture.display()));
    }

    fn publish_text(&self, text: &str) {
        let fixture = self.home.path().join("releases.json");
        fs::write(&fixture, text).unwrap();
        self.stand_in("curl", &format!("cat '{}'", fixture.display()));
    }

    fn offline(&self) {
        self.stand_in(
            "curl",
            "echo 'curl: (28) Connection timed out after 3001 milliseconds' >&2; exit 28",
        );
    }

    /// One line per run of `program`; empty when it never ran.
    fn runs(&self, program: &str) -> Vec<String> {
        fs::read_to_string(self.home.path().join(format!("{program}.runs")))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn state_path(&self) -> PathBuf {
        self.home.path().join("state/torlek-dev/update.json")
    }

    fn state(&self) -> Value {
        let text = fs::read_to_string(self.state_path()).expect("update.json was written");
        serde_json::from_str(&text).unwrap()
    }

    fn write_state(&self, state: &Value) {
        fs::create_dir_all(self.state_path().parent().unwrap()).unwrap();
        fs::write(self.state_path(), state.to_string()).unwrap();
    }

    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("XDG_STATE_HOME", self.home.path().join("state"))
            .env("XDG_RUNTIME_DIR", self.home.path().join("run"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path_dir().display()),
            );
        command
    }

    fn torlek(&self, arguments: &[&str]) -> Output {
        self.command(&self.prefix.join("bin/torlek"))
            .args(arguments)
            .output()
            .expect("torlek runs")
    }

    /// `torlek ARGUMENTS` with a terminal on stdout and stderr, as everything it printed.
    fn torlek_on_terminal(&self, arguments: &[&str], extra_environment: &[(&str, &str)]) -> String {
        let words = arguments.join(" ");
        let mut command = self.command(Path::new("script"));
        command
            .args(["--quiet", "--return", "--command"])
            .arg(format!(
                "{} {words}",
                self.prefix.join("bin/torlek").display()
            ))
            .arg("/dev/null");
        for (name, value) in extra_environment {
            command.env(name, value);
        }
        let output = command
            .output()
            .expect("util-linux `script` is needed to give torlek a terminal");
        String::from_utf8(output.stdout).unwrap()
    }

    fn assert_nothing_was_installed(&self) {
        for program in FORBIDDEN_EFFECTS {
            assert!(
                self.runs(program).is_empty(),
                "{program} ran: {:?}",
                self.runs(program)
            );
        }
        let unpacked: Vec<_> = fs::read_dir(self.prefix.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(unpacked.len(), 1, "{unpacked:?}");
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .try_into()
        .unwrap()
}

fn release(version: &str, platforms: &[&str], notes: &str) -> Value {
    let mut assets: Vec<Value> = platforms
        .iter()
        .map(|platform| json!({"name": format!("torlek-v{version}-{platform}.tar.gz")}))
        .collect();
    assets.push(json!({"name": "SHA256SUMS"}));
    json!({
        "tag_name": format!("v{version}"),
        "draft": false,
        "prerelease": true,
        "body": notes,
        "assets": assets,
    })
}

fn newer_alpha_published(setup: &Setup) {
    setup.publish(&json!([
        release(
            "0.1.0-alpha.4",
            &[PLATFORM, "linux-aarch64"],
            "Fixes the approval pane.\nKnown gaps: none."
        ),
        release("0.1.0-alpha.3", &[PLATFORM], "the installed one"),
    ]));
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn cached_offer_state(version: &str) -> Value {
    json!({
        "latest": {"version": version, "notes": "cached notes"},
        "last_attempt_at": now() - 100,
    })
}

#[test]
fn an_older_installation_is_told_about_the_newer_release_and_how_to_update() {
    let setup = Setup::archive_install();
    newer_alpha_published(&setup);
    let output = setup.torlek(&["update", "check"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let printed = stdout(&output);
    assert!(
        printed.contains("torlek 0.1.0-alpha.4 is available (you have 0.1.0-alpha.3)"),
        "{printed}"
    );
    assert!(printed.contains("Fixes the approval pane."), "{printed}");
    assert!(
        printed
            .contains("releases/download/v0.1.0-alpha.4/torlek-v0.1.0-alpha.4-linux-x86_64.tar.gz"),
        "{printed}"
    );
    assert!(printed.contains("sha256sum --check"), "{printed}");
    assert!(printed.contains("torlek daemon install"), "{printed}");
    assert!(
        printed.contains("nothing has been downloaded or changed"),
        "{printed}"
    );
}

#[test]
fn a_check_only_asks_for_the_release_list_and_changes_nothing_else() {
    let setup = Setup::archive_install();
    newer_alpha_published(&setup);
    let output = setup.torlek(&["update", "check"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let curl_runs = setup.runs("curl");
    assert_eq!(curl_runs.len(), 1, "{curl_runs:?}");
    assert!(curl_runs[0].contains("--max-time 3"), "{curl_runs:?}");
    assert!(
        curl_runs[0]
            .ends_with("https://api.github.com/repos/Androlax2/agent-cockpit/releases?per_page=30"),
        "{curl_runs:?}"
    );
    assert!(!curl_runs[0].contains("-O"), "{curl_runs:?}");
    setup.assert_nothing_was_installed();
}

#[test]
fn update_without_a_verb_reads_the_cache_and_does_not_ask_the_network() {
    let setup = Setup::archive_install();
    setup.write_state(&cached_offer_state("0.1.0-alpha.4"));
    let output = setup.torlek(&["update"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("0.1.0-alpha.4 is available"),
        "{}",
        stdout(&output)
    );
    assert!(setup.runs("curl").is_empty());
    setup.assert_nothing_was_installed();
}

#[test]
fn a_newer_release_without_this_platforms_archive_is_named_and_not_offered() {
    let setup = Setup::archive_install();
    setup.publish(&json!([release("0.1.0-alpha.4", &["linux-aarch64"], "n")]));
    let output = setup.torlek(&["update", "check"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let printed = stdout(&output);
    assert!(
        printed.contains("has no archive for linux-x86_64"),
        "{printed}"
    );
    assert!(!printed.contains("curl -fLO"), "{printed}");
}

#[test]
fn an_installation_that_is_already_the_newest_is_told_so() {
    let setup = Setup::archive_install();
    setup.publish(&json!([release(INSTALLED, &[PLATFORM], "n")]));
    let output = setup.torlek(&["update", "check"]);
    assert!(
        stdout(&output).contains("is the newest release for you"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn an_unreachable_release_source_fails_the_manual_check_and_installs_nothing() {
    let setup = Setup::archive_install();
    setup.offline();
    let output = setup.torlek(&["update", "check"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("asking for the release list failed"),
        "{}",
        stderr(&output)
    );
    assert!(stderr(&output).contains("timed out"), "{}", stderr(&output));
    setup.assert_nothing_was_installed();
}

#[test]
fn malformed_release_metadata_fails_the_manual_check_with_a_reason() {
    let setup = Setup::archive_install();
    for text in [
        "<html>rate limited</html>",
        r#"{"message": "API rate limit exceeded"}"#,
        r#"[{"tag_name": 5}]"#,
    ] {
        setup.publish_text(text);
        let output = setup.torlek(&["update", "check"]);
        assert!(!output.status.success(), "{text}");
        assert!(
            stderr(&output).contains("release"),
            "{text}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn a_build_outside_a_release_bundle_has_nothing_to_check() {
    let setup = Setup::installed_at("lib", "checkout", false);
    newer_alpha_published(&setup);
    let output = setup.torlek(&["update", "check"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("not from a release bundle"),
        "{}",
        stdout(&output)
    );
    assert!(setup.runs("curl").is_empty());
}

#[test]
fn an_installation_outside_the_readme_archive_route_is_said_to_be_unsupported() {
    let installs = [
        ("lib", "distribution-package"),
        ("opt", "torlek-v0.1.0-alpha.3-linux-x86_64"),
    ];
    for (parent, directory_name) in installs {
        let setup = Setup::installed_at(parent, directory_name, true);
        newer_alpha_published(&setup);
        let output = setup.torlek(&["update", "check"]);
        let printed = stdout(&output);
        assert!(printed.contains("0.1.0-alpha.4 is available"), "{printed}");
        assert!(
            printed.contains("is not a release archive unpacked in"),
            "{printed}"
        );
        assert!(!printed.contains("curl -fLO"), "{printed}");
    }
}

#[test]
fn later_postpones_the_notice_for_three_days_and_the_choice_persists() {
    let setup = Setup::archive_install();
    setup.write_state(&cached_offer_state("0.1.0-alpha.4"));
    let output = setup.torlek(&["update", "later"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let postponed_until = setup.state()["postponed_until"].as_i64().unwrap();
    assert!(
        (postponed_until - (now() + 3 * DAY)).abs() < 60,
        "{postponed_until}"
    );
    let printed = setup.torlek_on_terminal(&["doctor"], &[]);
    assert!(!printed.contains("is available"), "{printed}");
}

#[test]
fn skip_silences_that_version_but_a_newer_release_is_announced_again() {
    let setup = Setup::archive_install();
    setup.write_state(&cached_offer_state("0.1.0-alpha.4"));
    let output = setup.torlek(&["update", "skip"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(setup.state()["skipped_version"], "0.1.0-alpha.4");
    assert!(
        !setup
            .torlek_on_terminal(&["doctor"], &[])
            .contains("is available")
    );

    let mut state = setup.state();
    state["latest"] = json!({"version": "0.1.0-alpha.5", "notes": ""});
    setup.write_state(&state);
    let printed = setup.torlek_on_terminal(&["doctor"], &[]);
    assert!(
        printed.contains("torlek 0.1.0-alpha.5 is available"),
        "{printed}"
    );
}

#[test]
fn later_and_skip_with_no_known_newer_release_fail_instead_of_recording_nothing() {
    let setup = Setup::archive_install();
    for verb in ["later", "skip"] {
        let output = setup.torlek(&["update", verb]);
        assert!(!output.status.success(), "{verb}");
        assert!(
            stderr(&output).contains("no newer release is known"),
            "{verb}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn turning_checks_off_persists_and_stops_automatic_checks_but_not_the_manual_one() {
    let setup = Setup::archive_install();
    newer_alpha_published(&setup);
    let output = setup.torlek(&["update", "checks", "off"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(setup.state()["automatic_checks"], false);

    let printed = setup.torlek_on_terminal(&["doctor"], &[]);
    assert!(!printed.contains("is available"), "{printed}");
    assert!(
        setup.runs("curl").is_empty(),
        "an automatic check ran while off"
    );

    let manual = setup.torlek(&["update", "check"]);
    assert!(
        stdout(&manual).contains("0.1.0-alpha.4 is available"),
        "{}",
        stdout(&manual)
    );
    assert_eq!(setup.runs("curl").len(), 1);

    setup.torlek(&["update", "checks", "on"]);
    assert_eq!(setup.state()["automatic_checks"], true);
}

#[test]
fn an_interactive_command_checks_once_announces_and_does_not_repeat_the_notice() {
    let setup = Setup::archive_install();
    newer_alpha_published(&setup);
    let first = setup.torlek_on_terminal(&["doctor"], &[]);
    assert!(
        first.contains("torlek 0.1.0-alpha.4 is available (you have 0.1.0-alpha.3)"),
        "{first}"
    );
    assert!(first.contains("torlek update later"), "{first}");
    assert!(first.contains("torlek update skip"), "{first}");
    assert!(first.contains("torlek update checks off"), "{first}");
    assert_eq!(setup.runs("curl").len(), 1);

    let second = setup.torlek_on_terminal(&["doctor"], &[]);
    assert!(!second.contains("is available"), "{second}");
    assert_eq!(setup.runs("curl").len(), 1, "the second start asked again");
    setup.assert_nothing_was_installed();
}

#[test]
fn a_stale_cache_is_refreshed_after_a_day() {
    let setup = Setup::archive_install();
    newer_alpha_published(&setup);
    setup.write_state(&json!({"last_attempt_at": now() - 2 * DAY}));
    let printed = setup.torlek_on_terminal(&["doctor"], &[]);
    assert!(printed.contains("is available"), "{printed}");
    assert_eq!(setup.runs("curl").len(), 1);
}

#[test]
fn an_offline_start_is_not_blocked_shows_no_notice_and_is_not_retried_the_same_day() {
    let setup = Setup::archive_install();
    setup.offline();
    let first = setup.torlek_on_terminal(&["doctor"], &[]);
    assert!(!first.contains("is available"), "{first}");
    assert!(!first.contains("release list"), "{first}");
    assert!(
        first.contains("daemon") || first.contains("profile"),
        "doctor did not run: {first}"
    );
    setup.torlek_on_terminal(&["doctor"], &[]);
    assert_eq!(setup.runs("curl").len(), 1);
}

#[test]
fn a_failed_check_keeps_the_previous_answer_for_the_next_notice() {
    let setup = Setup::archive_install();
    setup.offline();
    let mut state = cached_offer_state("0.1.0-alpha.4");
    state["last_attempt_at"] = json!(now() - 2 * DAY);
    setup.write_state(&state);
    let printed = setup.torlek_on_terminal(&["doctor"], &[]);
    assert!(printed.contains("0.1.0-alpha.4 is available"), "{printed}");
}

#[test]
fn a_piped_command_neither_checks_nor_prints_a_notice() {
    let setup = Setup::archive_install();
    newer_alpha_published(&setup);
    let output = setup.torlek(&["doctor"]);
    assert!(!stdout(&output).contains("is available"));
    assert!(!stderr(&output).contains("is available"));
    assert!(setup.runs("curl").is_empty());
}

#[test]
fn a_launched_agent_session_neither_checks_nor_prints_a_notice() {
    let setup = Setup::archive_install();
    newer_alpha_published(&setup);
    let printed = setup.torlek_on_terminal(&["doctor"], &[("TORLEK_SESSION", "s1")]);
    assert!(!printed.contains("is available"), "{printed}");
    assert!(setup.runs("curl").is_empty());
}

#[test]
fn machine_readable_commands_never_check_or_print_a_notice() {
    let setup = Setup::archive_install();
    newer_alpha_published(&setup);
    let printed = setup.torlek_on_terminal(&["version"], &[]);
    assert!(!printed.contains("is available"), "{printed}");
    assert!(setup.runs("curl").is_empty());
}

#[test]
fn a_check_on_a_fresh_home_creates_the_state_folder_private_to_the_owner() {
    let setup = Setup::archive_install();
    newer_alpha_published(&setup);
    let output = setup.torlek(&["update", "check"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let mode = fs::metadata(setup.state_path().parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700, "mode is {:o}", mode & 0o777);
}

#[test]
fn a_damaged_state_file_is_reported_by_update_and_never_interrupts_another_command() {
    let setup = Setup::archive_install();
    newer_alpha_published(&setup);
    fs::create_dir_all(setup.state_path().parent().unwrap()).unwrap();
    fs::write(setup.state_path(), "{").unwrap();
    let output = setup.torlek(&["update"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("update.json"),
        "{}",
        stderr(&output)
    );
    let printed = setup.torlek_on_terminal(&["doctor"], &[]);
    assert!(!printed.contains("is available"), "{printed}");
    assert_eq!(fs::read_to_string(setup.state_path()).unwrap(), "{");
}

#[test]
fn unknown_update_arguments_are_usage_errors() {
    let setup = Setup::archive_install();
    for arguments in [
        &["update", "install"][..],
        &["update", "checks"],
        &["update", "checks", "maybe"],
    ] {
        let output = setup.torlek(arguments);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{arguments:?}: {}",
            stderr(&output)
        );
    }
}
