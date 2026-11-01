//! `Bridge::check`: the daemon's start check that the plugin and bridge it ships work here,
//! so a stale plugin or an unusable bridge shows at once, not as missing events.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_opencode::{Bridge, PLUGIN_SOURCE};

/// A bridge whose program is `bridge`, beside this release's plugin and a `cockpit` that is
/// an executable file.
fn bridge_with(dir: &Path, bridge: PathBuf) -> Bridge {
    let cockpit = dir.join("cockpit");
    std::fs::write(&cockpit, "").unwrap();
    std::fs::set_permissions(&cockpit, std::fs::Permissions::from_mode(0o755)).unwrap();
    let plugin = dir.join("cockpit-opencode-plugin.js");
    std::fs::write(&plugin, PLUGIN_SOURCE).unwrap();
    Bridge {
        plugin,
        bridge_program: bridge,
        cockpit_program: cockpit,
    }
}

fn built_bridge() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cockpit-opencode-bridge"))
}

#[test]
fn the_built_bridge_with_this_releases_plugin_passes() {
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(bridge_with(dir.path(), built_bridge()).check(), Ok(()));
}

#[test]
fn a_plugin_from_another_release_fails_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = bridge_with(dir.path(), built_bridge());
    std::fs::write(&bridge.plugin, "export const Old = async () => ({})\n").unwrap();

    let reason = bridge.check().unwrap_err();

    assert!(
        reason.contains(&bridge.plugin.display().to_string())
            && reason.contains("not this release's plugin"),
        "{reason}"
    );
}

#[test]
fn a_missing_plugin_fails_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = bridge_with(dir.path(), built_bridge());
    std::fs::remove_file(&bridge.plugin).unwrap();

    let reason = bridge.check().unwrap_err();

    assert!(
        reason.contains("cockpit-opencode-plugin.js missing"),
        "{reason}"
    );
}

#[test]
fn a_bridge_without_its_execute_bit_fails_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("cockpit-opencode-bridge");
    std::fs::copy(built_bridge(), &program).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o644)).unwrap();

    let reason = bridge_with(dir.path(), program.clone())
        .check()
        .unwrap_err();

    assert!(
        reason.contains(&program.display().to_string()) && reason.contains("cannot run it"),
        "{reason}"
    );
}

#[test]
fn another_program_in_the_bridges_place_fails() {
    let dir = tempfile::tempdir().unwrap();
    let impostor = dir.path().join("impostor");
    std::fs::write(&impostor, "#!/bin/sh\necho 'something else 1.0'\n").unwrap();
    std::fs::set_permissions(&impostor, std::fs::Permissions::from_mode(0o755)).unwrap();

    let reason = bridge_with(dir.path(), impostor).check().unwrap_err();

    assert!(reason.contains("not this release's"), "{reason}");
}

#[test]
fn a_cockpit_without_its_execute_bit_fails() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = bridge_with(dir.path(), built_bridge());
    std::fs::set_permissions(
        &bridge.cockpit_program,
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();

    let reason = bridge.check().unwrap_err();

    assert!(reason.contains("has no execute bit"), "{reason}");
}

#[test]
fn a_relative_plugin_path_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut bridge = bridge_with(dir.path(), built_bridge());
    let cwd = std::env::current_dir().unwrap();
    let relative = pathdiff(&bridge.plugin, &cwd);
    bridge.plugin = relative;

    let reason = bridge.check().unwrap_err();

    assert!(reason.contains("not an absolute path"), "{reason}");
}

/// `path` relative to `base`, both absolute.
fn pathdiff(path: &Path, base: &Path) -> PathBuf {
    let ups = base.components().count() - 1;
    let mut relative: PathBuf = std::iter::repeat_n("..", ups).collect();
    relative.push(path.strip_prefix("/").unwrap());
    relative
}
