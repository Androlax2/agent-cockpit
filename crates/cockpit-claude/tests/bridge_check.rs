//! `Bridge::check` (#203): the daemon's start check that the hook bridge it ships runs here,
//! so a bundle whose hook is present but unusable shows at once, not as missing events.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_claude::Bridge;

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
        PathBuf::from(env!("CARGO_BIN_EXE_cockpit-claude-hook")),
    );

    assert_eq!(bridge.check(), Ok(()));
}

#[test]
fn the_built_hook_bridge_answers_its_version() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit-claude-hook"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        cockpit_claude::hook_version_line()
    );
}

#[test]
fn a_hook_without_its_execute_bit_fails_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let hook = dir.path().join("cockpit-claude-hook");
    std::fs::copy(env!("CARGO_BIN_EXE_cockpit-claude-hook"), &hook).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o644)).unwrap();

    let refused = bridge_with(dir.path(), hook.clone()).check();

    let reason = refused.unwrap_err();
    assert!(
        reason.contains(&hook.display().to_string()) && reason.contains("cannot run it"),
        "{reason}"
    );
}

#[test]
fn a_hook_this_machine_cannot_execute_fails() {
    let dir = tempfile::tempdir().unwrap();
    let hook = dir.path().join("cockpit-claude-hook");
    // An ELF header for another machine, as a bundle of the wrong architecture holds.
    let mut foreign = b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0\x02\0\x99\x99".to_vec();
    foreign.resize(64, 0);
    std::fs::write(&hook, foreign).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    let refused = bridge_with(dir.path(), hook).check();

    assert!(refused.is_err(), "{refused:?}");
}

#[test]
fn a_program_that_is_not_a_hook_bridge_fails() {
    let dir = tempfile::tempdir().unwrap();
    let hook = common::script(
        dir.path(),
        "cockpit-claude-hook",
        "echo 'some other tool 1.0'",
    );

    let reason = bridge_with(dir.path(), hook).check().unwrap_err();

    assert!(reason.contains("not a hook bridge"), "{reason}");
}

#[test]
fn a_hook_bridge_of_another_release_fails() {
    let dir = tempfile::tempdir().unwrap();
    let hook = common::script(
        dir.path(),
        "cockpit-claude-hook",
        "echo 'cockpit-claude-hook 0.0.0-stale'",
    );

    let reason = bridge_with(dir.path(), hook).check().unwrap_err();

    assert!(reason.contains("not this release's"), "{reason}");
}

#[test]
fn a_cockpit_program_without_its_execute_bit_fails_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = bridge_with(
        dir.path(),
        PathBuf::from(env!("CARGO_BIN_EXE_cockpit-claude-hook")),
    );
    std::fs::set_permissions(
        &bridge.cockpit_program,
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();

    let reason = bridge.check().unwrap_err();

    assert!(
        reason.contains(&bridge.cockpit_program.display().to_string())
            && reason.contains("no execute bit"),
        "{reason}"
    );
}

#[test]
fn a_missing_cockpit_program_fails_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Bridge {
        hook_program: PathBuf::from(env!("CARGO_BIN_EXE_cockpit-claude-hook")),
        cockpit_program: dir.path().join("cockpit"),
    };

    let reason = bridge.check().unwrap_err();

    assert!(reason.contains("cockpit missing"), "{reason}");
}
