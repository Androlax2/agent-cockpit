//! The host paths, platforms and requests the sandbox tests build profiles and launches from.
#![allow(
    dead_code,
    reason = "each test file uses a different part of the fixture"
)]

use std::path::{Path, PathBuf};

use cockpit_sandbox::{
    DesktopSession, HostPaths, LaunchRequest, Platform, ProfileRequest, Secret, SecretHandling,
};

/// The user the fixtures' daemon runs as.
pub const OWNER_UID: u32 = 1000;

/// The fixtures' session directory for session `s1`.
pub const SESSION_DIR: &str = "/run/user/1000/cockpit/sessions/s1";

/// Session `s1`'s temporary folder, its `TMPDIR` (#766).
pub const TEMP_DIR: &str = "/home/owner/.local/state/cockpit/tmp/s1";

/// The daemon's folder for what sandboxes mount at another path.
pub const MOUNT_SOURCES: &str = "/run/user/1000/cockpit/mounts";

/// No host paths beyond the owner's user id.
pub fn no_host_paths() -> HostPaths {
    HostPaths {
        read_only: Vec::new(),
        denied: Vec::new(),
        denied_write: Vec::new(),
        owner_uid: OWNER_UID,
        mount_sources: PathBuf::from(MOUNT_SOURCES),
    }
}

pub fn wsl2() -> Platform {
    Platform::Wsl2 {
        windows_drives: vec![PathBuf::from("/mnt/c"), PathBuf::from("/mnt/d")],
        interop_sockets: PathBuf::from("/run/WSL"),
    }
}

pub fn launch_token() -> Secret {
    Secret {
        variable: "TORLEK_LAUNCH_TOKEN".to_owned(),
        handling: SecretHandling::PassThrough,
    }
}

pub fn profile_request(working_dir: &str) -> ProfileRequest {
    ProfileRequest {
        provider: "claude".to_owned(),
        working_dir: PathBuf::from(working_dir),
        session_dir: PathBuf::from(SESSION_DIR),
        temp_dir: PathBuf::from(TEMP_DIR),
        mount_dir: Path::new(MOUNT_SOURCES).join("s1"),
        owner_uid: OWNER_UID,
        read_only: vec![PathBuf::from("/run/user/1000/cockpit/agent")],
        denied: vec![PathBuf::from("/home/owner/.local/state/cockpit/cockpit.db")],
        denied_write: Vec::new(),
        allow_hosts: Vec::new(),
        environment: vec![("PATH".into(), "/usr/bin".into())],
        secrets: vec![launch_token()],
        desktop: DesktopSession::Withheld,
    }
}

pub fn launch_request(session: &str, working_dir: &str) -> LaunchRequest {
    LaunchRequest {
        session: session.to_owned(),
        provider: "claude".to_owned(),
        working_dir: PathBuf::from(working_dir),
        session_dir: PathBuf::from(format!("/run/user/1000/cockpit/sessions/{session}")),
        temp_dir: PathBuf::from(format!("/home/owner/.local/state/cockpit/tmp/{session}")),
        environment: vec![
            ("PATH".into(), "/usr/bin".into()),
            ("TORLEK_LAUNCH_TOKEN".into(), "token-1".into()),
        ],
        secrets: vec![launch_token()],
        denied: Vec::new(),
        desktop: DesktopSession::Withheld,
    }
}
