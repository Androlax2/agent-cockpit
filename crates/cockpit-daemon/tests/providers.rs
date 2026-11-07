//! #33: the daemon binary wires the provider adapters it finds at start. Claude Code, Codex
//! (#411) and OpenCode (#415) are looked up on the daemon's `PATH`; each one's version
//! decides whether its capabilities count as tested.

mod candidate;
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

/// A `claude` of version `9.0.0` whose `--help` documents `flags` and whose file holds every
/// launch environment name and hook event (#732).
fn probed_claude(dir: &Path, flags: &[&str]) {
    let help: String = flags
        .iter()
        .map(|flag| format!("  {flag} <value>\n"))
        .collect();
    let names: Vec<&str> = cockpit_claude::LAUNCH_ENVIRONMENT
        .iter()
        .map(|(name, _)| *name)
        .chain(cockpit_claude::hooks::HOOK_EVENTS)
        .collect();
    fake_claude(
        dir,
        &format!(
            "case \"$1\" in\n--version) echo '9.0.0 (Claude Code)' ;;\n\
             *) printf '{help}' ;;\nesac\n# {}",
            names.join(" ")
        ),
    );
}

#[test]
fn an_untested_claude_code_that_has_every_requirement_is_wired_as_probed() {
    let bin = tempfile::tempdir().unwrap();
    probed_claude(bin.path(), &cockpit_claude::LAUNCH_FLAGS);
    let home = home_with_path(bin.path());
    let _daemon = home.start();

    let log = home.log_once_it_contains("provider claude");

    assert!(
        log.contains("provider claude ready version=9.0.0 provenance=probed"),
        "{log}"
    );
}

#[test]
fn an_untested_claude_code_missing_a_flag_is_warned_about_with_the_flag_named() {
    let bin = tempfile::tempdir().unwrap();
    let flags: Vec<&str> = cockpit_claude::LAUNCH_FLAGS
        .into_iter()
        .filter(|flag| *flag != "--effort")
        .collect();
    probed_claude(bin.path(), &flags);
    let home = home_with_path(bin.path());
    let _daemon = home.start();

    let log = home.log_once_it_contains("provider claude");

    assert!(
        log.contains("is not tested on 9.0.0 and its probe failed")
            && log.contains("flag --effort in `claude --help`"),
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
    fake_program(bin.path(), "codex", "echo 'codex-cli 0.166.0'");
    let home = home_with_path(bin.path());
    let _daemon = home.start();

    let log = home.log_once_it_contains("provider codex");

    assert!(
        log.contains("provider codex ready version=0.166.0 provenance=tested"),
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

/// #623: OpenCode 2 prints `opencode v2.1.4`.
#[test]
fn opencode_that_prints_its_name_before_the_version_is_wired() {
    let bin = tempfile::tempdir().unwrap();
    fake_program(bin.path(), "opencode", "echo 'opencode v2.1.4'");
    let home = home_with_path(bin.path());
    let _daemon = home.start();

    let log = home.log_once_it_contains("provider opencode");

    assert!(
        log.contains("provider opencode ready version=2.1.4 provenance=unverified"),
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
