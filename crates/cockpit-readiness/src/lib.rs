#![doc = include_str!("../README.md")]

mod host;
mod launch;
mod project;

use std::path::PathBuf;

pub use cockpit_config::{AgentSandbox, EffectiveConfig, ExecutionEnvironment, ProjectNeeds};
pub use launch::{HarnessTrust, LaunchFacts, Toolchains, launch_gates, launch_summary};
pub use project::needs_docker;

/// One requirement's state (report 09).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Available,
    NeedsAuthorization,
    MissingPrerequisite,
    Unsupported,
    Unknown,
}

impl Status {
    pub const ALL: [Status; 5] = [
        Status::Available,
        Status::NeedsAuthorization,
        Status::MissingPrerequisite,
        Status::Unsupported,
        Status::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Available => "available",
            Status::NeedsAuthorization => "needs-authorization",
            Status::MissingPrerequisite => "missing-prerequisite",
            Status::Unsupported => "unsupported",
            Status::Unknown => "unknown",
        }
    }

    /// Whether work that needs it cannot go ahead: something must be installed, fixed or
    /// moved first, or this host cannot provide it. What the owner has not granted and what
    /// Cockpit cannot tell are reported without blocking.
    pub fn blocks(self) -> bool {
        matches!(self, Status::MissingPrerequisite | Status::Unsupported)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// Stable within a report: `tmux`, `provider claude`, `check test`.
    pub name: String,
    pub status: Status,
    /// What was found, in one line.
    pub detail: String,
    /// What makes it available; always set when it is not.
    pub next: Option<String>,
}

impl Requirement {
    pub fn available(name: impl Into<String>, detail: impl Into<String>) -> Requirement {
        Requirement {
            name: name.into(),
            status: Status::Available,
            detail: detail.into(),
            next: None,
        }
    }

    /// A requirement that is not available, with the action that makes it so.
    ///
    /// # Panics
    ///
    /// When `status` is [`Status::Available`]: use [`Requirement::available`].
    pub fn needs(
        name: impl Into<String>,
        status: Status,
        detail: impl Into<String>,
        next: impl Into<String>,
    ) -> Requirement {
        assert_ne!(
            status,
            Status::Available,
            "an available requirement has no next action to need"
        );
        Requirement {
            name: name.into(),
            status,
            detail: detail.into(),
            next: Some(next.into()),
        }
    }

    /// An available requirement with an action the owner may still want to take.
    pub fn with_advice(mut self, next: impl Into<String>) -> Requirement {
        self.next = Some(next.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub requirements: Vec<Requirement>,
}

impl Report {
    /// No requirement blocks ([`Status::blocks`]).
    pub fn is_ready(&self) -> bool {
        !self
            .requirements
            .iter()
            .any(|requirement| requirement.status.blocks())
    }
}

/// The engine a Docker probe answered for (`cockpit-containers` probes it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerEngine {
    /// The engine's own ID.
    pub id: String,
    /// The host name the engine reports.
    pub name: String,
    pub server_version: String,
    /// The engine runs as this user, not as root (its security options name `rootless`).
    pub rootless: bool,
    /// Docker Desktop: the engine runs in a Linux VM of its own, not on this host.
    pub in_desktop_vm: bool,
}

/// A project's own engine under `agents.sandbox = "required"` (#390): rootless Docker under
/// a user of its own, which the daemon verified through its socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEngine {
    /// The registered project's name, as `torlek containers record` takes it.
    pub project: String,
    /// The engine's system user: `torlek-e3`.
    pub user: String,
    /// The command that records it, in this profile: `torlek containers record shop`.
    pub record_command: String,
    pub state: EngineState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    /// No engine answers as the project's and none is recorded, with why.
    NotSetUp {
        reason: String,
        /// The setup script the owner reviews and runs, or why it was not written.
        setup: Result<PathBuf, String>,
    },
    /// The project's engine answers, rootless, as its user; the owner has not recorded it.
    Unrecorded(DockerEngine),
    /// The recorded engine answers as the project's own, rootless.
    Verified(DockerEngine),
    /// What failed, by name, and what makes it usable.
    Refused {
        reason: String,
        remedy: EngineRemedy,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineRemedy {
    /// Run the setup script again, or why it was not written.
    Setup(Result<PathBuf, String>),
    /// Start the engine's unit.
    Start { unit: String },
    /// Record the engine that answers now.
    Record,
}

/// The platform as the sandbox detected it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
    /// Linux in WSL2; `windows_drives` are the mounted Windows drives (`/mnt/c`...).
    Wsl2 {
        windows_drives: Vec<PathBuf>,
    },
    /// By its `std::env::consts::OS` name.
    Unsupported(String),
}

/// A provider the daemon tried to wire at its start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFacts {
    /// As `session new --agent` takes it.
    pub name: String,
    /// What it reported, or why it is not available.
    pub found: Result<ProviderFound, ProviderUnusable>,
}

/// Why the daemon could not wire a provider. The two need different fixes: installing a
/// program already on `PATH` does not help (#623).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderUnusable {
    /// No program of the provider's name on the daemon's `PATH`.
    NotOnPath,
    /// Its program is on the daemon's `PATH`, and `reason` is why the adapter refused it,
    /// such as a `--version` it did not understand.
    Refused { reason: String },
    /// Its adapter's sandbox declaration breaks the sandbox's rules (#861): refused when the
    /// daemon loaded its providers, for `reason`.
    DeclarationRefused { reason: String },
}

