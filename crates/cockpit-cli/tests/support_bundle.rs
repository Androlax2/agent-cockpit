//! `torlek bug-report new` against the real daemon: the support bundle it writes carries what
//! report 12 lists, and a secret in the owner's configuration does not reach it (#634). The
//! version manifest names each component, `--since` narrows the logs, and the CLI lists the
//! contents before the bundle is written (#652).
mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::TestHome;

const ROOT: &str = "cockpit-support-bundle";
/// A GitHub token's shape, as the approvals redactor recognises it.
const PLANTED_TOKEN: &str = "ghp_PlantedSupportBundleToken0123456789ab";
const TAR_BLOCK: usize = 512;

#[test]
fn a_new_report_bundles_logs_readiness_config_and_schema() {
    let home = TestHome::new();
    let _daemon = home.start();
    let entries = new_report_bundle(&home);
    let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
    for expected in [
        "logs/daemon/cockpit.log",
        "version.txt",
        "readiness.json",
        "config.txt",
        "schema.txt",
        "platform.json",
    ] {
        assert!(
            names.contains(&format!("{ROOT}/{expected}").as_str()),
            "{expected} is missing from {names:?}"
        );
    }
    let readiness: serde_json::Value =
        serde_json::from_slice(entry(&entries, "readiness.json")).unwrap();
    assert!(readiness["requirements"].is_array(), "{readiness}");
    let schema = String::from_utf8_lossy(entry(&entries, "schema.txt"));
    assert!(
        schema.lines().any(|line| line.starts_with("bug_reports ")),
        "{schema}"
    );
}

#[test]
fn a_token_in_the_owners_configuration_does_not_reach_the_bundle() {
    let home = TestHome::new();
    let config = home.home().join("config/torlek-dev");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!("[tasks]\nblocker_project = \"{PLANTED_TOKEN}\"\n"),
    )
    .unwrap();
    let _daemon = home.start();
    let entries = new_report_bundle(&home);
    let shown_config = String::from_utf8_lossy(entry(&entries, "config.txt"));
    assert!(
        shown_config.contains("tasks.blocker_project = \"[redacted]\""),
        "the configuration was not bundled redacted: {shown_config}"
    );
    for (name, contents) in &entries {
        assert!(
            !String::from_utf8_lossy(contents).contains(PLANTED_TOKEN),
            "the planted token reached {name}"
        );
    }
}

#[test]
fn the_version_manifest_names_the_cli_the_daemon_and_the_session_helper_bundles() {
    let home = TestHome::new();
    let _daemon = home.start();
    let entries = new_report_bundle(&home);
    let manifest = String::from_utf8_lossy(entry(&entries, "version.txt")).into_owned();
    let installed = home.cockpit_ok(&["version"]);
    let version = env!("CARGO_PKG_VERSION");
    for expected in [
        format!("installed CLI: {}\n", installed.trim_end()),
        format!("running daemon: torlek {version} (revision "),
        "session helper bundles:\n".to_owned(),
        format!("  agent instructions: torlek {version} (manifest m-"),
        format!("  Claude Code skills: torlek {version} (manifest m-"),
        "tmux: ".to_owned(),
    ] {
        assert!(
            manifest.contains(&expected),
            "{expected:?} is missing from {manifest}"
        );
    }
}

#[test]
fn since_leaves_out_the_logs_last_written_before_it() {
    let home = TestHome::new();
    let _daemon = home.start();
    let rotated = state_dir(&home).join("logs/daemon/cockpit.log.1");
    std::fs::write(&rotated, "an older run's line\n").unwrap();
    let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    std::fs::File::options()
        .write(true)
        .open(&rotated)
        .unwrap()
        .set_modified(two_hours_ago)
        .unwrap();

    let everything = new_report_bundle_with(&home, &[]);
    let last_hour = new_report_bundle_with(&home, &["--since", "1h"]);

    let has = |entries: &[(String, Vec<u8>)], name: &str| {
        entries
            .iter()
            .any(|(path, _)| *path == format!("{ROOT}/{name}"))
    };
    assert!(has(&everything, "logs/daemon/cockpit.log.1"));
    assert!(!has(&last_hour, "logs/daemon/cockpit.log.1"));
    assert!(has(&last_hour, "logs/daemon/cockpit.log"));
}

