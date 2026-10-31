use std::fs::{self, File, FileTimes};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use cockpit_diagnostics::{BundleRequest, BundleSection, Error, plan_bundle};

fn log_dir_with(state_dir: &Path, process: &str, files: &[(&str, &str)]) -> PathBuf {
    let log_dir = state_dir.join(process);
    fs::create_dir_all(&log_dir).unwrap();
    for (name, contents) in files {
        fs::write(log_dir.join(name), contents).unwrap();
    }
    log_dir
}

fn age(path: &Path, by: Duration) {
    let modified = SystemTime::now() - by;
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
}

fn listed_paths(request: &BundleRequest) -> Vec<String> {
    plan_bundle(request)
        .unwrap()
        .contents()
        .iter()
        .map(|item| item.path.clone())
        .collect()
}

fn tar_listing(archive: &Path) -> Vec<String> {
    let output = Command::new("tar")
        .arg("-tf")
        .arg(archive)
        .output()
        .expect("tar runs");
    assert!(
        output.status.success(),
        "tar rejects the archive: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_plan_lists_logs_sections_and_platform_facts_before_writing() {
    let state_dir = tempfile::tempdir().unwrap();
    let daemon_logs = log_dir_with(
        state_dir.path(),
        "daemon",
        &[("cockpit.log", "{}\n"), ("cockpit.log.1", "{}\n")],
    );
    let mut request = BundleRequest::new(vec![daemon_logs]);
    request
        .sections
        .push(BundleSection::new("schema-version.txt", b"7\n".to_vec()).unwrap());

    assert_eq!(
        listed_paths(&request),
        [
            "cockpit-support-bundle/logs/daemon/cockpit.log",
            "cockpit-support-bundle/logs/daemon/cockpit.log.1",
            "cockpit-support-bundle/schema-version.txt",
            "cockpit-support-bundle/platform.json",
        ]
    );
}

#[test]
fn the_listing_reports_each_entry_size() {
    let state_dir = tempfile::tempdir().unwrap();
    let daemon_logs = log_dir_with(state_dir.path(), "daemon", &[("cockpit.log", "0123456789")]);
    let plan = plan_bundle(&BundleRequest::new(vec![daemon_logs])).unwrap();
    assert_eq!(plan.contents()[0].bytes, 10);
}

#[test]
fn the_written_archive_holds_exactly_the_listed_entries() {
    let state_dir = tempfile::tempdir().unwrap();
    let daemon_logs = log_dir_with(
        state_dir.path(),
        "daemon",
        &[("cockpit.log", "{\"a\":1}\n")],
    );
    let plan = plan_bundle(&BundleRequest::new(vec![daemon_logs])).unwrap();
    let archive = state_dir.path().join("bundle.tar");

    plan.write(&archive).unwrap();

    let listed: Vec<String> = plan
        .contents()
        .iter()
        .map(|item| item.path.clone())
        .collect();
    assert_eq!(tar_listing(&archive), listed);
    assert_eq!(
        fs::metadata(&archive).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn the_archive_preserves_file_contents() {
    let state_dir = tempfile::tempdir().unwrap();
    let contents = "{\"line\":\"é\"}\n".repeat(300);
    let daemon_logs = log_dir_with(state_dir.path(), "daemon", &[("cockpit.log", &contents)]);
    let archive = state_dir.path().join("bundle.tar");
    plan_bundle(&BundleRequest::new(vec![daemon_logs]))
        .unwrap()
        .write(&archive)
        .unwrap();

    let output = Command::new("tar")
        .arg("-xOf")
        .arg(&archive)
        .arg("cockpit-support-bundle/logs/daemon/cockpit.log")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(output.stdout).unwrap(), contents);
}

#[test]
fn writing_never_replaces_an_existing_file() {
    let state_dir = tempfile::tempdir().unwrap();
    let archive = state_dir.path().join("bundle.tar");
    fs::write(&archive, "keep me").unwrap();
    let plan = plan_bundle(&BundleRequest::new(vec![])).unwrap();
    assert!(plan.write(&archive).is_err());
    assert_eq!(fs::read_to_string(&archive).unwrap(), "keep me");
}

#[test]
fn log_files_last_written_before_the_window_are_left_out() {
    let state_dir = tempfile::tempdir().unwrap();
    let daemon_logs = log_dir_with(
        state_dir.path(),
        "daemon",
        &[("cockpit.log", "{}\n"), ("cockpit.log.1", "{}\n")],
    );
    age(
        &daemon_logs.join("cockpit.log.1"),
        Duration::from_secs(3 * 3600),
    );
    let mut request = BundleRequest::new(vec![daemon_logs]);
    request.since = Some(SystemTime::now() - Duration::from_secs(3600));

    let paths = listed_paths(&request);

    assert!(paths.contains(&"cockpit-support-bundle/logs/daemon/cockpit.log".to_owned()));
    assert!(!paths.contains(&"cockpit-support-bundle/logs/daemon/cockpit.log.1".to_owned()));
}

#[test]
fn files_that_are_not_cockpit_logs_are_left_out() {
    let state_dir = tempfile::tempdir().unwrap();
    let daemon_logs = log_dir_with(
        state_dir.path(),
        "daemon",
        &[("cockpit.log", "{}\n"), ("notes.txt", "private")],
    );
    let paths = listed_paths(&BundleRequest::new(vec![daemon_logs]));
    assert!(
        !paths.iter().any(|path| path.ends_with("notes.txt")),
        "{paths:?}"
    );
}

#[test]
fn a_bundle_over_the_size_bound_is_refused_before_writing() {
    let state_dir = tempfile::tempdir().unwrap();
    let daemon_logs = log_dir_with(
        state_dir.path(),
        "daemon",
        &[("cockpit.log", &"x".repeat(4096))],
    );
    let mut request = BundleRequest::new(vec![daemon_logs]);
    request.max_bytes = 1024;
    let error = plan_bundle(&request).unwrap_err();
    assert!(matches!(error, Error::BundleTooLarge { .. }), "{error}");
}

#[test]
fn a_missing_log_directory_fails_loudly() {
    let state_dir = tempfile::tempdir().unwrap();
    let request = BundleRequest::new(vec![state_dir.path().join("absent")]);
    assert!(matches!(
        plan_bundle(&request).unwrap_err(),
        Error::Io { .. }
    ));
}

#[test]
fn a_section_name_must_be_a_plain_file_name() {
    for name in ["", ".", "..", "a/b", "logs", "platform.json"] {
        assert!(BundleSection::new(name, Vec::new()).is_err(), "{name:?}");
    }
}

#[test]
fn platform_facts_name_the_operating_system() {
    let plan = plan_bundle(&BundleRequest::new(vec![])).unwrap();
    let facts: serde_json::Value =
        serde_json::from_slice(plan.entry("platform.json").unwrap()).unwrap();
    assert_eq!(facts["os"], std::env::consts::OS);
    assert!(facts["wsl"].is_boolean());
}
