//! The gates one launch passes before its agent runs (#1250): what `torlek sandbox show`
//! prints one line each, and `torlek doctor --project` and `torlek init` report. The daemon
//! reads each fact with the launch's own code; nothing here launches, writes or grants.

use std::path::PathBuf;

use cockpit_config::{AgentSandbox, DesktopSession};

use crate::{Requirement, Status};

/// What each gate of a launch of `provider` found, in launch order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchFacts {
    pub provider: String,
    /// The owner's `agents.sandbox` the daemon applies now.
    pub setting: AgentSandbox,
    pub desktop: DesktopSession,
    /// The program the provider's adapter starts, or why it refused to build the command.
    pub command: Result<PathBuf, String>,
    /// The native sandbox the daemon's start probed, or why there is none.
    pub backend: Result<String, String>,
    /// Why the native sandbox refuses the desktop session the launch would share, when it
    /// does (#965).
    pub shared_desktop_refusal: Option<String>,
    /// The account a sandboxed launch runs on, with its credential or signed-in login (#968,
    /// #59): `None` when it runs on none; `Err` when an account's credential or login refuses
    /// the launch.
    pub account: Result<Option<String>, String>,
    /// The login accounts the launch passes over, each with why and the grant that lets it.
    pub passed_over: Vec<String>,
    /// Whether the launch binds a sign-in at all: not under `"off"`, nor for a provider that
    /// declares no credential variable.
    pub binds_sign_in: bool,
    /// Whether the project keeps the GitHub token this launch would use: the API token the
    /// session proxy adds under `"required"`, the project's token under `"off"`.
    pub github: Result<bool, String>,
    /// The checkout's toolchains and the install roots they bind (#859); `None` when the
    /// launch reads none: under `"off"`, and without a backend.
    pub toolchains: Option<Result<Toolchains, String>>,
    pub trust: HarnessTrust,
    /// The profile the backend would enforce, by its first evidence line; `None` when an
    /// earlier gate refuses, so it was not built.
    pub profile: Option<Result<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchains {
    pub ecosystems: Vec<String>,
    pub install_roots: Vec<PathBuf>,
}

/// Whether the harness's folder-trust question is answered before the agent starts (#702,
/// #968).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessTrust {
    /// Torlek records it from `from`, a checkout in the owner's `projects.trusted`.
    Recorded { from: PathBuf },
    /// The agent runs in the owner's own home, which keeps the owner's earlier answer.
    OwnersHome,
    /// Torlek records none, for `reason`: the harness asks at the agent's first start.
    Refused { reason: String },
    /// A review clone holds someone else's changes: never trusted.
    NeverForReview,
}

/// One requirement per gate, named `launch <gate>`, in launch order.
pub fn launch_gates(facts: &LaunchFacts) -> Vec<Requirement> {
    let is_sandboxed = facts.setting == AgentSandbox::Required;
    let mut gates = vec![
        command(facts),
        backend(facts.setting, &facts.backend),
        desktop(
            is_sandboxed,
            facts.desktop,
            facts.shared_desktop_refusal.as_deref(),
        ),
        sign_in(facts),
        github(is_sandboxed, &facts.github),
        toolchains(facts.setting, facts.toolchains.as_ref()),
        trust(&facts.provider, &facts.trust),
    ];
    gates.push(match &facts.profile {
        Some(Ok(enforcement)) => Requirement::available("launch profile", enforcement),
        Some(Err(reason)) => Requirement::needs(
            "launch profile",
            Status::MissingPrerequisite,
            format!("refused: {reason}"),
            "fix what is named here; a rule of Torlek's own adapter is a Torlek bug: report it",
        ),
        None => Requirement::needs(
            "launch profile",
            Status::Unknown,
            "not built: a gate above refuses the launch",
            "fix the gates above, then run this again",
        ),
    });
    gates
}

