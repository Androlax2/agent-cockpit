//! A Qwen Code adapter on fixed paths, the pinned hook payloads, and fake programs.
#![allow(dead_code, reason = "each test file uses part of these helpers")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_qwen::{Bridge, CONFORMANCE_VERSION, QwenCode};

pub fn bridge() -> Bridge {
    Bridge {
        defaults_file: PathBuf::from("/var/lib/torlek/qwen/system-defaults.json"),
        hook_program: PathBuf::from("/opt/torlek/bin/torlek-qwen-hook"),
        cockpit_program: PathBuf::from("/opt/torlek/bin/torlek"),
    }
}

/// The adapter on the version its fixtures come from.
pub fn qwen() -> QwenCode {
    QwenCode::new(
        PathBuf::from("/usr/bin/qwen"),
        Some(CONFORMANCE_VERSION.to_owned()),
        Some(bridge()),
    )
}

/// The installed Qwen Code the ignored live tests run.
pub fn installed() -> PathBuf {
    PathBuf::from(
        std::env::var_os("COCKPIT_QWEN_PROGRAM")
            .expect("COCKPIT_QWEN_PROGRAM names the Qwen Code program to test"),
    )
}

/// A pinned hook payload, `name` without its extension.
pub fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("qwen-code-{CONFORMANCE_VERSION}"))
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

/// The session id every pinned payload names.
pub const FIXTURE_SESSION: &str = "5b0e8f3a-2c4d-4e6f-8a1b-9c0d1e2f3a4b";
