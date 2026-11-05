//! The one entry `cockpit-sessions` calls for every agent launch: the owner's setting and
//! the launch's own facts in; the command to run and its evidence out. No backend enforces a
//! profile in this release (#828): a launch runs unsandboxed under `agents.sandbox = "off"`
//! and is refused under `"required"`, never run unsandboxed.

use std::ffi::OsString;
use std::path::PathBuf;

use cockpit_config::{AgentSandbox, DesktopSession};

use crate::{Enforcement, Evidence, Platform, Profile, ProfileRequest, Refusal, Secret};

/// What the daemon's own layout asks of every profile.
#[derive(Debug, Clone)]
pub struct HostPaths {
    /// Folders every agent reaches read-only: the agent socket's folder, the bridge programs.
    pub read_only: Vec<PathBuf>,
    /// Cockpit's own files no agent reads: the host-only socket, the state database, the
    /// logs, the owner's configuration.
    pub denied: Vec<PathBuf>,
    /// Files an agent may read but never replace, even through a writable ancestor.
    pub denied_write: Vec<PathBuf>,
    /// The user the daemon runs as, and its agents with it.
    pub owner_uid: u32,
    /// The daemon's folder for what sandboxes mount at another path, one folder per session
    /// inside: no agent is granted it or any folder above it, so no agent can swap a mount
    /// source between Cockpit's check and the backend's mount (#446).
    pub mount_sources: PathBuf,
}

/// The sandbox launches go through: the host's platform and the daemon's host paths.
#[derive(Debug, Clone)]
pub struct Sandbox {
    platform: Platform,
    /// None only for [`Sandbox::unavailable`], whose caller never read the host.
    host: Option<HostPaths>,
}

/// One launch's facts.
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub session: String,
    pub provider: String,
    /// The checkout the agent works in, whole: read and write.
    pub working_dir: PathBuf,
    /// The session's private directory, where its provider keeps what reaches the running
    /// agent: read and write.
    pub session_dir: PathBuf,
    /// The session's own temporary folder, which `TMPDIR` names: read and write.
    pub temp_dir: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
    /// Secrets the environment holds, and how each crosses into the sandbox.
    pub secrets: Vec<Secret>,
    /// What this launch alone may not read, beside the host's denials: the owner's `gh`
    /// login once the launch's project has a token of its own (#754).
    pub denied: Vec<PathBuf>,
    pub desktop: DesktopSession,
}

/// A launch ready to run: the program and arguments that start the agent and the evidence
/// to record with the launch.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
    pub evidence: Evidence,
}

impl Sandbox {
    /// `platform` is the host's, detected once by the caller.
    pub fn new(platform: Platform, host: HostPaths) -> Sandbox {
        Sandbox {
            platform,
            host: Some(host),
        }
    }

    /// A sandbox whose caller read none of the host's paths: profiles are refused.
    pub fn unavailable() -> Sandbox {
        Sandbox {
            platform: Platform::detect(),
            host: None,
        }
    }

    /// The host's platform, as the daemon detected it.
    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    /// The profile `request` gets on this host: what a backend would enforce.
    pub fn profile(&self, request: &LaunchRequest) -> Result<Profile, Refusal> {
        let host = self
            .host
            .as_ref()
            .ok_or_else(|| Refusal::new("sandbox", "the daemon read none of this host's paths"))?;
        let mount_dir = session_folder(&host.mount_sources, &request.session)?;
        Profile::build(
            ProfileRequest {
                provider: request.provider.clone(),
                working_dir: request.working_dir.clone(),
                session_dir: request.session_dir.clone(),
                temp_dir: request.temp_dir.clone(),
                mount_dir,
                owner_uid: host.owner_uid,
                read_only: host.read_only.clone(),
                denied: host.denied.iter().chain(&request.denied).cloned().collect(),
                denied_write: host.denied_write.clone(),
                allow_hosts: Vec::new(),
                environment: request.environment.clone(),
                secrets: request.secrets.clone(),
                desktop: request.desktop,
            },
            &self.platform,
        )
    }

    /// How `program` with `arguments` runs for `request` under `setting`: unchanged under
    /// `agents.sandbox = "off"`, which the evidence says (the caller logs it at every
    /// launch); refused under `"required"`, naming the missing backend.
    pub fn prepare(
        &self,
        setting: AgentSandbox,
        request: &LaunchRequest,
        program: PathBuf,
        arguments: Vec<OsString>,
    ) -> Result<Prepared, Refusal> {
        if setting == AgentSandbox::Required {
            return Err(Refusal::new(
                "agents.sandbox = \"required\"",
                format!(
                    "{}, so the launch is refused rather than run unsandboxed; \
                     agents.sandbox = \"off\" in the owner's file runs agents unsandboxed",
                    cockpit_config::NO_SANDBOX_BACKEND
                ),
            ));
        }
        Ok(Prepared {
            program,
            arguments,
            environment: request.environment.clone(),
            evidence: Evidence {
                enforcement: Enforcement::Unsandboxed {
                    reason: "no sandbox: agents.sandbox = \"off\"".to_owned(),
                },
                platform: self.platform.name().to_owned(),
                rules: Vec::new(),
                observed: Vec::new(),
            },
        })
    }
}

/// `session`'s own folder in `parent`: the id is one plain name, never a path.
fn session_folder(parent: &std::path::Path, session: &str) -> Result<PathBuf, Refusal> {
    let mut components = std::path::Path::new(session).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(name)), None) => Ok(parent.join(name)),
        _ => Err(Refusal::new(
            format!("session {session}"),
            "a session id must be one plain name to name its folders",
        )),
    }
}
