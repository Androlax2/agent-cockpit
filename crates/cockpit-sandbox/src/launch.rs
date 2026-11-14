//! The one entry `cockpit-sessions` calls for every agent launch: the owner's setting and
//! the launch's own facts in; the command to run and its evidence out. A launch runs
//! unsandboxed under `agents.sandbox = "off"`; under `"required"` it runs through the native
//! backend the daemon probed (#829), or is refused naming why there is none, never run
//! unsandboxed.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use cockpit_config::{AgentSandbox, DesktopSession};

use crate::{
    Backend, BoundCredential, Enforcement, Evidence, NativeBackend, Platform, Profile,
    ProfileRequest, ProviderNeeds, ProviderReach, Refusal, Secret, Staged, Subject, ToolNeeds,
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
    /// The daemon's folder of login accounts (#59), one folder per provider and account
    /// inside: the login the owner signed in, and the fresh home a sign-in runs in. Never
    /// granted to any agent above an account's login.
    pub logins: PathBuf,
}

/// Where one login account's folders are, in [`HostPaths::logins`] (#59).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountLogin {
    /// Holds the login file alone, under the name the harness gives it.
    pub login: PathBuf,
    /// The fresh home `torlek account sign-in` runs the harness's sign-in in, removed once its
    /// login moved to [`AccountLogin::login`].
    pub sign_in: PathBuf,
}

/// A login account's sign-in a sandboxed session uses (#59, #858): the session sees the
/// account's one login, and a sign-in the harness refreshes reaches the account's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLogin {
    /// The account's folder holding its login file alone, [`AccountLogin::login`].
    pub folder: PathBuf,
    /// Where the harness keeps the file, the `~/` path its adapter declares.
    pub file: String,
    pub mount: LoginMount,
}

/// How a session sees its login, by how the harness rewrites it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMount {
    /// The login file bound at the harness's path: the harness rewrites it in place.
    File,
    /// The account's folder bound inside the home and a link at the harness's path naming
    /// the login in it: the harness renames a new file over the link's target.
    FolderAndLink,
}

/// The sandbox launches go through: the host's platform and the daemon's host paths.
#[derive(Debug, Clone)]
pub struct Sandbox {
    platform: Platform,
    /// None only for [`Sandbox::unavailable`], whose caller never read the host.
    host: Option<HostPaths>,
    backend: Backend,
    /// Shared by every clone: orders the daemon's writes into a session's folder of mount
    /// sources with the removal of that folder (#1003).
    mount_writes: Arc<Mutex<()>>,
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
    /// Credentials the session proxy adds on their own hosts (#832). Only a sandboxed launch
    /// binds one; an unsandboxed launch is refused one rather than given the value.
    pub credentials: Vec<BoundCredential>,
    /// The owner's decisions on the session's network requests when it launches (#674): the
    /// proxy applies them from the first connection, and a host they let through counts as
    /// reached for a bound credential (#832).
    pub decisions: crate::EgressDecisions,
    /// The login account's sign-in the session uses (#59); only a sandboxed launch binds one.
    pub login: Option<SessionLogin>,
    /// The project's declared services the launcher serves (#880); each relays only while
    /// the owner's decisions allow its record. Only a sandboxed launch serves one.
    pub forwards: Vec<crate::Forward>,
    /// The project engine the session reaches at `DOCKER_HOST` (#390), verified by the
    /// daemon; only a sandboxed launch binds one.
    pub engine: Option<crate::EngineSocket>,
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
    /// The folder the command starts in, inside the checkout: its declared working directory.
    pub start_dir: PathBuf,
    /// The run's own folder: read and write, removed by the caller when the run ends.
    pub execution_dir: PathBuf,
    /// The run's own temporary folder, which `TMPDIR` names and its caches live in: read and
    /// write, removed by the caller when the run ends.
    pub temp_dir: PathBuf,
    /// The environment Torlek builds for the check: none of the daemon's own variables.
    pub environment: Vec<(OsString, OsString)>,
    /// What the checkout's toolchains and Git ask of the run (#859).
    pub tools: ToolNeeds,
    /// Where the run's proxy lets it go beside the owner's grants: the registries of the
    /// ecosystems its checkout uses (#862 Q2), never a provider's hosts.
    pub egress: Vec<crate::AllowEntry>,
    /// The owner's decisions when the run starts (#674): the project's host grants. A check
    /// serves no forward, so a service decision lets nothing through.
    pub decisions: crate::EgressDecisions,
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
            mount_writes: Arc::default(),
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
            mount_writes: Arc::default(),
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

    /// `account` of `provider`'s login folders; None when the daemon read no host paths.
    pub fn account_login(
        &self,
        provider: &str,
        account: &str,
    ) -> Option<Result<AccountLogin, Refusal>> {
        let host = self.host.as_ref()?;
        Some(
            account_folder(&host.logins, provider, account).map(|folder| AccountLogin {
                login: folder.join("login"),
                sign_in: folder.join("sign-in"),
            }),
        )
    }

