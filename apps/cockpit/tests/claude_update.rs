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
