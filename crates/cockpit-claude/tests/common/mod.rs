//! A Claude Code adapter on fixed paths, and the pinned hook payloads.
#![allow(dead_code, reason = "each test file uses part of these helpers")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_claude::{Bridge, CONFORMANCE_VERSION, ClaudeCode};

pub fn bridge() -> Bridge {
    Bridge {
        hook_program: PathBuf::from("/opt/cockpit/bin/cockpit-claude-hook"),
        cockpit_program: PathBuf::from("/opt/cockpit/bin/cockpit"),
    }
}

/// The adapter on the version its conformance tests pin.
pub fn claude() -> ClaudeCode {
    ClaudeCode::new(
        PathBuf::from("/usr/bin/claude"),
        Some(CONFORMANCE_VERSION.to_owned()),
        Some(bridge()),
    )
}

pub fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("claude-code-{CONFORMANCE_VERSION}"))
        .join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// An executable `sh` script named `name` in `dir`, running `body`.
pub fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}
