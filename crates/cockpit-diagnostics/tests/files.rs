mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use cockpit_diagnostics::{FileLimits, Options};
use support::{SHUTDOWN_TIMEOUT, mode, records};

fn start_with_limits(log_dir: &Path, limits: FileLimits) -> cockpit_diagnostics::Diagnostics {
    let mut options = Options::new(log_dir.to_path_buf(), "info".parse().unwrap());
    options.limits = limits;
    cockpit_diagnostics::start(options).unwrap()
}

fn log_files(log_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(log_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn records_are_json_lines_carrying_the_ids_of_their_spans() {
    let log_dir = tempfile::tempdir().unwrap();
    support::record(log_dir.path(), "info", |_| {
        let request = tracing::info_span!("request", request_id = "req-7", session_id = "ses-3");
        let _entered = request.enter();
        let attempt = tracing::info_span!("attempt", task_id = "task-1", attempt_id = "att-2");
        let _entered = attempt.enter();
        tracing::info!("task assigned");
    });
    let record = records(log_dir.path()).pop().unwrap();
    let span_fields = record["spans"].to_string();
    for id in ["req-7", "ses-3", "task-1", "att-2"] {
        assert!(span_fields.contains(id), "{id} missing from {record}");
    }
}

#[test]
fn the_log_directory_and_file_are_private_to_the_owner() {
    let state_dir = tempfile::tempdir().unwrap();
    let log_dir = state_dir.path().join("logs/daemon");
    support::record(&log_dir, "info", |_| tracing::info!("started"));
    assert_eq!(mode(&log_dir), 0o700);
    assert_eq!(mode(&log_dir.join("cockpit.log")), 0o600);
}

#[test]
fn an_existing_log_file_is_made_private_again() {
    let log_dir = tempfile::tempdir().unwrap();
    let log_file = log_dir.path().join("cockpit.log");
    fs::write(&log_file, "").unwrap();
    fs::set_permissions(&log_file, fs::Permissions::from_mode(0o644)).unwrap();
    support::record(log_dir.path(), "info", |_| tracing::info!("started"));
    assert_eq!(mode(&log_file), 0o600);
}

#[test]
fn files_rotate_by_size_and_keep_at_most_the_file_count() {
    let log_dir = tempfile::tempdir().unwrap();
    let limits = FileLimits {
        max_file_bytes: 1024,
        max_files: 3,
        ..FileLimits::default()
    };
    let diagnostics = start_with_limits(log_dir.path(), limits);
    tracing::dispatcher::with_default(diagnostics.dispatch(), || {
        for index in 0..200 {
            tracing::info!(index, "filling the log");
            // The queue is bounded: give the writer time instead of measuring drops here.
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    assert!(diagnostics.shutdown(SHUTDOWN_TIMEOUT));
    assert_eq!(
        log_files(log_dir.path()),
        ["cockpit.log", "cockpit.log.1", "cockpit.log.2"]
    );
    for name in log_files(log_dir.path()) {
        let path = log_dir.path().join(&name);
        assert!(
            fs::metadata(&path).unwrap().len() <= 1024,
            "{name} exceeds the file size"
        );
        assert_eq!(mode(&path), 0o600, "{name}");
    }
}

#[test]
fn rotated_files_older_than_the_age_limit_are_removed_at_start() {
    let log_dir = tempfile::tempdir().unwrap();
    let stale = log_dir.path().join("cockpit.log.1");
    fs::write(&stale, "{}\n").unwrap();
    support::age(&stale, Duration::from_secs(8 * 24 * 3600));
    let recent = log_dir.path().join("cockpit.log.2");
    fs::write(&recent, "{}\n").unwrap();

    support::record(log_dir.path(), "info", |_| tracing::info!("started"));

    assert!(!stale.exists());
    assert!(recent.exists());
}

#[test]
fn a_record_larger_than_a_file_is_dropped_and_counted() {
    let log_dir = tempfile::tempdir().unwrap();
    let limits = FileLimits {
        max_file_bytes: 512,
        ..FileLimits::default()
    };
    let diagnostics = start_with_limits(log_dir.path(), limits);
    tracing::dispatcher::with_default(diagnostics.dispatch(), || {
        tracing::info!(detail = "x".repeat(2048), "oversized");
    });
    assert!(diagnostics.shutdown(SHUTDOWN_TIMEOUT));
    assert_eq!(diagnostics.dropped_records(), 1);
    assert!(!support::current_log(log_dir.path()).contains("oversized"));
}

#[test]
fn the_default_limits_are_four_files_of_four_mebibytes_for_seven_days() {
    let limits = FileLimits::default();
    assert_eq!(
        (limits.max_file_bytes, limits.max_files),
        (4 * 1024 * 1024, 4)
    );
    assert_eq!(limits.max_age, Duration::from_secs(7 * 24 * 3600));
}

#[test]
fn the_log_directory_is_per_process_under_the_xdg_state_home() {
    let state_home = Path::new("/state");
    let log_dir = cockpit_diagnostics::log_dir_in(state_home, "daemon").unwrap();
    assert_eq!(log_dir, Path::new("/state/cockpit/logs/daemon"));
}

#[test]
fn a_process_name_that_is_not_a_plain_name_is_rejected() {
    for process in ["", ".", "..", "a/b"] {
        assert!(
            cockpit_diagnostics::log_dir_in(Path::new("/state"), process).is_err(),
            "{process:?}"
        );
    }
}

#[test]
fn a_second_process_cannot_share_a_log_directory() {
    let log_dir = tempfile::tempdir().unwrap();
    let _first = support::start(log_dir.path(), "info");
    let options = Options::new(log_dir.path().to_path_buf(), "info".parse().unwrap());
    let error = cockpit_diagnostics::start(options).unwrap_err();
    assert!(
        matches!(error, cockpit_diagnostics::Error::LogDirInUse { .. }),
        "{error}"
    );
}

#[test]
fn a_failed_rotation_keeps_writing_and_keeps_older_files() {
    let log_dir = tempfile::tempdir().unwrap();
    fs::write(log_dir.path().join("cockpit.log.1"), "first rotation\n").unwrap();
    let limits = FileLimits {
        max_file_bytes: 1024,
        max_files: 3,
        ..FileLimits::default()
    };
    let diagnostics = start_with_limits(log_dir.path(), limits);
    // Without write permission on the directory every rename fails; appending still works.
    fs::set_permissions(log_dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
    tracing::dispatcher::with_default(diagnostics.dispatch(), || {
        for index in 0..50 {
            tracing::info!(index, "filling the log");
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    assert!(diagnostics.shutdown(SHUTDOWN_TIMEOUT));
    fs::set_permissions(log_dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        fs::read_to_string(log_dir.path().join("cockpit.log.1")).unwrap(),
        "first rotation\n"
    );
    assert!(!log_dir.path().join("cockpit.log.2").exists());
    let log = support::current_log(log_dir.path());
    assert!(
        log.contains("\"index\":49"),
        "records after the failed rotation are kept"
    );
    assert!(log.contains("log rotation failed"), "{log}");
    assert_eq!(diagnostics.dropped_records(), 0);
}