#[test]
fn a_since_the_cli_cannot_read_files_no_draft() {
    let home = TestHome::new();
    let _daemon = home.start();
    let refused = home.cockpit_err(&["bug-report", "new", "a window", "--since", "yesterday"]);
    assert!(refused.contains("--since yesterday"), "{refused}");
    assert!(
        home.cockpit_ok(&["bug-report", "list"])
            .contains("no bug report drafts")
    );
}

#[test]
fn the_cli_lists_what_the_bundle_carries_before_writing_it() {
    let home = TestHome::new();
    let _daemon = home.start();
    let printed = home.cockpit_ok(&["bug-report", "new", "the listing is checked"]);
    let listed = listed_paths(&printed);
    let written = read_tar(&bundle_path(&printed));
    let written: Vec<&str> = written.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(listed, written, "{printed}");
}

#[test]
fn the_listing_comes_first_even_when_the_bundle_cannot_be_written() {
    let home = TestHome::new();
    let _daemon = home.start();
    // A file where the bundles' directory goes: the draft is filed, its bundle is not.
    std::fs::write(state_dir(&home).join("bug-reports"), "").unwrap();
    let printed = home.cockpit_ok(&["bug-report", "new", "the write fails"]);
    assert!(
        printed.starts_with("A new draft's support bundle will carry:\n"),
        "{printed}"
    );
    assert!(
        listed_paths(&printed).contains(&format!("{ROOT}/version.txt").as_str()),
        "{printed}"
    );
    assert!(printed.contains("Support bundle: none: "), "{printed}");
}

/// Files a report through the CLI and reads back the archive it names.
fn new_report_bundle(home: &TestHome) -> Vec<(String, Vec<u8>)> {
    new_report_bundle_with(home, &[])
}

fn new_report_bundle_with(home: &TestHome, flags: &[&str]) -> Vec<(String, Vec<u8>)> {
    let mut arguments = vec!["bug-report", "new", "the support bundle is checked"];
    arguments.extend_from_slice(flags);
    let printed = home.cockpit_ok(&arguments);
    read_tar(&bundle_path(&printed))
}

fn bundle_path(printed: &str) -> PathBuf {
    printed
        .lines()
        .find_map(|line| line.strip_prefix("Support bundle: "))
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("no bundle path in {printed}"))
}

/// The paths printed under the listing, before the draft.
fn listed_paths(printed: &str) -> Vec<&str> {
    printed
        .lines()
        .skip_while(|line| *line != "A new draft's support bundle will carry:")
        .skip(1)
        .map_while(|line| line.strip_prefix("  "))
        .collect()
}

fn state_dir(home: &TestHome) -> PathBuf {
    home.home().join("state/torlek-dev")
}

fn entry<'a>(entries: &'a [(String, Vec<u8>)], name: &str) -> &'a [u8] {
    let path = format!("{ROOT}/{name}");
    entries
        .iter()
        .find(|(entry_name, _)| *entry_name == path)
        .map(|(_, contents)| contents.as_slice())
        .unwrap_or_else(|| panic!("{path} is not in the bundle"))
}

/// The regular files of a ustar archive, as the bundle writes it.
fn read_tar(path: &Path) -> Vec<(String, Vec<u8>)> {
    let archive = std::fs::read(path).unwrap();
    let mut entries = Vec::new();
    let mut offset = 0;
    while offset + TAR_BLOCK <= archive.len() {
        let header = &archive[offset..offset + TAR_BLOCK];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let name = String::from_utf8_lossy(&header[..100])
            .trim_end_matches('\0')
            .to_owned();
        let size_field = String::from_utf8_lossy(&header[124..136]);
        let size =
            usize::from_str_radix(size_field.trim_matches(|c: char| c == '\0' || c == ' '), 8)
                .unwrap();
        let start = offset + TAR_BLOCK;
        entries.push((name, archive[start..start + size].to_vec()));
        offset = start + size.div_ceil(TAR_BLOCK) * TAR_BLOCK;
    }
    entries
}
