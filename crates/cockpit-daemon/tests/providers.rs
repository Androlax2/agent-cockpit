//! #33: the daemon binary wires the provider adapters it finds at start. Claude Code, Codex
//! (#411) and OpenCode (#415) are looked up on the daemon's `PATH`; each one's version
//! decides whether its capabilities count as tested.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::TestHome;

/// A `claude` in `dir` whose `--version` runs `body`.
fn fake_claude(dir: &Path, body: &str) {
    fake_program(dir, "claude", body);
}

/// A program `name` in `dir` that runs `body`.
fn fake_program(dir: &Path, name: &str, body: &str) {
    let program = dir.join(name);
    std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn home_with_path(dir: &Path) -> TestHome {
    let mut home = TestHome::new();
    home.path = Some(dir.as_os_str().to_owned());
    home
}

#[test]
fn claude_code_on_the_path_is_wired_with_its_version_tested() {
    let bin = tempfile::tempdir().unwrap();
    fake_claude(bin.path(), "echo '2.1.316 (Claude Code)'");
    let home = home_with_path(bin.path());
    let _daemon = home.start();

    let log = home.log_once_it_contains("provider claude");

    assert!(
        log.contains("provider claude ready version=2.1.316 provenance=tested"),
        "{log}"
    );
}

#[test]
fn another_claude_code_version_is_wired_as_unverified() {
    let bin = tempfile::tempdir().unwrap();
    fake_claude(bin.path(), "echo '9.0.0 (Claude Code)'");
    let home = home_with_path(bin.path());
    let _daemon = home.start();

    let log = home.log_once_it_contains("provider claude");

    assert!(
        log.contains("provider claude ready version=9.0.0 provenance=unverified"),
        "{log}"
    );
}

#[test]
fn without_claude_code_the_daemon_starts_and_says_so() {
    let empty = tempfile::tempdir().unwrap();
    let home = home_with_path(empty.path());
    let _daemon = home.start();

    let log = home.log_once_it_contains("provider claude");

    assert!(
        log.contains("provider claude unavailable: no `claude` on PATH"),
        "{log}"
    );
}

#[test]
fn a_claude_code_that_fails_its_version_is_not_wired() {
    let bin = tempfile::tempdir().unwrap();
    fake_claude(bin.path(), "echo 'not signed in' >&2; exit 1");
    let home = home_with_path(bin.path());
    let _daemon = home.start();

    let log = home.log_once_it_contains("provider claude");

    assert!(log.contains("provider claude unavailable:"), "{log}");
    assert!(log.contains("not signed in"), "{log}");
}

#[test]
fn codex_on_the_path_is_wired_with_its_version_tested() {
    let bin = tempfile::tempdir().unwrap();
    fake_program(bin.path(), "codex", "echo 'codex-cli 0.161.0'");
    let home = home_with_path(bin.path());
    let _daemon = home.start();

    let log = home.log_once_it_contains("provider codex");

    assert!(
        log.contains("provider codex ready version=0.161.0 provenance=tested"),
        "{log}"
    );
}

#[test]
fn without_codex_the_daemon_starts_and_says_so() {
    let empty = tempfile::tempdir().unwrap();
    let home = home_with_path(empty.path());
    let _daemon = home.start();

    let log = home.log_once_it_contains("provider codex");

    assert!(
        log.contains("provider codex unavailable: no `codex` on PATH"),
        "{log}"
    );
}

#[test]
fn opencode_on_the_path_is_wired_with_its_version_tested() {
    let bin = tempfile::tempdir().unwrap();
    fake_program(bin.path(), "opencode", "echo '1.19.5'");
    let home = home_with_path(bin.path());
    let _daemon = home.start();

    let log = home.log_once_it_contains("provider opencode");

    assert!(
        log.contains("provider opencode ready version=1.19.5 provenance=tested"),
        "{log}"
    );
}

#[test]
fn without_opencode_the_daemon_starts_and_says_so() {
    let empty = tempfile::tempdir().unwrap();
    let home = home_with_path(empty.path());
    let _daemon = home.start();

    let log = home.log_once_it_contains("provider opencode");

    assert!(
        log.contains("provider opencode unavailable: no `opencode` on PATH"),
        "{log}"
    );
}
