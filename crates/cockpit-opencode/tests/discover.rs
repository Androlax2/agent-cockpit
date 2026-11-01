//! Finding the installed OpenCode and its version, as the daemon does at start.

mod common;

use std::time::Duration;

use cockpit_opencode::{CONFORMANCE_VERSION, OpenCode};
use cockpit_provider_api::{Availability, Capability, Provider};

#[test]
fn the_version_opencode_prints_decides_the_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(
        dir.path(),
        "opencode",
        &format!("echo '{CONFORMANCE_VERSION}'"),
    );

    let opencode = OpenCode::discover(&program, Some(common::bridge())).unwrap();

    assert_eq!(opencode.version(), Some(CONFORMANCE_VERSION));
    assert_eq!(
        opencode.capabilities().availability(Capability::Resume),
        Availability::Available {
            version: CONFORMANCE_VERSION.to_owned()
        }
    );
}

#[test]
fn a_leading_v_is_not_part_of_the_version() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(dir.path(), "opencode", "echo v1.19.5");

    let opencode = OpenCode::discover(&program, None).unwrap();

    assert_eq!(opencode.version(), Some("1.19.5"));
}

#[test]
fn a_program_that_fails_its_version_is_not_discovered() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(dir.path(), "opencode", "echo broken >&2; exit 3");

    let error = OpenCode::discover(&program, None).unwrap_err();

    assert!(error.to_string().contains("broken"), "{error}");
}

#[test]
fn a_version_that_is_not_dotted_numbers_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(dir.path(), "opencode", "echo opencode");

    assert!(OpenCode::discover(&program, None).is_err());
}

#[test]
fn opencode_is_found_in_the_first_path_directory_that_has_it() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let program = common::script(second.path(), "opencode", "exit 0");
    std::fs::write(first.path().join("opencode"), "not executable").unwrap();
    let path = std::env::join_paths([first.path(), second.path()]).unwrap();

    assert_eq!(OpenCode::find_on_path(&path), Some(program));
}

#[test]
fn no_opencode_on_the_path_finds_nothing() {
    let empty = tempfile::tempdir().unwrap();

    assert_eq!(OpenCode::find_on_path(empty.path().as_os_str()), None);
}

#[test]
fn a_program_whose_output_never_ends_is_given_up_on() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(
        dir.path(),
        "opencode",
        &format!("sleep 60 & echo '{CONFORMANCE_VERSION}'"),
    );
    let started = std::time::Instant::now();

    let outcome = OpenCode::discover_within(&program, None, Duration::from_millis(500));

    assert!(outcome.is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
}
