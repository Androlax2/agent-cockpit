//! Finding the installed Codex and its version, as the daemon does at start.

mod common;

use cockpit_codex::{CONFORMANCE_VERSION, Codex};
use cockpit_provider_api::{Availability, Capability, Provider};

#[test]
fn the_version_codex_prints_decides_the_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(
        dir.path(),
        "codex",
        &format!("echo 'WARNING: proceeding' >&2; echo 'codex-cli {CONFORMANCE_VERSION}'"),
    );

    let codex = Codex::discover(&program, Some(common::bridge())).unwrap();

    assert_eq!(codex.version(), Some(CONFORMANCE_VERSION));
    assert_eq!(
        codex.capabilities().availability(Capability::Resume),
        Availability::Available {
            version: CONFORMANCE_VERSION.to_owned()
        }
    );
}

#[test]
fn a_program_that_fails_its_version_is_not_discovered() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(dir.path(), "codex", "echo broken >&2; exit 3");

    let error = Codex::discover(&program, Some(common::bridge())).unwrap_err();

    assert!(error.to_string().contains("broken"), "{error}");
}

#[test]
fn a_version_line_that_is_not_codex_clis_is_refused() {
    for line in ["codex-cli", "codex-cli latest", "0.161.0", "other 0.161.0"] {
        let dir = tempfile::tempdir().unwrap();
        let program = common::script(dir.path(), "codex", &format!("echo '{line}'"));

        assert!(
            Codex::discover(&program, Some(common::bridge())).is_err(),
            "{line}"
        );
    }
}

#[test]
fn codex_is_found_in_the_first_path_directory_that_has_it() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let program = common::script(second.path(), "codex", "exit 0");
    std::fs::write(first.path().join("codex"), "not executable").unwrap();
    let path = std::env::join_paths([first.path(), second.path()]).unwrap();

    assert_eq!(Codex::find_on_path(&path), Some(program));
}

#[test]
fn no_codex_on_the_path_finds_nothing() {
    let empty = tempfile::tempdir().unwrap();

    assert_eq!(Codex::find_on_path(empty.path().as_os_str()), None);
}

/// A wrapper that leaves a background process holding its output must not hold the daemon's
/// start past the deadline.
#[test]
fn a_program_whose_output_never_ends_is_given_up_on() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(
        dir.path(),
        "codex",
        &format!("sleep 60 & echo 'codex-cli {CONFORMANCE_VERSION}'"),
    );
    let started = std::time::Instant::now();

    let outcome = Codex::discover_within(
        &program,
        Some(common::bridge()),
        std::time::Duration::from_millis(500),
    );

    assert!(outcome.is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}