    /// `session`'s own folder of the daemon's mount sources, which its end removes (#482): the
    /// launch spec, the egress files, the tool configuration with the owner's Git identity
    /// (#859) and the folders its sandbox mounts. None when the daemon read no host paths.
    pub fn session_mount_sources(&self, session: &str) -> Option<Result<PathBuf, Refusal>> {
        let host = self.host.as_ref()?;
        Some(session_folder(&host.mount_sources, session))
    }

    /// Holds off the daemon's writes into every session's folder of mount sources until the
    /// guard drops: a caller removing one holds it, so [`Sandbox::write_egress_decisions`],
    /// which checks the folder and writes into it under the same lock, never stages a file
    /// into a folder being removed (#1003).
    pub fn hold_mount_writes(&self) -> MutexGuard<'_, ()> {
        // The lock guards no data, so a holder's panic leaves nothing half-written to refuse.
        self.mount_writes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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

    /// The destinations `session`'s running launcher refused for want of an entry (#674);
    /// None when no sandboxed launch of it refused one.
    pub fn egress_requests(&self, session: &str) -> Result<Option<crate::EgressRequests>, String> {
        let Some(host) = self.host.as_ref() else {
            return Ok(None);
        };
        let folder =
            session_folder(&host.mount_sources, session).map_err(|refusal| refusal.reason)?;
        read_egress_requests(&folder)
    }

    /// The destinations check run `run`'s launcher refused (#352), as
    /// [`Sandbox::egress_requests`] reads a session's.
    pub fn check_egress_requests(
        &self,
        run: &str,
    ) -> Result<Option<crate::EgressRequests>, String> {
        let Some(host) = self.host.as_ref() else {
            return Ok(None);
        };
        let folder = check_folder(&host.mount_sources, run).map_err(|refusal| refusal.reason)?;
        read_egress_requests(&folder)
    }

    /// Replaces the decisions `session`'s launcher enforces (#674); false, writing nothing,
    /// when the session has no sandbox folder, so no launcher reads them. Holds
    /// [`Sandbox::hold_mount_writes`] from the folder's check to the write's end.
    pub fn write_egress_decisions(
        &self,
        session: &str,
        decisions: &crate::EgressDecisions,
    ) -> Result<bool, String> {
        let Some(host) = self.host.as_ref() else {
            return Ok(false);
        };
        let folder =
            session_folder(&host.mount_sources, session).map_err(|refusal| refusal.reason)?;
        self.write_decisions_into(&folder, decisions)
    }

    /// Replaces the decisions check run `run`'s launcher enforces (#352), as
    /// [`Sandbox::write_egress_decisions`] does a session's.
    pub fn write_check_egress_decisions(
        &self,
        run: &str,
        decisions: &crate::EgressDecisions,
    ) -> Result<bool, String> {
        let Some(host) = self.host.as_ref() else {
            return Ok(false);
        };
        let folder = check_folder(&host.mount_sources, run).map_err(|refusal| refusal.reason)?;
        self.write_decisions_into(&folder, decisions)
    }

    /// Makes `project`'s engine proxy's folder (#390 part 6c), owner-only, before the daemon
    /// starts that proxy on it: the last proxy's refused hosts and counters are removed, so
    /// the daemon files only this one's, and its decisions too, so the new proxy allows
    /// nothing until the daemon writes them as they stand. None when the daemon read no host
    /// paths.
    pub fn prepare_engine_proxy_folder(&self, project: &str) -> Result<Option<PathBuf>, String> {
        let Some(host) = self.host.as_ref() else {
            return Ok(None);
        };
        let folder =
            engine_proxy_folder(&host.mount_sources, project).map_err(|refusal| refusal.reason)?;
        let failed =
            |error: std::io::Error| format!("cannot prepare {}: {error}", folder.display());
        let mut folders = std::fs::DirBuilder::new();
        std::os::unix::fs::DirBuilderExt::mode(folders.recursive(true), 0o700)
            .create(&folder)
            .map_err(failed)?;
        for stale in [
            crate::REQUESTS_FILE,
            crate::EGRESS_FILE,
            crate::DECISIONS_FILE,
        ] {
            match std::fs::remove_file(folder.join(stale)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(failed(error)),
            }
        }
        Ok(Some(folder))
    }

    /// The hosts `project`'s engine proxy refused, as [`Sandbox::egress_requests`] reads a
    /// session's.
    pub fn engine_egress_requests(
        &self,
        project: &str,
    ) -> Result<Option<crate::EgressRequests>, String> {
        let Some(host) = self.host.as_ref() else {
            return Ok(None);
        };
        let folder =
            engine_proxy_folder(&host.mount_sources, project).map_err(|refusal| refusal.reason)?;
        read_egress_requests(&folder)
    }

    /// Replaces the decisions `project`'s engine proxy enforces; false, writing nothing, until
    /// [`Sandbox::prepare_engine_proxy_folder`] made its folder.
    pub fn write_engine_egress_decisions(
        &self,
        project: &str,
        decisions: &crate::EgressDecisions,
    ) -> Result<bool, String> {
        let Some(host) = self.host.as_ref() else {
            return Ok(false);
        };
        let folder =
            engine_proxy_folder(&host.mount_sources, project).map_err(|refusal| refusal.reason)?;
        self.write_decisions_into(&folder, decisions)
    }

