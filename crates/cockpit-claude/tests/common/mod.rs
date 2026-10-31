//! A Claude Code adapter on fixed paths, and the pinned hook payloads.
#![allow(dead_code, reason = "each test file uses part of these helpers")]

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
        bridge(),
    )
}

pub fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("claude-code-{CONFORMANCE_VERSION}"))
        .join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}
