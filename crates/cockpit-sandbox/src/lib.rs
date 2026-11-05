#![doc = include_str!("../README.md")]

mod evidence;
mod launch;
mod native;
#[cfg(feature = "persistence")]
mod observations;
mod platform;
mod profile;
mod spec;

pub use cockpit_config::{AgentSandbox, DesktopSession};
pub use evidence::{AppliedRule, Enforcement, Evidence};
#[cfg(feature = "persistence")]
pub use evidence::{MIGRATIONS, latest_for_session, record};
pub use launch::{HostPaths, LaunchRequest, Prepared, Sandbox};
pub use native::{
    Backend, GitIdentity, NativeBackend, ProviderNeeds, Staged, ToolNeeds, ToolSetting,
};
#[cfg(feature = "persistence")]
pub use observations::{ObservationError, record_observations};
pub use platform::Platform;
pub use profile::{
    BASELINE_DENIED_READ, BASELINE_DENIED_WRITE, CODEX_SOCKET_FOLDER_NAME, DESKTOP_VARIABLES,
    DesktopEndpoint, EndpointKind, FolderMount, Profile, ProfileRequest, ProviderReach, Refusal,
    Secret, SecretHandling,
};
pub use spec::{
    Folder, Forward, InvalidSpec, LANDLOCK_ABI_FLOOR, LaunchSpec, NeedsSource, Protected,
    SPEC_FILE, SPEC_VERSION, SYSTEM_READ_ONLY, Subject, WhenAbsent,
};
