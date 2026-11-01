//! A Codex adapter on fixed paths, and the pinned hook payloads.
#![allow(dead_code, reason = "each test file uses part of these helpers")]

pub mod app_server;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use std::os::unix::net::UnixStream;

use cockpit_codex::{Bridge, CONFORMANCE_VERSION, Codex};
use cockpit_provider_api::{DeliveryTarget, ProviderError, SessionSockets};

pub fn bridge() -> Bridge {
    Bridge {
        session_program: PathBuf::from("/opt/cockpit/bin/cockpit-codex-session"),
        hook_program: PathBuf::from("/opt/cockpit/bin/cockpit-codex-hook"),
        cockpit_program: PathBuf::from("/opt/cockpit/bin/cockpit"),
    }
}

/// The adapter on the version its conformance tests pin.
pub fn codex() -> Codex {
    Codex::new(
        PathBuf::from("/usr/bin/codex"),
        Some(CONFORMANCE_VERSION.to_owned()),
        Some(bridge()),
    )
}

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("codex-{CONFORMANCE_VERSION}"))
}

pub fn fixture(name: &str) -> String {
    let path = fixtures_dir().join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// An executable `sh` script named `name` in `dir`, running `body`.
pub fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The words of `arguments`, which are all UTF-8 in these tests.
pub fn words(arguments: &[std::ffi::OsString]) -> Vec<String> {
    arguments
        .iter()
        .map(|argument| argument.to_str().unwrap().to_owned())
        .collect()
}

/// Connects to whatever listens at the path: the daemon's peer check is the daemon's to
/// test (#442); these tests drive the adapter's protocol.
pub struct UncheckedSockets;

impl SessionSockets for UncheckedSockets {
    fn connect(&self, socket: &Path) -> Result<UnixStream, ProviderError> {
        UnixStream::connect(socket).map_err(|error| ProviderError {
            reason: format!("cannot reach {}: {error}", socket.display()),
        })
    }
}

/// Refuses every socket, the way the daemon's check refuses one Cockpit did not launch.
pub struct RefusingSockets;

impl SessionSockets for RefusingSockets {
    fn connect(&self, socket: &Path) -> Result<UnixStream, ProviderError> {
        Err(ProviderError {
            reason: format!("{} is not the session's app-server", socket.display()),
        })
    }
}

pub fn target(session_dir: &Path) -> DeliveryTarget<'_> {
    DeliveryTarget {
        session_dir,
        sockets: &UncheckedSockets,
    }
}
