#![doc = include_str!("../README.md")]

mod docker;
mod host;
mod project;

use std::path::PathBuf;

pub use cockpit_config::{AgentSandbox, EffectiveConfig, ExecutionEnvironment, ProjectNeeds};
pub use docker::{DOCKER_DEADLINE, DockerEngine, probe_docker};
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

/// What the sandbox engine the daemon detected at its start is, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxEngine {
    /// `greywall 0.3.7+dotfiles (7c06d51), greyproxy 0.4.5 (cdd5cde)`.
    pub engine: String,
    /// The host capabilities the engine reported, one line each.
    pub observations: Vec<String>,
}

/// A provider the daemon tried to wire at its start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFacts {
    /// As `session new --agent` takes it.
    pub name: String,
    /// What it reported, or why it is not available.
    pub found: Result<ProviderFound, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFound {
    pub version: Option<String>,
    /// Its version is one the adapter's conformance tests ran on.
    pub tested: bool,
    /// The capabilities it reports, by their protocol names.
    pub capabilities: Vec<String>,
    pub sign_in: SignInFacts,
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
    pub sandbox_engine: Result<SandboxEngine, String>,
    /// The owner's `agents.sandbox` as the daemon applies it now.
    pub sandbox_setting: AgentSandbox,
    pub providers: Vec<ProviderFacts>,
    pub controls: Vec<ControlFacts>,
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
    /// The probed engine, when [`needs_docker`] said to probe one.
    pub docker: Option<Result<DockerEngine, String>>,
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
    requirements.push(host::sandbox(&host.sandbox_engine, host.sandbox_setting));
    requirements.extend(host::providers(&host.providers));
    requirements.push(host::resource_controls(&host.controls));
    if let (Some(project), Ok(config)) = (project, config) {
        requirements.extend(project::requirements(host, config, project));
    }
    Report { requirements }
}
