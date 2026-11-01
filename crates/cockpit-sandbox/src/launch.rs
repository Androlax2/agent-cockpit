//! The one entry `cockpit-sessions` calls for every agent launch: the owner's setting, the
//! detected engine and the launch's own facts in; the command to run and its evidence out.

use std::ffi::OsString;
use std::path::PathBuf;

use cockpit_config::{AgentSandbox, DesktopSession};

use crate::{
    AppliedRule, ClientIdentity, Enforcement, Evidence, Greywall, Platform, Profile,
    ProfileRequest, Refusal, Secret,
};

/// What the daemon's own layout asks of every profile.
#[derive(Debug, Clone, Default)]
pub struct HostPaths {
    /// Folders every agent reaches read-only: the agent socket's folder, the bridge programs.
    pub read_only: Vec<PathBuf>,
    /// Cockpit's own files no agent reads: the host-only socket, the state database, the
    /// logs, the owner's configuration.
    pub denied: Vec<PathBuf>,
}

/// The sandbox launches go through: the engine the daemon detected at its start (or why
/// there is none) and its host paths.
#[derive(Debug, Clone)]
pub struct Sandbox {
    platform: Platform,
    engine: Result<Greywall, String>,
    host: HostPaths,
}

/// One launch's facts.
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub session: String,
    pub provider: String,
    /// The checkout the agent works in, whole: read and write.
    pub working_dir: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
    /// Secrets the environment holds, and how each crosses into the sandbox.
    pub secrets: Vec<Secret>,
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
    /// The settings file to write before the agent starts, when sandboxed.
    pub settings: Option<crate::Plan>,
}

impl Sandbox {
    /// `platform` is the host's, detected once by the caller; a ready engine was checked on
    /// it.
    pub fn new(platform: Platform, engine: Result<Greywall, String>, host: HostPaths) -> Sandbox {
        Sandbox {
            platform,
            engine,
            host,
        }
    }

    /// No engine on this host: every sandboxed launch is refused with `reason`.
    pub fn unavailable(reason: impl Into<String>) -> Sandbox {
        Sandbox::new(Platform::detect(), Err(reason.into()), HostPaths::default())
    }

    /// The host's platform, as the daemon detected it.
    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    /// The engine, or why there is none.
    pub fn engine(&self) -> Result<&Greywall, &str> {
        self.engine.as_ref().map_err(String::as_str)
    }

    /// The profile `request` gets on this host, without launching anything (the dry run).
    pub fn profile(&self, request: &LaunchRequest) -> Result<Profile, Refusal> {
        self.build_profile(self.ready_engine()?, request)
    }

    fn ready_engine(&self) -> Result<&Greywall, Refusal> {
        self.engine
            .as_ref()
            .map_err(|reason| Refusal::new("sandbox", format!("no sandbox engine: {reason}")))
    }

    fn build_profile(
        &self,
        engine: &Greywall,
        request: &LaunchRequest,
    ) -> Result<Profile, Refusal> {
        Profile::build(
            ProfileRequest {
                provider: request.provider.clone(),
                working_dir: request.working_dir.clone(),
                read_only: self.host.read_only.clone(),
                denied: self.host.denied.clone(),
                allow_hosts: Vec::new(),
                environment: request.environment.clone(),
                secrets: request.secrets.clone(),
                desktop: request.desktop,
            },
            &engine.platform,
        )
    }

    /// How `program` with `arguments` runs for `request` under the owner's `setting`: inside
    /// the engine, or unchanged only when the owner turned the sandbox off, which the
    /// evidence says (the caller logs it at every launch). A profile this host cannot enforce
    /// refuses the launch.
    pub fn prepare(
        &self,
        setting: AgentSandbox,
        request: &LaunchRequest,
        program: PathBuf,
        arguments: Vec<OsString>,
    ) -> Result<Prepared, Refusal> {
        if setting == AgentSandbox::Off {
            return Ok(Prepared {
                program,
                arguments,
                environment: request.environment.clone(),
                evidence: Evidence {
                    enforcement: Enforcement::Unsandboxed {
                        reason: "the owner set agents.sandbox = \"off\"".to_owned(),
                    },
                    platform: self.platform.name().to_owned(),
                    rules: Vec::new(),
                    observed: Vec::new(),
                },
                settings: None,
            });
        }
        let engine = self.ready_engine()?;
        let profile = self.build_profile(engine, request)?;
        let client = ClientIdentity::for_launch(&request.session).map_err(|error| {
            Refusal::new(
                "network: through greyproxy only",
                format!("no randomness for the proxy client identity: {error}"),
            )
        })?;
        let plan = engine.plan(&profile, &client)?;
        let mut command_arguments = plan.arguments.clone();
        command_arguments.push(program.into_os_string());
        command_arguments.extend(arguments);
        Ok(Prepared {
            program: plan.program.clone(),
            arguments: command_arguments,
            environment: plan.environment.clone(),
            evidence: Evidence {
                enforcement: Enforcement::Sandboxed {
                    backend: "greywall".to_owned(),
                    engine: engine.engine(),
                    client_identity: client.as_str().to_owned(),
                },
                platform: engine.platform.name().to_owned(),
                rules: plan
                    .enforced_by
                    .iter()
                    .map(|(rule, enforced_by)| AppliedRule {
                        rule: rule.clone(),
                        enforced_by: enforced_by.clone(),
                    })
                    .collect(),
                observed: engine.observations(),
            },
            settings: Some(plan),
        })
    }
}
