#![doc = include_str!("../README.md")]

mod credentials;
mod egress;
mod evidence;
mod launch;
mod native;
#[cfg(feature = "persistence")]
mod network;
#[cfg(feature = "persistence")]
mod observations;
mod platform;
mod profile;
mod spec;

pub use cockpit_config::{AgentSandbox, DesktopSession};
pub use credentials::{
    BUNDLE_VARIABLES, BoundCredential, CA_BUNDLE_FILE, CREDENTIALS_FILE, CredentialHost,
    CredentialScheme, CredentialValue, EXTRA_CA_VARIABLE, Interception, PLACEHOLDER,
    SESSION_CA_FILE, SYSTEM_BUNDLES, SpecCredential,
};
pub use egress::{
    AllowEntry, AllowSource, Allowed, Allowlist, AskedDestination, AskingDestination,
    Counters as EgressCounters, DECISIONS_FILE, DeniedDestination, EGRESS_FILE, EgressDecisions,
    EgressReport, EgressRequests, MAX_ASKED_DESTINATIONS, MAX_REPORTED_DESTINATIONS, PROXY_ADDRESS,
    REQUESTS_FILE, Refusal as EgressRefusal, TLS_PORT, Target, destination, is_askable_destination,
    non_public_class, normalized_name,
};
pub use evidence::{AppliedRule, Enforcement, Evidence};
#[cfg(feature = "persistence")]
pub use evidence::{MIGRATIONS, first_launch_was_sandboxed, latest_for_session, record};
pub use launch::{AccountLogin, CheckRequest, HostPaths, LaunchRequest, Prepared, Sandbox};
pub use native::{
    Backend, GitIdentity, NativeBackend, ProviderNeeds, Staged, ToolNeeds, ToolSetting,
};
#[cfg(feature = "persistence")]
pub use network::{
    NETWORK_ALLOW_GRACE_SECONDS, NetworkError, NetworkGrant, NetworkRefusal, NetworkRequest,
    NetworkRequestState, OwnerClient, OwnerDecision, allow_network_request,
    asking_network_requests, deny_network_request, egress_decisions, file_network_request,
    network_grants, network_request, revoke_network_grant,
};
#[cfg(feature = "persistence")]
pub use observations::{ObservationError, record_observations};
pub use platform::Platform;
pub use profile::{
    BASELINE_DENIED_READ, BASELINE_DENIED_WRITE, CODEX_SOCKET_FOLDER_NAME, DESKTOP_VARIABLES,
    DesktopEndpoint, EndpointKind, FolderMount, Profile, ProfileRequest, ProviderReach, Refusal,
    Secret, SecretHandling, shared_desktop_refusal,
};
pub use spec::{
    Folder, Forward, InvalidSpec, LANDLOCK_ABI_FLOOR, LaunchSpec, NeedsSource, Protected,
    SPEC_FILE, SPEC_VERSION, SYSTEM_READ_ONLY, Subject, WhenAbsent,
};
