#![doc = include_str!("../README.md")]

mod credentials;
mod egress;
mod evidence;
mod forwards;
mod launch;
#[cfg(feature = "persistence")]
mod logins;
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
    REQUESTS_FILE, Refusal as EgressRefusal, TLS_PORT, Target, card_allowlist, card_entry,
    destination, is_askable_destination, non_public_class, normalized_name,
};
pub use evidence::{AppliedRule, Enforcement, Evidence};
#[cfg(feature = "persistence")]
pub use evidence::{MIGRATIONS, first_launch_was_sandboxed, latest_for_session, record};
pub use forwards::{
    AllowedService, AskedService, CONTAINERS_SERVICE, DecidedService, Endpoint, EngineRelay,
    Forward, MAX_FORWARDS, ServiceDecisions, is_service_name, proxy_port,
};
pub use launch::{
    AccountLogin, CheckRequest, HostPaths, LaunchRequest, LoginMount, Prepared, Sandbox,
    SessionLogin,
};
#[cfg(feature = "persistence")]
pub use logins::{
    LoginGrant, LoginGrantError, LoginGrantRefusal, grant_login, login_grants, revoke_login_grant,
    revoke_project_login_grants,
};
pub use native::{
    Backend, CheckoutMasks, GitIdentity, NativeBackend, ProviderNeeds, Staged, ToolNeeds,
    ToolSetting, checkout_masks,
};
#[cfg(feature = "persistence")]
pub use network::{
    GITHUB_HOST, NETWORK_ALLOW_GRACE_SECONDS, NetworkError, NetworkGrant, NetworkRefusal,
    NetworkRequest, NetworkRequestState, NetworkTarget, OwnerClient, OwnerDecision, ServiceAsk,
    allow_network_request, ask_service, asking_network_requests, decided_network_requests,
    deny_network_request, egress_decisions, file_network_request, file_service_request,
    grant_github, network_grants, network_request, revoke_network_grant, standing_check_request,
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
    ENGINE_SOCKET_INSIDE, EngineSocket, Folder, InvalidSpec, LANDLOCK_ABI_FLOOR, LaunchSpec,
    NeedsSource, Protected, SPEC_FILE, SPEC_VERSION, SYSTEM_READ_ONLY, Subject, WhenAbsent,
    engine_folder_inside,
};
