#![doc = include_str!("../README.md")]

mod evidence;
mod greywall;
mod launch;
mod platform;
mod profile;

pub use cockpit_config::{AgentSandbox, DesktopSession};
pub use evidence::{AppliedRule, Enforcement, Evidence, MIGRATIONS, latest_for_session, record};
pub use greywall::{
    ClientIdentity, EngineBuild, GREYPROXY_PROGRAM, GREYWALL_PROGRAM, Greywall, GreywallPrograms,
    HostCapability, PINNED_GREYPROXY_COMMIT, PINNED_GREYWALL_COMMIT, Plan, parse_greyproxy_version,
    parse_greywall_version,
};
pub use launch::{HostPaths, LaunchRequest, Prepared, Sandbox};
pub use platform::Platform;
pub use profile::{
    BASELINE_DENIED_READ, BASELINE_DENIED_WRITE, DESKTOP_VARIABLES, Profile, ProfileRequest,
    Refusal, Secret, SecretHandling,
};
