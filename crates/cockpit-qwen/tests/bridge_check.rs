//! The daemon's start check that the bridge's files are this release's and run here.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use cockpit_qwen::{Bridge, defaults_settings};

/// A bridge in `dir`: the real hook program copied there, a `torlek`, and its defaults file.
fn installed_bridge(dir: &Path) -> Bridge {
    let hook_program = dir.join("torlek-qwen-hook");
    std::fs::copy(env!("CARGO_BIN_EXE_torlek-qwen-hook"), &hook_program).unwrap();
    let cockpit_program = common::script(dir, "torlek", "exit 0");
    let defaults_file = dir.join("system-defaults.json");
    std::fs::write(
        &defaults_file,
        defaults_settings(&hook_program, &cockpit_program),
    )
    .unwrap();
    Bridge {
        defaults_file,
        hook_program,
        cockpit_program,
    }
}

#[test]
fn an_installed_bridge_passes() {
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(installed_bridge(dir.path()).check(), Ok(()));
}

#[test]
fn a_missing_file_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = installed_bridge(dir.path());
    std::fs::remove_file(&bridge.defaults_file).unwrap();

    let reason = bridge.check().unwrap_err();
    assert!(reason.contains("system-defaults.json missing"), "{reason}");
}

#[test]
fn a_defaults_file_holding_other_hooks_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = installed_bridge(dir.path());
    std::fs::write(
        &bridge.defaults_file,
        defaults_settings(Path::new("/elsewhere/hook"), &bridge.cockpit_program),
    )
    .unwrap();

    let reason = bridge.check().unwrap_err();
    assert!(
        reason.contains("does not hold this bridge's hooks"),
        "{reason}"
    );
}

#[test]
fn a_relative_defaults_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = installed_bridge(dir.path());
    let relative = Bridge {
        defaults_file: Path::new("system-defaults.json").to_path_buf(),
        ..bridge
    };
    std::env::set_current_dir(dir.path()).unwrap();

    let reason = relative.check().unwrap_err();
    assert!(reason.contains("not an absolute path"), "{reason}");
}

#[test]
fn a_torlek_without_its_execute_bit_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = installed_bridge(dir.path());
    std::fs::set_permissions(
        &bridge.cockpit_program,
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();

    let reason = bridge.check().unwrap_err();
    assert!(reason.contains("no execute bit"), "{reason}");
}

#[test]
fn a_hook_of_another_release_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = installed_bridge(dir.path());
    std::fs::remove_file(&bridge.hook_program).unwrap();
    common::script(
        dir.path(),
        "torlek-qwen-hook",
        "echo 'torlek-qwen-hook 0.0.1'",
    );

    let reason = bridge.check().unwrap_err();
    assert!(reason.contains("not this release's"), "{reason}");
}

#[test]
fn a_hook_that_does_not_answer_in_time_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = installed_bridge(dir.path());
    std::fs::remove_file(&bridge.hook_program).unwrap();
    common::script(dir.path(), "torlek-qwen-hook", "sleep 5");

    let reason = bridge.check_within(Duration::from_millis(200)).unwrap_err();
    assert!(reason.contains("did not answer"), "{reason}");
}

#[test]
fn a_program_path_that_is_not_utf8_is_refused() {
    use std::os::unix::ffi::OsStrExt;

    let dir = tempfile::tempdir().unwrap();
    let named = dir.path().join(std::ffi::OsStr::from_bytes(b"caf\xe9"));
    std::fs::create_dir(&named).unwrap();
    let mut bridge = installed_bridge(&named);
    bridge.defaults_file = dir.path().join("system-defaults.json");
    std::fs::write(
        &bridge.defaults_file,
        defaults_settings(&bridge.hook_program, &bridge.cockpit_program),
    )
    .unwrap();

    let reason = bridge.check().unwrap_err();
    assert!(reason.contains("is not UTF-8"), "{reason}");
}
