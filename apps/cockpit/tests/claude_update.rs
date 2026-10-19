//! claude-update: Claude Code's updates outside the sandbox, through the installed claude.

mod common;
use common::sandbox::*;

use std::os::unix::fs::symlink;

fn sandbox_with_claude(update_body: &str) -> (Sandbox, String) {
    let sandbox = Sandbox::new();
    sandbox.shim("claude-sessions", "");
    let versions = sandbox.root.join("versions");
    write_executable(&versions.join("2.1.300"), &format!("#!/bin/sh\necho \"claude $*\" >> \"$SHIM_LOG\"\n{update_body}\n"));
    let link = sandbox.root.join("claude");
    symlink(versions.join("2.1.300"), &link).unwrap();
    (sandbox, link.display().to_string())
}

#[test]
fn the_installed_claude_does_the_update() {
    let (sandbox, link) = sandbox_with_claude("exit 0");
    let output = sandbox.run(bin().join("claude-update"), &[], &[("CLAUDE_LINK", &link)]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.calls("claude"), ["claude update"]);
}

#[test]
fn a_version_file_dropped_in_the_folder_is_never_run() {
    // What a sandboxed session could plant: claude-update runs only the linked, trusted binary.
    let (sandbox, link) = sandbox_with_claude("exit 0");
    write_executable(&sandbox.root.join("versions/99.0.0"), "#!/bin/sh\ntouch \"$SHIM_LOG.planted-ran\"\n");
    sandbox.run(bin().join("claude-update"), &[], &[("CLAUDE_LINK", &link)]);
    assert!(!std::path::Path::new(&format!("{}.planted-ran", sandbox.log.display())).exists());
}

#[test]
fn a_failed_update_is_logged_and_fails() {
    let (sandbox, link) = sandbox_with_claude("echo 'network unreachable'; exit 1");
    let output = sandbox.run(bin().join("claude-update"), &[], &[("CLAUDE_LINK", &link)]);
    assert_eq!(output.status.code(), Some(1));
    assert!(sandbox.calls("claude-sessions").iter().any(|call| call.contains("WARN") && call.contains("network unreachable")));
}

/// A fake opencode at 1.19.3; `upgrade <version>` runs `upgrade_body`, which finds the version file
/// in $version and the requested version in $2. The feed says `newest` is the latest version.
fn sandbox_with_opencode(newest: &str, upgrade_body: &str) -> (Sandbox, Vec<(&'static str, String)>) {
    let sandbox = Sandbox::new();
    sandbox.shim("claude-sessions", "");
    let binary = sandbox.root.join("opencode");
    let version = sandbox.root.join("opencode-version");
    std::fs::write(&version, "opencode v1.19.3\n").unwrap();
    write_executable(&binary, &format!("#!/bin/sh\necho \"opencode $*\" >> \"$SHIM_LOG\"\nversion='{}'\n\
                                         [ \"$1\" = --version ] && exec cat \"$version\"\n{upgrade_body}\n", version.display()));
    let feed = sandbox.root.join("latest.json");
    std::fs::write(&feed, format!(r#"{{"channel":"latest","name":"cli","version":"{newest}"}}"#)).unwrap();
    let environment = vec![("OPENCODE_BINARY", binary.display().to_string()), ("OPENCODE_LATEST_URL", format!("file://{}", feed.display()))];
    (sandbox, environment)
}

fn run_opencode_update(sandbox: &Sandbox, environment: &[(&str, String)]) -> std::process::Output {
    let pairs: Vec<(&str, &str)> = environment.iter().map(|(name, value)| (*name, value.as_str())).collect();
    sandbox.run(bin().join("opencode-update"), &[], &pairs)
}

#[test]
fn the_installed_opencode_upgrades_to_the_newest_version_of_its_major_and_says_which_starts_now() {
    let (sandbox, environment) = sandbox_with_opencode("1.20.0", "echo \"opencode v$2\" > \"$version\"");
    let output = run_opencode_update(&sandbox, &environment);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(sandbox.calls("opencode").contains(&"opencode upgrade 1.20.0".to_string()), "{:?}", sandbox.calls("opencode"));
    assert!(sandbox.calls("claude-sessions").iter().any(|call| call.contains("opencode now starts 1.20.0 (was 1.19.3)")), "{:?}", sandbox.calls("claude-sessions"));
}

#[test]
fn a_new_major_version_is_held_and_logged() {
    let (sandbox, environment) = sandbox_with_opencode("2.0.32", "echo \"opencode v$2\" > \"$version\"");
    let output = run_opencode_update(&sandbox, &environment);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!sandbox.calls("opencode").iter().any(|call| call.starts_with("opencode upgrade")), "{:?}", sandbox.calls("opencode"));
    assert!(sandbox.calls("claude-sessions").iter().any(|call| call.contains("WARN") && call.contains("opencode 2.0.32 held")), "{:?}", sandbox.calls("claude-sessions"));
}

#[test]
fn an_older_version_in_the_feed_is_never_installed() {
    let (sandbox, environment) = sandbox_with_opencode("1.18.0", "echo \"opencode v$2\" > \"$version\"");
    let output = run_opencode_update(&sandbox, &environment);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!sandbox.calls("opencode").iter().any(|call| call.starts_with("opencode upgrade")), "{:?}", sandbox.calls("opencode"));
}

#[test]
fn a_feed_without_a_version_is_logged_and_fails() {
    let (sandbox, environment) = sandbox_with_opencode("not-a-version", "exit 0");
    let output = run_opencode_update(&sandbox, &environment);
    assert_eq!(output.status.code(), Some(1));
    assert!(!sandbox.calls("opencode").iter().any(|call| call.starts_with("opencode upgrade")));
    assert!(sandbox.calls("claude-sessions").iter().any(|call| call.contains("WARN") && call.contains("no version from")), "{:?}", sandbox.calls("claude-sessions"));
}

#[test]
fn a_failed_opencode_upgrade_is_logged_and_fails() {
    let (sandbox, environment) = sandbox_with_opencode("1.20.0", "echo 'network unreachable'; exit 1");
    let output = run_opencode_update(&sandbox, &environment);
    assert_eq!(output.status.code(), Some(1));
    assert!(sandbox.calls("claude-sessions").iter().any(|call| call.contains("WARN") && call.contains("network unreachable")));
}

#[test]
fn no_opencode_installed_is_nothing_to_update() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(bin().join("opencode-update"), &[], &[("OPENCODE_BINARY", "/nonexistent/opencode")]);
    assert!(output.status.success(), "{}", stderr(&output));
}