    fn write_decisions_into(
        &self,
        folder: &std::path::Path,
        decisions: &crate::EgressDecisions,
    ) -> Result<bool, String> {
        let _writing = self.hold_mount_writes();
        if !folder.is_dir() {
            return Ok(false);
        }
        let path = folder.join(crate::DECISIONS_FILE);
        let text = serde_json::to_vec(decisions).map_err(|error| error.to_string())?;
        if std::fs::read(&path).is_ok_and(|written| written == text) {
            return Ok(true);
        }
        crate::native::replace_atomically(&path, &text)
            .map(|()| true)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
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
    /// mount folder and home when the run ends, beside the run's own folders.
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
        let mount_dir = check_folder(&host.mount_sources, &request.run)?;
        // In the daemon's folder of homes, as a session's, never in a folder the check
        // writes: no program can swap a mount source between its creation and the bind.
        let home_source = check_folder(&host.homes, &request.run)?;
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
            start_dir: request.start_dir.clone(),
            // The profile's: without what the platform withholds, `WSL_INTEROP` on WSL2.
            environment: profile.environment.clone(),
            needs: ProviderNeeds {
                egress: request.egress.clone(),
                ..ProviderNeeds::floor()
            },
            tools: request.tools.clone(),
            reaches_agent: false,
            credentials: Vec::new(),
            decisions: request.decisions.clone(),
            login: None,
            forwards: Vec::new(),
            engine: None,
        };
        crate::native::prepare(
            native,
            host,
            &profile,
            &launch,
            mount_dir,
            home_source,
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
            if request.login.is_some() {
                return Err(Refusal::new(
                    "login account",
                    "only a sandboxed launch binds a login account's sign-in in its own home; \
                     an unsandboxed launch runs in the owner's",
                ));
            }
            if let Some(credential) = request.credentials.first() {
                return Err(Refusal::new(
                    format!("credential {}", credential.name),
                    "only the session proxy of a sandboxed launch adds a bound credential; \
                     an unsandboxed launch has no proxy",
                ));
            }
            if let Some(forward) = request.forwards.first() {
                return Err(Refusal::new(
                    format!("service {}", forward.service),
                    "only a sandboxed launch's launcher serves a declared service; an \
                     unsandboxed launch reaches the host's services itself",
                ));
            }
            if let Some(engine) = &request.engine {
                return Err(Refusal::new(
                    format!("project engine of {}", engine.user),
                    "only a sandboxed launch binds the project engine's socket; an \
                     unsandboxed launch reaches the owner's own engine",
                ));
            }
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
            environment: profile.environment.clone(),
            needs: request.needs.clone(),
            tools: request.tools.clone(),
            reaches_agent: true,
            credentials: request.credentials.clone(),
            decisions: request.decisions.clone(),
            login: request.login.clone(),
            forwards: request.forwards.clone(),
            engine: request.engine.clone(),
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
    if !is_plain_name(session) {
        return Err(Refusal::new(
            format!("session {session}"),
            "a session id must be one plain name to name its folders",
        ));
    }
    Ok(parent.join(session))
}

fn account_folder(
    parent: &std::path::Path,
    provider: &str,
    account: &str,
) -> Result<PathBuf, Refusal> {
    for (kind, name) in [("provider", provider), ("account", account)] {
        if !is_plain_name(name) {
            return Err(Refusal::new(
                format!("{kind} {name}"),
                format!("a {kind} name must be one plain name to name its folders"),
            ));
        }
    }
    Ok(parent.join(provider).join(account))
}

/// The requests file in a launcher's mount folder; None when its launcher refused nothing.
fn read_egress_requests(folder: &std::path::Path) -> Result<Option<crate::EgressRequests>, String> {
    let path = folder.join(crate::REQUESTS_FILE);
    let text = match std::fs::read(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    serde_json::from_slice(&text)
        .map(Some)
        .map_err(|error| format!("{} is malformed: {error}", path.display()))
}

/// Check run `run`'s own folder in `parent`, beside sessions' folders: a session id is `s`
/// and hexadecimal digits, so `check-` never names one.
fn check_folder(parent: &std::path::Path, run: &str) -> Result<PathBuf, Refusal> {
    if !is_plain_name(run) {
        return Err(Refusal::new(
            format!("check run {run}"),
            "a check run id must be one plain name to name its folders",
        ));
    }
    Ok(parent.join(format!("check-{run}")))
}

/// `project`'s engine proxy's folder in `parent`, beside sessions' and check runs' folders:
/// `engine-` names neither.
fn engine_proxy_folder(parent: &std::path::Path, project: &str) -> Result<PathBuf, Refusal> {
    if !is_plain_name(project) {
        return Err(Refusal::new(
            format!("project {project}"),
            "a project's name must be one plain name to name its engine proxy's folder",
        ));
    }
    Ok(parent.join(format!("engine-{project}")))
}

fn is_plain_name(name: &str) -> bool {
    let mut components = std::path::Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    )
}