/// The gates that depend on the provider; the others are the host's and the project's, the
/// same for every provider.
const PROVIDER_GATES: [&str; 3] = ["launch command", "launch sign-in", "launch profile"];

/// The gates to show beside a project's readiness, for a session of each provider in `all`:
/// each one that is not available, once, with the provider in its name when it depends on it
/// and there are several; or one `launch` line when every launch passes them all. A profile
/// that was not built is left out: the gate that refused it is listed.
pub fn launch_summary(all: &[LaunchFacts], dir: &std::path::Path) -> Vec<Requirement> {
    let mut open: Vec<Requirement> = Vec::new();
    for facts in all {
        for mut gate in launch_gates(facts) {
            let is_unbuilt_profile = gate.name == "launch profile" && facts.profile.is_none();
            if gate.status == Status::Available || is_unbuilt_profile {
                continue;
            }
            if all.len() > 1 && PROVIDER_GATES.contains(&gate.name.as_str()) {
                gate.name =
                    gate.name
                        .replacen("launch ", &format!("launch {} ", facts.provider), 1);
            }
            if !open.iter().any(|listed| listed.name == gate.name) {
                open.push(gate);
            }
        }
    }
    if !open.is_empty() {
        return open;
    }
    let providers: Vec<&str> = all.iter().map(|facts| facts.provider.as_str()).collect();
    vec![Requirement::available(
        "launch",
        format!(
            "a session of {} in {} passes every launch gate; `torlek sandbox show {} --agent \
             NAME` lists them",
            providers.join(", "),
            dir.display(),
            dir.display()
        ),
    )]
}

fn command(facts: &LaunchFacts) -> Requirement {
    match &facts.command {
        Ok(program) => Requirement::available(
            "launch command",
            format!("{} starts {}", facts.provider, program.display()),
        ),
        Err(reason) => Requirement::needs(
            "launch command",
            Status::MissingPrerequisite,
            format!(
                "the {} adapter refuses to build it: {reason}",
                facts.provider
            ),
            "fix what is named here; `torlek doctor` shows the provider's state",
        ),
    }
}

fn backend(setting: AgentSandbox, backend: &Result<String, String>) -> Requirement {
    match (setting, backend) {
        (AgentSandbox::Off, _) => Requirement::available(
            "launch backend",
            format!(
                "none: agents.sandbox = \"off\", so the agent runs UNSANDBOXED; {}",
                cockpit_config::UNSANDBOXED_RISK
            ),
        ),
        (AgentSandbox::Required, Ok(backend)) => Requirement::available("launch backend", backend),
        (AgentSandbox::Required, Err(reason)) => Requirement::needs(
            "launch backend",
            Status::MissingPrerequisite,
            format!("refused: no sandbox backend on this host: {reason}"),
            "fix what the detail names and restart the daemon; to run agents unsandboxed, with \
             your rights, set agents.sandbox = \"off\" in the owner config and run `torlek \
             config reload`",
        ),
    }
}

fn desktop(is_sandboxed: bool, desktop: DesktopSession, refusal: Option<&str>) -> Requirement {
    let name = "launch desktop session";
    match (desktop, refusal) {
        (DesktopSession::Withheld, _) => Requirement::available(
            name,
            "withheld: the agent gets none of your desktop session",
        ),
        (DesktopSession::Shared, Some(refusal)) if is_sandboxed => Requirement::needs(
            name,
            Status::MissingPrerequisite,
            format!("refused: {refusal}"),
            "set agents.desktop_session = \"withheld\" in the owner config and run `torlek \
             config reload`",
        ),
        (DesktopSession::Shared, _) => {
            Requirement::available(name, "shared: the agent gets your display and session bus")
        }
    }
}

