//! The one entry `cockpit-sessions` calls for every agent launch: the owner's setting and
//! the launch's own facts in; the command to run and its evidence out. A launch runs
//! unsandboxed under `agents.sandbox = "off"`; under `"required"` it runs through the native
//! backend the daemon probed (#829), or is refused naming why there is none, never run
//! unsandboxed.

use std::ffi::OsString;
use std::path::PathBuf;

use cockpit_config::{AgentSandbox, DesktopSession};

use crate::{
    Backend, Enforcement, Evidence, NativeBackend, Platform, Profile, ProfileRequest,
    ProviderNeeds, ProviderReach, Refusal, Secret, Staged, Subject, ToolNeeds,
};

/// What the daemon's own layout asks of every profile.
#[derive(Debug, Clone)]
pub struct HostPaths {
    /// Folders every agent reaches read-only: the bridge programs, the instruction bundle.
    pub read_only: Vec<PathBuf>,
    /// Folders whose sockets every agent connects to, read-only otherwise: the agent
    /// socket's folder.
    pub sockets: Vec<PathBuf>,
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
    /// The daemon's folder of sandboxed sessions' own homes, one per session inside: kept
    /// across a session's relaunches, removed at its end, never granted to any agent above
    /// its own (#858, #859).
    pub homes: PathBuf,
}

/// The sandbox launches go through: the host's platform and the daemon's host paths.
#[derive(Debug, Clone)]
pub struct Sandbox {
    platform: Platform,
    /// None only for [`Sandbox::unavailable`], whose caller never read the host.
    host: Option<HostPaths>,
    backend: Backend,
}

/// One launch's facts.
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub session: String,
    pub provider: String,
    /// The checkout the agent works in, whole: read and write.
    pub working_dir: PathBuf,
    /// The folder the agent starts in: the session's working folder, inside the checkout.
    pub start_dir: PathBuf,
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
    /// What the provider adapter declares for the sandbox.
    pub needs: ProviderNeeds,
    /// What the checkout's toolchains and Git ask of a sandboxed launch (#859).
    pub tools: ToolNeeds,
}

/// One check run's facts (#862): the project's check as the program, with no provider, agent
/// socket, launch token or session identity. The run's execution stays the daemon
/// supervisor's, so it runs in the same systemd scope placement as a session (#834).
#[derive(Debug, Clone)]
pub struct CheckRequest {
    /// The run's id: it names the run's folder of the daemon's mount sources.
    pub run: String,
    /// The check's name in the project file.
    pub check: String,
    /// The shell the check's command runs in, with `-c` and the command.
    pub shell: PathBuf,
    pub command: String,
    /// The checkout the check runs in, whole: read and write.
    pub checkout: PathBuf,
    /// The run's own folder, where its home and caches live: read and write, removed by the
    /// caller when the run ends.
    pub execution_dir: PathBuf,
    /// The run's own temporary folder, which `TMPDIR` names: read and write.
    pub temp_dir: PathBuf,
    /// The environment Torlek builds for the check: none of the daemon's own variables.
    pub environment: Vec<(OsString, OsString)>,
    /// What the checkout's toolchains and Git ask of the run (#859).
    pub tools: ToolNeeds,
}

/// A launch ready to run: the program and arguments that start the agent and the evidence
/// to record with the launch.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
    pub evidence: Evidence,
    /// How the daemon reaches the provider's sockets in this launch.
    pub reach: ProviderReach,
    /// What [`Staged::write`] must create before the program runs; None for a launch that
    /// runs as is.
    pub staged: Option<Staged>,
}

impl Sandbox {
    /// `platform` is the host's, detected once by the caller; `backend` what its probe
    /// found.
    pub fn new(platform: Platform, host: HostPaths, backend: Backend) -> Sandbox {
        Sandbox {
            platform,
            host: Some(host),
            backend,
        }
    }

    /// A sandbox whose caller read none of the host's paths: profiles are refused.
    pub fn unavailable() -> Sandbox {
        Sandbox {
            platform: Platform::detect(),
            host: None,
            backend: Backend::Missing {
                reason: "the daemon read none of this host's paths".to_owned(),
            },
        }
    }

    /// What the daemon's probe found.
    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    /// `session`'s own home folder under the sandbox, which its end removes; None when the
    /// daemon read no host paths.
    pub fn session_home(&self, session: &str) -> Option<Result<PathBuf, Refusal>> {
        let host = self.host.as_ref()?;
        Some(session_folder(&host.homes, session))
    }

    /// `session`'s tool configuration folder under the sandbox, with the owner's Git identity
    /// in it, which its end removes (#859); None when the daemon read no host paths.
    pub fn session_tool_config(&self, session: &str) -> Option<Result<PathBuf, Refusal>> {
        let host = self.host.as_ref()?;
        Some(
            session_folder(&host.mount_sources, session)
                .map(|folder| folder.join(crate::native::CONFIG_FOLDER)),
        )
    }

