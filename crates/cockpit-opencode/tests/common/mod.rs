//! An OpenCode adapter on fixed paths, the pinned events, and small process helpers.
#![allow(dead_code, reason = "each test file uses part of these helpers")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_opencode::{Bridge, CONFORMANCE_VERSION, OpenCode};

pub fn bridge() -> Bridge {
    Bridge {
        plugin: PathBuf::from("/opt/cockpit/share/cockpit-opencode-plugin.js"),
        bridge_program: PathBuf::from("/opt/cockpit/bin/cockpit-opencode-bridge"),
        cockpit_program: PathBuf::from("/opt/cockpit/bin/cockpit"),
    }
}

/// The adapter on the version its conformance tests pin.
pub fn opencode() -> OpenCode {
    OpenCode::new(
        PathBuf::from("/usr/bin/opencode"),
        Some(CONFORMANCE_VERSION.to_owned()),
        Some(bridge()),
    )
}

/// The installed OpenCode the ignored live tests run.
pub fn installed() -> PathBuf {
    PathBuf::from(
        std::env::var_os("COCKPIT_OPENCODE_PROGRAM")
            .expect("COCKPIT_OPENCODE_PROGRAM names the OpenCode binary to test"),
    )
}

pub fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("opencode-{CONFORMANCE_VERSION}"))
        .join(format!("{name}.json"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .trim_end()
        .to_owned()
}

/// An executable `sh` script named `name` in `dir`, running `body`.
pub fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The value of variable `name` in a launch's environment.
pub fn variable<'launch>(
    launch: &'launch cockpit_provider_api::LaunchCommand,
    name: &str,
) -> Option<&'launch str> {
    launch
        .environment
        .iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| value.to_str())
}