fn sign_in(facts: &LaunchFacts) -> Requirement {
    let name = "launch sign-in";
    let provider = &facts.provider;
    let passed_over = if facts.passed_over.is_empty() {
        String::new()
    } else {
        format!("; passed over: {}", crate::host::listed(&facts.passed_over))
    };
    match &facts.account {
        Err(reason) => Requirement::needs(
            name,
            Status::MissingPrerequisite,
            format!("refused: {reason}"),
            "fix what it names; `torlek doctor` shows each account's sign-in",
        ),
        Ok(_) if !facts.binds_sign_in => Requirement::available(
            name,
            format!("{provider} signs in as it does outside Torlek; Torlek binds no credential"),
        ),
        Ok(Some(account)) => {
            Requirement::available(name, format!("runs on account {account}{passed_over}"))
        }
        Ok(None) => Requirement::needs(
            name,
            Status::NeedsAuthorization,
            format!(
                "no account holds a credential or a login this project may use, so {provider} \
                 asks for a sign-in at every new session and cannot store it{passed_over}"
            ),
            "`torlek doctor` names the file to write for each account",
        ),
    }
}

fn github(is_sandboxed: bool, has_token: &Result<bool, String>) -> Requirement {
    let name = "launch github";
    match (is_sandboxed, has_token) {
        (_, Err(reason)) => Requirement::needs(
            name,
            Status::MissingPrerequisite,
            format!("refused: {reason}"),
            "fix the token file it names, or remove it",
        ),
        (true, Ok(true)) => Requirement::available(
            name,
            "the session proxy adds the project's API token on GitHub's hosts; pushes go through \
             `torlek push`",
        ),
        (true, Ok(false)) => Requirement::available(
            name,
            "the project keeps no API token: the agent reaches GitHub without one",
        ),
        (false, Ok(true)) => {
            Requirement::available(name, "the agent holds the project's own GitHub token")
        }
        (false, Ok(false)) => Requirement::available(
            name,
            "the project keeps no token: the agent uses your own GitHub login",
        ),
    }
}

fn toolchains(
    setting: AgentSandbox,
    toolchains: Option<&Result<Toolchains, String>>,
) -> Requirement {
    let name = "launch toolchains";
    match toolchains {
        None if setting == AgentSandbox::Off => Requirement::available(
            name,
            "not read: unsandboxed, the agent keeps your own caches and configuration",
        ),
        None => Requirement::needs(
            name,
            Status::Unknown,
            "not read: the launch has no sandbox backend",
            "fix the launch backend first",
        ),
        Some(Err(reason)) => Requirement::needs(
            name,
            Status::MissingPrerequisite,
            format!("refused: {reason}"),
            "fix what is named here in the checkout or on the agent's PATH",
        ),
        Some(Ok(found)) if found.ecosystems.is_empty() => {
            Requirement::available(name, "none detected in the checkout")
        }
        Some(Ok(found)) => {
            let roots: Vec<String> = found
                .install_roots
                .iter()
                .map(|root| root.display().to_string())
                .collect();
            Requirement::available(
                name,
                format!(
                    "{}; install roots bound read-only: {}",
                    found.ecosystems.join(", "),
                    crate::host::listed(&roots)
                ),
            )
        }
    }
}

fn trust(provider: &str, trust: &HarnessTrust) -> Requirement {
    let name = "launch folder trust";
    match trust {
        HarnessTrust::Recorded { from } => Requirement::available(
            name,
            format!(
                "recorded before the agent starts, from {} in projects.trusted",
                from.display()
            ),
        ),
        HarnessTrust::OwnersHome => Requirement::available(
            name,
            format!("the agent runs in your own home, which keeps your earlier {provider} answer"),
        ),
        HarnessTrust::Refused { reason } => Requirement::needs(
            name,
            Status::NeedsAuthorization,
            format!("not recorded ({reason}): {provider} asks at the agent's first start"),
            "if you trust the project, add its main checkout to projects.trusted in the owner \
             config; otherwise answer the question when the session waits on you",
        ),
        HarnessTrust::NeverForReview => Requirement::available(
            name,
            format!("a review clone is never trusted: {provider} asks at the agent's first start"),
        ),
    }
}
