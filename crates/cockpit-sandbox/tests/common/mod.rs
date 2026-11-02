//! A checked engine and a launch request, without running greywall: the versions are the
//! pinned builds as `--version` reports them on a host with every capability.
#![allow(
    dead_code,
    reason = "each test file uses a different part of the fixture"
)]

use std::path::{Path, PathBuf};

use cockpit_sandbox::{
    DesktopSession, EngineBuild, Greywall, GreywallPrograms, HostCapability, HostPaths,
    LaunchRequest, PINNED_GREYPROXY_COMMIT, PINNED_GREYWALL_COMMIT, Platform, ProfileRequest,
    Secret, SecretHandling,
};

pub const LINUX_CAPABILITIES: [&str; 5] = [
    "bubblewrap",
    "landlock",
    "seccomp",
    "network isolation",
    "D-Bus isolated",
];

/// The user the fixtures' daemon runs as.
pub const OWNER_UID: u32 = 1000;

/// The fixtures' session directory for session `s1`.
pub const SESSION_DIR: &str = "/run/user/1000/cockpit/sessions/s1";

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

pub fn capabilities(available: &[&str], missing: &[&str]) -> Vec<HostCapability> {
    let capability = |name: &&str, available: bool| HostCapability {
        name: (*name).to_owned(),
        available,
        detail: None,
    };
    available
        .iter()
        .map(|name| capability(name, true))
        .chain(missing.iter().map(|name| capability(name, false)))
        .collect()
}

pub fn pinned_greywall() -> EngineBuild {
    EngineBuild {
        version: "0.3.7-cockpit.1".to_owned(),
        commit: PINNED_GREYWALL_COMMIT.to_owned(),
    }
}

pub fn pinned_greyproxy() -> EngineBuild {
    EngineBuild {
        version: "0.4.5".to_owned(),
        commit: PINNED_GREYPROXY_COMMIT.to_owned(),
    }
}

pub fn programs() -> GreywallPrograms {
    GreywallPrograms {
        greywall: PathBuf::from("/opt/sandbox/greywall"),
        greyproxy: PathBuf::from("/opt/sandbox/greyproxy"),
    }
}

/// The pinned engine on `platform` with `capabilities`, writing settings under `settings_dir`.
pub fn engine_with(
    platform: Platform,
    capabilities: Vec<HostCapability>,
    settings_dir: &Path,
) -> Greywall {
    Greywall::from_builds(
        programs(),
        pinned_greywall(),
        pinned_greyproxy(),
        capabilities,
        platform,
        settings_dir.to_owned(),
    )
    .unwrap()
}

pub fn linux_engine(settings_dir: &Path) -> Greywall {
    engine_with(
        Platform::Linux,
        capabilities(&LINUX_CAPABILITIES, &[]),
        settings_dir,
    )
}

pub fn wsl2() -> Platform {
    Platform::Wsl2 {
        windows_drives: vec![PathBuf::from("/mnt/c"), PathBuf::from("/mnt/d")],
        interop_sockets: PathBuf::from("/run/WSL"),
    }
}

pub fn launch_token() -> Secret {
    Secret {
        variable: "COCKPIT_LAUNCH_TOKEN".to_owned(),
        handling: SecretHandling::PassThrough,
    }
}

pub fn profile_request(working_dir: &str) -> ProfileRequest {
    ProfileRequest {
        provider: "claude".to_owned(),
        working_dir: PathBuf::from(working_dir),
        session_dir: PathBuf::from(SESSION_DIR),
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
        environment: vec![
            ("PATH".into(), "/usr/bin".into()),
            ("COCKPIT_LAUNCH_TOKEN".into(), "token-1".into()),
        ],
        secrets: vec![launch_token()],
        desktop: DesktopSession::Withheld,
    }
}
