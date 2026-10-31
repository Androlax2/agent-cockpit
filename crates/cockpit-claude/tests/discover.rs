//! Finding the installed Claude Code and its version, as the daemon does at start.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_claude::{CONFORMANCE_VERSION, ClaudeCode};
use cockpit_provider_api::{Availability, Capability, Provider};

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn the_version_claude_prints_decides_the_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let program = script(
        dir.path(),
        "claude",
        &format!("echo '{CONFORMANCE_VERSION} (Claude Code)'"),
    );

    let claude = ClaudeCode::discover(&program, common::bridge()).unwrap();

    assert_eq!(claude.version(), Some(CONFORMANCE_VERSION));
    assert_eq!(
        claude.capabilities().availability(Capability::Resume),
        Availability::Available {
            version: CONFORMANCE_VERSION.to_owned()
        }
    );
}

#[test]
fn a_program_that_fails_its_version_is_not_discovered() {
    let dir = tempfile::tempdir().unwrap();
    let program = script(dir.path(), "claude", "echo broken >&2; exit 3");

    let error = ClaudeCode::discover(&program, common::bridge()).unwrap_err();

    assert!(error.to_string().contains("broken"), "{error}");
}

#[test]
fn a_version_that_is_not_dotted_numbers_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let program = script(dir.path(), "claude", "echo 'Claude Code'");

    assert!(ClaudeCode::discover(&program, common::bridge()).is_err());
}

#[test]
fn claude_is_found_in_the_first_path_directory_that_has_it() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let program = script(second.path(), "claude", "exit 0");
    std::fs::write(first.path().join("claude"), "not executable").unwrap();
    let path = std::env::join_paths([first.path(), second.path()]).unwrap();

    assert_eq!(ClaudeCode::find_on_path(&path), Some(program));
}

#[test]
fn no_claude_on_the_path_finds_nothing() {
    let empty = tempfile::tempdir().unwrap();

    assert_eq!(ClaudeCode::find_on_path(empty.path().as_os_str()), None);
}