    /// What `session`'s launcher last reported letting out of the sandbox and refusing
    /// (#831); None when no sandboxed launch of it reported anything.
    pub fn egress_report(&self, session: &str) -> Result<Option<crate::EgressReport>, String> {
        let Some(host) = self.host.as_ref() else {
            return Ok(None);
        };
        let path = session_folder(&host.mount_sources, session)
            .map_err(|refusal| refusal.reason)?
            .join(crate::EGRESS_FILE);
        let text = match std::fs::read(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        serde_json::from_slice(&text)
            .map(Some)
            .map_err(|error| format!("{} is malformed: {error}", path.display()))
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
                provider: Some(request.provider.clone()),
                working_dir: request.working_dir.clone(),
                session_dir: request.session_dir.clone(),
                temp_dir: request.temp_dir.clone(),
                mount_dir,
                owner_uid: host.owner_uid,
                read_only: host
                    .read_only
                    .iter()
                    .chain(&host.sockets)
                    .cloned()
                    .collect(),
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

    /// How the check of `request` runs under `setting` (#862): as a session's program would,
    /// unchanged under `agents.sandbox = "off"`, through the native launcher under
    /// `"required"`, or refused naming why there is none, never run unsandboxed. Writes
    /// nothing: the caller writes [`Prepared::staged`] before running it, and removes its
    /// mount folder and [`CheckRequest::execution_dir`] when the run ends.
    pub fn prepare_check(
        &self,
        setting: AgentSandbox,
        request: &CheckRequest,
    ) -> Result<Prepared, Refusal> {
        let arguments = vec![OsString::from("-c"), OsString::from(&request.command)];
        if setting == AgentSandbox::Off {
            return Ok(self.unsandboxed(request.shell.clone(), arguments, &request.environment));
        }
        let native = self.native()?;
        let host = self.host()?;
        let mount_dir = session_folder(&host.mount_sources, &format!("check-{}", request.run))?;
        let profile = Profile::build(
            ProfileRequest {
                provider: None,
                working_dir: request.checkout.clone(),
                session_dir: request.execution_dir.clone(),
                temp_dir: request.temp_dir.clone(),
                mount_dir: mount_dir.clone(),
                owner_uid: host.owner_uid,
                read_only: Vec::new(),
                denied: host.denied.clone(),
                denied_write: host.denied_write.clone(),
                allow_hosts: Vec::new(),
                environment: request.environment.clone(),
                secrets: Vec::new(),
                desktop: DesktopSession::Withheld,
            },
            &self.platform,
        )?;
        let launch = crate::native::Launch {
            subject: Subject::Check {
                run: request.run.clone(),
                check: request.check.clone(),
            },
            start_dir: request.checkout.clone(),
            environment: request.environment.clone(),
            needs: ProviderNeeds::floor(),
            tools: request.tools.clone(),
            reaches_agent: false,
        };
        crate::native::prepare(
            native,
            host,
            &profile,
            &launch,
            mount_dir,
            request.execution_dir.join("home"),
            request.shell.clone(),
            arguments,
        )
    }

    /// How `program` with `arguments` runs for `request` under `setting`: unchanged under
    /// `agents.sandbox = "off"`, which the evidence says (the caller logs it at every
    /// launch); under `"required"`, through the native launcher, or refused naming why there
    /// is none. Writes nothing: the caller writes [`Prepared::staged`] before running it.
    pub fn prepare(
        &self,
        setting: AgentSandbox,
        request: &LaunchRequest,
        program: PathBuf,
        arguments: Vec<OsString>,
    ) -> Result<Prepared, Refusal> {
        if setting == AgentSandbox::Off {
            return Ok(self.unsandboxed(program, arguments, &request.environment));
        }
        let native = self.native()?;
        let host = self.host()?;
        let profile = self.profile(request)?;
        let mount_dir = session_folder(&host.mount_sources, &request.session)?;
        let home_source = session_folder(&host.homes, &request.session)?;
        let launch = crate::native::Launch {
            subject: Subject::Session {
                session: request.session.clone(),
            },
            start_dir: request.start_dir.clone(),
            environment: request.environment.clone(),
            needs: request.needs.clone(),
            tools: request.tools.clone(),
            reaches_agent: true,
        };
        crate::native::prepare(
            native,
            host,
            &profile,
            &launch,
            mount_dir,
            home_source,
            program,
            arguments,
        )
    }

    /// `program` with `arguments` as it is, under `agents.sandbox = "off"`, which the
    /// evidence says (the caller logs it at every launch).
    fn unsandboxed(
        &self,
        program: PathBuf,
        arguments: Vec<OsString>,
        environment: &[(OsString, OsString)],
    ) -> Prepared {
        Prepared {
            program,
            arguments,
            environment: environment.to_vec(),
            evidence: Evidence {
                enforcement: Enforcement::Unsandboxed {
                    reason: "no sandbox: agents.sandbox = \"off\"".to_owned(),
                },
                platform: self.platform.name().to_owned(),
                rules: Vec::new(),
                observed: Vec::new(),
            },
            reach: ProviderReach::Direct,
            staged: None,
        }
    }

    /// The native backend the daemon probed; refused under `required` naming why there is
    /// none, never run unsandboxed.
    fn native(&self) -> Result<&NativeBackend, Refusal> {
        match &self.backend {
            Backend::Native(native) => Ok(native),
            Backend::Missing { reason } => Err(Refusal::new(
                "agents.sandbox = \"required\"",
                format!(
                    "no sandbox backend on this host: {reason}; the launch is refused rather \
                     than run unsandboxed; agents.sandbox = \"off\" in the owner's file runs \
                     agents unsandboxed"
                ),
            )),
        }
    }

    fn host(&self) -> Result<&HostPaths, Refusal> {
        self.host
            .as_ref()
            .ok_or_else(|| Refusal::new("sandbox", "the daemon read none of this host's paths"))
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