/// The runtime probe's result for one provider version: whether it found every requirement, and
/// the sentence naming each one it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeFacts {
    pub is_clean: bool,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFound {
    pub version: Option<String>,
    /// Its version is one the adapter's conformance tests ran on.
    pub tested: bool,
    /// What the runtime probe found in a version that is not tested (#732); `None` for a
    /// tested version, or when the probe could not run.
    pub probe: Option<ProbeFacts>,
    /// The capabilities it reports, by their protocol names.
    pub capabilities: Vec<String>,
    pub sign_in: SignInFacts,
    /// Under `agents.sandbox = "required"`, the owner's credential the session proxy adds for
    /// it (#968), for each of its accounts in the order a launch tries them (#59): a sandboxed
    /// agent never reads the owner's own sign-in. Empty when agents run unsandboxed or the
    /// provider takes no bound credential.
    pub sandbox_credentials: Vec<cockpit_config::AccountCredentialState>,
}

/// The adapter's current credential observation, mapped by the daemon. Unknown stays
/// distinct from an explicit signed-out answer; detail includes the probe's provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInFacts {
    pub signed_in: Option<bool>,
    pub detail: String,
    pub next: String,
}

/// One resource control and whether the admission backend enforces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlFacts {
    pub control: String,
    /// The mechanism that enforces it, or what is missing.
    pub enforced: Result<String, String>,
}

/// What the daemon knows about this host, each fact from its owner.
#[derive(Debug, Clone)]
pub struct HostFacts {
    pub platform: Platform,
    /// The tmux version the daemon's start found, or why it refused it.
    pub tmux: Result<String, String>,
    /// The owner's `agents.sandbox` as the daemon applies it now: what it last read, at its
    /// start or at a `config reload`, which the owner's file may no longer say (#963).
    pub sandbox_setting: AgentSandbox,
    /// The native sandbox the daemon's start probed (#829): what launches under `required`
    /// run in, or why none can.
    pub sandbox_backend: Result<String, String>,
    /// Why the native sandbox refuses every launch under the desktop session the daemon
    /// would share, when it does (#965): `None` when the owner withholds it or it names
    /// nothing the sandbox refuses.
    pub shared_desktop_refusal: Option<String>,
    pub providers: Vec<ProviderFacts>,
    pub controls: Vec<ControlFacts>,
    /// What the daemon sets on running sessions' processes (#1351).
    pub session_controls: Vec<ControlFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutFacts {
    pub project: String,
    /// The project's main checkout: what the owner trusts.
    pub project_root: PathBuf,
    /// As `cockpit-projects` decides it from `projects.trusted`.
    pub trust: Trust,
}

/// Whose trust covers a checkout (`cockpit_projects::Trust`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    Trusted,
    /// A parallel clone of a trusted project.
    Inherited {
        from: PathBuf,
    },
    Untrusted,
    /// Review clones hold someone else's changes: never trusted.
    NeverForReview,
}

/// A workflow preset and the provider capabilities its agent steps need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetFacts {
    pub name: String,
    pub capabilities: Vec<String>,
}

/// What the daemon knows about one project, beyond its configuration.
#[derive(Debug, Clone)]
pub struct ProjectFacts {
    /// Absolute: the checkout's root when it is registered, else the directory asked about.
    pub dir: PathBuf,
    /// The registered checkout `dir` is, `None` when it is none.
    pub checkout: Option<CheckoutFacts>,
    pub presets: Vec<PresetFacts>,
    /// The host's budget, in the units of [`ProjectNeeds`].
    pub capacity: ProjectNeeds,
    /// The engine the owner's `docker` reaches, when [`needs_docker`] said to probe one and
    /// agents run unsandboxed.
    pub docker: Option<Result<DockerEngine, String>>,
    /// The project's own engine, when [`needs_docker`] said to probe one and agents run
    /// sandboxed (#390), or why it could not be examined.
    pub engine: Option<Result<ProjectEngine, String>>,
    /// The gates a session of each wired provider in `dir` would pass (#1250), when `dir` is
    /// a registered checkout, or why one could not be read.
    pub launch: Option<Result<Vec<LaunchFacts>, String>>,
}

/// The report for this host and, with `project`, for that project: `config` is the
/// effective configuration loaded with the project's own file, or why it cannot be read.
pub fn assess(
    host: &HostFacts,
    config: &Result<EffectiveConfig, String>,
    project: Option<&ProjectFacts>,
) -> Report {
    let mut requirements = vec![host::config(config), host::platform(&host.platform)];
    requirements.push(host::tmux(&host.tmux));
    let unapplied_sandbox = config
        .as_ref()
        .ok()
        .map(|config| &config.agents_sandbox)
        .filter(|written| written.value != host.sandbox_setting);
    requirements.push(match unapplied_sandbox {
        Some(written) => {
            host::unapplied_sandbox(written, host.sandbox_setting, &host.sandbox_backend)
        }
        None => host::sandbox(
            host.sandbox_setting,
            &host.sandbox_backend,
            host.shared_desktop_refusal.as_deref(),
        ),
    });
    requirements.extend(host::providers(&host.providers));
    requirements.push(host::resource_controls(&host.controls));
    requirements.push(host::session_controls(&host.session_controls));
    if let (Some(project), Ok(config)) = (project, config) {
        requirements.extend(project::requirements(host, config, project));
    }
    Report { requirements }
}
