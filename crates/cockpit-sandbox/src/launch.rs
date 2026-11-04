//! The one entry `cockpit-sessions` calls for every agent launch: the owner's setting, the
//! detected engine and the launch's own facts in; the command to run and its evidence out.

use std::ffi::OsString;
use std::path::PathBuf;

use cockpit_config::{AgentSandbox, DesktopSession};

use crate::{
    AppliedRule, ClientIdentity, Enforcement, Evidence, FilterWriteFailure, Greywall, ManagedProxy,
    Platform, Profile, ProfileRequest, ProxyError, Refusal, Secret,
};

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
    /// source between Cockpit's check and the engine's mount (#446).
    pub mount_sources: PathBuf,
}

/// The sandbox launches go through: the engine the daemon detected at its start (or why
/// there is none) and its host paths.
#[derive(Debug, Clone)]
pub struct Sandbox {
    platform: Platform,
    engine: Result<Greywall, String>,
    /// None only for [`Sandbox::unavailable`], whose caller never read the host.
    host: Option<HostPaths>,
    proxy: Result<ManagedProxy, String>,
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
            host: Some(host),
            proxy: Err(ProxyError::NotConfigured.to_string()),
        }
    }

    /// No engine on this host: every sandboxed launch is refused with `reason`.
    pub fn unavailable(reason: impl Into<String>) -> Sandbox {
        Sandbox {
            platform: Platform::detect(),
            engine: Err(reason.into()),
            host: None,
            proxy: Err(ProxyError::NotConfigured.to_string()),
        }
    }

    /// The host's platform, as the daemon detected it.
    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    /// The engine, or why there is none.
    pub fn engine(&self) -> Result<&Greywall, &str> {
        self.engine.as_ref().map_err(String::as_str)
    }

    /// Attach this profile's lifecycle owner and protect its state even when the checkout
    /// is an ancestor of the profile directory. The agent must not read the API token or
    /// replace configuration that a later owner restart would execute.
    pub fn with_managed_proxy(mut self, proxy: Result<ManagedProxy, ProxyError>) -> Self {
        if let (Ok(proxy), Some(host)) = (&proxy, &mut self.host) {
            for path in [proxy.data_dir(), proxy.unit_path()] {
                host.denied.push(path.to_owned());
                host.denied_write.push(path.to_owned());
            }
            // A writable unit directory also permits drop-ins or replacement launch agents.
            if let Some(directory) = proxy.unit_path().parent() {
                host.denied_write.push(directory.to_owned());
            }
        }
        self.proxy = proxy.map_err(|error| error.to_string());
        self
    }

    pub fn proxy(&self) -> Result<&ManagedProxy, &str> {
        self.proxy.as_ref().map_err(String::as_str)
    }

    /// Bounded live health, called on a blocking worker before reserving a launch.
    pub fn require_proxy(&self) -> Result<(), Refusal> {
        let proxy = self
            .proxy()
            .map_err(|reason| Refusal::new("sandbox proxy", reason))?;
        proxy
            .launch_health(self.engine().ok())
            .map(|_| ())
            .map_err(|error| proxy.refusal(error))
    }

    /// Whether the engine can write the seccomp filter a launch with `environment` loads
    /// (#628). Passes without an engine, whose launches are refused anyway, and on macOS,
    /// where greywall loads no seccomp filter.
    pub fn check_seccomp_filter(
        &self,
        environment: &[(OsString, OsString)],
    ) -> Result<(), FilterWriteFailure> {
        match &self.engine {
            Ok(engine) if matches!(engine.platform, Platform::Linux | Platform::Wsl2 { .. }) => {
                crate::seccomp::check_filter_write(&crate::seccomp::seccomp_filter_dir(environment))
            }
            _ => Ok(()),
        }
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
                denied: host.denied.clone(),
                denied_write: host.denied_write.clone(),
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
        let mut plan = engine.plan(&profile, &client)?;
        if let Ok(proxy) = self.proxy() {
            plan.environment
                .retain(|(key, _)| key != "GREYPROXY_DATA_HOME");
            plan.environment
                .push(("GREYPROXY_DATA_HOME".into(), proxy.data_dir().into()));
            // The pinned management API uses localhost; traffic is explicitly IPv4 so
            // another listener on ::1 cannot receive this launch's proxy credentials.
            let at = plan.arguments.len() - 1; // Greywall's terminating `--`.
            plan.arguments.splice(
                at..at,
                [
                    "--proxy",
                    "socks5://127.0.0.1:43052",
                    "--http-proxy",
                    "http://127.0.0.1:43051",
                    "--dns",
                    "127.0.0.1:43053",
                ]
                .map(OsString::from),
            );
        }
        self.check_seccomp_filter(&plan.environment)
            .map_err(|failure| Refusal::new("seccomp filter", failure.to_string()))?;
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
