//! `torlek bug-report new` against the real daemon: the support bundle it writes carries what
//! report 12 lists, and a secret in the owner's configuration does not reach it (#634).
mod common;

use std::path::Path;

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

/// Files a report through the CLI and reads back the archive it names.
fn new_report_bundle(home: &TestHome) -> Vec<(String, Vec<u8>)> {
    let printed = home.cockpit_ok(&["bug-report", "new", "the support bundle is checked"]);
    let path = printed
        .lines()
        .find_map(|line| line.strip_prefix("Support bundle: "))
        .unwrap_or_else(|| panic!("no bundle path in {printed}"));
    read_tar(Path::new(path))
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
