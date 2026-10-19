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

/// A fake opencode at 1.19.3; `upgrade` runs `upgrade_body`, which finds the version file in $version.
fn sandbox_with_opencode(upgrade_body: &str) -> (Sandbox, String) {
    let sandbox = Sandbox::new();
    sandbox.shim("claude-sessions", "");
    let binary = sandbox.root.join("opencode");
    let version = sandbox.root.join("opencode-version");
    std::fs::write(&version, "1.19.3\n").unwrap();
    write_executable(&binary, &format!("#!/bin/sh\necho \"opencode $*\" >> \"$SHIM_LOG\"\nversion='{}'\n\
                                         [ \"$1\" = --version ] && exec cat \"$version\"\n{upgrade_body}\n", version.display()));
    (sandbox, binary.display().to_string())
}

#[test]
fn the_installed_opencode_does_its_upgrade_and_says_which_version_starts_now() {
    let (sandbox, binary) = sandbox_with_opencode("echo 1.20.0 > \"$version\"");
    let output = sandbox.run(bin().join("opencode-update"), &[], &[("OPENCODE_BINARY", &binary)]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(sandbox.calls("opencode").contains(&"opencode upgrade".to_string()));
    assert!(sandbox.calls("claude-sessions").iter().any(|call| call.contains("opencode now starts 1.20.0 (was 1.19.3)")), "{:?}", sandbox.calls("claude-sessions"));
}

#[test]
fn a_failed_opencode_upgrade_is_logged_and_fails() {
    let (sandbox, binary) = sandbox_with_opencode("echo 'network unreachable'; exit 1");
    let output = sandbox.run(bin().join("opencode-update"), &[], &[("OPENCODE_BINARY", &binary)]);
    assert_eq!(output.status.code(), Some(1));
    assert!(sandbox.calls("claude-sessions").iter().any(|call| call.contains("WARN") && call.contains("network unreachable")));
}

#[test]
fn no_opencode_installed_is_nothing_to_update() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(bin().join("opencode-update"), &[], &[("OPENCODE_BINARY", "/nonexistent/opencode")]);
    assert!(output.status.success(), "{}", stderr(&output));
}
