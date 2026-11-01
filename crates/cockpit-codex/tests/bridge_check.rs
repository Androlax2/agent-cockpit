//! `Bridge::check` (#203): the daemon's start check that the hook bridge it ships runs here,
//! so a bundle whose hook is present but unusable shows at once, not as missing events.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_codex::Bridge;

/// A bridge whose hook is `hook`, beside a `cockpit` that is an executable file.
fn bridge_with(dir: &Path, hook: PathBuf) -> Bridge {
    let cockpit = dir.join("cockpit");
    std::fs::write(&cockpit, "").unwrap();
    std::fs::set_permissions(&cockpit, std::fs::Permissions::from_mode(0o755)).unwrap();
    Bridge {
        hook_program: hook,
        cockpit_program: cockpit,
    }
}

#[test]
fn the_built_hook_bridge_passes() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = bridge_with(
        dir.path(),
        PathBuf::from(env!("CARGO_BIN_EXE_cockpit-codex-hook")),
    );

    assert_eq!(bridge.check(), Ok(()));
}

#[test]
fn the_built_hook_bridge_answers_its_version() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit-codex-hook"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        cockpit_codex::hook_version_line()
    );
}

#[test]
fn a_missing_program_fails_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let hook = dir.path().join("cockpit-codex-hook");

    let reason = bridge_with(dir.path(), hook.clone()).check().unwrap_err();

    assert_eq!(reason, format!("{} missing", hook.display()));
}

#[test]
fn a_cockpit_without_its_execute_bit_fails_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut bridge = bridge_with(
        dir.path(),
        PathBuf::from(env!("CARGO_BIN_EXE_cockpit-codex-hook")),
    );
    let cockpit = dir.path().join("cockpit-plain");
    std::fs::write(&cockpit, "").unwrap();
    std::fs::set_permissions(&cockpit, std::fs::Permissions::from_mode(0o644)).unwrap();
    bridge.cockpit_program = cockpit.clone();

    let reason = bridge.check().unwrap_err();

    assert!(
        reason.contains(&cockpit.display().to_string()) && reason.contains("execute bit"),
        "{reason}"
    );
}

#[test]
fn another_program_in_the_hooks_place_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let hook = common::script(dir.path(), "cockpit-codex-hook", "echo 'something 1.0'");

    let reason = bridge_with(dir.path(), hook).check().unwrap_err();

    assert!(reason.contains("not a hook bridge"), "{reason}");
}

#[test]
fn a_hook_of_another_release_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let hook = common::script(
        dir.path(),
        "cockpit-codex-hook",
        "echo 'cockpit-codex-hook 0.0.0'",
    );

    let reason = bridge_with(dir.path(), hook).check().unwrap_err();

    assert!(reason.contains("not this release's"), "{reason}");
}

#[test]
fn a_hook_that_never_answers_is_given_up_on() {
    let dir = tempfile::tempdir().unwrap();
    let hook = common::script(dir.path(), "cockpit-codex-hook", "exec sleep 60");

    let reason = bridge_with(dir.path(), hook)
        .check_within(std::time::Duration::from_millis(300))
        .unwrap_err();

    assert!(reason.contains("did not answer"), "{reason}");
}
